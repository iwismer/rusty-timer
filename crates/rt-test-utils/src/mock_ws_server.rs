// mock_ws_server: A mock WebSocket server for testing forwarder/receiver clients.
//
// Accepts connections on ws://localhost:<port>, validates hello messages,
// responds with heartbeat (session_id + device_id), and sends configurable
// ack responses for event batches.

use std::collections::HashMap;
use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use rt_protocol::*;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::Message;

/// A mock WebSocket server for integration testing.
///
/// Binds to port 0 (random) and exposes the actual bound port. Each test
/// can spin up its own isolated server instance.
///
/// # Protocol behavior
///
/// - First message from a client must be `forwarder_hello` or `receiver_hello`.
///   Any other message produces an `error` response with code `PROTOCOL_ERROR`.
/// - After a valid hello, the server responds with a `heartbeat` carrying a
///   generated `session_id` (UUID v4) and `device_id` (from the hello).
/// - Subsequent `forwarder_event_batch` messages are acked with `forwarder_ack`,
///   computing high-water-mark entries per (forwarder_id, reader_ip, stream_epoch).
pub struct MockWsServer {
    addr: SocketAddr,
    /// Handle to the background accept loop; dropped when the server is dropped.
    _task: tokio::task::JoinHandle<()>,
}

impl MockWsServer {
    /// Start the mock server, binding to a random available port.
    ///
    /// Returns immediately once the listener is bound. Client connections are
    /// handled in a background tokio task (one spawned task per connection).
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let task = tokio::spawn(async move {
            Self::accept_loop(listener).await;
        });

        Ok(Self { addr, _task: task })
    }

    /// Return the address the server is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    // -- internal --

    /// Accept loop: accepts TCP connections and spawns a handler per connection.
    async fn accept_loop(listener: TcpListener) {
        loop {
            match listener.accept().await {
                Ok((stream, _peer)) => {
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream).await {
                            // In tests, connection errors are expected (e.g. client drops).
                            // Swallow silently.
                            let _ = e;
                        }
                    });
                }
                Err(_) => break,
            }
        }
    }

    /// Handle a single WebSocket connection.
    async fn handle_connection(
        stream: tokio::net::TcpStream,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let ws_stream = tokio_tungstenite::accept_async(stream).await?;
        let (mut write, mut read) = ws_stream.split();

        // State: waiting for hello
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut hello_received = false;

        while let Some(msg_result) = read.next().await {
            let msg = msg_result?;

            // Only handle text frames (protocol requirement: all WS messages are JSON text)
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => break,
                Message::Ping(data) => {
                    write.send(Message::Pong(data)).await?;
                    continue;
                }
                _ => continue,
            };

            let ws_msg: WsMessage = serde_json::from_str(&text)?;

            if !hello_received {
                // First message must be a hello
                match &ws_msg {
                    WsMessage::ForwarderHello(hello) => {
                        hello_received = true;
                        let heartbeat = WsMessage::Heartbeat(Heartbeat {
                            session_id: session_id.clone(),
                            device_id: hello.forwarder_id.clone(),
                        });
                        let json = serde_json::to_string(&heartbeat)?;
                        write.send(Message::Text(json.into())).await?;
                    }
                    WsMessage::ReceiverHello(hello) => {
                        hello_received = true;
                        let heartbeat = WsMessage::Heartbeat(Heartbeat {
                            session_id: session_id.clone(),
                            device_id: hello.receiver_id.clone(),
                        });
                        let json = serde_json::to_string(&heartbeat)?;
                        write.send(Message::Text(json.into())).await?;
                    }
                    _ => {
                        // Protocol error: first message was not a hello
                        let error = WsMessage::Error(ErrorMessage {
                            code: error_codes::PROTOCOL_ERROR.to_owned(),
                            message: "first message must be forwarder_hello or receiver_hello"
                                .to_owned(),
                            retryable: false,
                        });
                        let json = serde_json::to_string(&error)?;
                        write.send(Message::Text(json.into())).await?;
                        // Keep connection open so client can read the error
                        continue;
                    }
                }
            } else {
                // Post-hello message handling
                match ws_msg {
                    WsMessage::ForwarderEventBatch(batch) => {
                        let ack = Self::build_forwarder_ack(&session_id, &batch);
                        let json = serde_json::to_string(&ack)?;
                        write.send(Message::Text(json.into())).await?;
                    }
                    // Additional message types can be handled here as needed.
                    // For now, other post-hello messages are silently ignored.
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// Build a `ForwarderAck` from a `ForwarderEventBatch`.
    ///
    /// Computes high-water-mark entries per unique
    /// (forwarder_id, reader_ip, stream_epoch) key.
    fn build_forwarder_ack(session_id: &str, batch: &ForwarderEventBatch) -> WsMessage {
        // Group events by (forwarder_id, reader_ip, stream_epoch) and find max seq
        let mut high_water: HashMap<(String, String, u64), u64> = HashMap::new();

        for event in &batch.events {
            let key = (
                event.forwarder_id.clone(),
                event.reader_ip.clone(),
                event.stream_epoch,
            );
            let entry = high_water.entry(key).or_insert(0);
            if event.seq > *entry {
                *entry = event.seq;
            }
        }

        let mut entries: Vec<AckEntry> = high_water
            .into_iter()
            .map(|((fwd_id, reader_ip, epoch), last_seq)| AckEntry {
                forwarder_id: fwd_id,
                reader_ip,
                stream_epoch: epoch,
                last_seq,
            })
            .collect();

        // Sort for deterministic output in tests
        entries.sort_by(|a, b| {
            a.forwarder_id
                .cmp(&b.forwarder_id)
                .then(a.reader_ip.cmp(&b.reader_ip))
                .then(a.stream_epoch.cmp(&b.stream_epoch))
        });

        WsMessage::ForwarderAck(ForwarderAck {
            session_id: session_id.to_owned(),
            entries,
        })
    }
}

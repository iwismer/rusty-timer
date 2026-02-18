//! Forwarder uplink WebSocket client.
//!
//! Connects to the server, performs the ForwarderHello handshake, and
//! provides methods to send event batches and receive acks.
//!
//! # Protocol
//! 1. Connect to `server_url` (ws:// or wss://)
//! 2. Send `ForwarderHello` with advisory `forwarder_id` and resume cursors
//! 3. Receive `Heartbeat` — extract `session_id` and `device_id`
//! 4. Send `ForwarderEventBatch` messages; receive `ForwarderAck` per batch
//! 5. Track `session_id` for all subsequent messages

use futures_util::{SinkExt, StreamExt};
use rt_protocol::{
    EpochResetCommand, ForwarderAck, ForwarderEventBatch, ForwarderHello, ReadEvent, ResumeCursor,
    WsMessage,
};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the uplink WS session.
#[derive(Debug, Clone)]
pub struct UplinkConfig {
    /// WebSocket URL of the server endpoint, e.g. `wss://timing.example.com/ws/v1/forwarders`
    pub server_url: String,
    /// Bearer token (raw, read from token file).
    pub token: String,
    /// Advisory forwarder identity (must match token claims).
    pub forwarder_id: String,
    /// Optional human-readable name for this forwarder.
    pub display_name: Option<String>,
    /// `"immediate"` or `"batched"`.
    pub batch_mode: String,
    /// Flush interval in ms when `batch_mode = "batched"`.
    pub batch_flush_ms: u64,
    /// Max events per batch when `batch_mode = "batched"`.
    pub batch_max_events: u32,
}

// ---------------------------------------------------------------------------
// SendBatchResult
// ---------------------------------------------------------------------------

/// Outcome of a `send_batch` call.
///
/// The server may interleave an `EpochResetCommand` before the expected ack.
/// Callers must handle both variants.
#[derive(Debug)]
pub enum SendBatchResult {
    /// Normal ack — update journal ack cursors.
    Ack(ForwarderAck),
    /// Server requested an epoch bump — caller should update the journal and
    /// reconnect with a fresh `ForwarderHello`.
    EpochReset(EpochResetCommand),
}

// ---------------------------------------------------------------------------
// UplinkSession
// ---------------------------------------------------------------------------

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// An active WebSocket session with the server.
///
/// Created by `UplinkSession::connect`; holds an authenticated session
/// after the hello/heartbeat handshake completes.
pub struct UplinkSession {
    ws: WsStream,
    session_id: String,
    device_id: String,
    _forwarder_id: String,
}

impl UplinkSession {
    /// Connect to the server, perform the ForwarderHello handshake,
    /// and return a ready-to-use session.
    pub async fn connect(cfg: UplinkConfig) -> Result<Self, UplinkError> {
        use tokio_tungstenite::connect_async;

        let request = build_ws_request(&cfg.server_url, &cfg.token)?;
        let (ws, _response) = connect_async(request)
            .await
            .map_err(|e| UplinkError::Connect(e.to_string()))?;

        let mut session = UplinkSession {
            ws,
            session_id: String::new(),
            device_id: String::new(),
            _forwarder_id: cfg.forwarder_id.clone(),
        };

        // Send ForwarderHello
        let hello = WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: cfg.forwarder_id.clone(),
            reader_ips: vec![],
            resume: vec![],
            display_name: cfg.display_name.clone(),
        });
        session.send_ws_message(&hello).await?;

        // Receive Heartbeat with session_id and device_id
        let hb = session.recv_ws_message().await?;
        match hb {
            WsMessage::Heartbeat(hb) => {
                session.session_id = hb.session_id;
                session.device_id = hb.device_id;
                info!(
                    session_id = %session.session_id,
                    device_id = %session.device_id,
                    "uplink session established"
                );
            }
            WsMessage::Error(e) => {
                return Err(UplinkError::Protocol(format!(
                    "server error: {} - {}",
                    e.code, e.message
                )));
            }
            other => {
                return Err(UplinkError::Protocol(format!(
                    "expected Heartbeat, got: {:?}",
                    other
                )));
            }
        }

        Ok(session)
    }

    /// Connect with explicit resume cursors (for reconnect-replay scenarios).
    pub async fn connect_with_resume(
        cfg: UplinkConfig,
        reader_ips: Vec<String>,
        resume: Vec<ResumeCursor>,
    ) -> Result<Self, UplinkError> {
        use tokio_tungstenite::connect_async;

        let request = build_ws_request(&cfg.server_url, &cfg.token)?;
        let (ws, _response) = connect_async(request)
            .await
            .map_err(|e| UplinkError::Connect(e.to_string()))?;

        let mut session = UplinkSession {
            ws,
            session_id: String::new(),
            device_id: String::new(),
            _forwarder_id: cfg.forwarder_id.clone(),
        };

        let hello = WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: cfg.forwarder_id.clone(),
            reader_ips,
            resume,
            display_name: cfg.display_name.clone(),
        });
        session.send_ws_message(&hello).await?;

        let hb = session.recv_ws_message().await?;
        match hb {
            WsMessage::Heartbeat(hb) => {
                session.session_id = hb.session_id;
                session.device_id = hb.device_id;
            }
            WsMessage::Error(e) => {
                return Err(UplinkError::Protocol(format!(
                    "server error: {} - {}",
                    e.code, e.message
                )));
            }
            other => {
                return Err(UplinkError::Protocol(format!(
                    "expected Heartbeat after hello, got: {:?}",
                    other
                )));
            }
        }

        Ok(session)
    }

    /// The session ID assigned by the server after the handshake.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The device ID resolved from the token claims (returned in the initial Heartbeat).
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Send a batch of events and wait for the server's response.
    ///
    /// Returns [`SendBatchResult::Ack`] on normal ack, or
    /// [`SendBatchResult::EpochReset`] if the server sends an epoch-reset
    /// command before the ack arrives.  The caller must handle both.
    pub async fn send_batch(
        &mut self,
        events: Vec<ReadEvent>,
    ) -> Result<SendBatchResult, UplinkError> {
        let batch_id = Uuid::new_v4().to_string();
        let batch = WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: self.session_id.clone(),
            batch_id,
            events,
        });
        self.send_ws_message(&batch).await?;

        // Wait for ack or epoch reset
        loop {
            let msg = self.recv_ws_message().await?;
            match msg {
                WsMessage::ForwarderAck(ack) => return Ok(SendBatchResult::Ack(ack)),
                WsMessage::Heartbeat(_) => {
                    // Heartbeat received mid-batch; ignore and continue waiting
                    debug!("heartbeat received while waiting for ack");
                    continue;
                }
                WsMessage::EpochResetCommand(cmd) => {
                    info!(
                        reader_ip = %cmd.reader_ip,
                        new_epoch = cmd.new_stream_epoch,
                        "epoch reset command received, surfacing to caller"
                    );
                    return Ok(SendBatchResult::EpochReset(cmd));
                }
                WsMessage::Error(e) => {
                    return Err(UplinkError::Protocol(format!(
                        "server error while waiting for ack: {} - {}",
                        e.code, e.message
                    )));
                }
                other => {
                    warn!("unexpected message while waiting for ack: {:?}", other);
                    continue;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn send_ws_message(&mut self, msg: &WsMessage) -> Result<(), UplinkError> {
        let json =
            serde_json::to_string(msg).map_err(|e| UplinkError::Serialization(e.to_string()))?;
        self.ws
            .send(Message::Text(json.into()))
            .await
            .map_err(|e| UplinkError::Ws(e.to_string()))?;
        Ok(())
    }

    async fn recv_ws_message(&mut self) -> Result<WsMessage, UplinkError> {
        loop {
            match self.ws.next().await {
                None => return Err(UplinkError::Disconnected),
                Some(Err(e)) => return Err(UplinkError::Ws(e.to_string())),
                Some(Ok(msg)) => match msg {
                    Message::Text(t) => {
                        let ws_msg: WsMessage = serde_json::from_str(&t)
                            .map_err(|e| UplinkError::Protocol(format!("JSON parse: {}", e)))?;
                        return Ok(ws_msg);
                    }
                    Message::Close(_) => return Err(UplinkError::Disconnected),
                    Message::Ping(data) => {
                        // Reply to pings
                        let _ = self.ws.send(Message::Pong(data)).await;
                        continue;
                    }
                    _ => continue,
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum UplinkError {
    Connect(String),
    Ws(String),
    Protocol(String),
    Serialization(String),
    Disconnected,
}

impl std::fmt::Display for UplinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UplinkError::Connect(s) => write!(f, "Connection error: {}", s),
            UplinkError::Ws(s) => write!(f, "WebSocket error: {}", s),
            UplinkError::Protocol(s) => write!(f, "Protocol error: {}", s),
            UplinkError::Serialization(s) => write!(f, "Serialization error: {}", s),
            UplinkError::Disconnected => write!(f, "WebSocket disconnected"),
        }
    }
}

impl std::error::Error for UplinkError {}

// ---------------------------------------------------------------------------
// Private: build WS request with Bearer auth header
// ---------------------------------------------------------------------------

fn build_ws_request(
    url: &str,
    token: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, UplinkError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = url
        .into_client_request()
        .map_err(|e| UplinkError::Connect(format!("invalid URL '{}': {}", url, e)))?;

    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", token).parse().map_err(
            |e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue| {
                UplinkError::Connect(format!("invalid auth header: {}", e))
            },
        )?,
    );

    Ok(request)
}

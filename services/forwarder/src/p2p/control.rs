//! Forwarder control-stream handler.
//!
//! The control stream is the first bidirectional stream a receiver opens on an
//! admitted P2P connection. This module owns its lifecycle:
//!
//! 1. `Hello`/`HelloOk` version negotiation via [`rt_p2p_protocol::negotiate`].
//! 2. Serving a [`StreamCatalog`] snapshot from a [`CatalogProvider`].
//! 3. A `Ping`/`Pong` heartbeat that closes the stream once the peer misses
//!    enough consecutive pongs.
//!
//! Version mismatches are reported back to the peer as a control-plane
//! [`WireProtocolError`] (with the [`ProtocolErrorCode::UnsupportedVersion`]
//! code) before the stream is failed.
//!
//! Data-plane subscriber delivery and the persistent allow-list / revocation
//! flows are intentionally out of scope here and handled by later tasks.

use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rt_iroh::{Connection, RecvStream, SendStream};
use rt_p2p_protocol::{
    ControlC2F, ControlF2C, Hello, MAX_FRAME_BYTES, Ping, Pong, ProtocolError, ProtocolErrorCode,
    StreamCatalog, WireProtocolError, control_c2f, control_f2c, encode_frame, negotiate,
};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

/// Protocol minor version this forwarder speaks for the P2P transport.
pub(crate) const PROTOCOL_MINOR: u32 = 1;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Supplies the current [`StreamCatalog`] served on the control plane.
///
/// This abstraction lets the control handler stay agnostic of how stream
/// metadata is sourced: tests use a [`StaticCatalog`], while production wiring
/// can supply a live view of the forwarder's reader configuration.
pub trait CatalogProvider: std::fmt::Debug + Send + Sync + 'static {
    /// Returns a snapshot of the streams currently exposed by the forwarder.
    fn catalog(&self) -> StreamCatalog;
}

/// A [`CatalogProvider`] that always returns a fixed catalog snapshot.
#[derive(Clone, Debug)]
pub struct StaticCatalog {
    catalog: StreamCatalog,
}

impl StaticCatalog {
    /// Builds a provider that always serves `catalog`.
    #[must_use]
    pub fn new(catalog: StreamCatalog) -> Self {
        Self { catalog }
    }
}

impl CatalogProvider for StaticCatalog {
    fn catalog(&self) -> StreamCatalog {
        self.catalog.clone()
    }
}

impl<C: CatalogProvider + ?Sized> CatalogProvider for Arc<C> {
    fn catalog(&self) -> StreamCatalog {
        (**self).catalog()
    }
}

/// Heartbeat (`Ping`/`Pong`) timing for the control stream.
#[derive(Clone, Copy, Debug)]
pub struct HeartbeatConfig {
    /// How often the forwarder sends a `Ping`.
    pub interval: Duration,
    /// Number of consecutive unanswered pings that marks the peer dead.
    pub max_missed: u32,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(15),
            max_missed: 3,
        }
    }
}

/// The forwarder's own `Hello`, used to negotiate against the client's.
pub(crate) fn forwarder_hello() -> Hello {
    Hello {
        min_minor: PROTOCOL_MINOR,
        max_minor: PROTOCOL_MINOR,
        capabilities: Vec::new(),
        max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap_or(u32::MAX),
        catalog_generation: 0,
    }
}

/// Maps a [`ProtocolErrorCode`] to its stable wire value.
pub(crate) const fn wire_error_code(code: ProtocolErrorCode) -> u32 {
    match code {
        ProtocolErrorCode::UnsupportedVersion => 1,
        ProtocolErrorCode::AuthDenied => 2,
        ProtocolErrorCode::RevokedPeer => 3,
        ProtocolErrorCode::UnknownStream => 4,
        ProtocolErrorCode::StreamDisabled => 5,
        ProtocolErrorCode::InvalidCursor => 6,
        ProtocolErrorCode::RetentionGap => 7,
        ProtocolErrorCode::ProtocolViolation => 8,
        ProtocolErrorCode::FrameTooLarge => 9,
        ProtocolErrorCode::DecodeError => 10,
        ProtocolErrorCode::BackpressureTimeout => 11,
        ProtocolErrorCode::Internal => 12,
    }
}

/// Builds the wire [`WireProtocolError`] for a runtime [`ProtocolError`].
fn wire_protocol_error(error: &ProtocolError) -> WireProtocolError {
    WireProtocolError {
        code: wire_error_code(error.code()),
        message: error.to_string(),
        retryable: error.retryable(),
        stream_id: error.stream_id().map(<[u8]>::to_vec),
    }
}

/// Serves the control stream for an admitted connection until it is closed.
///
/// The `Hello`/`HelloOk` negotiation and catalog delivery must complete within
/// `handshake_timeout`; afterwards the heartbeat governs the stream's lifetime.
/// Returns `Ok(())` when the peer disconnects cleanly and `Err` when the
/// handshake fails/times out or the heartbeat declares the peer dead.
pub(crate) async fn serve_control(
    connection: &Connection,
    catalog: &dyn CatalogProvider,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
) -> Result<(), BoxError> {
    let (send, recv) = match tokio::time::timeout(
        handshake_timeout,
        negotiate_and_serve_catalog(connection, catalog, heartbeat),
    )
    .await
    {
        Ok(result) => result?,
        Err(_elapsed) => return Err("control handshake timed out".into()),
    };

    run_heartbeat(send, recv, heartbeat).await
}

/// Accepts the control stream, negotiates versions, and serves the catalog.
///
/// A single [`StreamCatalog`] snapshot is captured up front and used for both
/// `HelloOk.catalog_generation` and the `StreamCatalog` frame, so the two
/// always agree at open time as the protocol requires. The advertised
/// `HelloOk.heartbeat_interval_secs` is taken from the heartbeat config the
/// handler actually pings with.
async fn negotiate_and_serve_catalog(
    connection: &Connection,
    catalog: &dyn CatalogProvider,
    heartbeat: HeartbeatConfig,
) -> Result<(SendStream, RecvStream), BoxError> {
    let (mut send, mut recv) = connection.accept_bi().await?;

    let control = read_frame::<ControlC2F>(&mut recv).await?;
    let client_hello = match control.msg {
        Some(control_c2f::Msg::Hello(hello)) => hello,
        other => return Err(format!("expected control Hello, got {other:?}").into()),
    };

    // Snapshot the catalog once: its generation must match what the peer sees
    // in both HelloOk and the StreamCatalog frame below.
    let snapshot = catalog.catalog();
    let mut server_hello = forwarder_hello();
    server_hello.catalog_generation = snapshot.generation;

    let hello_ok = match negotiate(&client_hello, &server_hello) {
        Ok(mut hello_ok) => {
            // negotiate() leaves heartbeat_interval_secs at 0; advertise the
            // interval the heartbeat loop actually uses.
            hello_ok.heartbeat_interval_secs =
                u32::try_from(heartbeat.interval.as_secs()).unwrap_or(u32::MAX);
            hello_ok
        }
        Err(error) => {
            // Report the failure to the peer, then flush and wait for receipt
            // so the error is delivered before the caller closes the
            // connection.
            let _ = write_frame(
                &mut send,
                &ControlF2C {
                    msg: Some(control_f2c::Msg::ProtocolError(wire_protocol_error(&error))),
                },
            )
            .await;
            let _ = send.finish();
            let _ = send.stopped().await;
            return Err(Box::new(error));
        }
    };

    write_frame(
        &mut send,
        &ControlF2C {
            msg: Some(control_f2c::Msg::HelloOk(hello_ok)),
        },
    )
    .await?;
    write_frame(
        &mut send,
        &ControlF2C {
            msg: Some(control_f2c::Msg::StreamCatalog(snapshot)),
        },
    )
    .await?;

    Ok((send, recv))
}

/// Runs the `Ping`/`Pong` heartbeat until the peer misses `max_missed`
/// consecutive pongs (returns `Err`) or disconnects cleanly (returns `Ok`).
async fn run_heartbeat(
    mut send: SendStream,
    mut recv: RecvStream,
    config: HeartbeatConfig,
) -> Result<(), BoxError> {
    // Read frames on a dedicated task so heartbeat ticks never cancel a
    // partially-read frame (which would desync the length-prefixed framing).
    let (tx, mut rx) = mpsc::channel::<ControlC2F>(16);
    let reader = tokio::spawn(async move {
        while let Ok(frame) = read_frame::<ControlC2F>(&mut recv).await {
            if tx.send(frame).await.is_err() {
                break;
            }
        }
    });

    let mut ticker = tokio::time::interval(config.interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first tick fires immediately; consume it so pings are spaced by the
    // configured interval.
    ticker.tick().await;

    let mut nonce: u64 = 0;
    let mut outstanding: u32 = 0;

    let result = loop {
        tokio::select! {
            _ = ticker.tick() => {
                nonce += 1;
                if write_frame(
                    &mut send,
                    &ControlF2C { msg: Some(control_f2c::Msg::Ping(Ping { nonce })) },
                )
                .await
                .is_err()
                {
                    break Ok(());
                }
                outstanding += 1;
                if outstanding >= config.max_missed {
                    break Err(format!(
                        "heartbeat timed out after {outstanding} unanswered pings"
                    )
                    .into());
                }
            }
            frame = rx.recv() => {
                match frame {
                    Some(control) => match control.msg {
                        Some(control_c2f::Msg::Pong(_)) => outstanding = 0,
                        Some(control_c2f::Msg::Ping(ping)) => {
                            let pong = ControlF2C {
                                msg: Some(control_f2c::Msg::Pong(Pong { nonce: ping.nonce })),
                            };
                            if write_frame(&mut send, &pong).await.is_err() {
                                break Ok(());
                            }
                        }
                        // ReaderControlRequest and unknown messages are ignored
                        // until later tasks add their handling.
                        _ => {}
                    },
                    None => break Ok(()),
                }
            }
        }
    };

    reader.abort();
    result
}

/// Writes a single length-prefixed protobuf frame to a send stream.
pub(crate) async fn write_frame(
    send: &mut SendStream,
    message: &impl Message,
) -> Result<(), BoxError> {
    send.write_all(encode_frame(message).as_ref()).await?;
    Ok(())
}

/// Reads a single length-prefixed protobuf frame from a receive stream.
pub(crate) async fn read_frame<M>(recv: &mut RecvStream) -> Result<M, BoxError>
where
    M: Message + Default,
{
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(format!("frame length {len} exceeds MAX_FRAME_BYTES {MAX_FRAME_BYTES}").into());
    }

    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload).await?;
    Ok(M::decode(payload.as_slice())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use rt_iroh::{Endpoint, EndpointBuilder, NodeAddr};
    use rt_p2p_protocol::StreamEntry;
    use tokio::task::JoinHandle;

    type TestResult = Result<(), BoxError>;

    const LONG_HANDSHAKE: Duration = Duration::from_secs(5);

    /// A heartbeat config that effectively never fires, for tests that only
    /// exercise the handshake/catalog phase.
    const QUIET_HEARTBEAT_SECS: u64 = 3600;
    fn quiet_heartbeat() -> HeartbeatConfig {
        HeartbeatConfig {
            interval: Duration::from_secs(QUIET_HEARTBEAT_SECS),
            max_missed: 3,
        }
    }

    fn sample_catalog() -> StreamCatalog {
        StreamCatalog {
            generation: 7,
            entries: vec![StreamEntry {
                stream_id: vec![1u8; 16],
                display_name: "Finish Line".to_string(),
                network_addr: "10.0.0.5:10000".to_string(),
                reader_connected: true,
                hardware_reader_id: "RDR-1".to_string(),
            }],
        }
    }

    /// Spawns a forwarder endpoint that serves `serve_control` for a single
    /// inbound connection. Returns the endpoint, its dialable address, and a
    /// handle yielding the `serve_control` result (the connection is closed on
    /// error so the peer observes the close).
    async fn spawn_forwarder(
        seed: [u8; 32],
        catalog: StaticCatalog,
        handshake_timeout: Duration,
        heartbeat: HeartbeatConfig,
    ) -> Result<(Endpoint, NodeAddr, JoinHandle<Result<(), String>>), BoxError> {
        let endpoint = EndpointBuilder::test(seed).bind().await?;
        let node_addr = endpoint.node_addr().await;

        let accept_endpoint = endpoint.clone();
        let handle = tokio::spawn(async move {
            let connection = match accept_endpoint.accept().await {
                Ok(Some(connection)) => connection,
                Ok(None) => return Err("endpoint closed before a connection arrived".to_string()),
                Err(error) => return Err(format!("accept failed: {error}")),
            };
            let result = serve_control(&connection, &catalog, handshake_timeout, heartbeat).await;
            if let Err(error) = &result {
                connection.close(1u32.into(), b"control stream failed");
                return Err(error.to_string());
            }
            Ok(())
        });

        Ok((endpoint, node_addr, handle))
    }

    /// Dials `forwarder_addr` and opens the control stream, sending `hello`.
    async fn open_control(
        receiver: &Endpoint,
        forwarder_addr: NodeAddr,
        hello: Hello,
    ) -> Result<(Connection, SendStream, RecvStream), BoxError> {
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;
        let (mut send, recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(hello)),
            },
        )
        .await?;
        Ok((connection, send, recv))
    }

    #[tokio::test]
    async fn hello_returns_catalog() -> TestResult {
        let catalog = sample_catalog();
        let (forwarder, forwarder_addr, handle) = spawn_forwarder(
            [40; 32],
            StaticCatalog::new(catalog.clone()),
            LONG_HANDSHAKE,
            quiet_heartbeat(),
        )
        .await?;

        let receiver = EndpointBuilder::test([41; 32]).bind().await?;
        let (connection, _send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, forwarder_hello()),
        )
        .await??;

        let hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let hello_ok = match hello_ok.msg {
            Some(control_f2c::Msg::HelloOk(ok)) => {
                assert_eq!(ok.protocol_minor, PROTOCOL_MINOR);
                // The advertised heartbeat interval must reflect the configured
                // interval the handler actually pings with.
                assert_eq!(
                    u64::from(ok.heartbeat_interval_secs),
                    QUIET_HEARTBEAT_SECS,
                    "HelloOk.heartbeat_interval_secs must match the configured interval"
                );
                ok
            }
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        };

        let catalog_frame = read_frame::<ControlF2C>(&mut recv).await?;
        match catalog_frame.msg {
            Some(control_f2c::Msg::StreamCatalog(served)) => {
                assert_eq!(served, catalog);
                // HelloOk.catalog_generation must agree with the StreamCatalog
                // generation served in the same handshake.
                assert_eq!(
                    hello_ok.catalog_generation, served.generation,
                    "HelloOk.catalog_generation must match StreamCatalog.generation"
                );
            }
            other => return Err(format!("expected StreamCatalog, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn version_mismatch_returns_unsupported() -> TestResult {
        let (forwarder, forwarder_addr, handle) = spawn_forwarder(
            [42; 32],
            StaticCatalog::new(sample_catalog()),
            LONG_HANDSHAKE,
            quiet_heartbeat(),
        )
        .await?;

        let receiver = EndpointBuilder::test([43; 32]).bind().await?;
        let incompatible = Hello {
            min_minor: PROTOCOL_MINOR + 99,
            max_minor: PROTOCOL_MINOR + 99,
            capabilities: Vec::new(),
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap_or(u32::MAX),
            catalog_generation: 0,
        };
        let (connection, _send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, incompatible),
        )
        .await??;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ProtocolError(error)) => {
                assert_eq!(
                    error.code,
                    wire_error_code(ProtocolErrorCode::UnsupportedVersion)
                );
            }
            other => return Err(format!("expected ProtocolError, got {other:?}").into()),
        }

        // The forwarder must fail the control stream after reporting the error.
        let result = tokio::time::timeout(LONG_HANDSHAKE, handle).await??;
        assert!(
            result.is_err(),
            "version mismatch must fail the control stream, got {result:?}"
        );

        connection.close(0u32.into(), b"done");
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn heartbeat_timeout_closes() -> TestResult {
        let heartbeat = HeartbeatConfig {
            interval: Duration::from_millis(50),
            max_missed: 3,
        };
        let (forwarder, forwarder_addr, handle) = spawn_forwarder(
            [44; 32],
            StaticCatalog::new(sample_catalog()),
            LONG_HANDSHAKE,
            heartbeat,
        )
        .await?;

        let receiver = EndpointBuilder::test([45; 32]).bind().await?;
        let (connection, _send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, forwarder_hello()),
        )
        .await??;

        // Drain HelloOk + catalog, then ignore the pings the forwarder sends so
        // they go unanswered. The stream must be closed once enough pings miss.
        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if read_frame::<ControlF2C>(&mut recv).await.is_err() {
                    break;
                }
            }
        })
        .await;
        assert!(
            closed.is_ok(),
            "forwarder must close the control stream after the heartbeat misses"
        );

        let result = tokio::time::timeout(Duration::from_secs(5), handle).await??;
        assert!(
            result.is_err(),
            "heartbeat timeout must fail the control stream, got {result:?}"
        );

        connection.close(0u32.into(), b"done");
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }
}

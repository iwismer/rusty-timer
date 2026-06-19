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

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use prost::Message;
#[cfg(test)]
use rt_iroh::Connection;
use rt_iroh::{RecvStream, SendStream};
use rt_p2p_protocol::{
    CAP_CONTROL_EVENTS, CAP_REMOTE_CONFIG, ConfigGetRequest, ConfigGetResponse, ConfigSetRequest,
    ConfigSetResponse, ControlC2F, ControlF2C, DownloadProgress, Hello, MAX_FRAME_BYTES, Ping,
    Pong, ProtocolError, ProtocolErrorCode, ReaderControlRequest, ReaderControlResponse,
    ReaderInfo, ReaderStatus, RestartRequest, RestartResponse, StreamCatalog, SyncClock, UpsStatus,
    WireProtocolError, control_c2f, control_f2c, encode_frame, negotiate,
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

/// Future returned by a typed reader-control handler.
pub type ReaderControlFuture<'a> = Pin<Box<dyn Future<Output = ReaderControlResponse> + Send + 'a>>;

/// Handles typed reader-control requests received from the P2P control stream.
pub trait ReaderControlHandler: std::fmt::Debug + Send + Sync + 'static {
    /// Performs the requested action and returns the response to send back to
    /// the receiver. Implementations must not tunnel control results through
    /// data-plane read records.
    fn handle(&self, request: ReaderControlRequest) -> ReaderControlFuture<'_>;
}

/// Future returned by a P2P sync-clock drift reader.
pub type SyncClockFuture<'a> =
    Pin<Box<dyn Future<Output = Result<crate::reader_control::ClockInfo, String>> + Send + 'a>>;

/// Future returned by a clock rewrite operation.
pub type RewriteClockFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Reader-clock operations available to the P2P sync-clock adapter.
pub trait SyncClockSource: std::fmt::Debug + Send + Sync + 'static {
    /// Reads/records reader-clock drift for status reporting.
    fn record_clock_drift(&self) -> SyncClockFuture<'_>;

    /// Rewrites the reader clock. P2P sync-clock handling intentionally does
    /// not invoke this operation; it is present so tests can enforce that
    /// contract against the same source abstraction.
    fn set_date_time(&self) -> RewriteClockFuture<'_>;
}

/// P2P reader-control adapter for sync-clock requests.
#[derive(Debug)]
pub struct SyncClockDriftHandler<C> {
    clock_source: Arc<C>,
}

impl<C> SyncClockDriftHandler<C>
where
    C: SyncClockSource,
{
    /// Builds a sync-clock drift handler backed by `clock_source`.
    #[must_use]
    pub fn new(clock_source: Arc<C>) -> Self {
        Self { clock_source }
    }
}

impl<C> ReaderControlHandler for SyncClockDriftHandler<C>
where
    C: SyncClockSource,
{
    fn handle(&self, request: ReaderControlRequest) -> ReaderControlFuture<'_> {
        let clock_source = Arc::clone(&self.clock_source);
        Box::pin(async move {
            if request.command != "sync_clock" {
                return ReaderControlResponse {
                    stream_id: request.stream_id,
                    request_id: request.request_id,
                    success: false,
                    message: format!("unsupported reader control command: {}", request.command),
                };
            }

            match clock_source.record_clock_drift().await {
                Ok(clock) => ReaderControlResponse {
                    stream_id: request.stream_id,
                    request_id: request.request_id,
                    success: true,
                    message: format!("clock drift recorded: {}ms", clock.drift_ms),
                },
                Err(error) => ReaderControlResponse {
                    stream_id: request.stream_id,
                    request_id: request.request_id,
                    success: false,
                    message: error,
                },
            }
        })
    }
}

/// Default reader-control handler used until production wiring installs a real
/// adapter to the forwarder's reader-control runtime.
#[derive(Debug)]
pub struct NoopReaderControlHandler;

impl ReaderControlHandler for NoopReaderControlHandler {
    fn handle(&self, request: ReaderControlRequest) -> ReaderControlFuture<'_> {
        Box::pin(async move {
            ReaderControlResponse {
                stream_id: request.stream_id,
                request_id: request.request_id,
                success: false,
                message: "reader control handler not configured".to_owned(),
            }
        })
    }
}

/// Future returned by a remote-config get handler.
pub type ConfigGetFuture<'a> = Pin<Box<dyn Future<Output = ConfigGetResponse> + Send + 'a>>;
/// Future returned by a remote-config set handler.
pub type ConfigSetFuture<'a> = Pin<Box<dyn Future<Output = ConfigSetResponse> + Send + 'a>>;
/// Future returned by a remote-restart handler.
pub type RestartFuture<'a> = Pin<Box<dyn Future<Output = RestartResponse> + Send + 'a>>;

/// Serves remote forwarder configuration verbs (get/set/restart) received on
/// the P2P control stream.
///
/// The whole feature is gated by a single forwarder config flag
/// (`control.allow_remote_config`). [`Self::allow_remote_config`] drives both
/// capability advertisement (the forwarder only advertises
/// [`CAP_REMOTE_CONFIG`] when it returns `true`) and per-request gating, so
/// even a peer that skips capability negotiation cannot mutate config or
/// restart a forwarder that has the feature disabled.
pub trait RemoteConfigHandler: std::fmt::Debug + Send + Sync + 'static {
    /// Whether remote config is currently allowed. Controls capability
    /// advertisement and gating of all three verbs.
    fn allow_remote_config(&self) -> bool;

    /// Returns the current forwarder config serialized as JSON (the same shape
    /// `GET /api/v1/config` returns) plus the current `restart_needed` state.
    ///
    /// When remote config is disabled, implementations return an empty
    /// `config_json` as the explicit "disabled" signal (the response shape has
    /// no error field, and the capability is not advertised in that case).
    fn get_config(&self, request: ConfigGetRequest) -> ConfigGetFuture<'_>;

    /// Persists `config_json` (same shape as get) to the forwarder's TOML
    /// config file and marks a restart as needed. On validation or IO failure
    /// returns `ok = false` with a descriptive error and never panics.
    fn set_config(&self, request: ConfigSetRequest) -> ConfigSetFuture<'_>;

    /// Triggers the same graceful restart path as the HTTP restart endpoint.
    fn restart(&self, request: RestartRequest) -> RestartFuture<'_>;
}

impl<C: RemoteConfigHandler + ?Sized> RemoteConfigHandler for Arc<C> {
    fn allow_remote_config(&self) -> bool {
        (**self).allow_remote_config()
    }

    fn get_config(&self, request: ConfigGetRequest) -> ConfigGetFuture<'_> {
        (**self).get_config(request)
    }

    fn set_config(&self, request: ConfigSetRequest) -> ConfigSetFuture<'_> {
        (**self).set_config(request)
    }

    fn restart(&self, request: RestartRequest) -> RestartFuture<'_> {
        (**self).restart(request)
    }
}

/// Default remote-config handler used until production wiring installs a real
/// adapter. Reports the feature as disabled and rejects every verb, so the
/// capability is never advertised and no config mutation/restart is possible.
#[derive(Debug)]
pub struct NoopRemoteConfigHandler;

/// Error returned for any remote-config verb when the feature is disabled.
pub(crate) const REMOTE_CONFIG_DISABLED: &str = "remote config disabled";

impl RemoteConfigHandler for NoopRemoteConfigHandler {
    fn allow_remote_config(&self) -> bool {
        false
    }

    fn get_config(&self, request: ConfigGetRequest) -> ConfigGetFuture<'_> {
        Box::pin(async move {
            ConfigGetResponse {
                request_id: request.request_id,
                config_json: String::new(),
                restart_needed: false,
            }
        })
    }

    fn set_config(&self, request: ConfigSetRequest) -> ConfigSetFuture<'_> {
        Box::pin(async move {
            ConfigSetResponse {
                request_id: request.request_id,
                ok: false,
                restart_needed: false,
                error: REMOTE_CONFIG_DISABLED.to_owned(),
            }
        })
    }

    fn restart(&self, request: RestartRequest) -> RestartFuture<'_> {
        Box::pin(async move {
            RestartResponse {
                request_id: request.request_id,
                accepted: false,
                error: REMOTE_CONFIG_DISABLED.to_owned(),
            }
        })
    }
}

/// Typed status/control-plane events sent from the forwarder to a receiver.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    /// Reader connection/liveness status.
    ReaderStatus(ReaderStatus),
    /// Static descriptive information about a reader.
    ReaderInfo(ReaderInfo),
    /// Stored-read download progress.
    DownloadProgress(DownloadProgress),
    /// UPS status for the forwarder host.
    UpsStatus(UpsStatus),
    /// Clock-status publication for receiver-side clock alignment.
    SyncClock(SyncClock),
}

impl ControlEvent {
    fn into_frame(self) -> ControlF2C {
        let msg = match self {
            Self::ReaderStatus(status) => control_f2c::Msg::ReaderStatus(status),
            Self::ReaderInfo(info) => control_f2c::Msg::ReaderInfo(info),
            Self::DownloadProgress(progress) => control_f2c::Msg::DownloadProgress(progress),
            Self::UpsStatus(status) => control_f2c::Msg::UpsStatus(status),
            Self::SyncClock(clock) => control_f2c::Msg::SyncClock(clock),
        };
        ControlF2C { msg: Some(msg) }
    }
}

pub type ControlEventSender = mpsc::Sender<ControlEvent>;
pub type ControlEventReceiver = mpsc::Receiver<ControlEvent>;

/// Builds a channel for publishing typed control-plane events to the control
/// stream.
#[must_use]
pub fn control_event_channel(capacity: usize) -> (ControlEventSender, ControlEventReceiver) {
    mpsc::channel(capacity)
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
        capabilities: vec![CAP_CONTROL_EVENTS.to_owned()],
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

/// Performs `Hello`/`HelloOk` negotiation and catalog delivery on a
/// pre-accepted control stream, bounded by `handshake_timeout`.
///
/// Returns the negotiated stream halves so the caller can run the heartbeat
/// loop. Crucially, the caller must not serve any data streams until this
/// completes successfully: data delivery is gated on a finished control
/// handshake so an allow-listed peer cannot receive journal data before
/// `Hello`/catalog negotiation succeeds.
pub(crate) async fn negotiate_control_stream(
    send: SendStream,
    recv: RecvStream,
    catalog: &dyn CatalogProvider,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
    remote_config: &dyn RemoteConfigHandler,
) -> Result<(SendStream, RecvStream, Vec<String>), BoxError> {
    match tokio::time::timeout(
        handshake_timeout,
        negotiate_and_serve_catalog_stream(send, recv, catalog, heartbeat, remote_config),
    )
    .await
    {
        Ok(result) => result,
        Err(_elapsed) => Err("control handshake timed out".into()),
    }
}

/// Runs the post-negotiation heartbeat/control loop on an already-negotiated
/// control stream until the peer disconnects cleanly (`Ok`) or is declared dead
/// (`Err`). Uses the default no-op reader-control handler and forwards status
/// updates from the optional `outbound_events` channel to the peer.
pub(crate) async fn run_control_stream_loop(
    send: SendStream,
    recv: RecvStream,
    heartbeat: HeartbeatConfig,
    outbound_events: Option<ControlEventReceiver>,
    remote_config: Arc<dyn RemoteConfigHandler>,
) -> Result<(), BoxError> {
    run_control_loop(
        send,
        recv,
        heartbeat,
        Arc::new(NoopReaderControlHandler),
        outbound_events,
        remote_config,
    )
    .await
}

/// Serves the control stream with a typed reader-control handler and optional
/// outbound status-event channel.
#[cfg(test)]
pub(crate) async fn serve_control_with_typed_control(
    connection: &Connection,
    catalog: &dyn CatalogProvider,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
    reader_control: Arc<dyn ReaderControlHandler>,
    outbound_events: Option<ControlEventReceiver>,
    remote_config: Arc<dyn RemoteConfigHandler>,
) -> Result<(), BoxError> {
    let (send, recv) = connection.accept_bi().await?;
    serve_control_stream_with_typed_control(
        send,
        recv,
        catalog,
        handshake_timeout,
        heartbeat,
        reader_control,
        outbound_events,
        remote_config,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve_control_stream_with_typed_control(
    send: SendStream,
    recv: RecvStream,
    catalog: &dyn CatalogProvider,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
    reader_control: Arc<dyn ReaderControlHandler>,
    outbound_events: Option<ControlEventReceiver>,
    remote_config: Arc<dyn RemoteConfigHandler>,
) -> Result<(), BoxError> {
    let (send, recv, _capabilities) = negotiate_control_stream(
        send,
        recv,
        catalog,
        handshake_timeout,
        heartbeat,
        remote_config.as_ref(),
    )
    .await?;

    run_control_loop(
        send,
        recv,
        heartbeat,
        reader_control,
        outbound_events,
        remote_config,
    )
    .await
}

/// Accepts the control stream, negotiates versions, and serves the catalog.
///
/// A single [`StreamCatalog`] snapshot is captured up front and used for both
/// `HelloOk.catalog_generation` and the `StreamCatalog` frame, so the two
/// always agree at open time as the protocol requires. The advertised
/// `HelloOk.heartbeat_interval_secs` is taken from the heartbeat config the
/// handler actually pings with.
async fn negotiate_and_serve_catalog_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    catalog: &dyn CatalogProvider,
    heartbeat: HeartbeatConfig,
    remote_config: &dyn RemoteConfigHandler,
) -> Result<(SendStream, RecvStream, Vec<String>), BoxError> {
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
    // Advertise remote-config support only when the feature is enabled, so a
    // receiver UI can tell whether remote config is available before it sends a
    // request. Negotiation keeps the intersection, so this capability survives
    // only when both peers advertise it.
    if remote_config.allow_remote_config() {
        server_hello.capabilities.push(CAP_REMOTE_CONFIG.to_owned());
    }

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

    let capabilities = hello_ok.capabilities.clone();
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

    Ok((send, recv, capabilities))
}

/// Runs the typed control loop until the peer misses `max_missed` consecutive
/// pongs (returns `Err`) or disconnects cleanly (returns `Ok`).
async fn run_control_loop(
    mut send: SendStream,
    mut recv: RecvStream,
    config: HeartbeatConfig,
    reader_control: Arc<dyn ReaderControlHandler>,
    mut outbound_events: Option<ControlEventReceiver>,
    remote_config: Arc<dyn RemoteConfigHandler>,
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
    let (control_response_tx, mut control_response_rx) = mpsc::channel::<ReaderControlResponse>(16);
    // Remote-config verbs (get/set/restart) reply with arbitrary ControlF2C
    // frames rather than a ReaderControlResponse, so they use a dedicated
    // response channel served by its own select arm below.
    let (config_response_tx, mut config_response_rx) = mpsc::channel::<ControlF2C>(16);
    let mut control_tasks = tokio::task::JoinSet::new();

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
                        Some(control_c2f::Msg::ReaderControlRequest(request)) => {
                            let stream_id = request.stream_id.clone();
                            let request_id = request.request_id.clone();
                            let reader_control = Arc::clone(&reader_control);
                            let control_response_tx = control_response_tx.clone();
                            control_tasks.spawn(async move {
                                let mut response = reader_control.handle(request).await;
                                response.stream_id = stream_id;
                                response.request_id = request_id;
                                let _ = control_response_tx.send(response).await;
                            });
                        }
                        Some(control_c2f::Msg::ConfigGetRequest(request)) => {
                            let remote_config = Arc::clone(&remote_config);
                            let config_response_tx = config_response_tx.clone();
                            control_tasks.spawn(async move {
                                let response = remote_config.get_config(request).await;
                                let frame = ControlF2C {
                                    msg: Some(control_f2c::Msg::ConfigGetResponse(response)),
                                };
                                let _ = config_response_tx.send(frame).await;
                            });
                        }
                        Some(control_c2f::Msg::ConfigSetRequest(request)) => {
                            let remote_config = Arc::clone(&remote_config);
                            let config_response_tx = config_response_tx.clone();
                            control_tasks.spawn(async move {
                                let response = remote_config.set_config(request).await;
                                let frame = ControlF2C {
                                    msg: Some(control_f2c::Msg::ConfigSetResponse(response)),
                                };
                                let _ = config_response_tx.send(frame).await;
                            });
                        }
                        Some(control_c2f::Msg::RestartRequest(request)) => {
                            let remote_config = Arc::clone(&remote_config);
                            let config_response_tx = config_response_tx.clone();
                            control_tasks.spawn(async move {
                                let response = remote_config.restart(request).await;
                                let frame = ControlF2C {
                                    msg: Some(control_f2c::Msg::RestartResponse(response)),
                                };
                                let _ = config_response_tx.send(frame).await;
                            });
                        }
                        Some(control_c2f::Msg::Hello(_)) | None => {}
                    },
                    None => break Ok(()),
                }
            }
            response = control_response_rx.recv() => {
                match response {
                    Some(response) => {
                        let frame = ControlF2C {
                            msg: Some(control_f2c::Msg::ReaderControlResponse(response)),
                        };
                        if write_frame(&mut send, &frame).await.is_err() {
                            break Ok(());
                        }
                    }
                    None => break Ok(()),
                }
            }
            frame = config_response_rx.recv() => {
                match frame {
                    Some(frame) => {
                        if write_frame(&mut send, &frame).await.is_err() {
                            break Ok(());
                        }
                    }
                    None => break Ok(()),
                }
            }
            event = recv_control_event(&mut outbound_events) => {
                match event {
                    Some(event) => {
                        let frame = event.into_frame();
                        if write_frame(&mut send, &frame).await.is_err() {
                            break Ok(());
                        }
                    }
                    None => outbound_events = None,
                }
            }
            _ = control_tasks.join_next(), if !control_tasks.is_empty() => {}
        }
    };

    reader.abort();
    control_tasks.abort_all();
    result
}

async fn recv_control_event(events: &mut Option<ControlEventReceiver>) -> Option<ControlEvent> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
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
    use rt_p2p_protocol::{ReaderControlRequest, StreamEntry, control_f2c};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::oneshot;
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
        spawn_forwarder_with_control(
            seed,
            catalog,
            handshake_timeout,
            heartbeat,
            Arc::new(NoopReaderControlHandler),
            None,
            Arc::new(NoopRemoteConfigHandler),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn spawn_forwarder_with_control(
        seed: [u8; 32],
        catalog: StaticCatalog,
        handshake_timeout: Duration,
        heartbeat: HeartbeatConfig,
        handler: Arc<dyn ReaderControlHandler>,
        outbound_events: Option<ControlEventReceiver>,
        remote_config: Arc<dyn RemoteConfigHandler>,
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
            let result = serve_control_with_typed_control(
                &connection,
                &catalog,
                handshake_timeout,
                heartbeat,
                handler,
                outbound_events,
                remote_config,
            )
            .await;
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

    #[derive(Debug)]
    struct EchoControlHandler;

    impl ReaderControlHandler for EchoControlHandler {
        fn handle(&self, request: ReaderControlRequest) -> ReaderControlFuture<'_> {
            Box::pin(async move {
                rt_p2p_protocol::ReaderControlResponse {
                    stream_id: vec![0],
                    request_id: "handler-local-id".to_owned(),
                    success: true,
                    message: format!("handled {}", request.command),
                }
            })
        }
    }

    #[derive(Debug)]
    struct SlowControlHandler {
        started_tx: Mutex<Option<oneshot::Sender<()>>>,
        release_rx: Mutex<Option<oneshot::Receiver<()>>>,
    }

    impl ReaderControlHandler for SlowControlHandler {
        fn handle(&self, request: ReaderControlRequest) -> ReaderControlFuture<'_> {
            let started_tx = self.started_tx.lock().unwrap().take();
            let release_rx = self.release_rx.lock().unwrap().take();
            Box::pin(async move {
                if let Some(started_tx) = started_tx {
                    let _ = started_tx.send(());
                }
                if let Some(release_rx) = release_rx {
                    let _ = release_rx.await;
                }
                rt_p2p_protocol::ReaderControlResponse {
                    stream_id: vec![0],
                    request_id: "handler-local-slow-id".to_owned(),
                    success: true,
                    message: format!("handled {}", request.command),
                }
            })
        }
    }

    #[derive(Debug)]
    struct FakeSyncClockSource {
        record_drift_calls: AtomicUsize,
        set_date_time_calls: AtomicUsize,
    }

    impl SyncClockSource for FakeSyncClockSource {
        fn record_clock_drift(&self) -> SyncClockFuture<'_> {
            Box::pin(async move {
                self.record_drift_calls.fetch_add(1, Ordering::SeqCst);
                Ok(crate::reader_control::ClockInfo {
                    reader_clock: "2026-06-16T12:00:00.000".to_owned(),
                    drift_ms: 125,
                })
            })
        }

        fn set_date_time(&self) -> RewriteClockFuture<'_> {
            Box::pin(async move {
                self.set_date_time_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    /// A configurable in-memory remote-config handler for exercising the
    /// control-loop wiring without touching a TOML file. Gating mirrors the
    /// production handler: when `allow` is false every verb is rejected.
    #[derive(Debug)]
    struct FakeRemoteConfigHandler {
        allow: bool,
        config_json: String,
        restart_needed: bool,
        last_set: Mutex<Option<String>>,
        restart_calls: AtomicUsize,
    }

    impl FakeRemoteConfigHandler {
        fn new(allow: bool) -> Self {
            Self {
                allow,
                config_json: r#"{"schema_version":1}"#.to_owned(),
                restart_needed: false,
                last_set: Mutex::new(None),
                restart_calls: AtomicUsize::new(0),
            }
        }
    }

    impl RemoteConfigHandler for FakeRemoteConfigHandler {
        fn allow_remote_config(&self) -> bool {
            self.allow
        }

        fn get_config(&self, request: ConfigGetRequest) -> ConfigGetFuture<'_> {
            Box::pin(async move {
                if !self.allow {
                    return ConfigGetResponse {
                        request_id: request.request_id,
                        config_json: String::new(),
                        restart_needed: false,
                    };
                }
                ConfigGetResponse {
                    request_id: request.request_id,
                    config_json: self.config_json.clone(),
                    restart_needed: self.restart_needed,
                }
            })
        }

        fn set_config(&self, request: ConfigSetRequest) -> ConfigSetFuture<'_> {
            Box::pin(async move {
                if !self.allow {
                    return ConfigSetResponse {
                        request_id: request.request_id,
                        ok: false,
                        restart_needed: false,
                        error: REMOTE_CONFIG_DISABLED.to_owned(),
                    };
                }
                *self.last_set.lock().unwrap() = Some(request.config_json.clone());
                ConfigSetResponse {
                    request_id: request.request_id,
                    ok: true,
                    restart_needed: true,
                    error: String::new(),
                }
            })
        }

        fn restart(&self, request: RestartRequest) -> RestartFuture<'_> {
            Box::pin(async move {
                if !self.allow {
                    return RestartResponse {
                        request_id: request.request_id,
                        accepted: false,
                        error: REMOTE_CONFIG_DISABLED.to_owned(),
                    };
                }
                self.restart_calls.fetch_add(1, Ordering::SeqCst);
                RestartResponse {
                    request_id: request.request_id,
                    accepted: true,
                    error: String::new(),
                }
            })
        }
    }

    /// Builds a client `Hello` that advertises remote-config support so the
    /// negotiated capability set can include [`CAP_REMOTE_CONFIG`].
    fn hello_with_remote_config() -> Hello {
        let mut hello = forwarder_hello();
        hello.capabilities.push(CAP_REMOTE_CONFIG.to_owned());
        hello
    }

    async fn spawn_forwarder_with_remote_config(
        seed: [u8; 32],
        remote_config: Arc<dyn RemoteConfigHandler>,
    ) -> Result<(Endpoint, NodeAddr, JoinHandle<Result<(), String>>), BoxError> {
        spawn_forwarder_with_control(
            seed,
            StaticCatalog::new(sample_catalog()),
            LONG_HANDSHAKE,
            quiet_heartbeat(),
            Arc::new(NoopReaderControlHandler),
            None,
            remote_config,
        )
        .await
    }

    #[tokio::test]
    async fn remote_config_capability_advertised_when_enabled() -> TestResult {
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_remote_config(
            [60; 32],
            Arc::new(FakeRemoteConfigHandler::new(true)),
        )
        .await?;

        let receiver = EndpointBuilder::test([61; 32]).bind().await?;
        let (connection, _send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, hello_with_remote_config()),
        )
        .await??;

        let hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        match hello_ok.msg {
            Some(control_f2c::Msg::HelloOk(ok)) => {
                assert!(
                    rt_p2p_protocol::has_capability(&ok.capabilities, CAP_REMOTE_CONFIG),
                    "remote-config capability must be advertised when enabled"
                );
            }
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn remote_config_capability_absent_when_disabled() -> TestResult {
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_remote_config(
            [62; 32],
            Arc::new(FakeRemoteConfigHandler::new(false)),
        )
        .await?;

        let receiver = EndpointBuilder::test([63; 32]).bind().await?;
        let (connection, _send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, hello_with_remote_config()),
        )
        .await??;

        let hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        match hello_ok.msg {
            Some(control_f2c::Msg::HelloOk(ok)) => {
                assert!(
                    !rt_p2p_protocol::has_capability(&ok.capabilities, CAP_REMOTE_CONFIG),
                    "remote-config capability must NOT be advertised when disabled"
                );
            }
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn config_get_returns_config_json() -> TestResult {
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_remote_config(
            [64; 32],
            Arc::new(FakeRemoteConfigHandler::new(true)),
        )
        .await?;

        let receiver = EndpointBuilder::test([65; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, hello_with_remote_config()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::ConfigGetRequest(ConfigGetRequest {
                    request_id: "get-1".to_owned(),
                })),
            },
        )
        .await?;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ConfigGetResponse(response)) => {
                assert_eq!(response.request_id, "get-1");
                assert!(
                    !response.config_json.is_empty(),
                    "config_json must be non-empty when remote config is enabled"
                );
            }
            other => return Err(format!("expected ConfigGetResponse, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn config_set_roundtrip_reports_restart_needed() -> TestResult {
        let handler = Arc::new(FakeRemoteConfigHandler::new(true));
        let (forwarder, forwarder_addr, handle) =
            spawn_forwarder_with_remote_config([66; 32], Arc::clone(&handler) as _).await?;

        let receiver = EndpointBuilder::test([67; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, hello_with_remote_config()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        let payload = r#"{"schema_version":1,"display_name":"Edited"}"#;
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::ConfigSetRequest(ConfigSetRequest {
                    request_id: "set-1".to_owned(),
                    config_json: payload.to_owned(),
                })),
            },
        )
        .await?;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ConfigSetResponse(response)) => {
                assert_eq!(response.request_id, "set-1");
                assert!(response.ok, "set should succeed when enabled");
                assert!(response.restart_needed);
                assert!(response.error.is_empty());
            }
            other => return Err(format!("expected ConfigSetResponse, got {other:?}").into()),
        }
        assert_eq!(
            handler.last_set.lock().unwrap().as_deref(),
            Some(payload),
            "handler must receive the config_json sent over the wire"
        );

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn config_set_rejected_when_disabled() -> TestResult {
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_remote_config(
            [68; 32],
            Arc::new(FakeRemoteConfigHandler::new(false)),
        )
        .await?;

        let receiver = EndpointBuilder::test([69; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, hello_with_remote_config()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::ConfigSetRequest(ConfigSetRequest {
                    request_id: "set-2".to_owned(),
                    config_json: r#"{"schema_version":1}"#.to_owned(),
                })),
            },
        )
        .await?;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ConfigSetResponse(response)) => {
                assert_eq!(response.request_id, "set-2");
                assert!(!response.ok, "set must be rejected when disabled");
                assert_eq!(response.error, REMOTE_CONFIG_DISABLED);
            }
            other => return Err(format!("expected ConfigSetResponse, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn restart_request_accepted_when_enabled() -> TestResult {
        let handler = Arc::new(FakeRemoteConfigHandler::new(true));
        let (forwarder, forwarder_addr, handle) =
            spawn_forwarder_with_remote_config([70; 32], Arc::clone(&handler) as _).await?;

        let receiver = EndpointBuilder::test([71; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, hello_with_remote_config()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::RestartRequest(RestartRequest {
                    request_id: "restart-1".to_owned(),
                })),
            },
        )
        .await?;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::RestartResponse(response)) => {
                assert_eq!(response.request_id, "restart-1");
                assert!(response.accepted, "restart should be accepted when enabled");
                assert!(response.error.is_empty());
            }
            other => return Err(format!("expected RestartResponse, got {other:?}").into()),
        }
        assert_eq!(handler.restart_calls.load(Ordering::SeqCst), 1);

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn restart_request_rejected_when_disabled() -> TestResult {
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_remote_config(
            [72; 32],
            Arc::new(FakeRemoteConfigHandler::new(false)),
        )
        .await?;

        let receiver = EndpointBuilder::test([73; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, hello_with_remote_config()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::RestartRequest(RestartRequest {
                    request_id: "restart-2".to_owned(),
                })),
            },
        )
        .await?;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::RestartResponse(response)) => {
                assert_eq!(response.request_id, "restart-2");
                assert!(!response.accepted, "restart must be rejected when disabled");
                assert_eq!(response.error, REMOTE_CONFIG_DISABLED);
            }
            other => return Err(format!("expected RestartResponse, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
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
    async fn reader_control_roundtrip() -> TestResult {
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_control(
            [46; 32],
            StaticCatalog::new(sample_catalog()),
            LONG_HANDSHAKE,
            quiet_heartbeat(),
            Arc::new(EchoControlHandler),
            None,
            Arc::new(NoopRemoteConfigHandler),
        )
        .await?;

        let receiver = EndpointBuilder::test([47; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, forwarder_hello()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        let stream_id = vec![9u8; 16];
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::ReaderControlRequest(
                    ReaderControlRequest {
                        stream_id: stream_id.clone(),
                        command: "refresh".to_owned(),
                        request_id: "req-1".to_owned(),
                    },
                )),
            },
        )
        .await?;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ReaderControlResponse(response)) => {
                assert_eq!(response.stream_id, stream_id);
                assert_eq!(response.request_id, "req-1");
                assert!(response.success);
                assert_eq!(response.message, "handled refresh");
            }
            other => return Err(format!("expected ReaderControlResponse, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn reader_control_handler_does_not_block_outbound_events() -> TestResult {
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (event_tx, outbound_events) = control_event_channel(4);
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_control(
            [48; 32],
            StaticCatalog::new(sample_catalog()),
            LONG_HANDSHAKE,
            quiet_heartbeat(),
            Arc::new(SlowControlHandler {
                started_tx: Mutex::new(Some(started_tx)),
                release_rx: Mutex::new(Some(release_rx)),
            }),
            Some(outbound_events),
            Arc::new(NoopRemoteConfigHandler),
        )
        .await?;

        let receiver = EndpointBuilder::test([49; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, forwarder_hello()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        let stream_id = vec![8u8; 16];
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::ReaderControlRequest(
                    ReaderControlRequest {
                        stream_id,
                        command: "refresh".to_owned(),
                        request_id: "slow-1".to_owned(),
                    },
                )),
            },
        )
        .await?;
        tokio::time::timeout(LONG_HANDSHAKE, started_rx).await??;

        event_tx
            .send(ControlEvent::ReaderStatus(rt_p2p_protocol::ReaderStatus {
                stream_id: vec![7u8; 16],
                connected: true,
                state: "online".to_owned(),
                last_read_unix_ms: 1_700_000_000_125,
            }))
            .await
            .expect("control event receiver alive");

        let frame = tokio::time::timeout(
            Duration::from_millis(250),
            read_frame::<ControlF2C>(&mut recv),
        )
        .await??;
        match frame.msg {
            Some(control_f2c::Msg::ReaderStatus(status)) => {
                assert_eq!(status.stream_id, vec![7u8; 16]);
                assert!(status.connected);
            }
            other => {
                return Err(format!(
                    "expected ReaderStatus while handler is pending, got {other:?}"
                )
                .into());
            }
        }

        release_tx.send(()).expect("handler still pending");
        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ReaderControlResponse(response)) => {
                assert_eq!(response.request_id, "slow-1");
                assert!(response.success);
                assert_eq!(response.message, "handled refresh");
            }
            other => return Err(format!("expected ReaderControlResponse, got {other:?}").into()),
        }

        connection.close(0u32.into(), b"done");
        handle.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn sync_clock_records_drift_not_rewrite() -> TestResult {
        let clock_source = Arc::new(FakeSyncClockSource {
            record_drift_calls: AtomicUsize::new(0),
            set_date_time_calls: AtomicUsize::new(0),
        });
        let (event_tx, outbound_events) = control_event_channel(4);
        let (forwarder, forwarder_addr, handle) = spawn_forwarder_with_control(
            [50; 32],
            StaticCatalog::new(sample_catalog()),
            LONG_HANDSHAKE,
            quiet_heartbeat(),
            Arc::new(SyncClockDriftHandler::new(Arc::clone(&clock_source))),
            Some(outbound_events),
            Arc::new(NoopRemoteConfigHandler),
        )
        .await?;

        let receiver = EndpointBuilder::test([51; 32]).bind().await?;
        let (connection, mut send, mut recv) = tokio::time::timeout(
            LONG_HANDSHAKE,
            open_control(&receiver, forwarder_addr, forwarder_hello()),
        )
        .await??;

        let _hello_ok = read_frame::<ControlF2C>(&mut recv).await?;
        let _catalog = read_frame::<ControlF2C>(&mut recv).await?;

        event_tx
            .send(ControlEvent::SyncClock(rt_p2p_protocol::SyncClock {
                server_unix_ms: 1_700_000_000_125,
            }))
            .await
            .expect("control event receiver alive");

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::SyncClock(sync)) => {
                assert_eq!(sync.server_unix_ms, 1_700_000_000_125);
            }
            other => return Err(format!("expected SyncClock, got {other:?}").into()),
        }

        let stream_id = vec![8u8; 16];
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::ReaderControlRequest(
                    ReaderControlRequest {
                        stream_id: stream_id.clone(),
                        command: "sync_clock".to_owned(),
                        request_id: "sync-1".to_owned(),
                    },
                )),
            },
        )
        .await?;

        let frame =
            tokio::time::timeout(LONG_HANDSHAKE, read_frame::<ControlF2C>(&mut recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ReaderControlResponse(response)) => {
                assert_eq!(response.stream_id, stream_id);
                assert_eq!(response.request_id, "sync-1");
                assert!(response.success);
                assert_eq!(response.message, "clock drift recorded: 125ms");
            }
            other => return Err(format!("expected ReaderControlResponse, got {other:?}").into()),
        }

        assert_eq!(clock_source.record_drift_calls.load(Ordering::SeqCst), 1);
        assert_eq!(clock_source.set_date_time_calls.load(Ordering::SeqCst), 0);

        connection.close(0u32.into(), b"done");
        handle.abort();
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

//! Forwarder iroh endpoint and inbound accept loop.
//!
//! [`P2pEndpoint`] wraps an [`rt_iroh::Endpoint`] and serves inbound receiver
//! connections. For each accepted connection the remote node id is read from
//! the transport handshake and checked against an [`AllowList`]; unknown peers
//! are closed immediately. Admitted peers are handed to the control-stream
//! handler ([`crate::p2p::control`]), which performs the `Hello`/`HelloOk`
//! negotiation, serves the [`StreamCatalog`](rt_p2p_protocol::StreamCatalog),
//! and runs the heartbeat until the peer disconnects or is declared dead.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rt_iroh::{Connection, Endpoint, EndpointBuilder, NodeAddr, NodeId, load_or_create_secret_key};
use rt_p2p_protocol::{CAP_CONTROL_EVENTS, has_capability};
use tokio::sync::{Mutex, broadcast, mpsc};

use crate::status_http::{
    ForwarderStatusEvent, ForwarderStatusFeed, ForwarderStatusSnapshot, ReaderConnectionState,
    ReaderStatus, UpsStatusState,
};
use crate::storage::journal::Journal;

use super::allowlist::AllowList;
use super::control::{
    CatalogProvider, ControlEvent, ControlEventSender, HeartbeatConfig, NoopReaderControlHandler,
    NoopRemoteConfigHandler, RemoteConfigHandler, control_event_channel, negotiate_control_stream,
    run_control_stream_loop,
};
use super::data::{DataConfig, serve_data_streams};

/// QUIC application error code used when closing a rejected connection.
const REJECT_ERROR_CODE: u32 = 1;

/// QUIC application error code used when closing a connection whose control
/// stream failed, timed out, or whose peer was declared dead by the heartbeat.
const CONTROL_ERROR_CODE: u32 = 2;

/// Maximum time an admitted peer is given to complete the control-plane
/// `Hello` handshake before the connection is closed. Bounds the lifetime of
/// broken or malicious allow-listed peers that connect but never make progress.
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-connection buffer for outbound status events waiting on the control loop.
const CONTROL_EVENT_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone)]
struct ConnectionConfig {
    data_config: DataConfig,
    status_feed: Option<ForwarderStatusFeed>,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
    remote_config: Arc<dyn RemoteConfigHandler>,
    reader_control: Arc<dyn super::control::ReaderControlHandler>,
}

/// The forwarder's P2P endpoint and accept loop.
#[derive(Clone)]
pub struct P2pEndpoint {
    endpoint: Endpoint,
    allow_list: AllowList,
    catalog: Arc<dyn CatalogProvider>,
    journal: Arc<Mutex<Journal>>,
    data_config: DataConfig,
    status_feed: Option<ForwarderStatusFeed>,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
    remote_config: Arc<dyn RemoteConfigHandler>,
    reader_control: Arc<dyn super::control::ReaderControlHandler>,
    #[cfg(test)]
    _test_tempdir: Option<Arc<tempfile::TempDir>>,
}

impl std::fmt::Debug for P2pEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P2pEndpoint")
            .field("endpoint", &self.endpoint)
            .field("allow_list", &self.allow_list)
            .field("catalog", &self.catalog)
            .field("data_config", &self.data_config)
            .field("status_feed", &self.status_feed.is_some())
            .field("handshake_timeout", &self.handshake_timeout)
            .field("heartbeat", &self.heartbeat)
            .field("remote_config", &self.remote_config)
            .field("reader_control", &self.reader_control)
            .finish_non_exhaustive()
    }
}

impl P2pEndpoint {
    /// Binds the endpoint, loading or creating the persistent secret key at
    /// `secret_key_path`. Admitted peers are served the catalog from `catalog`.
    pub async fn bind(
        secret_key_path: impl AsRef<Path>,
        allow_list: AllowList,
        catalog: Arc<dyn CatalogProvider>,
        journal: Arc<Mutex<Journal>>,
    ) -> Result<Self, rt_iroh::Error> {
        let secret_key = load_or_create_secret_key(secret_key_path)?;
        Self::bind_with_builder(
            EndpointBuilder::default().secret_key(secret_key),
            allow_list,
            catalog,
            journal,
            DataConfig::default(),
        )
        .await
    }

    /// Binds an endpoint from an explicit builder. Production startup uses this
    /// to apply loopback/test transport knobs from config without exposing raw
    /// iroh APIs to the binary.
    pub async fn bind_with_builder(
        builder: EndpointBuilder,
        allow_list: AllowList,
        catalog: Arc<dyn CatalogProvider>,
        journal: Arc<Mutex<Journal>>,
        data_config: DataConfig,
    ) -> Result<Self, rt_iroh::Error> {
        let endpoint = builder.bind().await?;
        Ok(Self {
            endpoint,
            allow_list,
            catalog,
            journal,
            data_config,
            status_feed: None,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            heartbeat: HeartbeatConfig::default(),
            remote_config: Arc::new(NoopRemoteConfigHandler),
            reader_control: Arc::new(NoopReaderControlHandler),
            #[cfg(test)]
            _test_tempdir: None,
        })
    }

    /// Installs the forwarder status feed used to publish live control events.
    #[must_use]
    pub fn with_status_feed(mut self, status_feed: ForwarderStatusFeed) -> Self {
        self.status_feed = Some(status_feed);
        self
    }

    /// Installs the remote-config handler that serves config get/set/restart
    /// verbs on the control plane and drives `CAP_REMOTE_CONFIG` advertisement.
    #[must_use]
    pub fn with_remote_config(mut self, remote_config: Arc<dyn RemoteConfigHandler>) -> Self {
        self.remote_config = remote_config;
        self
    }

    /// Installs the reader-control handler that serves reader-specific verbs
    /// on the control plane and drives `CAP_READER_CONTROL` advertisement.
    #[must_use]
    pub fn with_reader_control(
        mut self,
        reader_control: Arc<dyn super::control::ReaderControlHandler>,
    ) -> Self {
        self.reader_control = reader_control;
        self
    }

    /// The underlying iroh endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// This endpoint's dialable address.
    pub async fn node_addr(&self) -> NodeAddr {
        self.endpoint.node_addr().await
    }

    /// This endpoint's node id.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.endpoint.node_id()
    }

    /// Runs the accept loop until the endpoint is closed.
    ///
    /// Each inbound connection is handled on its own task so a slow or
    /// misbehaving peer cannot stall admission of others.
    pub async fn run(&self) {
        loop {
            match self.endpoint.accept().await {
                Ok(Some(connection)) => {
                    let allow_list = self.allow_list.clone();
                    let catalog = Arc::clone(&self.catalog);
                    let journal = Arc::clone(&self.journal);
                    let config = ConnectionConfig {
                        data_config: self.data_config,
                        status_feed: self.status_feed.clone(),
                        handshake_timeout: self.handshake_timeout,
                        heartbeat: self.heartbeat,
                        remote_config: Arc::clone(&self.remote_config),
                        reader_control: Arc::clone(&self.reader_control),
                    };
                    tokio::spawn(async move {
                        handle_connection(connection, allow_list, catalog, journal, config).await;
                    });
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "p2p: failed to accept inbound connection");
                }
            }
        }
    }
}

/// Admits or rejects a single inbound connection based on the allow-list, then
/// negotiates the control stream and only afterwards serves data streams.
///
/// Stream ordering contract: the receiver must open the control stream as the
/// first bidirectional stream on the connection. The forwarder accepts that
/// first stream as control, completes `Hello`/catalog negotiation, and only
/// then begins accepting subsequent bidirectional streams as data subscriptions.
/// Data is never served before negotiation succeeds.
///
/// Lifecycle: any control-loop termination (clean close, handshake/heartbeat
/// failure) closes the QUIC connection, which ends the data accept loop and
/// every in-flight data stream. The data task is awaited before returning so no
/// data stream outlives the allow-list connection guard (`_guard`).
async fn handle_connection(
    connection: Connection,
    allow_list: AllowList,
    catalog: Arc<dyn CatalogProvider>,
    journal: Arc<Mutex<Journal>>,
    config: ConnectionConfig,
) {
    let Ok(node_id) = connection.remote_node_id() else {
        tracing::warn!("p2p: rejecting connection without a remote node id");
        connection.close(REJECT_ERROR_CODE.into(), b"missing remote node id");
        return;
    };

    let Some(_guard) = allow_list.try_register_connection(node_id, connection.clone()) else {
        tracing::warn!(%node_id, "p2p: rejecting peer not on allow-list");
        connection.close(REJECT_ERROR_CODE.into(), b"unauthorized peer");
        return;
    };

    tracing::info!(%node_id, "p2p: admitted allow-listed peer");
    let (control_send, control_recv) =
        match tokio::time::timeout(config.handshake_timeout, connection.accept_bi()).await {
            Ok(Ok(streams)) => streams,
            Ok(Err(error)) => {
                tracing::warn!(%node_id, %error, "p2p: failed to accept control stream");
                connection.close(CONTROL_ERROR_CODE.into(), b"control stream failed");
                return;
            }
            Err(_elapsed) => {
                tracing::warn!(%node_id, "p2p: control stream accept timed out");
                connection.close(CONTROL_ERROR_CODE.into(), b"control stream timed out");
                return;
            }
        };

    // Gate data delivery on a completed control handshake: negotiate Hello and
    // serve the catalog before any data stream is accepted.
    let (control_send, control_recv, capabilities) = match negotiate_control_stream(
        control_send,
        control_recv,
        catalog.as_ref(),
        config.handshake_timeout,
        config.heartbeat,
        config.remote_config.as_ref(),
        config.reader_control.as_ref(),
    )
    .await
    {
        Ok(streams) => streams,
        Err(error) => {
            tracing::warn!(%node_id, %error, "p2p: control handshake failed");
            connection.close(CONTROL_ERROR_CODE.into(), b"control handshake failed");
            return;
        }
    };

    // Handshake succeeded: now safe to serve data subscriptions concurrently
    // with the heartbeat/control loop.
    let data_connection = connection.clone();
    let data_task = tokio::spawn(async move {
        if let Err(error) = serve_data_streams(
            data_connection,
            journal,
            node_id.to_string(),
            config.data_config,
        )
        .await
        {
            tracing::debug!(%node_id, %error, "p2p: data stream accept loop ended");
        }
    });

    let (outbound_events, status_bridge_task) = if has_capability(&capabilities, CAP_CONTROL_EVENTS)
    {
        config.status_feed.map_or((None, None), |feed| {
            let (tx, rx) = control_event_channel(CONTROL_EVENT_CHANNEL_CAPACITY);
            let task = tokio::spawn(bridge_status_feed_to_control(feed, tx));
            (Some(rx), Some(task))
        })
    } else {
        (None, None)
    };

    let control_result = run_control_stream_loop(
        control_send,
        control_recv,
        config.heartbeat,
        outbound_events,
        config.reader_control,
        config.remote_config,
    )
    .await;
    if let Some(task) = status_bridge_task {
        task.abort();
    }
    match &control_result {
        Ok(()) => tracing::info!(%node_id, "p2p: control stream closed by peer"),
        Err(error) => tracing::warn!(%node_id, %error, "p2p: control stream failed"),
    }

    // Any control-loop exit closes the connection, which ends the data accept
    // loop and all in-flight data streams. Await the data task so no data stream
    // outlives the control lifecycle or the allow-list guard dropped below.
    connection.close(CONTROL_ERROR_CODE.into(), b"control stream closed");
    let _ = data_task.await;
}

async fn bridge_status_feed_to_control(feed: ForwarderStatusFeed, tx: ControlEventSender) {
    // Subscribe and snapshot atomically so the initial snapshot and the delta
    // stream cannot overlap: deltas are strictly those emitted after the
    // snapshot was taken.
    let (mut status_rx, snapshot) = feed.subscribe_and_snapshot().await;
    if !publish_status_snapshot(&snapshot, &tx) {
        return;
    }

    loop {
        match status_rx.recv().await {
            Ok(event) => {
                if !publish_status_event(&tx, event) {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    skipped,
                    "p2p: status event receiver lagged; dropping updates"
                );
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn publish_status_snapshot(snapshot: &ForwarderStatusSnapshot, tx: &ControlEventSender) -> bool {
    for (stream_id, status) in &snapshot.readers {
        if !try_send_control_event(tx, reader_status_event(stream_id, status)) {
            return false;
        }
        if let Some(info) = &status.reader_info
            && !try_send_control_event(tx, reader_info_event(stream_id, info))
        {
            return false;
        }
    }
    let Some(ups_status) = &snapshot.ups_status else {
        return true;
    };
    ups_status_event(ups_status).is_none_or(|event| try_send_control_event(tx, event))
}

fn publish_status_event(tx: &ControlEventSender, event: ForwarderStatusEvent) -> bool {
    match event {
        ForwarderStatusEvent::ReaderStatus { stream_id, status } => {
            try_send_control_event(tx, reader_status_event(&stream_id, &status))
        }
        ForwarderStatusEvent::ReaderInfo { stream_id, info } => {
            try_send_control_event(tx, reader_info_event(&stream_id, &info))
        }
        ForwarderStatusEvent::DownloadProgress { stream_id, event } => {
            try_send_control_event(tx, download_progress_event(&stream_id, &event))
        }
        ForwarderStatusEvent::UpsStatus(status) => {
            ups_status_event(&status).is_none_or(|event| try_send_control_event(tx, event))
        }
    }
}

fn try_send_control_event(tx: &ControlEventSender, event: ControlEvent) -> bool {
    match tx.try_send(event) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_event)) => {
            tracing::warn!("p2p: outbound control-event channel full; dropping status update");
            true
        }
        Err(mpsc::error::TrySendError::Closed(_event)) => false,
    }
}

fn reader_status_event(stream_id: &str, status: &ReaderStatus) -> ControlEvent {
    ControlEvent::ReaderStatus(rt_p2p_protocol::ReaderStatus {
        stream_id: stream_id.as_bytes().to_vec(),
        connected: status.state == ReaderConnectionState::Connected,
        state: reader_state_token(&status.state).to_owned(),
        last_read_unix_ms: last_read_unix_ms(status.last_seen),
    })
}

fn reader_info_event(stream_id: &str, info: &crate::reader_control::ReaderInfo) -> ControlEvent {
    let domain_info = crate::reader_control_service::native_info_to_domain(info);
    match super::reader_control::domain_info_to_p2p_event(stream_id.as_bytes(), domain_info) {
        Ok(info) => ControlEvent::ReaderInfo(info),
        Err(error) => {
            tracing::warn!(%error, "failed to serialize reader info for p2p event");
            let hardware = info.hardware.as_ref();
            ControlEvent::ReaderInfo(rt_p2p_protocol::ReaderInfo {
                stream_id: stream_id.as_bytes().to_vec(),
                hardware_reader_id: hardware
                    .map_or_else(String::new, |hardware| hardware.reader_id.to_string()),
                firmware_version: hardware
                    .map_or_else(String::new, |hardware| hardware.fw_version.clone()),
                model: hardware.map_or_else(String::new, |hardware| hardware.hw_code.to_string()),
                reader_info_json: None,
            })
        }
    }
}

fn download_progress_event(
    stream_id: &str,
    event: &crate::reader_control::DownloadEvent,
) -> ControlEvent {
    let (state, reads_received, progress, total, error) = match event {
        crate::reader_control::DownloadEvent::Downloading {
            progress,
            total,
            reads_received,
        } => (
            "downloading".to_owned(),
            *reads_received,
            u64::from(*progress),
            u64::from(*total),
            String::new(),
        ),
        crate::reader_control::DownloadEvent::Complete { reads_received } => {
            ("complete".to_owned(), *reads_received, 0, 0, String::new())
        }
        crate::reader_control::DownloadEvent::Error { message } => {
            ("error".to_owned(), 0, 0, 0, message.clone())
        }
        crate::reader_control::DownloadEvent::Idle => ("idle".to_owned(), 0, 0, 0, String::new()),
    };
    ControlEvent::DownloadProgress(rt_p2p_protocol::DownloadProgress {
        stream_id: stream_id.as_bytes().to_vec(),
        downloaded_bytes: progress,
        total_bytes: total,
        state,
        reads_received,
        progress,
        total,
        error,
    })
}

fn ups_status_event(status: &UpsStatusState) -> Option<ControlEvent> {
    let status = status.status.as_ref()?;
    Some(ControlEvent::UpsStatus(rt_p2p_protocol::UpsStatus {
        on_battery: !status.power_plugged,
        battery_percent: u32::from(status.battery_percent),
        runtime_seconds: 0,
    }))
}

fn reader_state_token(state: &ReaderConnectionState) -> &'static str {
    match state {
        ReaderConnectionState::Connecting => "connecting",
        ReaderConnectionState::Connected => "connected",
        ReaderConnectionState::Disconnected => "disconnected",
    }
}

fn last_read_unix_ms(last_seen: Option<std::time::Instant>) -> i64 {
    last_seen
        .and_then(|instant| SystemTime::now().checked_sub(instant.elapsed()))
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::p2p::control::{
        PROTOCOL_MINOR, StaticCatalog, forwarder_hello, read_frame, write_frame,
    };
    use crate::status_http::{ReaderConnectionState, StatusConfig, StatusServer, SubsystemStatus};
    use rt_p2p_protocol::{
        CAP_CONTROL_EVENTS, ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, HelloOk,
        StreamCatalog, SubscribeMode, control_c2f, control_f2c, data_c2f, data_f2c, has_capability,
    };

    type BoxError = Box<dyn std::error::Error + Send + Sync>;
    type TestResult = Result<(), BoxError>;

    /// Reader-address journal key seeded into every test endpoint's journal so
    /// data subscriptions have stream state to resolve against.
    const DATA_STREAM_KEY: &str = "10.0.0.7:10000";

    fn empty_catalog() -> Arc<dyn CatalogProvider> {
        Arc::new(StaticCatalog::new(StreamCatalog::default()))
    }

    impl P2pEndpoint {
        /// Binds a hermetic loopback endpoint seeded with `seed` for tests.
        async fn bind_test(seed: [u8; 32], allow_list: AllowList) -> Result<Self, rt_iroh::Error> {
            let endpoint = EndpointBuilder::test(seed).bind().await?;
            let tempdir = Arc::new(tempfile::tempdir().expect("tempdir"));
            let journal_path = tempdir.path().join("journal.sqlite3");
            let mut journal = Journal::open(&journal_path).expect("open journal");
            journal
                .ensure_stream_state(DATA_STREAM_KEY, 1)
                .expect("seed data stream state");
            Ok(Self {
                endpoint,
                allow_list,
                catalog: empty_catalog(),
                journal: Arc::new(Mutex::new(journal)),
                data_config: DataConfig::default(),
                status_feed: None,
                handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
                heartbeat: HeartbeatConfig::default(),
                remote_config: Arc::new(NoopRemoteConfigHandler),
                reader_control: Arc::new(NoopReaderControlHandler),
                _test_tempdir: Some(tempdir),
            })
        }

        /// Overrides the handshake timeout, keeping stalled-peer tests fast and
        /// deterministic.
        fn with_handshake_timeout(mut self, handshake_timeout: Duration) -> Self {
            self.handshake_timeout = handshake_timeout;
            self
        }
    }

    /// Dials `forwarder_addr`, opens the control stream, sends `Hello`, and
    /// returns the negotiated `HelloOk`.
    async fn dial_hello(
        receiver: &Endpoint,
        forwarder_addr: NodeAddr,
    ) -> Result<HelloOk, BoxError> {
        let (_connection, hello_ok, _control_send, _control_recv) =
            dial_hello_connection(receiver, forwarder_addr).await?;
        Ok(hello_ok)
    }

    async fn dial_hello_connection(
        receiver: &Endpoint,
        forwarder_addr: NodeAddr,
    ) -> Result<
        (
            Connection,
            HelloOk,
            rt_iroh::SendStream,
            rt_iroh::RecvStream,
        ),
        BoxError,
    > {
        dial_hello_connection_with_capabilities(
            receiver,
            forwarder_addr,
            vec![CAP_CONTROL_EVENTS.to_owned()],
        )
        .await
    }

    async fn dial_hello_connection_with_capabilities(
        receiver: &Endpoint,
        forwarder_addr: NodeAddr,
        capabilities: Vec<String>,
    ) -> Result<
        (
            Connection,
            HelloOk,
            rt_iroh::SendStream,
            rt_iroh::RecvStream,
        ),
        BoxError,
    > {
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;

        let mut hello = forwarder_hello();
        hello.capabilities = capabilities;

        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(hello)),
            },
        )
        .await?;

        let response = read_frame::<ControlF2C>(&mut recv).await?;
        let hello_ok = match response.msg {
            Some(control_f2c::Msg::HelloOk(hello_ok)) => hello_ok,
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        };
        let catalog = read_frame::<ControlF2C>(&mut recv).await?;
        match catalog.msg {
            Some(control_f2c::Msg::StreamCatalog(_)) => Ok((connection, hello_ok, send, recv)),
            other => Err(format!("expected StreamCatalog, got {other:?}").into()),
        }
    }

    async fn status_server_with_reader(
        reader_state: ReaderConnectionState,
    ) -> Result<StatusServer, BoxError> {
        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await?;
        status
            .init_readers(&[(DATA_STREAM_KEY.to_owned(), 10000)])
            .await;
        status
            .update_reader_state(DATA_STREAM_KEY, reader_state)
            .await;
        Ok(status)
    }

    async fn next_reader_status(
        recv: &mut rt_iroh::RecvStream,
    ) -> Result<rt_p2p_protocol::ReaderStatus, BoxError> {
        let frame =
            tokio::time::timeout(Duration::from_secs(5), read_frame::<ControlF2C>(recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ReaderStatus(status)) => Ok(status),
            other => Err(format!("expected ReaderStatus, got {other:?}").into()),
        }
    }

    async fn next_reader_info(
        recv: &mut rt_iroh::RecvStream,
    ) -> Result<rt_p2p_protocol::ReaderInfo, BoxError> {
        let frame =
            tokio::time::timeout(Duration::from_secs(5), read_frame::<ControlF2C>(recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::ReaderInfo(info)) => Ok(info),
            other => Err(format!("expected ReaderInfo, got {other:?}").into()),
        }
    }

    async fn next_ups_status(
        recv: &mut rt_iroh::RecvStream,
    ) -> Result<rt_p2p_protocol::UpsStatus, BoxError> {
        let frame =
            tokio::time::timeout(Duration::from_secs(5), read_frame::<ControlF2C>(recv)).await??;
        match frame.msg {
            Some(control_f2c::Msg::UpsStatus(status)) => Ok(status),
            other => Err(format!("expected UpsStatus, got {other:?}").into()),
        }
    }

    async fn subscribe_data(connection: &Connection) -> Result<DataF2C, BoxError> {
        let (send, mut recv) = open_data_subscribe(connection).await?;
        drop(send);
        tokio::time::timeout(Duration::from_secs(1), read_frame::<DataF2C>(&mut recv)).await?
    }

    /// Opens a data stream and sends a `DataSubscribe` for the seeded stream,
    /// returning the stream halves so callers can keep them open.
    async fn open_data_subscribe(
        connection: &Connection,
    ) -> Result<(rt_iroh::SendStream, rt_iroh::RecvStream), BoxError> {
        let (mut send, recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id: DATA_STREAM_KEY.as_bytes().to_vec(),
                    after_seq: 0,
                    mode: SubscribeMode::Replay as i32,
                })),
            },
        )
        .await?;
        Ok((send, recv))
    }

    #[tokio::test]
    async fn accepts_allowlisted_peer() -> TestResult {
        let receiver = EndpointBuilder::test([20; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([21; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let hello_ok = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello(&receiver, forwarder_addr),
        )
        .await??;
        assert_eq!(hello_ok.protocol_minor, PROTOCOL_MINOR);

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn remote_config_get_roundtrips_over_p2p() -> TestResult {
        use rt_p2p_protocol::{CAP_REMOTE_CONFIG, ConfigGetRequest, Pong};

        let receiver = EndpointBuilder::test([80; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);

        // Seed a minimal valid config (with a real token file) so the handler
        // can serialize it.
        let dir = tempfile::tempdir()?;
        let token_path = dir.path().join("token");
        std::fs::write(&token_path, "tok\n")?;
        let config_path = dir.path().join("forwarder.toml");
        std::fs::write(
            &config_path,
            format!(
                "schema_version = 1\n\n[auth]\ntoken_file = '{}'\n\n[[readers]]\ntarget = \"192.168.1.100\"\n\n[control]\nallow_remote_config = true\n",
                token_path.display()
            ),
        )?;

        let (ui_tx, _ui_rx) = broadcast::channel(16);
        let handler = Arc::new(crate::p2p::ForwarderRemoteConfigHandler::new(
            true,
            Arc::new(crate::status_http::ConfigState::new(config_path)),
            Arc::new(Mutex::new(SubsystemStatus::ready())),
            ui_tx,
            Arc::new(tokio::sync::Notify::new()),
        ));

        let forwarder = P2pEndpoint::bind_test([81; 32], allow_list)
            .await?
            .with_remote_config(handler);
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (connection, hello_ok, mut send, mut recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection_with_capabilities(
                &receiver,
                forwarder_addr,
                vec![CAP_CONTROL_EVENTS.to_owned(), CAP_REMOTE_CONFIG.to_owned()],
            ),
        )
        .await??;
        assert!(
            has_capability(&hello_ok.capabilities, CAP_REMOTE_CONFIG),
            "forwarder must advertise remote-config when enabled"
        );

        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::ConfigGetRequest(ConfigGetRequest {
                    request_id: "e2e".to_owned(),
                })),
            },
        )
        .await?;

        // Answer any heartbeat pings while waiting for the config response.
        let response = loop {
            let frame =
                tokio::time::timeout(Duration::from_secs(5), read_frame::<ControlF2C>(&mut recv))
                    .await??;
            match frame.msg {
                Some(control_f2c::Msg::ConfigGetResponse(response)) => break response,
                Some(control_f2c::Msg::Ping(ping)) => {
                    write_frame(
                        &mut send,
                        &ControlC2F {
                            msg: Some(control_c2f::Msg::Pong(Pong { nonce: ping.nonce })),
                        },
                    )
                    .await?;
                }
                other => return Err(format!("unexpected control frame: {other:?}").into()),
            }
        };

        assert_eq!(response.request_id, "e2e");
        let value: serde_json::Value = serde_json::from_str(&response.config_json)?;
        assert_eq!(value["schema_version"], serde_json::json!(1));

        connection.close(0u32.into(), b"done");
        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn serves_data_streams_after_control_handshake_on_same_connection() -> TestResult {
        let receiver = EndpointBuilder::test([26; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([27; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (connection, hello_ok, _control_send, _control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection(&receiver, forwarder_addr),
        )
        .await??;
        assert_eq!(hello_ok.protocol_minor, PROTOCOL_MINOR);

        let frame = subscribe_data(&connection).await?;
        match frame.msg {
            Some(data_f2c::Msg::SubscribeOk(_)) => {}
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        }

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn data_not_served_before_control_handshake() -> TestResult {
        let receiver = EndpointBuilder::test([28; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([29; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;

        // Open the control stream first (per the ordering contract) but do NOT
        // send Hello yet, so negotiation cannot complete.
        let (mut control_send, mut control_recv) = connection.open_bi().await?;

        // Open a data stream and subscribe before negotiating control.
        let (_data_send, mut data_recv) = open_data_subscribe(&connection).await?;

        // The forwarder must not serve data before the handshake: no SubscribeOk
        // (or any data frame) should arrive while control is un-negotiated.
        let premature = tokio::time::timeout(
            Duration::from_millis(750),
            read_frame::<DataF2C>(&mut data_recv),
        )
        .await;
        assert!(
            premature.is_err(),
            "data must not be served before the control handshake completes, got {premature:?}"
        );

        // Now negotiate control: send Hello, read HelloOk + catalog.
        write_frame(
            &mut control_send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(forwarder_hello())),
            },
        )
        .await?;
        match read_frame::<ControlF2C>(&mut control_recv).await?.msg {
            Some(control_f2c::Msg::HelloOk(_)) => {}
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        }
        match read_frame::<ControlF2C>(&mut control_recv).await?.msg {
            Some(control_f2c::Msg::StreamCatalog(_)) => {}
            other => return Err(format!("expected StreamCatalog, got {other:?}").into()),
        }

        // Post-negotiation, the already-open data subscription must now be served.
        let frame = tokio::time::timeout(
            Duration::from_secs(5),
            read_frame::<DataF2C>(&mut data_recv),
        )
        .await??;
        match frame.msg {
            Some(data_f2c::Msg::SubscribeOk(_)) => {}
            other => {
                return Err(format!("expected SubscribeOk after handshake, got {other:?}").into());
            }
        }

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn data_stream_terminates_when_control_stream_closes() -> TestResult {
        let receiver = EndpointBuilder::test([30; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([31; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (connection, hello_ok, control_send, control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection(&receiver, forwarder_addr),
        )
        .await??;
        assert_eq!(hello_ok.protocol_minor, PROTOCOL_MINOR);

        // Establish a live data subscription.
        let (_data_send, mut data_recv) = open_data_subscribe(&connection).await?;
        match tokio::time::timeout(
            Duration::from_secs(5),
            read_frame::<DataF2C>(&mut data_recv),
        )
        .await??
        .msg
        {
            Some(data_f2c::Msg::SubscribeOk(_)) => {}
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        }

        // Close the control stream cleanly. The forwarder's control loop should
        // observe the close, tear down the connection, and end the data stream.
        drop(control_send);
        drop(control_recv);

        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if read_frame::<DataF2C>(&mut data_recv).await.is_err() {
                    break;
                }
            }
        })
        .await;
        assert!(
            closed.is_ok(),
            "data stream must terminate once the control stream closes"
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn control_events_capability_sends_initial_snapshot_and_reader_deltas() -> TestResult {
        let receiver = EndpointBuilder::test([32; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let status = status_server_with_reader(ReaderConnectionState::Connected).await?;
        let forwarder = P2pEndpoint::bind_test([33; 32], allow_list)
            .await?
            .with_status_feed(status.status_feed());
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (_connection, hello_ok, _control_send, mut control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection(&receiver, forwarder_addr),
        )
        .await??;
        assert!(has_capability(&hello_ok.capabilities, CAP_CONTROL_EVENTS));

        let snapshot = next_reader_status(&mut control_recv).await?;
        assert_eq!(snapshot.stream_id, DATA_STREAM_KEY.as_bytes());
        assert!(
            snapshot.connected,
            "initial snapshot should reflect connected reader"
        );
        assert_eq!(snapshot.state, "connected");

        status
            .update_reader_state(DATA_STREAM_KEY, ReaderConnectionState::Disconnected)
            .await;
        let delta = next_reader_status(&mut control_recv).await?;
        assert_eq!(delta.stream_id, DATA_STREAM_KEY.as_bytes());
        assert!(
            !delta.connected,
            "reader disconnect delta should be delivered"
        );
        assert_eq!(delta.state, "disconnected");

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn control_events_capability_sends_reader_info_frame() -> TestResult {
        let receiver = EndpointBuilder::test([60; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let status = status_server_with_reader(ReaderConnectionState::Connected).await?;
        status
            .update_reader_info(
                DATA_STREAM_KEY,
                crate::reader_control::ReaderInfo {
                    hardware: Some(crate::reader_control::HardwareInfo {
                        fw_version: "1.2.3".to_owned(),
                        hw_code: 42,
                        reader_id: 7,
                        config3: 0,
                    }),
                    ..Default::default()
                },
            )
            .await;
        let forwarder = P2pEndpoint::bind_test([61; 32], allow_list)
            .await?
            .with_status_feed(status.status_feed());
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (_connection, hello_ok, _control_send, mut control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection(&receiver, forwarder_addr),
        )
        .await??;
        assert!(has_capability(&hello_ok.capabilities, CAP_CONTROL_EVENTS));

        // The snapshot publishes the ReaderStatus first, then the ReaderInfo.
        let _reader_status = next_reader_status(&mut control_recv).await?;
        let info = next_reader_info(&mut control_recv).await?;
        assert_eq!(info.stream_id, DATA_STREAM_KEY.as_bytes());
        assert_eq!(info.hardware_reader_id, "7");
        assert_eq!(info.firmware_version, "1.2.3");
        assert_eq!(info.model, "42");
        let rich: rt_domain::ReaderInfo = serde_json::from_str(
            info.reader_info_json
                .as_deref()
                .expect("rich reader info json"),
        )?;
        assert_eq!(
            rich.hardware.and_then(|hardware| hardware.reader_id),
            Some("7".to_owned())
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[test]
    fn download_progress_status_event_maps_to_control_event() {
        let event = download_progress_event(
            DATA_STREAM_KEY,
            &crate::reader_control::DownloadEvent::Downloading {
                progress: 11,
                total: 99,
                reads_received: 3,
            },
        );

        let ControlEvent::DownloadProgress(progress) = event else {
            panic!("expected download progress event");
        };
        assert_eq!(progress.stream_id, DATA_STREAM_KEY.as_bytes());
        assert_eq!(progress.state, "downloading");
        assert_eq!(progress.reads_received, 3);
        assert_eq!(progress.progress, 11);
        assert_eq!(progress.total, 99);
        assert_eq!(progress.downloaded_bytes, 11);
        assert_eq!(progress.total_bytes, 99);
    }

    #[tokio::test]
    async fn control_events_capability_sends_ups_status_frame() -> TestResult {
        let receiver = EndpointBuilder::test([62; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let status = status_server_with_reader(ReaderConnectionState::Connected).await?;
        status
            .set_ups_status(UpsStatusState {
                available: true,
                status: Some(rt_domain::UpsStatus {
                    battery_percent: 88,
                    battery_voltage_mv: 4100,
                    charging: false,
                    power_plugged: false,
                    temperature_cdeg: 2500,
                    sampled_at: 1_700_000_000,
                }),
            })
            .await;
        let forwarder = P2pEndpoint::bind_test([63; 32], allow_list)
            .await?
            .with_status_feed(status.status_feed());
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (_connection, hello_ok, _control_send, mut control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection(&receiver, forwarder_addr),
        )
        .await??;
        assert!(has_capability(&hello_ok.capabilities, CAP_CONTROL_EVENTS));

        // The snapshot publishes the ReaderStatus (no ReaderInfo set) first,
        // then the UpsStatus.
        let _reader_status = next_reader_status(&mut control_recv).await?;
        let ups = next_ups_status(&mut control_recv).await?;
        assert!(
            ups.on_battery,
            "power_plugged=false maps to on_battery=true"
        );
        assert_eq!(ups.battery_percent, 88);
        assert_eq!(ups.runtime_seconds, 0);

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn control_events_not_sent_without_negotiated_capability() -> TestResult {
        let receiver = EndpointBuilder::test([34; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let status = status_server_with_reader(ReaderConnectionState::Connected).await?;
        let forwarder = P2pEndpoint::bind_test([35; 32], allow_list)
            .await?
            .with_status_feed(status.status_feed());
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (_connection, hello_ok, _control_send, mut control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection_with_capabilities(&receiver, forwarder_addr, Vec::new()),
        )
        .await??;
        assert!(!has_capability(&hello_ok.capabilities, CAP_CONTROL_EVENTS));

        status
            .update_reader_state(DATA_STREAM_KEY, ReaderConnectionState::Disconnected)
            .await;
        let unsolicited = tokio::time::timeout(
            Duration::from_millis(300),
            read_frame::<ControlF2C>(&mut control_recv),
        )
        .await;
        assert!(
            unsolicited.is_err(),
            "control events must not be sent unless both peers negotiate the capability"
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn stalled_handshake_is_timed_out() -> TestResult {
        let receiver = EndpointBuilder::test([24; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([25; 32], allow_list)
            .await?
            .with_handshake_timeout(Duration::from_millis(200));
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        // Connect and open the control stream as an allow-listed peer, but never
        // send `Hello`. The forwarder must close the connection on its own once
        // the handshake deadline elapses rather than leaking the task/connection.
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;
        let (mut _send, mut recv) = connection.open_bi().await?;

        // A read that would otherwise block forever must observe the connection
        // being closed promptly after the (short) handshake timeout.
        let mut buf = [0u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(5), recv.read_exact(&mut buf)).await;
        assert!(
            closed.is_ok(),
            "stalled handshake must be closed before the outer timeout elapses"
        );
        assert!(
            closed.unwrap().is_err(),
            "forwarder must close the stream after the handshake timeout, not deliver data"
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unknown_peer() -> TestResult {
        let receiver = EndpointBuilder::test([22; 32]).bind().await?;
        let forwarder = P2pEndpoint::bind_test([23; 32], AllowList::default()).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello(&receiver, forwarder_addr),
        )
        .await?;
        assert!(
            result.is_err(),
            "unknown peer must be rejected, got {result:?}"
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }
}

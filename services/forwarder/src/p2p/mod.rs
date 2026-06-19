//! Production forwarder peer-to-peer (P2P) transport.
//!
//! This module owns the forwarder's [`rt_iroh`] endpoint and the accept loop
//! that admits inbound receiver connections. Admission is gated by a persistent
//! [`AllowList`] keyed on the remote peer's iroh node id (the transport-layer
//! `EndpointId`): connections from peers that are not on the allow-list are
//! closed before any control-plane work happens. The allow-list caches its set
//! on disk (fail-to-last-known on refresh failures) and force-closes a peer's
//! open connections when an update revokes it.
//!
//! Scope: production startup wires the endpoint, accept loop, allow-listed
//! control-plane handshake, data-stream subscriber handler, server allow-list
//! distribution components ([`ServerAllowListClient`] and
//! [`run_allowlist_distribution`]), and forwarder status events. Reader control
//! actions are still handled by the no-op adapter until a production adapter is
//! installed.

mod allowlist;
mod control;
mod data;
mod endpoint;

use std::net::SocketAddrV4;
use std::num::TryFromIntError;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crate::config::P2pConfig;
use crate::status_http::ForwarderStatusFeed;
use crate::storage::journal::Journal;
use rt_iroh::{EndpointBuilder, NodeAddr, NodeId, RelayMode, SecretKey};
use rt_p2p_protocol::{StreamCatalog, StreamEntry};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

pub use allowlist::{
    AllowList, AllowListRefreshError, CatalogPushError, DEFAULT_ALLOWLIST_POLL_INTERVAL,
    ForwarderCatalog, ForwarderCatalogStream, ReceiverAllowListUpdate, ServerAllowListClient,
    ServerCatalogClient, apply_receiver_update, fetch_and_apply_once, run_allowlist_distribution,
};
pub use control::{
    CatalogProvider, ControlEvent, ControlEventReceiver, ControlEventSender, HeartbeatConfig,
    NoopReaderControlHandler, ReaderControlFuture, ReaderControlHandler, RewriteClockFuture,
    StaticCatalog, SyncClockDriftHandler, SyncClockFuture, SyncClockSource, control_event_channel,
};
pub use data::{DataConfig, serve_data_streams};
pub use endpoint::P2pEndpoint;

const DEFAULT_P2P_SECRET_KEY_PATH: &str = "/var/lib/rusty-timer/p2p-secret.key";
const DEFAULT_FORWARDER_CATALOG_PUSH_INTERVAL: Duration = Duration::from_secs(30);

/// Running forwarder P2P server tasks.
#[derive(Debug)]
pub struct P2pRuntime {
    endpoint: P2pEndpoint,
    tasks: Vec<JoinHandle<()>>,
}

impl P2pRuntime {
    /// This forwarder's dialable iroh node address.
    pub async fn node_addr(&self) -> NodeAddr {
        self.endpoint.node_addr().await
    }

    /// This forwarder's iroh node id / endpoint id.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.endpoint.node_id()
    }

    /// Closes the endpoint and aborts background P2P tasks.
    pub async fn shutdown(mut self) {
        self.endpoint.endpoint().close().await;
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
    }
}

/// Starts the forwarder P2P endpoint when enabled in config.
///
/// `reader_streams` are the durable journal stream ids to advertise in the
/// control catalog. The current forwarder journal keys reader streams by their
/// network address (for example `10.0.0.5:10000`), so catalog stream ids are the
/// UTF-8 bytes of those same keys and data subscriptions resolve back to the
/// existing journal rows.
pub async fn start_forwarder_p2p(
    config: &P2pConfig,
    journal: Arc<Mutex<Journal>>,
    reader_streams: &[String],
    display_name: Option<String>,
    status_feed: ForwarderStatusFeed,
) -> Result<Option<P2pRuntime>, P2pStartError> {
    if !config.enabled {
        return Ok(None);
    }

    let allow_list = build_allow_list(config)?;

    // Initialize durable stream state for every advertised reader before serving
    // the catalog. Data subscriptions require `stream_retention` rows; without
    // this a freshly configured reader could be advertised and subscribed before
    // its reader task has appended anything, and the subscription would fail on
    // missing stream state instead of returning SubscribeOk/CaughtUp.
    {
        let mut journal = journal.lock().await;
        for stream in reader_streams {
            journal.ensure_stream_state(stream, 1)?;
        }
    }

    let catalog = Arc::new(ReaderCatalog::new(reader_streams));
    let endpoint = P2pEndpoint::bind_with_builder(
        endpoint_builder(config)?,
        allow_list.clone(),
        catalog,
        Arc::clone(&journal),
        DataConfig::default(),
    )
    .await?
    .with_status_feed(status_feed);

    let run_endpoint = endpoint.clone();
    let mut tasks = vec![tokio::spawn(async move { run_endpoint.run().await })];

    if let Some((base_url, bearer_token)) = server_credentials(config)? {
        // Allow-list freshness is polling-only for now: the server has no
        // server-push channel wired yet, so we hand `run_allowlist_distribution`
        // a receiver whose sender is dropped immediately. The distribution loop
        // treats the closed push channel as "no pushes" and relies on its
        // periodic poll backstop. When a push transport is added, replace this
        // dropped sender with the real one.
        let (push_tx, push_rx) = mpsc::channel(16);
        drop(push_tx);
        let request_timeout = Duration::from_secs(config.allowlist_request_timeout_secs);
        let poll_interval = Duration::from_secs(config.allowlist_poll_interval_secs);
        tasks.push(tokio::spawn(run_allowlist_distribution(
            allow_list,
            ServerAllowListClient::with_timeout(
                base_url.clone(),
                bearer_token.clone(),
                request_timeout,
            ),
            push_rx,
            poll_interval,
        )));

        tasks.push(tokio::spawn(run_forwarder_catalog_distribution(
            ServerCatalogClient::with_timeout(base_url, bearer_token, request_timeout),
            endpoint.clone(),
            display_name,
            Arc::clone(&journal),
            reader_streams.to_vec(),
            DEFAULT_FORWARDER_CATALOG_PUSH_INTERVAL,
        )));
    }

    Ok(Some(P2pRuntime { endpoint, tasks }))
}

#[derive(Debug, thiserror::Error)]
pub enum P2pStartError {
    #[error("p2p is enabled but no allow-list source is configured")]
    MissingAllowList,
    #[error("p2p server URL and token file must be configured together")]
    IncompleteServerConfig,
    #[error("invalid p2p receiver node id '{value}': {source}")]
    InvalidReceiverNodeId {
        value: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("invalid p2p secret key seed")]
    InvalidSecretKeySeed,
    #[error("invalid p2p bind_addr_v4")]
    InvalidBindAddr(#[from] std::net::AddrParseError),
    #[error("p2p io error")]
    Io(#[from] std::io::Error),
    #[error("p2p endpoint error")]
    Iroh(#[from] rt_iroh::Error),
    #[error("failed to initialize p2p stream state")]
    Journal(#[from] crate::storage::journal::JournalError),
    #[error("p2p stream catalog value out of range")]
    CatalogValueOutOfRange(#[from] TryFromIntError),
}

#[derive(Debug)]
struct ReaderCatalog {
    catalog: StreamCatalog,
}

impl ReaderCatalog {
    fn new(reader_streams: &[String]) -> Self {
        Self {
            catalog: StreamCatalog {
                generation: 1,
                entries: reader_streams
                    .iter()
                    .map(|stream| StreamEntry {
                        stream_id: stream.as_bytes().to_vec(),
                        display_name: stream.clone(),
                        network_addr: stream.clone(),
                        reader_connected: true,
                        hardware_reader_id: stream.clone(),
                    })
                    .collect(),
            },
        }
    }
}

impl CatalogProvider for ReaderCatalog {
    fn catalog(&self) -> StreamCatalog {
        self.catalog.clone()
    }
}

fn endpoint_builder(config: &P2pConfig) -> Result<EndpointBuilder, P2pStartError> {
    let secret_key = match config.secret_key_seed_hex.as_deref() {
        Some(seed) => SecretKey::from_bytes(&decode_seed(seed)?),
        None => rt_iroh::load_or_create_secret_key(
            config
                .secret_key_path
                .as_deref()
                .unwrap_or(DEFAULT_P2P_SECRET_KEY_PATH),
        )?,
    };

    let mut builder = EndpointBuilder::default()
        .secret_key(secret_key)
        .bind_addr_v4(config.bind_addr_v4.parse::<SocketAddrV4>()?);
    if config.relay_disabled {
        builder = builder.relay_mode(RelayMode::Disabled);
    }
    if config.discovery_disabled {
        builder = builder.clear_discovery();
    }
    if let Some(max_streams) = config.max_concurrent_bidi_streams {
        builder = builder.max_concurrent_bidi_streams(max_streams);
    }
    Ok(builder)
}

fn build_allow_list(config: &P2pConfig) -> Result<AllowList, P2pStartError> {
    if config.static_allowed_receivers.is_empty()
        && config.allowlist_cache_path.is_none()
        && config.server_url.is_none()
    {
        return Err(P2pStartError::MissingAllowList);
    }

    let static_receivers = parse_node_ids(&config.static_allowed_receivers)?;
    let allow_list = match &config.allowlist_cache_path {
        Some(path) => {
            // Static receivers are additive: union them on top of the cached
            // last-known set rather than replacing it (which would revoke every
            // cached receiver). `add_allowed` does not persist or revoke.
            let allow_list = AllowList::load(path)?;
            allow_list.add_allowed(static_receivers);
            allow_list
        }
        None => AllowList::new(static_receivers),
    };
    Ok(allow_list)
}

fn server_credentials(config: &P2pConfig) -> Result<Option<(String, String)>, P2pStartError> {
    match (&config.server_url, &config.server_token_file) {
        (Some(url), Some(token_file)) => {
            let token = std::fs::read_to_string(Path::new(token_file))?;
            Ok(Some((url.clone(), token.trim().to_owned())))
        }
        (None, None) => Ok(None),
        _ => Err(P2pStartError::IncompleteServerConfig),
    }
}

async fn run_forwarder_catalog_distribution(
    client: ServerCatalogClient,
    endpoint: P2pEndpoint,
    display_name: Option<String>,
    journal: Arc<Mutex<Journal>>,
    reader_streams: Vec<String>,
    push_interval: Duration,
) {
    push_forwarder_catalog_once(
        &client,
        &endpoint,
        display_name.as_deref(),
        Arc::clone(&journal),
        &reader_streams,
    )
    .await;

    let push_interval = if push_interval.is_zero() {
        DEFAULT_FORWARDER_CATALOG_PUSH_INTERVAL
    } else {
        push_interval
    };
    let mut ticker = tokio::time::interval(push_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;

    loop {
        ticker.tick().await;
        push_forwarder_catalog_once(
            &client,
            &endpoint,
            display_name.as_deref(),
            Arc::clone(&journal),
            &reader_streams,
        )
        .await;
    }
}

async fn push_forwarder_catalog_once(
    client: &ServerCatalogClient,
    endpoint: &P2pEndpoint,
    display_name: Option<&str>,
    journal: Arc<Mutex<Journal>>,
    reader_streams: &[String],
) {
    let endpoint_id = endpoint.node_id().to_string();
    if let Err(error) = client.register_forwarder(&endpoint_id).await {
        tracing::warn!(%endpoint_id, %error, "forwarder server registration failed");
    }

    let node_addr = endpoint.node_addr().await;
    let direct_addrs = node_addr
        .direct_addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let catalog = match build_forwarder_catalog(
        &endpoint_id,
        display_name,
        &direct_addrs,
        journal,
        reader_streams,
    )
    .await
    {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(%endpoint_id, %error, "failed to build forwarder catalog push");
            return;
        }
    };

    if let Err(error) = client.push_catalog(&catalog).await {
        tracing::warn!(%endpoint_id, %error, "forwarder catalog push failed");
    }
}

async fn build_forwarder_catalog(
    endpoint_id: &str,
    display_name: Option<&str>,
    direct_addrs: &[String],
    journal: Arc<Mutex<Journal>>,
    reader_streams: &[String],
) -> Result<ForwarderCatalog, P2pStartError> {
    let mut journal = journal.lock().await;
    let mut streams = Vec::with_capacity(reader_streams.len());
    for stream_id in reader_streams {
        let (epoch, next_seq) = journal.current_epoch_and_next_seq(stream_id)?;
        streams.push(ForwarderCatalogStream {
            stream_id: stream_id.clone(),
            epoch: u64::try_from(epoch)?,
            next_seq: u64::try_from(next_seq)?,
        });
    }

    Ok(ForwarderCatalog {
        endpoint_id: endpoint_id.to_owned(),
        display_name: display_name.map(ToOwned::to_owned),
        direct_addrs: direct_addrs.to_vec(),
        streams,
    })
}

fn parse_node_ids(values: &[String]) -> Result<Vec<NodeId>, P2pStartError> {
    values
        .iter()
        .map(|value| {
            NodeId::from_str(value).map_err(|source| P2pStartError::InvalidReceiverNodeId {
                value: value.clone(),
                source: Box::new(source),
            })
        })
        .collect()
}

fn decode_seed(seed: &str) -> Result<[u8; 32], P2pStartError> {
    let mut bytes = [0_u8; 32];
    if seed.len() != 64 {
        return Err(P2pStartError::InvalidSecretKeySeed);
    }
    for (idx, chunk) in seed.as_bytes().chunks_exact(2).enumerate() {
        let hex = std::str::from_utf8(chunk).map_err(|_| P2pStartError::InvalidSecretKeySeed)?;
        bytes[idx] =
            u8::from_str_radix(hex, 16).map_err(|_| P2pStartError::InvalidSecretKeySeed)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::P2pConfig;
    use crate::status_http::{StatusConfig, StatusServer, SubsystemStatus};
    use crate::storage::journal::Journal;
    use rt_iroh::EndpointBuilder;
    use rt_p2p_protocol::{
        ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, SubscribeMode, control_c2f,
        control_f2c, data_c2f, data_f2c,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    type BoxError = Box<dyn std::error::Error + Send + Sync>;
    type TestResult = Result<(), BoxError>;

    async fn status_feed() -> Result<ForwarderStatusFeed, BoxError> {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await?;
        Ok(server.status_feed())
    }

    fn p2p_config(receiver_id: String) -> P2pConfig {
        P2pConfig {
            enabled: true,
            secret_key_path: None,
            secret_key_seed_hex: Some(
                "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a".to_owned(),
            ),
            bind_addr_v4: "127.0.0.1:0".to_owned(),
            relay_disabled: true,
            discovery_disabled: true,
            max_concurrent_bidi_streams: Some(64),
            static_allowed_receivers: vec![receiver_id],
            allowlist_cache_path: None,
            server_url: None,
            server_token_file: None,
            allowlist_poll_interval_secs: 60,
            allowlist_request_timeout_secs: 10,
        }
    }

    #[tokio::test]
    async fn startup_helper_binds_seeded_loopback_and_serves_control_plus_data() -> TestResult {
        let receiver = EndpointBuilder::test([43; 32]).bind().await?;
        let dir = tempfile::tempdir()?;
        let journal_path = dir.path().join("journal.sqlite3");
        let mut journal = Journal::open(&journal_path)?;
        let stream_key = "10.0.0.5:10000";
        journal.ensure_stream_state(stream_key, 1)?;
        journal.append_read(stream_key, Some("1000"), b"startup-record", "chip")?;
        let journal = Arc::new(Mutex::new(journal));

        let runtime = start_forwarder_p2p(
            &p2p_config(receiver.node_id().to_string()),
            Arc::clone(&journal),
            &[stream_key.to_owned()],
            None,
            status_feed().await?,
        )
        .await?
        .expect("p2p enabled");
        let forwarder_addr = runtime.node_addr().await;
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;

        let (mut control_send, mut control_recv) = connection.open_bi().await?;
        control::write_frame(
            &mut control_send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(control::forwarder_hello())),
            },
        )
        .await?;
        match control::read_frame::<ControlF2C>(&mut control_recv)
            .await?
            .msg
        {
            Some(control_f2c::Msg::HelloOk(_)) => {}
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        }
        match control::read_frame::<ControlF2C>(&mut control_recv)
            .await?
            .msg
        {
            Some(control_f2c::Msg::StreamCatalog(catalog)) => {
                assert_eq!(catalog.entries[0].stream_id, stream_key.as_bytes());
            }
            other => return Err(format!("expected StreamCatalog, got {other:?}").into()),
        }

        let (mut data_send, mut data_recv) = connection.open_bi().await?;
        control::write_frame(
            &mut data_send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id: stream_key.as_bytes().to_vec(),
                    after_seq: 0,
                    mode: SubscribeMode::Replay as i32,
                })),
            },
        )
        .await?;
        match control::read_frame::<DataF2C>(&mut data_recv).await?.msg {
            Some(data_f2c::Msg::SubscribeOk(ok)) => assert_eq!(ok.latest_seq_at_open, 1),
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        }

        runtime.shutdown().await;
        receiver.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn build_allow_list_unions_cache_and_static() -> TestResult {
        let cached = EndpointBuilder::test([60; 32]).bind().await?;
        let static_receiver = EndpointBuilder::test([61; 32]).bind().await?;

        let dir = tempfile::tempdir()?;
        let cache_path = dir.path().join("allowlist.cache");
        std::fs::write(&cache_path, format!("{}\n", cached.node_id()))?;

        let mut config = p2p_config(static_receiver.node_id().to_string());
        config.allowlist_cache_path = Some(cache_path.to_string_lossy().into_owned());

        let allow_list = build_allow_list(&config)?;
        assert!(
            allow_list.contains(&cached.node_id()),
            "cached last-known receiver must remain allowed when static receivers are configured"
        );
        assert!(
            allow_list.contains(&static_receiver.node_id()),
            "statically configured receiver must be allowed alongside the cached set"
        );

        cached.close().await;
        static_receiver.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn freshly_configured_reader_subscribable_before_any_data() -> TestResult {
        let receiver = EndpointBuilder::test([62; 32]).bind().await?;
        let dir = tempfile::tempdir()?;
        let journal_path = dir.path().join("journal.sqlite3");
        // No ensure_stream_state / append_read here: start_forwarder_p2p must
        // initialize stream state for every advertised reader so a fresh reader
        // can be subscribed before its task has appended anything.
        let journal = Arc::new(Mutex::new(Journal::open(&journal_path)?));
        let stream_key = "10.0.0.5:10000";

        let runtime = start_forwarder_p2p(
            &p2p_config(receiver.node_id().to_string()),
            Arc::clone(&journal),
            &[stream_key.to_owned()],
            None,
            status_feed().await?,
        )
        .await?
        .expect("p2p enabled");
        let forwarder_addr = runtime.node_addr().await;
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;

        let (mut control_send, mut control_recv) = connection.open_bi().await?;
        control::write_frame(
            &mut control_send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(control::forwarder_hello())),
            },
        )
        .await?;
        match control::read_frame::<ControlF2C>(&mut control_recv)
            .await?
            .msg
        {
            Some(control_f2c::Msg::HelloOk(_)) => {}
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        }
        match control::read_frame::<ControlF2C>(&mut control_recv)
            .await?
            .msg
        {
            Some(control_f2c::Msg::StreamCatalog(catalog)) => {
                assert_eq!(catalog.entries[0].stream_id, stream_key.as_bytes());
            }
            other => return Err(format!("expected StreamCatalog, got {other:?}").into()),
        }

        let (mut data_send, mut data_recv) = connection.open_bi().await?;
        control::write_frame(
            &mut data_send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id: stream_key.as_bytes().to_vec(),
                    after_seq: 0,
                    mode: SubscribeMode::Replay as i32,
                })),
            },
        )
        .await?;
        match control::read_frame::<DataF2C>(&mut data_recv).await?.msg {
            Some(data_f2c::Msg::SubscribeOk(ok)) => {
                assert_eq!(ok.earliest_available_seq, 1);
                assert_eq!(ok.latest_seq_at_open, 0);
            }
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        }
        match control::read_frame::<DataF2C>(&mut data_recv).await?.msg {
            Some(data_f2c::Msg::CaughtUp(caught_up)) => assert_eq!(caught_up.through_seq, 0),
            other => return Err(format!("expected CaughtUp, got {other:?}").into()),
        }

        runtime.shutdown().await;
        receiver.close().await;
        Ok(())
    }
}

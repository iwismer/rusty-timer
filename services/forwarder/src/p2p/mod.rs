//! Production forwarder peer-to-peer (P2P) transport.
//!
//! This module owns the forwarder's [`rt_iroh`] endpoint and the accept loop
//! that admits inbound receiver connections. Admission is gated by a persistent
//! [`AllowList`] keyed on the remote peer's iroh endpoint id (the transport-layer
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
mod reader_control;
mod remote_config;

use std::net::SocketAddrV4;
use std::num::TryFromIntError;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crate::config::P2pConfig;
use crate::status_http::ForwarderStatusFeed;
use crate::storage::journal::Journal;
use rt_iroh::{EndpointAddr, EndpointBuilder, EndpointId, RelayMode, SecretKey};
use rt_p2p_protocol::{StreamCatalog, StreamEntry};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

pub use allowlist::{
    ALLOWLIST_PUSH_HOLD, AllowList, AllowListRefreshError, CatalogPushError,
    DEFAULT_ALLOWLIST_POLL_INTERVAL, ForwarderCatalog, ForwarderCatalogStream,
    ReceiverAllowListUpdate, ServerAllowListClient, ServerCatalogClient, apply_receiver_update,
    fetch_and_apply_once, run_allowlist_distribution, run_allowlist_push_subscription,
};
pub use control::{
    CatalogProvider, ConfigGetFuture, ConfigSetFuture, ControlEvent, ControlEventReceiver,
    ControlEventSender, HeartbeatConfig, NoopReaderControlHandler, NoopRemoteConfigHandler,
    ReaderControlFuture, ReaderControlHandler, RemoteConfigHandler, RestartFuture,
    RewriteClockFuture, StaticCatalog, SyncClockDriftHandler, SyncClockFuture, SyncClockSource,
    control_event_channel,
};
pub use data::{DataConfig, serve_data_streams};
pub use endpoint::P2pEndpoint;
pub use reader_control::ForwarderReaderControlHandler;
pub use remote_config::ForwarderRemoteConfigHandler;

const DEFAULT_P2P_SECRET_KEY_PATH: &str = "/var/lib/rusty-timer/p2p-secret.key";
const DEFAULT_FORWARDER_CATALOG_PUSH_INTERVAL: Duration = Duration::from_secs(30);

/// Backoff bounds for retrying the first-boot device-token bootstrap when the
/// server is unreachable (e.g. the forwarder started before the network came up).
const BOOTSTRAP_RETRY_INITIAL: Duration = Duration::from_secs(5);
const BOOTSTRAP_RETRY_MAX: Duration = Duration::from_secs(300);

/// Running forwarder P2P server tasks.
#[derive(Debug)]
pub struct P2pRuntime {
    endpoint: P2pEndpoint,
    tasks: Vec<JoinHandle<()>>,
}

impl P2pRuntime {
    /// This forwarder's dialable iroh endpoint address.
    pub async fn endpoint_addr(&self) -> EndpointAddr {
        self.endpoint.endpoint_addr().await
    }

    /// This forwarder's iroh endpoint id / endpoint id.
    #[must_use]
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.endpoint_id()
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
#[allow(clippy::too_many_arguments)]
pub async fn start_forwarder_p2p(
    config: &P2pConfig,
    journal: Arc<Mutex<Journal>>,
    reader_streams: &[String],
    display_name: Option<String>,
    status_feed: ForwarderStatusFeed,
    remote_config: Arc<dyn RemoteConfigHandler>,
    reader_control: Arc<dyn ReaderControlHandler>,
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
    //
    // NOTE: startup stream-identity restore (main.rs) runs BEFORE this, so this
    // idempotent seq-1 seeding never pre-empts a registry high-water restore
    // for a journal-lost stream.
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
    .with_status_feed(status_feed)
    .with_remote_config(remote_config)
    .with_reader_control(reader_control);

    let run_endpoint = endpoint.clone();
    let mut tasks = vec![tokio::spawn(async move { run_endpoint.run().await })];

    if let Some((base_url, voucher)) = server_credentials(config)? {
        let request_timeout = Duration::from_secs(config.allowlist_request_timeout_secs);
        let poll_interval = Duration::from_secs(config.allowlist_poll_interval_secs);
        let endpoint_id = endpoint.endpoint_id().to_string();

        // Resolve the minted per-device token (load persisted, else bootstrap
        // via the voucher) BEFORE starting any server-facing task, so every
        // request carries the device token rather than the bootstrap voucher.
        // The first attempt runs synchronously so file I/O errors still fail
        // startup; if the server is merely unreachable (first boot before the
        // network is up), the spawned task below keeps retrying with backoff.
        let token_path = device_token_path(config);
        let bootstrap_client =
            ServerCatalogClient::with_timeout(base_url.clone(), voucher, request_timeout);
        let device_token =
            resolve_device_token(&token_path, &endpoint_id, &bootstrap_client).await?;
        if device_token.is_none() {
            tracing::warn!(
                %endpoint_id,
                "forwarder has no minted device token yet (bootstrap unavailable); \
                 retrying in the background"
            );
        }

        let server_endpoint = endpoint.clone();
        let server_journal = Arc::clone(&journal);
        let reader_streams = reader_streams.to_vec();
        // One task owns the whole server integration so aborting it on shutdown
        // stops the bootstrap retry loop and all three server-facing loops.
        tasks.push(tokio::spawn(async move {
            let device_token = match device_token {
                Some(token) => token,
                None => {
                    let mut backoff = BOOTSTRAP_RETRY_INITIAL;
                    loop {
                        tokio::time::sleep(backoff).await;
                        match resolve_device_token(&token_path, &endpoint_id, &bootstrap_client)
                            .await
                        {
                            Ok(Some(token)) => break token,
                            // Bootstrap failure (server unreachable/rejected):
                            // already logged inside resolve_device_token.
                            Ok(None) => {}
                            Err(error) => {
                                tracing::warn!(
                                    %endpoint_id, %error,
                                    "device token bootstrap retry failed"
                                );
                            }
                        }
                        backoff = (backoff * 2).min(BOOTSTRAP_RETRY_MAX);
                    }
                }
            };

            // Allow-list freshness comes from two sources feeding the same
            // channel: a long-poll push subscription (near-instant on
            // approval) and the periodic poll backstop inside
            // `run_allowlist_distribution` (covers any push gap). Both apply
            // idempotent snapshots, so overlap is harmless. A pending
            // forwarder's 401 is logged and retried by those loops.
            let (push_tx, push_rx) = mpsc::channel(16);
            let allow_list_client = ServerAllowListClient::with_timeout(
                base_url.clone(),
                device_token.clone(),
                request_timeout,
            );
            tokio::join!(
                run_allowlist_push_subscription(
                    allow_list_client.clone(),
                    push_tx,
                    ALLOWLIST_PUSH_HOLD,
                ),
                run_allowlist_distribution(allow_list, allow_list_client, push_rx, poll_interval),
                run_forwarder_catalog_distribution(
                    ServerCatalogClient::with_timeout(base_url, device_token, request_timeout),
                    server_endpoint,
                    display_name,
                    server_journal,
                    reader_streams,
                    DEFAULT_FORWARDER_CATALOG_PUSH_INTERVAL,
                ),
            );
        }));
    }

    Ok(Some(P2pRuntime { endpoint, tasks }))
}

#[derive(Debug, thiserror::Error)]
pub enum P2pStartError {
    #[error("p2p is enabled but no allow-list source is configured")]
    MissingAllowList,
    #[error("p2p server URL and token file must be configured together")]
    IncompleteServerConfig,
    #[error("invalid p2p receiver endpoint id '{value}': {source}")]
    InvalidReceiverEndpointId {
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

    let static_receivers = parse_endpoint_ids(&config.static_allowed_receivers)?;
    let allow_list = match &config.allowlist_cache_path {
        Some(path) => AllowList::load(path)?,
        None => AllowList::new(vec![]),
    };
    // Static receivers are pinned: always allowed on top of the cached
    // last-known set, never revoked or persisted by later server snapshots.
    allow_list.set_pinned(static_receivers);
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

/// Writable path for the minted per-device token: the configured
/// `device_token_file`, else a `p2p-device-token` sibling of the secret-key path.
///
/// `pub` so startup stream-identity restore (main.rs) can read the persisted
/// token before the P2P endpoint exists.
pub fn device_token_path(config: &P2pConfig) -> PathBuf {
    if let Some(path) = &config.device_token_file {
        return PathBuf::from(path);
    }
    let key_path = config
        .secret_key_path
        .as_deref()
        .unwrap_or(DEFAULT_P2P_SECRET_KEY_PATH);
    Path::new(key_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("p2p-device-token")
}

/// Whether a persistent P2P identity already exists for this config: true
/// when a deterministic seed is configured or the secret-key file is present.
///
/// `pub` so startup stream-identity restore (main.rs) can distinguish a true
/// first boot (fresh identity → the server registry cannot hold records for
/// it) from a lost device token on an established identity.
#[must_use]
pub fn persistent_identity_exists(config: &P2pConfig) -> bool {
    if config.secret_key_seed_hex.is_some() {
        return true;
    }
    Path::new(
        config
            .secret_key_path
            .as_deref()
            .unwrap_or(DEFAULT_P2P_SECRET_KEY_PATH),
    )
    .exists()
}

/// Resolve the minted per-device token: return the persisted one if present,
/// otherwise bootstrap via the voucher and persist the result.
///
/// Returns `Ok(None)` when no token is persisted and bootstrap fails (e.g. the
/// server is unreachable at first boot); the caller retries in the background
/// with backoff until a token is minted. A persist failure after a successful
/// mint is logged loudly but the in-memory token is still used for this run.
async fn resolve_device_token(
    path: &Path,
    endpoint_id: &str,
    bootstrap_client: &ServerCatalogClient,
) -> Result<Option<String>, P2pStartError> {
    if let Some(existing) = read_device_token(path)? {
        return Ok(Some(existing));
    }
    match bootstrap_client.bootstrap(endpoint_id).await {
        Ok(minted) => {
            if let Err(error) = write_device_token(path, &minted) {
                tracing::error!(
                    %endpoint_id, %error,
                    "failed to persist minted device token; will re-bootstrap on next start"
                );
            }
            Ok(Some(minted))
        }
        Err(error) => {
            tracing::warn!(%endpoint_id, %error, "forwarder bootstrap failed");
            Ok(None)
        }
    }
}

/// Read the persisted minted device token, if any. `pub` for startup
/// stream-identity restore; it never bootstraps (that needs the endpoint id).
pub fn read_device_token(path: &Path) -> Result<Option<String>, P2pStartError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let trimmed = contents.trim();
            Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(P2pStartError::Io(error)),
    }
}

/// Persist `token` to `path` via write-then-rename so a crash never leaves a
/// partially written credential.
fn write_device_token(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(token.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
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
    let endpoint_id = endpoint.endpoint_id().to_string();
    let endpoint_addr = endpoint.endpoint_addr().await;
    let direct_addrs = endpoint_addr
        .ip_addrs()
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

fn parse_endpoint_ids(values: &[String]) -> Result<Vec<EndpointId>, P2pStartError> {
    values
        .iter()
        .map(|value| {
            EndpointId::from_str(value).map_err(|source| P2pStartError::InvalidReceiverEndpointId {
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
            device_token_file: None,
            allowlist_poll_interval_secs: 60,
            allowlist_request_timeout_secs: 10,
        }
    }

    #[test]
    fn device_token_path_prefers_explicit_then_secret_key_sibling() {
        let mut config = p2p_config("a".repeat(64));
        config.device_token_file = Some("/tmp/explicit-device-token".to_owned());
        assert_eq!(
            device_token_path(&config),
            PathBuf::from("/tmp/explicit-device-token")
        );

        config.device_token_file = None;
        config.secret_key_path = Some("/var/lib/rt/p2p-secret.key".to_owned());
        assert_eq!(
            device_token_path(&config),
            PathBuf::from("/var/lib/rt/p2p-device-token")
        );
    }

    #[test]
    fn read_write_device_token_roundtrip() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("nested/p2p-device-token");
        assert!(read_device_token(&path)?.is_none());
        write_device_token(&path, "rtk_abc_def")?;
        assert_eq!(read_device_token(&path)?.as_deref(), Some("rtk_abc_def"));
        Ok(())
    }

    #[tokio::test]
    async fn resolve_device_token_returns_persisted_without_contacting_server() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("p2p-device-token");
        write_device_token(&path, "rtk_persisted_token")?;
        // Unreachable server: a persisted token must short-circuit before any call.
        let client = ServerCatalogClient::new("http://127.0.0.1:1", "unused-voucher");
        let token = resolve_device_token(&path, "ep-1", &client).await?;
        assert_eq!(token.as_deref(), Some("rtk_persisted_token"));
        Ok(())
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
            &p2p_config(receiver.endpoint_id().to_string()),
            Arc::clone(&journal),
            &[stream_key.to_owned()],
            None,
            status_feed().await?,
            Arc::new(NoopRemoteConfigHandler),
            Arc::new(NoopReaderControlHandler),
        )
        .await?
        .expect("p2p enabled");
        let forwarder_addr = runtime.endpoint_addr().await;
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
        std::fs::write(&cache_path, format!("{}\n", cached.endpoint_id()))?;

        let mut config = p2p_config(static_receiver.endpoint_id().to_string());
        config.allowlist_cache_path = Some(cache_path.to_string_lossy().into_owned());

        let allow_list = build_allow_list(&config)?;
        assert!(
            allow_list.contains(&cached.endpoint_id()),
            "cached last-known receiver must remain allowed when static receivers are configured"
        );
        assert!(
            allow_list.contains(&static_receiver.endpoint_id()),
            "statically configured receiver must be allowed alongside the cached set"
        );

        // Regression guard: the first server sync (an empty snapshot here) must
        // never revoke the statically configured receiver.
        let revoked = allow_list.apply_update([])?;
        assert!(
            allow_list.contains(&static_receiver.endpoint_id()),
            "statically configured receiver must survive a server allow-list sync"
        );
        assert!(
            !revoked.contains(&static_receiver.endpoint_id()),
            "statically configured receiver must never be revoked by a server sync"
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
            &p2p_config(receiver.endpoint_id().to_string()),
            Arc::clone(&journal),
            &[stream_key.to_owned()],
            None,
            status_feed().await?,
            Arc::new(NoopRemoteConfigHandler),
            Arc::new(NoopReaderControlHandler),
        )
        .await?
        .expect("p2p enabled");
        let forwarder_addr = runtime.endpoint_addr().await;
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

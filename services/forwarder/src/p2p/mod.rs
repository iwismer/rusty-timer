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
//! [`run_allowlist_distribution`]), and forwarder status events. Reader
//! control actions are served by [`ForwarderReaderControlHandler`], gated by
//! the negotiated `CAP_READER_CONTROL` capability and
//! `control.allow_reader_control`.

mod allowlist;
mod control;
mod data;
mod endpoint;
mod reader_control;
mod remote_config;
mod server_client;

use std::net::SocketAddrV4;
use std::num::TryFromIntError;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crate::config::P2pConfig;
use crate::status_store::{
    ForwarderStatusEvent, ForwarderStatusFeed, ForwarderStatusSnapshot, ReaderConnectionState,
};
use crate::storage::journal::Journal;
use rt_iroh::{EndpointAddr, EndpointBuilder, EndpointId, RelayMode, SecretKey};
use rt_p2p_protocol::{StreamCatalog, StreamEntry, StreamEpochSummary};
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

pub use allowlist::AllowList;
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
pub use server_client::{
    ALLOWLIST_PUSH_HOLD, AllowListRefreshError, CatalogPushError, DEFAULT_ALLOWLIST_POLL_INTERVAL,
    ForwarderCatalog, ForwarderCatalogStream, ReceiverAllowListUpdate, ServerAllowListClient,
    ServerCatalogClient, apply_receiver_update, fetch_and_apply_once, run_allowlist_distribution,
    run_allowlist_push_subscription,
};

const DEFAULT_P2P_SECRET_KEY_PATH: &str = "/var/lib/rusty-timer/p2p-secret.key";
const DEFAULT_FORWARDER_CATALOG_PUSH_INTERVAL: Duration = Duration::from_secs(30);
/// Backoff bounds for retrying the first-boot device-token bootstrap when the
/// server is unreachable (e.g. the forwarder started before the network came
/// up). Test builds use tiny delays so the retry loop is exercised quickly.
#[cfg(not(test))]
const BOOTSTRAP_RETRY_INITIAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const BOOTSTRAP_RETRY_INITIAL: Duration = Duration::from_millis(10);
#[cfg(not(test))]
const BOOTSTRAP_RETRY_MAX: Duration = Duration::from_secs(300);
#[cfg(test)]
const BOOTSTRAP_RETRY_MAX: Duration = Duration::from_millis(40);

/// Running forwarder P2P server tasks.
#[derive(Debug)]
pub struct P2pRuntime {
    endpoint: P2pEndpoint,
    tasks: Vec<JoinHandle<()>>,
    /// Signals the catalog-push loop to make one final registry push and
    /// acknowledge, so a graceful shutdown lands the freshest epoch/next_seq
    /// high-water on the server. `None` when no server is configured.
    catalog_shutdown: Option<(
        tokio::sync::watch::Sender<bool>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
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
    ///
    /// When a server is configured, a final catalog push runs first (bounded
    /// at 5s) so the registry restore high-water covers everything journaled
    /// this session.
    pub async fn shutdown(mut self) {
        if let Some((shutdown_tx, done_rx)) = self.catalog_shutdown.take() {
            let _ = shutdown_tx.send(true);
            if tokio::time::timeout(Duration::from_secs(5), done_rx)
                .await
                .is_err()
            {
                tracing::warn!("final forwarder catalog push did not complete within 5s");
            }
        }
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
    journal_path: &Path,
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

    // Seed the catalog from the status store's current reader states: main.rs
    // initializes reader status (init_readers) before starting P2P, so the
    // snapshot already knows every configured reader's connectivity. The
    // returned delta receiver is handed to the catalog task so no state change
    // between snapshot and task startup can be missed.
    let (status_rx, status_snapshot) = status_feed.subscribe_and_snapshot().await;
    // A second feed handle for the server catalog-push task (out-of-cadence
    // pushes on epoch advances), cloned before `status_feed` moves into the
    // endpoint below.
    let server_status_feed = status_feed.clone();
    let epoch_summaries = {
        let journal = journal.lock().await;
        reader_streams
            .iter()
            .map(|stream| {
                Ok((
                    stream.clone(),
                    journal
                        .epoch_summaries(stream)?
                        .into_iter()
                        .map(|summary| {
                            Ok(StreamEpochSummary {
                                epoch: summary.epoch,
                                created_unix_ms: summary.created_unix_ms,
                                start_seq: u64::try_from(summary.start_seq)?,
                                end_seq: summary.end_seq.map(u64::try_from).transpose()?,
                                name: summary.name,
                            })
                        })
                        .collect::<Result<Vec<_>, TryFromIntError>>()?,
                ))
            })
            .collect::<Result<std::collections::HashMap<_, _>, P2pStartError>>()?
    };
    let (catalog, catalog_task) = LiveReaderCatalog::start(
        reader_streams,
        status_feed.clone(),
        &status_snapshot,
        status_rx,
        &epoch_summaries,
    );
    let catalog = Arc::new(catalog);
    let endpoint = P2pEndpoint::bind_with_builder(
        endpoint_builder(config)?,
        allow_list.clone(),
        catalog,
        Arc::clone(&journal),
        DataConfig::default().with_read_journal_path(journal_path),
    )
    .await?
    .with_status_feed(status_feed)
    .with_remote_config(remote_config)
    .with_reader_control(reader_control);

    let run_endpoint = endpoint.clone();
    let mut tasks = vec![
        tokio::spawn(async move { run_endpoint.run().await }),
        catalog_task,
    ];

    let mut catalog_shutdown = None;
    if let Some((base_url, voucher)) = server_credentials(config)? {
        let request_timeout = Duration::from_secs(config.allowlist_request_timeout_secs);
        let poll_interval = Duration::from_secs(config.allowlist_poll_interval_secs);
        let endpoint_id = endpoint.endpoint_id().to_string();
        let (catalog_shutdown_tx, catalog_shutdown_rx) = tokio::sync::watch::channel(false);
        let (catalog_done_tx, catalog_done_rx) = tokio::sync::oneshot::channel();
        catalog_shutdown = Some((catalog_shutdown_tx, catalog_done_rx));

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
            let (server_status_rx, _server_status_snapshot) =
                server_status_feed.subscribe_and_snapshot().await;
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
                    server_status_rx,
                    catalog_shutdown_rx,
                    catalog_done_tx,
                ),
            );
        }));
    }

    Ok(Some(P2pRuntime {
        endpoint,
        tasks,
        catalog_shutdown,
    }))
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

/// Live [`CatalogProvider`]: serves the forwarder's advertised streams with
/// current per-reader connectivity.
///
/// A background task (tracked in [`P2pRuntime`] tasks) observes the status
/// feed and updates `reader_connected` per entry, bumping `generation`
/// monotonically on every real connectivity change, so a receiver that
/// (re)connects sees fresh state and a changed `HelloOk.catalog_generation`.
/// Mid-connection catalog pushes are out of scope: peers observe changes at
/// connect time only.
#[derive(Debug)]
struct LiveReaderCatalog {
    rx: watch::Receiver<StreamCatalog>,
}

impl LiveReaderCatalog {
    /// Builds the initial catalog (generation 1) seeded from `snapshot` and
    /// spawns the task that applies status-feed deltas.
    fn start(
        reader_streams: &[String],
        feed: ForwarderStatusFeed,
        snapshot: &ForwarderStatusSnapshot,
        status_rx: broadcast::Receiver<ForwarderStatusEvent>,
        epoch_summaries: &std::collections::HashMap<String, Vec<StreamEpochSummary>>,
    ) -> (Self, JoinHandle<()>) {
        let initial = StreamCatalog {
            generation: 1,
            entries: reader_streams
                .iter()
                .map(|stream| StreamEntry {
                    stream_id: stream.as_bytes().to_vec(),
                    display_name: stream.clone(),
                    network_addr: stream.clone(),
                    reader_connected: snapshot_reader_connected(snapshot, stream),
                    hardware_reader_id: stream.clone(),
                    epoch_summaries: epoch_summaries.get(stream).cloned().unwrap_or_default(),
                })
                .collect(),
        };
        let (tx, rx) = watch::channel(initial);
        let task = tokio::spawn(run_catalog_updates(feed, status_rx, tx));
        (Self { rx }, task)
    }
}

impl CatalogProvider for LiveReaderCatalog {
    fn catalog(&self) -> StreamCatalog {
        self.rx.borrow().clone()
    }
}

fn snapshot_reader_connected(snapshot: &ForwarderStatusSnapshot, stream: &str) -> bool {
    snapshot
        .readers
        .iter()
        .any(|(id, status)| id == stream && status.state == ReaderConnectionState::Connected)
}

/// Applies reader connectivity deltas from the status feed to the catalog.
///
/// Runs until aborted at P2P shutdown: this task holds a feed clone (and thus
/// a sender clone) itself, so the status channel never closes underneath it.
async fn run_catalog_updates(
    feed: ForwarderStatusFeed,
    mut status_rx: broadcast::Receiver<ForwarderStatusEvent>,
    tx: watch::Sender<StreamCatalog>,
) {
    loop {
        match status_rx.recv().await {
            Ok(ForwarderStatusEvent::ReaderStatus { stream_id, status }) => {
                apply_reader_status(&tx, &stream_id, &status);
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    skipped,
                    "p2p: catalog status receiver lagged; reseeding from snapshot"
                );
                // A dropped delta could be a connect/disconnect; re-subscribe
                // atomically with a fresh snapshot so the catalog cannot stay
                // stale.
                let (new_rx, snapshot) = feed.subscribe_and_snapshot().await;
                status_rx = new_rx;
                for (stream_id, status) in &snapshot.readers {
                    apply_reader_status(&tx, stream_id, status);
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Applies reader status to the catalog, bumping the generation only when the
/// advertised connectivity or epoch list actually changes.
fn apply_reader_status(
    tx: &watch::Sender<StreamCatalog>,
    stream_id: &str,
    status: &crate::status_store::ReaderStatus,
) {
    tx.send_if_modified(|catalog| {
        let Some(entry) = catalog
            .entries
            .iter_mut()
            .find(|entry| entry.stream_id == stream_id.as_bytes())
        else {
            return false;
        };
        let mut changed = false;
        let connected = status.state == ReaderConnectionState::Connected;
        if entry.reader_connected != connected {
            entry.reader_connected = connected;
            changed = true;
        }
        if let Some(epoch) = status.current_epoch {
            if let Some(summary) = entry
                .epoch_summaries
                .iter_mut()
                .find(|summary| summary.epoch == epoch)
            {
                // Known epoch: keep its advertised name in sync so receivers
                // connecting later see name edits.
                if summary.name != status.current_epoch_name {
                    summary.name = status.current_epoch_name.clone();
                    changed = true;
                }
            } else {
                let start_seq = status
                    .current_epoch_start_seq
                    .and_then(|seq| u64::try_from(seq).ok())
                    .unwrap_or_default();
                // The previously-open head epoch is now closed by this advance.
                if let Some(head) = entry.epoch_summaries.first_mut()
                    && head.end_seq.is_none()
                    && start_seq > 0
                {
                    head.end_seq = Some(start_seq - 1);
                }
                entry.epoch_summaries.insert(
                    0,
                    StreamEpochSummary {
                        epoch,
                        created_unix_ms: status.current_epoch_created_unix_ms,
                        start_seq,
                        end_seq: None,
                        name: status.current_epoch_name.clone(),
                    },
                );
                changed = true;
            }
        }
        if changed {
            catalog.generation += 1;
        }
        changed
    });
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

/// Whether a reader-status event carries an epoch change that warrants an
/// out-of-cadence catalog push.
///
/// The first observation of a stream's epoch seeds the map WITHOUT pushing
/// (startup status seeding would otherwise trigger a burst of redundant
/// pushes right after the initial push); only a subsequent different epoch
/// reports a change.
fn epoch_changed_for_push(
    last_epochs: &mut std::collections::HashMap<String, i64>,
    stream_id: &str,
    current_epoch: Option<i64>,
) -> bool {
    let Some(epoch) = current_epoch else {
        return false;
    };
    match last_epochs.insert(stream_id.to_owned(), epoch) {
        Some(previous) => previous != epoch,
        None => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_forwarder_catalog_distribution(
    client: ServerCatalogClient,
    endpoint: P2pEndpoint,
    display_name: Option<String>,
    journal: Arc<Mutex<Journal>>,
    reader_streams: Vec<String>,
    push_interval: Duration,
    mut status_rx: broadcast::Receiver<ForwarderStatusEvent>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    done_tx: tokio::sync::oneshot::Sender<()>,
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

    // Last-seen current epoch per stream, for out-of-cadence pushes on epoch
    // advances. Registry-restore staleness scales with the push interval, so
    // an advance must reach the server immediately, not up to a tick later.
    let mut last_epochs = std::collections::HashMap::new();

    loop {
        let push_now = tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                // Final push on graceful shutdown so the registry high-water
                // reflects everything journaled this session.
                if changed.is_err() || *shutdown_rx.borrow() {
                    push_forwarder_catalog_once(
                        &client,
                        &endpoint,
                        display_name.as_deref(),
                        Arc::clone(&journal),
                        &reader_streams,
                    )
                    .await;
                    let _ = done_tx.send(());
                    return;
                }
                false
            }
            event = status_rx.recv() => match event {
                Ok(ForwarderStatusEvent::ReaderStatus { stream_id, status }) => {
                    epoch_changed_for_push(&mut last_epochs, &stream_id, status.current_epoch)
                }
                Ok(_) => false,
                // Lagged: we may have missed an epoch change; push to be safe.
                Err(broadcast::error::RecvError::Lagged(_)) => true,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = ticker.tick() => true,
        };
        if push_now {
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
mod epoch_push_tests {
    use super::epoch_changed_for_push;
    use std::collections::HashMap;

    #[test]
    fn first_observation_seeds_without_pushing() {
        let mut last = HashMap::new();
        assert!(!epoch_changed_for_push(&mut last, "reader-a", Some(3)));
        assert!(!epoch_changed_for_push(&mut last, "reader-b", Some(1)));
    }

    #[test]
    fn same_epoch_does_not_push_and_advance_pushes() {
        let mut last = HashMap::new();
        assert!(!epoch_changed_for_push(&mut last, "reader-a", Some(3)));
        assert!(!epoch_changed_for_push(&mut last, "reader-a", Some(3)));
        assert!(
            epoch_changed_for_push(&mut last, "reader-a", Some(4)),
            "an epoch advance must trigger an out-of-cadence push"
        );
        assert!(!epoch_changed_for_push(&mut last, "reader-a", Some(4)));
    }

    #[test]
    fn missing_epoch_never_pushes() {
        let mut last = HashMap::new();
        assert!(!epoch_changed_for_push(&mut last, "reader-a", None));
        assert!(!epoch_changed_for_push(&mut last, "reader-a", Some(2)));
        assert!(!epoch_changed_for_push(&mut last, "reader-a", None));
        // The tracked epoch survives a None in between.
        assert!(!epoch_changed_for_push(&mut last, "reader-a", Some(2)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::P2pConfig;
    use crate::status_http::{StatusConfig, StatusServer};
    use crate::status_store::SubsystemStatus;
    use crate::storage::journal::Journal;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode, header::AUTHORIZATION},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
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

    #[derive(Clone)]
    struct BootstrapRetryServerState {
        register_attempts: Arc<std::sync::atomic::AtomicU64>,
        catalog_pushes: tokio::sync::watch::Sender<u64>,
    }

    async fn bootstrap_retry_register_handler(
        State(state): State<BootstrapRetryServerState>,
        headers: HeaderMap,
        Json(_body): Json<serde_json::Value>,
    ) -> Response {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "Bearer thin-voucher");
        if !authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }

        let attempt = state
            .register_attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if attempt <= 2 {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }

        Json(serde_json::json!({
            "endpoint_id": "fwd-node-1",
            "device_kind": "forwarder",
            "approval_state": "pending",
            "device_token": "rtk_minted_secret"
        }))
        .into_response()
    }

    async fn bootstrap_retry_catalog_handler(
        State(state): State<BootstrapRetryServerState>,
        headers: HeaderMap,
        Json(_body): Json<serde_json::Value>,
    ) -> Response {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "Bearer rtk_minted_secret");
        if !authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }

        state.catalog_pushes.send_modify(|count| *count += 1);
        StatusCode::OK.into_response()
    }

    async fn bootstrap_retry_allowlist_handler(headers: HeaderMap) -> Response {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "Bearer rtk_minted_secret");
        if !authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(serde_json::json!({ "receiver_endpoint_ids": [] })).into_response()
    }

    #[tokio::test]
    async fn server_integration_retries_bootstrap_until_catalog_starts() -> TestResult {
        let receiver = EndpointBuilder::test([44; 32]).bind().await?;
        let dir = tempfile::tempdir()?;
        let journal_path = dir.path().join("journal.sqlite3");
        let journal = Arc::new(Mutex::new(Journal::open(&journal_path)?));
        let token_file = dir.path().join("server-token");
        std::fs::write(&token_file, "thin-voucher\n")?;
        let device_token_file = dir.path().join("p2p-device-token");
        let (catalog_pushes, mut catalog_pushes_rx) = tokio::sync::watch::channel(0u64);
        let state = BootstrapRetryServerState {
            register_attempts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            catalog_pushes,
        };
        let app = Router::new()
            .route("/register", post(bootstrap_retry_register_handler))
            .route("/forwarder/catalog", post(bootstrap_retry_catalog_handler))
            .route(
                "/allowlist/receivers",
                get(bootstrap_retry_allowlist_handler),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut config = p2p_config(receiver.endpoint_id().to_string());
        config.server_url = Some(base_url);
        config.server_token_file = Some(token_file.to_string_lossy().into_owned());
        config.device_token_file = Some(device_token_file.to_string_lossy().into_owned());
        config.allowlist_request_timeout_secs = 1;
        let runtime = start_forwarder_p2p(
            &config,
            &journal_path,
            Arc::clone(&journal),
            &["10.0.0.5:10000".to_owned()],
            None,
            status_feed().await?,
            Arc::new(NoopRemoteConfigHandler),
            Arc::new(NoopReaderControlHandler),
        )
        .await?
        .expect("p2p enabled");

        tokio::time::timeout(
            Duration::from_secs(2),
            catalog_pushes_rx.wait_for(|&count| count >= 1),
        )
        .await??;
        assert_eq!(
            state
                .register_attempts
                .load(std::sync::atomic::Ordering::SeqCst),
            3,
            "bootstrap should retry in-process after initial failures"
        );
        assert_eq!(
            read_device_token(&device_token_file)?.as_deref(),
            Some("rtk_minted_secret")
        );

        runtime.shutdown().await;
        server.abort();
        receiver.close().await;
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
            &journal_path,
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

    /// Connects, performs the control-plane Hello handshake, and returns the
    /// negotiated `HelloOk.catalog_generation` together with the served
    /// `StreamCatalog` frame, then drops the connection.
    async fn hello_and_catalog(
        receiver: &rt_iroh::Endpoint,
        forwarder_addr: &EndpointAddr,
    ) -> Result<(u64, StreamCatalog), BoxError> {
        let connection = receiver.connect(forwarder_addr.clone()).await?;
        let (mut control_send, mut control_recv) = connection.open_bi().await?;
        control::write_frame(
            &mut control_send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(control::forwarder_hello())),
            },
        )
        .await?;
        let hello_generation = match control::read_frame::<ControlF2C>(&mut control_recv)
            .await?
            .msg
        {
            Some(control_f2c::Msg::HelloOk(hello_ok)) => hello_ok.catalog_generation,
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        };
        let catalog = match control::read_frame::<ControlF2C>(&mut control_recv)
            .await?
            .msg
        {
            Some(control_f2c::Msg::StreamCatalog(catalog)) => catalog,
            other => return Err(format!("expected StreamCatalog, got {other:?}").into()),
        };
        connection.close(0u32.into(), b"test done");
        Ok((hello_generation, catalog))
    }

    #[tokio::test]
    async fn reconnect_serves_current_reader_connectivity_and_bumped_generation() -> TestResult {
        let receiver = EndpointBuilder::test([63; 32]).bind().await?;
        let dir = tempfile::tempdir()?;
        let journal_path = dir.path().join("journal.sqlite3");
        let journal = Arc::new(Mutex::new(Journal::open(&journal_path)?));
        let stream_key = "10.0.0.5:10000";

        // Reader is already Connected before P2P starts (main.rs initializes
        // reader status before start_forwarder_p2p), so the initial catalog
        // must be seeded from the status snapshot, not hardcoded.
        let store = crate::status_store::StatusStore::new(SubsystemStatus::ready());
        store.init_readers(&[(stream_key.to_owned(), 10001)]).await;
        store
            .update_reader_state(
                stream_key,
                crate::status_store::ReaderConnectionState::Connected,
            )
            .await;

        let runtime = start_forwarder_p2p(
            &p2p_config(receiver.endpoint_id().to_string()),
            &journal_path,
            Arc::clone(&journal),
            &[stream_key.to_owned()],
            None,
            store.status_feed(),
            Arc::new(NoopRemoteConfigHandler),
            Arc::new(NoopReaderControlHandler),
        )
        .await?
        .expect("p2p enabled");
        let forwarder_addr = runtime.endpoint_addr().await;

        let (g1, catalog) = hello_and_catalog(&receiver, &forwarder_addr).await?;
        assert_eq!(
            g1, catalog.generation,
            "HelloOk.catalog_generation must match the served StreamCatalog generation"
        );
        assert!(
            catalog.entries[0].reader_connected,
            "initial catalog must reflect the reader's Connected state at p2p start"
        );

        store
            .update_reader_state(
                stream_key,
                crate::status_store::ReaderConnectionState::Disconnected,
            )
            .await;

        // The catalog task observes the status feed asynchronously: poll with
        // fresh connections until the disconnect is visible at connect time.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (g2, catalog) = hello_and_catalog(&receiver, &forwarder_addr).await?;
            assert_eq!(
                g2, catalog.generation,
                "HelloOk.catalog_generation must match the served StreamCatalog generation"
            );
            if g2 > g1 && !catalog.entries[0].reader_connected {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "catalog never reflected reader disconnect: generation {g2} (initial {g1}), \
                     reader_connected {}",
                    catalog.entries[0].reader_connected
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        runtime.shutdown().await;
        receiver.close().await;
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
            &journal_path,
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

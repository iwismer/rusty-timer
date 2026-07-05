//! Receiver control API — business logic for the receiver.
//!
//! All handler functions are plain async functions that take `&AppState`
//! and return `Result<T, ReceiverError>`.  The Tauri app wraps these as
//! IPC commands.

use crate::db::{Db, StreamSubscription};
use crate::error::ReceiverError;
use crate::stream_key::{LocalStreamKey, is_valid_endpoint_id, is_valid_identity_part};
use crate::ui_events::ReceiverUiEvent;
use rt_p2p_protocol::{
    ConfigGetResponse, ConfigSetResponse, DownloadProgress, ReaderControlResponse, ReaderInfo,
    ReaderStatus, RestartResponse, StreamCatalog, UpsStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

/// A chip's resolved participant identity, held in the in-memory chip lookup.
/// `division` is the RD division display name (`None` for `.ppl` imports or
/// when the division code has no name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipEntry {
    pub bib: String,
    pub name: Option<String>,
    pub division: Option<String>,
}

pub type ChipLookup = HashMap<String, HashMap<String, ChipEntry>>;

/// One stream a discovered forwarder exposes, learned from the server
/// `GET /forwarders` discovery feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredEpochSummary {
    pub stream_epoch: i64,
    pub created_unix_ms: Option<i64>,
    /// First seq recorded in this epoch, from the forwarder catalog. `None`
    /// when the forwarder did not advertise a usable value; such an epoch
    /// cannot be resolved to an earliest-epoch override floor.
    pub start_seq: Option<i64>,
    /// Operator label for the epoch, if any.
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredStream {
    pub stream_id: String,
    pub epoch: i64,
    pub next_seq: i64,
    pub epoch_options: Vec<DiscoveredEpochSummary>,
}

/// An approved forwarder discovered from the server (or seeded from an
/// explicit local forwarder config). `direct_addrs` are the addresses the
/// receiver dials; `streams` is the forwarder's advertised stream catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredForwarder {
    pub display_name: Option<String>,
    pub direct_addrs: Vec<SocketAddr>,
    pub streams: Vec<DiscoveredStream>,
}

/// Shared map of discovered forwarders keyed by their endpoint id (string
/// form). Populated by the discovery task and/or seeded from explicit config.
pub type DiscoveredForwarders = HashMap<String, DiscoveredForwarder>;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot, watch};
use tracing::warn;

/// How long a remote-config command waits for the forwarder's response before
/// failing, so a missing/late reply can never hang the caller. The response is
/// routed back to the awaiting command by the per-forwarder control loop.
pub(crate) const FORWARDER_CONFIG_TIMEOUT: Duration = Duration::from_secs(10);

/// A remote-config request bridged from a control-API command (`&AppState`) to
/// the live [`ForwarderConnection`](crate::p2p_forwarder) control loop that
/// owns the QUIC control session. Each variant carries a `oneshot` responder
/// the control loop completes once the forwarder replies (routed by
/// `request_id`). Only registered for forwarders whose live session negotiated
/// `CAP_REMOTE_CONFIG`.
pub(crate) enum ConfigCommand {
    Get {
        resp: oneshot::Sender<ConfigGetResponse>,
    },
    Set {
        config_json: String,
        resp: oneshot::Sender<ConfigSetResponse>,
    },
    Restart {
        resp: oneshot::Sender<RestartResponse>,
    },
}

/// A reader-control request bridged from control API commands to the live
/// P2P control loop for a forwarder that negotiated `CAP_READER_CONTROL`.
pub(crate) enum ReaderCommand {
    Request {
        stream_id: String,
        action: rt_domain::ReaderControlAction,
        resp: oneshot::Sender<ReaderControlResponse>,
    },
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwarderConnState {
    Subscribed,
    Connected,
    Unavailable,
    Disconnected,
}

/// A point-in-time view of one forwarder's connection state.
///
/// Contract for consumers (notably the UI): while `pending == true` the
/// forwarder is still within the initial 5s connect grace window
/// ([`FORWARDER_PENDING_GRACE`]) after a dial attempt began, and consumers MUST
/// present it as a transient "connecting" status — NOT as the underlying
/// `state`, which during the grace window is reported as
/// [`ForwarderConnState::Unavailable`] even though no real failure has been
/// confirmed yet. Only once `pending == false` does `state` reflect the
/// settled, user-facing condition. (Phase 2 UI maps `pending` → an amber
/// "Connecting…" badge.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ForwarderStateSnapshot {
    pub state: ForwarderConnState,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectAttempt {
    pub version: u64,
    pub endpoint_id: Option<String>,
    pub restart: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ForwarderRuntimeStatus {
    pub control_up: bool,
    pub data_sessions: usize,
    pub pending_started_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionsFingerprint {
    aggregate_state: ConnectionState,
    forwarders: Vec<ForwarderConnectionsFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwarderConnectionsFingerprint {
    endpoint_id: String,
    state: ForwarderConnState,
    pending: bool,
    intent: bool,
    discovered: bool,
    display_name: Option<String>,
    remote_config_available: bool,
    reader_control_available: bool,
    subscribed_count: usize,
    available_count: usize,
    readers: Vec<ReaderLiveStatus>,
    ups: Option<UpsStatusPayload>,
    failed_stream_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ForwarderLiveStatus {
    readers: HashMap<String, ReaderLiveStatus>,
    pub(crate) ups: Option<UpsStatusPayload>,
    /// Wire stream ids whose data subscription failed terminally on the live
    /// connection (protocol/data-integrity error). Set by
    /// [`AppState::mark_forwarder_stream_failed`]; cleared on control
    /// disconnect (the whole live status is dropped) or on a subscription
    /// config change ([`AppState::clear_forwarder_stream_failed`]).
    pub(crate) failed_streams: BTreeSet<String>,
    /// Wire stream ids whose data task is held fail-closed because their
    /// earliest-epoch override cannot be resolved against the connection's
    /// catalog. Cleared when the override resolves, is removed, or the
    /// control connection drops (whole live status dropped).
    pub(crate) override_held_streams: BTreeSet<String>,
}

const FORWARDER_PENDING_GRACE: Duration = Duration::from_secs(5);

pub(crate) fn derive_forwarder_state(
    runtime: ForwarderRuntimeStatus,
    intent: bool,
) -> ForwarderStateSnapshot {
    if runtime.data_sessions > 0 {
        return ForwarderStateSnapshot {
            state: ForwarderConnState::Subscribed,
            pending: false,
        };
    }
    if runtime.control_up {
        return ForwarderStateSnapshot {
            state: ForwarderConnState::Connected,
            pending: false,
        };
    }
    if intent {
        let pending = runtime
            .pending_started_at
            .is_some_and(|started| started.elapsed() < FORWARDER_PENDING_GRACE);
        return ForwarderStateSnapshot {
            state: ForwarderConnState::Unavailable,
            pending,
        };
    }
    ForwarderStateSnapshot {
        state: ForwarderConnState::Disconnected,
        pending: false,
    }
}

fn decode_stream_id(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(&error.into_bytes()).into_owned())
}

pub(crate) fn optional_non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

pub(crate) fn validate_stream_identity(
    forwarder_endpoint_id: &str,
    stream_id: &str,
) -> Result<(), ReceiverError> {
    if !is_valid_identity_part(forwarder_endpoint_id) {
        return Err(ReceiverError::BadRequest(
            "forwarder_endpoint_id must not be empty".to_owned(),
        ));
    }
    if !is_valid_endpoint_id(forwarder_endpoint_id) {
        return Err(ReceiverError::BadRequest(
            "forwarder_endpoint_id must not contain the stream key separator".to_owned(),
        ));
    }
    if !is_valid_identity_part(stream_id) {
        return Err(ReceiverError::BadRequest(
            "stream_id must not be empty".to_owned(),
        ));
    }
    Ok(())
}

fn merge_epoch_options(
    discovered_stream: Option<&DiscoveredStream>,
    live_reader: Option<&ReaderLiveStatus>,
) -> Vec<DiscoveredEpochSummary> {
    let mut options = Vec::new();
    if let Some(reader) = live_reader
        && let Some(epoch) = reader.current_epoch.filter(|epoch| *epoch > 0)
    {
        options.push(DiscoveredEpochSummary {
            stream_epoch: epoch,
            created_unix_ms: reader.current_epoch_created_unix_ms,
            start_seq: reader.current_epoch_start_seq.filter(|seq| *seq >= 1),
            name: reader.current_epoch_name.clone(),
        });
    }
    if let Some(stream) = discovered_stream {
        for option in &stream.epoch_options {
            if option.stream_epoch <= 0 {
                continue;
            }
            if let Some(existing) = options
                .iter_mut()
                .find(|existing| existing.stream_epoch == option.stream_epoch)
            {
                // The live status entry wins where populated; the catalog
                // fills the gaps (e.g. start_seq before a status delta).
                if existing.start_seq.is_none() {
                    existing.start_seq = option.start_seq;
                }
                if existing.created_unix_ms.is_none() {
                    existing.created_unix_ms = option.created_unix_ms;
                }
                if existing.name.is_none() {
                    existing.name = option.name.clone();
                }
            } else {
                options.push(option.clone());
            }
        }
        if stream.epoch > 0
            && !options
                .iter()
                .any(|existing| existing.stream_epoch == stream.epoch)
        {
            options.push(DiscoveredEpochSummary {
                stream_epoch: stream.epoch,
                created_unix_ms: None,
                start_seq: None,
                name: None,
            });
        }
    }
    options.sort_by_key(|option| std::cmp::Reverse(option.stream_epoch));
    options
}

fn parse_reader_info_json(
    reader_info_json: Option<&str>,
    endpoint_id: &str,
    stream_id: &str,
) -> Option<rt_domain::ReaderInfo> {
    let json = reader_info_json.filter(|json| !json.is_empty())?;
    match serde_json::from_str(json) {
        Ok(reader_info) => Some(reader_info),
        Err(error) => {
            warn!(%endpoint_id, %stream_id, %error, "forwarder sent invalid reader_info_json");
            None
        }
    }
}

fn download_state_from_str(state: &str) -> rt_domain::DownloadState {
    match state {
        "downloading" => rt_domain::DownloadState::Downloading,
        "complete" => rt_domain::DownloadState::Complete,
        "error" => rt_domain::DownloadState::Error,
        _ => rt_domain::DownloadState::Idle,
    }
}

pub(crate) fn subscription_local_ports(
    subscriptions: &[StreamSubscription],
) -> HashMap<(String, String), Option<u16>> {
    subscriptions
        .iter()
        .map(|subscription| {
            let display_reader_ip = subscription.reader_ip.clone().or_else(|| {
                crate::ports::reader_addr_if_port_mappable(&subscription.stream_id)
                    .map(std::borrow::ToOwned::to_owned)
            });
            let local_port = subscription.local_port_override.or_else(|| {
                display_reader_ip
                    .as_deref()
                    .and_then(crate::ports::default_port)
            });
            (
                (
                    subscription.forwarder_endpoint_id.clone(),
                    subscription.stream_id.clone(),
                ),
                local_port,
            )
        })
        .collect()
}

/// Reader statuses with volatile read-count fields cleared, for the
/// connections-changed fingerprint.
///
/// Read counters and last-seen timestamps change on every coalesced
/// `ReaderStatus` delta from a forwarder; including them in the fingerprint
/// would turn each count refresh into a `ConnectionsChanged` event and a full
/// connections reload in the UI. Those fields are instead delivered through
/// the targeted `ForwarderReaderCountsUpdated` event.
fn fingerprint_reader_statuses(mut readers: Vec<ReaderLiveStatus>) -> Vec<ReaderLiveStatus> {
    for reader in &mut readers {
        reader.reads_session = None;
        reader.reads_total = None;
        reader.last_read_unix_ms = None;
        reader.last_seen_secs = None;
    }
    readers
}

pub(crate) fn sorted_reader_statuses(
    live_status: &ForwarderLiveStatus,
    local_ports: &HashMap<(String, String), Option<u16>>,
    endpoint_id: &str,
) -> Vec<ReaderLiveStatus> {
    let mut readers = live_status.readers.values().cloned().collect::<Vec<_>>();
    for reader in &mut readers {
        reader.local_port = local_ports
            .get(&(endpoint_id.to_owned(), reader.stream_id.clone()))
            .copied()
            .flatten();
    }
    readers.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
    readers
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    None,
    Disconnect,
    Terminate,
}

/// Durable storage handles (SQLite database access).
///
/// Lock rules: `db` is a **tokio** `Mutex` (await-safe; guards may be held
/// across `.await`). `writer` and `read_source` synchronize internally.
/// When a std-mutex guard and the `db` guard are held together, `db` is
/// acquired first; code never awaits `db` while a std guard is live.
pub struct StorageHandles {
    /// Cold control-plane SQLite connection.
    pub db: Arc<Mutex<Db>>,
    /// Handle to the dedicated SQLite writer thread. All hot-path persistence
    /// (P2P EventBatch/GapNotice) flows through it; the `db` mutex above is
    /// the cold control-plane connection.
    pub writer: crate::writer::WriterHandle,
    /// Read access for hot readers (proxy replay, projection rebuild): the
    /// read-only pool in production, or the cold mutex where no file-backed
    /// pool exists (in-memory test states).
    pub read_source: crate::read_pool::ReadSource,
}

/// UI-facing event fan-out and display caches.
///
/// Lock rules: `stream_delta_buffer` is a **std** mutex — its guard must
/// NEVER be held across an `.await` (take, mutate, drop within one
/// statement/block). `stream_metrics_cache` is a **tokio** `RwLock`
/// (await-safe). `stream_counts`, `ui_tx` and `logger` synchronize
/// internally and are safe from any context.
pub struct UiState {
    /// Dirty per-stream UI deltas keyed by `(forwarder_endpoint_id, wire_stream_id)`,
    /// drained by the global coalescing emitter (see
    /// `p2p_runtime::run_stream_delta_emitter`) into single
    /// [`ReceiverUiEvent::StreamDeltas`] events.
    pub stream_delta_buffer:
        Arc<StdMutex<HashMap<(String, String), crate::ui_events::StreamDelta>>>,
    pub logger: Arc<rt_ui_log::UiLogger<ReceiverUiEvent>>,
    pub ui_tx: broadcast::Sender<ReceiverUiEvent>,
    /// Per-stream read counts keyed by the composite receiver-local stream
    /// identity (`LocalStreamKey`).
    pub stream_counts: crate::cache::StreamCounts,
    /// Latest per-stream metrics payloads keyed by
    /// `(forwarder_endpoint_id, wire_stream_id)`. Display metadata
    /// (`forwarder_id`, `reader_ip`) can collide across forwarders and must
    /// never key this cache.
    pub stream_metrics_cache:
        Arc<RwLock<HashMap<(String, String), crate::ui_events::StreamMetricsPayload>>>,
}

/// Per-forwarder connection state, discovery, and live control-channel
/// registries.
///
/// Lock rules: `discovered_forwarders` and `p2p_endpoint_id` are **tokio**
/// `RwLock`s (await-safe). Every other field is a **std** mutex whose guard
/// must NEVER be held across an `.await`; existing code takes each guard
/// briefly (lock, read/mutate/clone, drop) and never nests two of these std
/// mutexes. Std guards may be taken while the (tokio) `StorageHandles::db`
/// lock is held, but never the reverse-with-await.
pub struct ForwarderControl {
    /// Approved forwarders discovered from the server (or seeded from an
    /// explicit local forwarder config), keyed by endpoint id. Drives both the
    /// available-but-unsubscribed entries in the streams response and the
    /// per-subscription dial address resolution in the P2P runtime.
    pub discovered_forwarders: Arc<tokio::sync::RwLock<DiscoveredForwarders>>,
    pub p2p_endpoint_id: Arc<RwLock<Option<String>>>,
    pub(crate) forwarder_runtime: Arc<StdMutex<HashMap<String, ForwarderRuntimeStatus>>>,
    /// Endpoint ids whose persisted intent is explicit disconnect (a
    /// `forwarder_intent` row with `connect = 0`). This mirrors the table so
    /// [`AppState::recompute_aggregate_connection_state_sync_default_trying`],
    /// which cannot await the DB mutex, can still exclude intentionally
    /// disconnected forwarders from the trying-aggregation. Seeded from the DB
    /// during construction (before any worker can trigger the sync fallback)
    /// and updated via [`AppState::cache_forwarder_intent`] only after the
    /// corresponding DB write succeeds. Never held across an await.
    pub(crate) disconnected_intents: Arc<StdMutex<HashSet<String>>>,
    pub(crate) forwarder_live_status: Arc<StdMutex<HashMap<String, ForwarderLiveStatus>>>,
    /// Per-forwarder remote-config request channels, keyed by endpoint id. An
    /// entry exists only while that forwarder has a live control session whose
    /// negotiated `HelloOk` advertised `CAP_REMOTE_CONFIG`; the
    /// [`ForwarderConnection`](crate::p2p_forwarder) registers its sender on
    /// connect and deregisters on disconnect/stop. Presence therefore doubles
    /// as the `remote_config_available` signal, so a command to a down or
    /// incapable forwarder fails fast.
    forwarder_config_tx: StdMutex<HashMap<String, mpsc::Sender<ConfigCommand>>>,
    forwarder_reader_control_tx: StdMutex<HashMap<String, mpsc::Sender<ReaderCommand>>>,
    last_connections_fingerprint: StdMutex<Option<ConnectionsFingerprint>>,
}

/// Watch channels and counters that coordinate the runtime workers.
///
/// Lock rules: no caller-held guards (`watch` channels and atomics); safe
/// to use from any context, sync or async. The `_*_keepalive`
/// receivers exist solely so a `send()` on the paired sender never fails
/// for lack of subscribers; they are never read.
pub struct Signals {
    pub connection_state: watch::Sender<ConnectionState>,
    // Keepalive receiver so that `connection_state.send()` never fails due
    // to "no receivers" even when no external subscriber is active.
    _conn_state_keepalive: watch::Receiver<ConnectionState>,
    /// State transitions already applied by the sync guard-drop fallback but
    /// still needing the async path's UI/log side effects.
    ///
    /// Holds at most one pending transition: a second sync-fallback transition
    /// before the async recompute consumes the marker overwrites the first, so
    /// intermediate transitions coalesce and only the final state's side
    /// effects (UI status event + log line) are emitted. That is intentional:
    /// final-state emission is what matters; intermediate states may skip
    /// their UI/log emission.
    pending_connection_state_side_effect: StdMutex<Option<ConnectionState>>,
    pub shutdown_tx: watch::Sender<ShutdownSignal>,
    connect_attempt: AtomicU64,
    connect_attempt_version: watch::Sender<ConnectAttempt>,
    /// Keepalive receiver to prevent the connect-attempt watch channel from being dropped
    /// when no runtime subscriber is active.
    _connect_attempt_keepalive: watch::Receiver<ConnectAttempt>,
    retry_streak: AtomicU64,
    /// Monotonic counter incremented when DBF config changes; subscribers
    /// (runtime.rs) use this to restart the DBF writer. Use
    /// `notify_dbf_config_changed()` and `dbf_config_rx()` to interact.
    dbf_config_version: watch::Sender<u64>,
    /// Keepalive receiver to prevent the watch channel from being dropped
    /// when no external subscribers exist.
    _dbf_config_keepalive: watch::Receiver<u64>,
    /// Monotonic counter incremented when the subscription set changes; the
    /// shared DBF worker uses this to reset its pass state and force a
    /// cross-stream regenerate, since a set change can reassign a freed
    /// reader index to a different stream while rows already in the file
    /// still carry the old digit. Use `notify_subscriptions_changed()` and
    /// `subscriptions_rx()` to interact.
    subscriptions_version: watch::Sender<u64>,
    /// Keepalive receiver so the subscriptions watch channel is not dropped
    /// when no subscriber is active.
    _subscriptions_keepalive: watch::Receiver<u64>,
    /// Monotonic counter incremented when the Race Director import config
    /// changes; the background poller (runtime.rs) uses this to pick up a new
    /// directory/interval/toggle without waiting a full interval.
    rd_import_config_version: watch::Sender<u64>,
    _rd_import_config_keepalive: watch::Receiver<u64>,
    /// Monotonic counter incremented when the server URL+token changes (e.g. a
    /// profile save); the P2P reconcile loop uses this to rebind server-bound
    /// tasks. Use `notify_server_config_changed()` and `server_config_rx()`.
    server_config_version: watch::Sender<u64>,
    /// Keepalive receiver so the watch channel is not dropped when the P2P
    /// runtime (the only subscriber) is not yet started.
    _server_config_keepalive: watch::Receiver<u64>,
}

pub struct AppState {
    /// Durable storage handles; see [`StorageHandles`] for lock rules.
    pub storage: StorageHandles,
    /// Per-forwarder connection/discovery state; see [`ForwarderControl`]
    /// for lock rules.
    pub forwarders: ForwarderControl,
    /// UI event fan-out and caches; see [`UiState`] for lock rules.
    pub ui: UiState,
    /// Runtime coordination channels and counters; see [`Signals`].
    pub signals: Signals,
    /// Live durable-proxy consumer cursors (retention floor input; see
    /// `crate::retention`).
    pub proxy_consumer_cursors: crate::retention::ProxyConsumerCursors,
    pub receiver_id: Arc<RwLock<String>>,
    pub db_integrity_ok: bool,
    pub http_client: reqwest::Client,
    pub chip_lookup: Arc<tokio::sync::RwLock<ChipLookup>>,
    /// The raw server URL+token override, set once at startup: env vars for the
    /// desktop app, CLI flags for headless. Source of truth for "is an override
    /// active" and for resolving the effective server in control handlers, so
    /// both desktop and headless report/persist the same server the P2P runtime
    /// targets. `(None, None)` when there is no override source.
    server_override: tokio::sync::RwLock<(Option<String>, Option<String>)>,
}

impl AppState {
    /// Test-only convenience constructor: the state carries a *disconnected*
    /// writer (every `state.storage.writer` call fails). Tests that persist
    /// through the writer must use [`AppState::new_for_test`], which shares
    /// one temp-file DB between `state.storage.db` and a live writer thread.
    pub fn new(db: Db, receiver_id: String) -> (Arc<Self>, watch::Receiver<ShutdownSignal>) {
        Self::with_integrity(
            db,
            receiver_id,
            true,
            crate::writer::WriterHandle::disconnected_for_test(),
            None,
        )
    }

    /// Test constructor with a *live* writer: creates a temp-file DB opened
    /// by both `state.storage.db` and the writer thread. Keep the returned
    /// `TempDir` alive for the duration of the test.
    #[doc(hidden)]
    pub fn new_for_test() -> (
        Arc<Self>,
        watch::Receiver<ShutdownSignal>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db_path = dir.path().join("receiver-test.sqlite3");
        let db = Db::open(&db_path).expect("open test db");
        let (writer, _thread) =
            crate::writer::spawn_writer(&db_path, crate::writer::WriterConfig::default())
                .expect("spawn test writer");
        let read_pool = crate::read_pool::ReadPool::open(&db_path, 2).expect("open test read pool");
        let (state, shutdown_rx) =
            Self::with_integrity(db, "recv-test".to_owned(), true, writer, Some(read_pool));
        (state, shutdown_rx, dir)
    }

    pub fn with_integrity(
        db: Db,
        receiver_id: String,
        db_integrity_ok: bool,
        writer: crate::writer::WriterHandle,
        read_pool: Option<std::sync::Arc<crate::read_pool::ReadPool>>,
    ) -> (Arc<Self>, watch::Receiver<ShutdownSignal>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(ShutdownSignal::None);
        let (ui_tx, _) = broadcast::channel(256);
        let (conn_tx, conn_keepalive_rx) = watch::channel(ConnectionState::Disconnected);
        let (connect_attempt_version, _connect_attempt_keepalive) =
            watch::channel(ConnectAttempt {
                version: 0,
                endpoint_id: None,
                restart: false,
            });
        let (dbf_config_version, _dbf_config_keepalive) = watch::channel(0u64);
        let (subscriptions_version, _subscriptions_keepalive) = watch::channel(0u64);
        let (rd_import_config_version, _rd_import_config_keepalive) = watch::channel(0u64);
        let (server_config_version, _server_config_keepalive) = watch::channel(0u64);
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .expect("failed to build HTTP client");
        let disconnected_intents: HashSet<String> = match db.load_forwarder_intents() {
            Ok(intents) => intents
                .into_iter()
                .filter_map(|(endpoint_id, connect)| (!connect).then_some(endpoint_id))
                .collect(),
            Err(error) => {
                warn!(error = %error, "failed to seed forwarder intent cache from DB");
                HashSet::new()
            }
        };
        let db = Arc::new(Mutex::new(db));
        let read_source = match read_pool {
            Some(pool) => crate::read_pool::ReadSource::Pool(pool),
            None => crate::read_pool::ReadSource::Mutex(Arc::clone(&db)),
        };
        let state = Arc::new(Self {
            storage: StorageHandles {
                db,
                writer,
                read_source,
            },
            ui: UiState {
                stream_delta_buffer: Arc::new(StdMutex::new(HashMap::new())),
                logger: Arc::new(rt_ui_log::UiLogger::with_buffer(
                    ui_tx.clone(),
                    |entry| ReceiverUiEvent::LogEntry { entry },
                    500,
                )),
                ui_tx,
                stream_counts: crate::cache::StreamCounts::new(),
                stream_metrics_cache: Arc::new(RwLock::new(HashMap::new())),
            },
            proxy_consumer_cursors: Arc::default(),
            signals: Signals {
                connection_state: conn_tx,
                _conn_state_keepalive: conn_keepalive_rx,
                pending_connection_state_side_effect: StdMutex::new(None),
                shutdown_tx,
                connect_attempt: AtomicU64::new(0),
                connect_attempt_version,
                _connect_attempt_keepalive,
                retry_streak: AtomicU64::new(0),
                dbf_config_version,
                _dbf_config_keepalive,
                subscriptions_version,
                _subscriptions_keepalive,
                rd_import_config_version,
                _rd_import_config_keepalive,
                server_config_version,
                _server_config_keepalive,
            },
            receiver_id: Arc::new(RwLock::new(receiver_id)),
            db_integrity_ok,
            http_client,
            chip_lookup: Arc::new(tokio::sync::RwLock::new(ChipLookup::new())),
            forwarders: ForwarderControl {
                discovered_forwarders: Arc::new(tokio::sync::RwLock::new(
                    DiscoveredForwarders::new(),
                )),
                p2p_endpoint_id: Arc::new(RwLock::new(None)),
                forwarder_runtime: Arc::new(StdMutex::new(HashMap::new())),
                disconnected_intents: Arc::new(StdMutex::new(disconnected_intents)),
                forwarder_live_status: Arc::new(StdMutex::new(HashMap::new())),
                forwarder_config_tx: StdMutex::new(HashMap::new()),
                forwarder_reader_control_tx: StdMutex::new(HashMap::new()),
                last_connections_fingerprint: StdMutex::new(None),
            },
            server_override: tokio::sync::RwLock::new((None, None)),
        });
        (state, shutdown_rx)
    }

    /// Record the server URL+token override (env for desktop, CLI for headless).
    /// Called once at startup before control handlers serve.
    pub async fn set_server_override(&self, override_: (Option<String>, Option<String>)) {
        *self.server_override.write().await = override_;
    }

    /// The server URL+token override captured at startup.
    pub async fn server_override(&self) -> (Option<String>, Option<String>) {
        self.server_override.read().await.clone()
    }

    /// Subscribe to connection state changes.
    pub fn conn_rx(&self) -> watch::Receiver<ConnectionState> {
        self.signals.connection_state.subscribe()
    }

    pub fn notify_dbf_config_changed(&self) {
        self.signals.dbf_config_version.send_modify(|v| *v += 1);
    }

    pub fn dbf_config_rx(&self) -> watch::Receiver<u64> {
        self.signals.dbf_config_version.subscribe()
    }

    /// Signal that the subscription set changed so the shared DBF worker
    /// regenerates the file with the current per-subscription reader indices.
    pub fn notify_subscriptions_changed(&self) {
        self.signals.subscriptions_version.send_modify(|v| *v += 1);
    }

    pub fn subscriptions_rx(&self) -> watch::Receiver<u64> {
        self.signals.subscriptions_version.subscribe()
    }

    pub fn notify_rd_import_config_changed(&self) {
        self.signals
            .rd_import_config_version
            .send_modify(|v| *v += 1);
    }

    pub fn rd_import_config_rx(&self) -> watch::Receiver<u64> {
        self.signals.rd_import_config_version.subscribe()
    }

    /// Signal that the server URL+token configuration changed so the P2P
    /// reconcile loop rebinds its server-bound tasks.
    pub fn notify_server_config_changed(&self) {
        self.signals.server_config_version.send_modify(|v| *v += 1);
    }

    pub fn server_config_rx(&self) -> watch::Receiver<u64> {
        self.signals.server_config_version.subscribe()
    }

    pub fn connect_attempt_rx(&self) -> watch::Receiver<ConnectAttempt> {
        self.signals.connect_attempt_version.subscribe()
    }

    fn bump_connect_attempt(&self, endpoint_id: Option<String>, restart: bool) -> u64 {
        let next = self.signals.connect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.signals.connect_attempt_version.send(ConnectAttempt {
            version: next,
            endpoint_id,
            restart,
        });
        next
    }

    pub async fn cache_stream_metrics(&self, payload: &crate::ui_events::StreamMetricsPayload) {
        let key = (
            payload.forwarder_endpoint_id.clone(),
            payload.stream_id.clone(),
        );
        self.ui
            .stream_metrics_cache
            .write()
            .await
            .insert(key, payload.clone());
    }

    pub async fn clear_stream_metrics_cache(&self) {
        self.ui.stream_metrics_cache.write().await.clear();
    }

    pub async fn set_p2p_endpoint_id(&self, endpoint_id: String) {
        *self.forwarders.p2p_endpoint_id.write().await = Some(endpoint_id);
    }

    pub async fn get_stream_metrics_snapshot(&self) -> Vec<crate::ui_events::StreamMetricsPayload> {
        self.ui
            .stream_metrics_cache
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    pub fn request_disconnect_shutdown(&self) {
        let _ = self.signals.shutdown_tx.send(ShutdownSignal::Disconnect);
    }

    pub fn request_process_shutdown(&self) {
        let _ = self.signals.shutdown_tx.send(ShutdownSignal::Terminate);
    }

    pub fn current_connect_attempt(&self) -> u64 {
        self.signals.connect_attempt.load(Ordering::SeqCst)
    }

    pub fn current_retry_streak(&self) -> u64 {
        self.signals.retry_streak.load(Ordering::SeqCst)
    }

    pub fn reset_retry_streak(&self) {
        self.signals.retry_streak.store(0, Ordering::SeqCst);
    }

    pub async fn request_connect(&self) {
        self.reset_retry_streak();
        self.bump_connect_attempt(None, true);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_forwarder_reconnect(&self, endpoint_id: String) {
        self.reset_retry_streak();
        self.bump_connect_attempt(Some(endpoint_id), true);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub(crate) fn wake_reconcile(&self) {
        self.bump_connect_attempt(None, false);
    }

    pub(crate) fn update_forwarder_runtime_sync(
        &self,
        endpoint_id: &str,
        update: impl FnOnce(&mut ForwarderRuntimeStatus),
    ) {
        let mut statuses = self.forwarders.forwarder_runtime.lock().unwrap();
        let status = statuses.entry(endpoint_id.to_owned()).or_default();
        update(status);
    }

    /// Mirror a forwarder intent into the sync-fallback cache. Call only
    /// after the corresponding `forwarder_intent` DB write succeeded, so the
    /// cache never runs ahead of the persisted truth.
    pub(crate) fn cache_forwarder_intent(&self, endpoint_id: &str, connect: bool) {
        let mut disconnected = self.forwarders.disconnected_intents.lock().unwrap();
        if connect {
            disconnected.remove(endpoint_id);
        } else {
            disconnected.insert(endpoint_id.to_owned());
        }
    }

    /// Drop all cached disconnect intents. Call only after a DB operation
    /// that deleted every `forwarder_intent` row (factory reset).
    pub(crate) fn clear_forwarder_intent_cache(&self) {
        self.forwarders.disconnected_intents.lock().unwrap().clear();
    }

    pub(crate) async fn mark_forwarder_runtime(
        &self,
        endpoint_id: &str,
        update: impl FnOnce(&mut ForwarderRuntimeStatus),
    ) {
        self.update_forwarder_runtime_sync(endpoint_id, update);
        self.recompute_aggregate_connection_state().await;
    }

    /// Start the per-forwarder pending grace clock when a dial attempt begins,
    /// but only if it is not already running. This is idempotent: repeated calls
    /// while still dialing (e.g. across reconnect retries) do not reset the
    /// clock, so the grace window measures from the *first* dial attempt and can
    /// actually elapse to [`ForwarderConnState::Unavailable`]. The clock is
    /// cleared on control connect and re-armed on control disconnect.
    pub async fn mark_forwarder_dial_started(&self, endpoint_id: &str) {
        self.mark_forwarder_runtime(endpoint_id, |status| {
            if status.pending_started_at.is_none() {
                status.pending_started_at = Some(Instant::now());
            }
        })
        .await;
    }

    pub(crate) async fn store_forwarder_catalog(&self, endpoint_id: &str, catalog: &StreamCatalog) {
        let mut discovered = self.forwarders.discovered_forwarders.write().await;
        let forwarder =
            discovered
                .entry(endpoint_id.to_owned())
                .or_insert_with(|| DiscoveredForwarder {
                    display_name: None,
                    direct_addrs: Vec::new(),
                    streams: Vec::new(),
                });
        let existing = forwarder
            .streams
            .iter()
            .map(|stream| (stream.stream_id.clone(), stream.clone()))
            .collect::<HashMap<_, _>>();
        forwarder.streams = catalog
            .entries
            .iter()
            .map(|entry| {
                let stream_id = decode_stream_id(entry.stream_id.clone());
                let epoch_options = entry
                    .epoch_summaries
                    .iter()
                    .map(|summary| {
                        // The wire start_seq is uint64 while the floor math is
                        // i64: reject 0 and > i64::MAX values so an invalid
                        // advertisement can never resolve an override.
                        let start_seq = i64::try_from(summary.start_seq)
                            .ok()
                            .filter(|seq| *seq >= 1);
                        if start_seq.is_none() {
                            warn!(
                                %endpoint_id,
                                %stream_id,
                                epoch = summary.epoch,
                                advertised_start_seq = summary.start_seq,
                                "forwarder advertised epoch summary with invalid start_seq"
                            );
                        }
                        DiscoveredEpochSummary {
                            stream_epoch: summary.epoch,
                            created_unix_ms: summary.created_unix_ms,
                            start_seq,
                            name: summary.name.clone(),
                        }
                    })
                    .collect::<Vec<_>>();
                let existing = existing.get(&stream_id);
                let epoch = epoch_options
                    .first()
                    .map(|summary| summary.stream_epoch)
                    .or_else(|| existing.map(|stream| stream.epoch))
                    .unwrap_or_default();
                let next_seq = existing.map(|stream| stream.next_seq).unwrap_or_default();
                DiscoveredStream {
                    stream_id,
                    epoch,
                    next_seq,
                    epoch_options,
                }
            })
            .collect();
        drop(discovered);
        self.recompute_aggregate_connection_state().await;
        self.emit_streams_snapshot().await;
    }

    pub async fn forwarder_state(&self, endpoint_id: &str) -> ForwarderStateSnapshot {
        let runtime = self
            .forwarders
            .forwarder_runtime
            .lock()
            .unwrap()
            .get(endpoint_id)
            .copied()
            .unwrap_or_default();
        let intent = self
            .storage
            .db
            .lock()
            .await
            .forwarder_should_connect(endpoint_id)
            .unwrap_or(true);
        derive_forwarder_state(runtime, intent)
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_reader_status(
        &self,
        endpoint_id: &str,
        status: ReaderStatus,
    ) {
        self.store_forwarder_reader_status_sync(endpoint_id, status);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a reader's live status with NO aggregate
    /// recompute. Used on the control read path so a status frame never blocks
    /// the reader (and any queued heartbeat `Ping`) on the DB/discovered locks
    /// the recompute takes. Callers must trigger the recompute off-path.
    pub(crate) fn store_forwarder_reader_status_sync(
        &self,
        endpoint_id: &str,
        status: ReaderStatus,
    ) {
        let stream_id = decode_stream_id(status.stream_id);
        // Volatile counters are excluded from the connections fingerprint (see
        // `fingerprint_reader_statuses`), so push them to the UI as a targeted
        // patch event instead.
        let _ = self
            .ui
            .ui_tx
            .send(ReceiverUiEvent::ForwarderReaderCountsUpdated(
                crate::ui_events::ForwarderReaderCounts {
                    forwarder_id: endpoint_id.to_owned(),
                    stream_id: stream_id.clone(),
                    reads_session: status.reads_session,
                    reads_total: status.reads_total,
                    reads_epoch: status.reads_epoch,
                    last_read_unix_ms: (status.last_read_unix_ms != 0)
                        .then_some(status.last_read_unix_ms),
                    last_seen_secs: status.last_seen_secs,
                },
            ));
        let reader = ReaderLiveStatus {
            stream_id: stream_id.clone(),
            connected: status.connected,
            state: status.state,
            last_read_unix_ms: (status.last_read_unix_ms != 0).then_some(status.last_read_unix_ms),
            reads_session: Some(status.reads_session),
            reads_total: Some(status.reads_total),
            reads_epoch: status.reads_epoch,
            last_seen_secs: status.last_seen_secs,
            current_epoch: status.current_epoch,
            current_epoch_created_unix_ms: status.current_epoch_created_unix_ms,
            current_epoch_start_seq: status.current_epoch_start_seq,
            current_epoch_name: status.current_epoch_name,
            hardware_reader_id: None,
            firmware_version: None,
            model: None,
            reader_info: None,
            download_progress: None,
            local_port: None,
        };
        let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
        let live_status = live_statuses.entry(endpoint_id.to_owned()).or_default();
        live_status
            .readers
            .entry(stream_id)
            .and_modify(|existing| {
                let hardware_reader_id = existing.hardware_reader_id.clone();
                let firmware_version = existing.firmware_version.clone();
                let model = existing.model.clone();
                let reader_info = existing.reader_info.clone();
                let download_progress = existing.download_progress.clone();
                let local_port = existing.local_port;
                *existing = ReaderLiveStatus {
                    hardware_reader_id,
                    firmware_version,
                    model,
                    reader_info,
                    download_progress,
                    local_port,
                    ..reader.clone()
                };
            })
            .or_insert(reader);
    }

    pub(crate) fn store_forwarder_reader_epoch_sync(
        &self,
        endpoint_id: &str,
        stream_id: &str,
        current_epoch: Option<i64>,
        current_epoch_created_unix_ms: Option<i64>,
        current_epoch_name: Option<String>,
    ) {
        let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
        let live_status = live_statuses.entry(endpoint_id.to_owned()).or_default();
        live_status
            .readers
            .entry(stream_id.to_owned())
            .and_modify(|reader| {
                if current_epoch.is_some() {
                    if reader.current_epoch.is_some() && reader.current_epoch != current_epoch {
                        // Epoch changed: the new epoch starts with zero reads
                        // until the next authoritative status delta arrives,
                        // and the previous epoch's start_seq no longer applies.
                        reader.reads_epoch = Some(0);
                        reader.current_epoch_start_seq = None;
                    }
                    reader.current_epoch = current_epoch;
                    reader.current_epoch_created_unix_ms = current_epoch_created_unix_ms;
                    // The wire name is authoritative whenever epoch info is
                    // present: absent means the epoch has no name (cleared or
                    // never set), so a stale cached name must not survive.
                    reader.current_epoch_name = current_epoch_name.clone();
                } else if current_epoch_name.is_some() {
                    reader.current_epoch_name = current_epoch_name.clone();
                }
            })
            .or_insert_with(|| ReaderLiveStatus {
                stream_id: stream_id.to_owned(),
                connected: false,
                state: "unknown".to_owned(),
                last_read_unix_ms: None,
                reads_session: None,
                reads_total: None,
                reads_epoch: None,
                last_seen_secs: None,
                current_epoch,
                current_epoch_created_unix_ms,
                current_epoch_start_seq: None,
                current_epoch_name,
                hardware_reader_id: None,
                firmware_version: None,
                model: None,
                reader_info: None,
                download_progress: None,
                local_port: None,
            });
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_reader_info(&self, endpoint_id: &str, info: ReaderInfo) {
        self.store_forwarder_reader_info_sync(endpoint_id, info);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a reader's static info with NO aggregate
    /// recompute (see [`Self::store_forwarder_reader_status_sync`]).
    pub(crate) fn store_forwarder_reader_info_sync(&self, endpoint_id: &str, info: ReaderInfo) {
        let stream_id = decode_stream_id(info.stream_id);
        let reader_info =
            parse_reader_info_json(info.reader_info_json.as_deref(), endpoint_id, &stream_id);
        let hardware = reader_info.as_ref().and_then(|info| info.hardware.as_ref());
        let hardware_reader_id = optional_non_empty(info.hardware_reader_id)
            .or_else(|| hardware.and_then(|h| h.reader_id.clone()));
        let firmware_version = optional_non_empty(info.firmware_version)
            .or_else(|| hardware.and_then(|h| h.fw_version.clone()));
        let model =
            optional_non_empty(info.model).or_else(|| hardware.and_then(|h| h.hw_code.clone()));
        let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
        let live_status = live_statuses.entry(endpoint_id.to_owned()).or_default();
        live_status
            .readers
            .entry(stream_id.clone())
            .and_modify(|reader| {
                if hardware_reader_id.is_some() {
                    reader.hardware_reader_id = hardware_reader_id.clone();
                }
                if firmware_version.is_some() {
                    reader.firmware_version = firmware_version.clone();
                }
                if model.is_some() {
                    reader.model = model.clone();
                }
                if reader_info.is_some() {
                    reader.reader_info = reader_info.clone();
                }
            })
            .or_insert_with(|| ReaderLiveStatus {
                stream_id,
                connected: false,
                state: "unknown".to_owned(),
                last_read_unix_ms: None,
                reads_session: None,
                reads_total: None,
                reads_epoch: None,
                last_seen_secs: None,
                current_epoch: None,
                current_epoch_created_unix_ms: None,
                current_epoch_start_seq: None,
                current_epoch_name: None,
                hardware_reader_id,
                firmware_version,
                model,
                reader_info,
                download_progress: None,
                local_port: None,
            });
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_download_progress(
        &self,
        endpoint_id: &str,
        progress: DownloadProgress,
    ) {
        self.store_forwarder_download_progress_sync(endpoint_id, progress);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a reader's download progress with NO aggregate
    /// recompute (see [`Self::store_forwarder_reader_status_sync`]).
    pub(crate) fn store_forwarder_download_progress_sync(
        &self,
        endpoint_id: &str,
        progress: DownloadProgress,
    ) {
        let stream_id = decode_stream_id(progress.stream_id);
        let progress_update = rt_domain::DownloadProgressUpdate {
            reader_ip: stream_id.clone(),
            state: download_state_from_str(&progress.state),
            stored_reads: (progress.total != 0).then_some(progress.total as u32),
            downloaded_reads: progress.reads_received,
            progress: progress.progress,
            total: (progress.total != 0).then_some(progress.total),
            last_read_at: None,
            error: optional_non_empty(progress.error),
        };
        let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
        let live_status = live_statuses.entry(endpoint_id.to_owned()).or_default();
        live_status
            .readers
            .entry(stream_id.clone())
            .and_modify(|reader| {
                reader.download_progress = Some(progress_update.clone());
            })
            .or_insert_with(|| ReaderLiveStatus {
                stream_id,
                connected: false,
                state: "unknown".to_owned(),
                last_read_unix_ms: None,
                reads_session: None,
                reads_total: None,
                reads_epoch: None,
                last_seen_secs: None,
                current_epoch: None,
                current_epoch_created_unix_ms: None,
                current_epoch_start_seq: None,
                current_epoch_name: None,
                hardware_reader_id: None,
                firmware_version: None,
                model: None,
                reader_info: None,
                download_progress: Some(progress_update),
                local_port: None,
            });
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_ups_status(&self, endpoint_id: &str, status: UpsStatus) {
        self.store_forwarder_ups_status_sync(endpoint_id, status);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a forwarder's UPS status with NO aggregate
    /// recompute (see [`Self::store_forwarder_reader_status_sync`]).
    pub(crate) fn store_forwarder_ups_status_sync(&self, endpoint_id: &str, status: UpsStatus) {
        let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
        live_statuses.entry(endpoint_id.to_owned()).or_default().ups = Some(UpsStatusPayload {
            on_battery: status.on_battery,
            battery_percent: status.battery_percent,
            runtime_seconds: status.runtime_seconds,
        });
    }

    /// Register the remote-config request channel for a forwarder whose live
    /// control session negotiated `CAP_REMOTE_CONFIG`. Called by the
    /// [`ForwarderConnection`](crate::p2p_forwarder) on control connect.
    pub(crate) fn register_forwarder_config_tx(
        self: &Arc<Self>,
        endpoint_id: &str,
        tx: mpsc::Sender<ConfigCommand>,
    ) -> ForwarderConfigRegistrationGuard {
        self.forwarders
            .forwarder_config_tx
            .lock()
            .unwrap()
            .insert(endpoint_id.to_owned(), tx.clone());
        ForwarderConfigRegistrationGuard {
            state: Arc::clone(self),
            endpoint_id: endpoint_id.to_owned(),
            tx,
        }
    }

    /// Drop a forwarder's remote-config channel on control disconnect/stop so
    /// subsequent config commands fail fast instead of hanging on a dead
    /// session.
    fn deregister_forwarder_config_tx(&self, endpoint_id: &str, tx: &mpsc::Sender<ConfigCommand>) {
        let mut registrations = self.forwarders.forwarder_config_tx.lock().unwrap();
        if registrations
            .get(endpoint_id)
            .is_some_and(|registered| registered.same_channel(tx))
        {
            registrations.remove(endpoint_id);
        }
    }

    pub(crate) fn forwarder_config_tx(
        &self,
        endpoint_id: &str,
    ) -> Option<mpsc::Sender<ConfigCommand>> {
        self.forwarders
            .forwarder_config_tx
            .lock()
            .unwrap()
            .get(endpoint_id)
            .cloned()
    }

    /// Whether the forwarder's live session negotiated `CAP_REMOTE_CONFIG`
    /// (mirrors the presence of its registered remote-config channel).
    /// Production callers consume the batched [`Self::forwarder_config_endpoints`]
    /// snapshot instead; this per-endpoint probe remains for tests.
    #[cfg(test)]
    pub(crate) fn forwarder_remote_config_available(&self, endpoint_id: &str) -> bool {
        self.forwarders
            .forwarder_config_tx
            .lock()
            .unwrap()
            .contains_key(endpoint_id)
    }

    /// Endpoint ids that currently have a live remote-config channel, so
    /// [`get_connections`] can list a forwarder whose only notable state is
    /// remote-config availability.
    pub(crate) fn forwarder_config_endpoints(&self) -> Vec<String> {
        self.forwarders
            .forwarder_config_tx
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// Register the reader-control request channel for a forwarder whose live
    /// control session negotiated `CAP_READER_CONTROL`.
    pub(crate) fn register_forwarder_reader_control_tx(
        self: &Arc<Self>,
        endpoint_id: &str,
        tx: mpsc::Sender<ReaderCommand>,
    ) -> ForwarderReaderControlRegistrationGuard {
        self.forwarders
            .forwarder_reader_control_tx
            .lock()
            .unwrap()
            .insert(endpoint_id.to_owned(), tx.clone());
        ForwarderReaderControlRegistrationGuard {
            state: Arc::clone(self),
            endpoint_id: endpoint_id.to_owned(),
            tx,
        }
    }

    fn deregister_forwarder_reader_control_tx(
        &self,
        endpoint_id: &str,
        tx: &mpsc::Sender<ReaderCommand>,
    ) {
        let mut registrations = self.forwarders.forwarder_reader_control_tx.lock().unwrap();
        if registrations
            .get(endpoint_id)
            .is_some_and(|registered| registered.same_channel(tx))
        {
            registrations.remove(endpoint_id);
        }
    }

    pub(crate) fn forwarder_reader_control_tx(
        &self,
        endpoint_id: &str,
    ) -> Option<mpsc::Sender<ReaderCommand>> {
        self.forwarders
            .forwarder_reader_control_tx
            .lock()
            .unwrap()
            .get(endpoint_id)
            .cloned()
    }

    pub(crate) fn forwarder_reader_control_endpoints(&self) -> Vec<String> {
        self.forwarders
            .forwarder_reader_control_tx
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// Mark a stream's data subscription as terminally failed on this
    /// forwarder's live connection (protocol/data-integrity error, see
    /// [`crate::p2p_session::P2pSessionError::is_retryable`]). Surfaced through
    /// the connections payload/fingerprint so the UI sees the failed stream via
    /// the existing `ConnectionsChanged` event.
    pub(crate) async fn mark_forwarder_stream_failed(&self, endpoint_id: &str, stream_id: &str) {
        {
            let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
            live_statuses
                .entry(endpoint_id.to_owned())
                .or_default()
                .failed_streams
                .insert(stream_id.to_owned());
        }
        self.recompute_aggregate_connection_state().await;
    }

    /// Clear a stream's terminal-failure marker (subscription config change or
    /// unsubscribe). Control disconnect clears it wholesale through
    /// [`Self::clear_forwarder_live_status`].
    pub(crate) async fn clear_forwarder_stream_failed(&self, endpoint_id: &str, stream_id: &str) {
        let removed = {
            let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
            live_statuses
                .get_mut(endpoint_id)
                .is_some_and(|status| status.failed_streams.remove(stream_id))
        };
        if removed {
            self.recompute_aggregate_connection_state().await;
        }
    }

    /// Mark a stream's data task as held by an unresolvable earliest-epoch
    /// override (fail closed). Returns `true` when the stream was not already
    /// marked, so the caller can log the transition exactly once.
    pub(crate) async fn mark_forwarder_stream_override_held(
        &self,
        endpoint_id: &str,
        stream_id: &str,
    ) -> bool {
        let inserted = {
            let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
            live_statuses
                .entry(endpoint_id.to_owned())
                .or_default()
                .override_held_streams
                .insert(stream_id.to_owned())
        };
        if inserted {
            self.emit_streams_snapshot().await;
        }
        inserted
    }

    /// Clear a stream's held-override marker (override resolved or removed).
    pub(crate) async fn clear_forwarder_stream_override_held(
        &self,
        endpoint_id: &str,
        stream_id: &str,
    ) {
        let removed = {
            let mut live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
            live_statuses
                .get_mut(endpoint_id)
                .is_some_and(|status| status.override_held_streams.remove(stream_id))
        };
        if removed {
            self.emit_streams_snapshot().await;
        }
    }

    /// Forwarder-advertised epoch options for a stream, merged from the
    /// discovered catalog and the live reader status (see
    /// [`merge_epoch_options`]). Used by the earliest-epoch picker.
    pub(crate) async fn merged_epoch_options(
        &self,
        endpoint_id: &str,
        stream_id: &str,
    ) -> Vec<DiscoveredEpochSummary> {
        let discovered = self.forwarders.discovered_forwarders.read().await;
        let discovered_stream = discovered.get(endpoint_id).and_then(|forwarder| {
            forwarder
                .streams
                .iter()
                .find(|stream| stream.stream_id == stream_id)
        });
        let live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
        let live_reader = live_statuses
            .get(endpoint_id)
            .and_then(|status| status.readers.get(stream_id));
        merge_epoch_options(discovered_stream, live_reader)
    }

    /// The live reader's `(current_epoch, current_epoch_start_seq)` pair, used
    /// as the fallback source when resolving an earliest-epoch override that
    /// targets the stream's current epoch.
    pub(crate) fn forwarder_reader_epoch_hint(
        &self,
        endpoint_id: &str,
        stream_id: &str,
    ) -> Option<(i64, Option<i64>)> {
        let live_statuses = self.forwarders.forwarder_live_status.lock().unwrap();
        let reader = live_statuses.get(endpoint_id)?.readers.get(stream_id)?;
        Some((reader.current_epoch?, reader.current_epoch_start_seq))
    }

    pub(crate) async fn clear_forwarder_live_status(&self, endpoint_id: &str) {
        {
            self.forwarders
                .forwarder_live_status
                .lock()
                .unwrap()
                .remove(endpoint_id);
        }
        self.recompute_aggregate_connection_state().await;
    }

    pub(crate) fn recompute_aggregate_connection_state_sync_default_trying(&self) {
        let statuses = self.forwarders.forwarder_runtime.lock().unwrap().clone();
        let any_connected = statuses
            .values()
            .any(|status| status.control_up || status.data_sessions > 0);
        let any_trying = {
            let disconnected = self.forwarders.disconnected_intents.lock().unwrap();
            statuses.iter().any(|(endpoint_id, status)| {
                !status.control_up
                    && status.data_sessions == 0
                    && !disconnected.contains(endpoint_id)
            })
        };
        let next = if any_connected {
            ConnectionState::Connected
        } else if any_trying {
            ConnectionState::Connecting
        } else {
            ConnectionState::Disconnected
        };
        let changed = self.signals.connection_state.send_if_modified(|state| {
            if *state == next {
                false
            } else {
                *state = next.clone();
                true
            }
        });
        if changed {
            *self
                .signals
                .pending_connection_state_side_effect
                .lock()
                .unwrap() = Some(next);
        }
    }

    pub(crate) async fn recompute_aggregate_connection_state(&self) {
        let statuses = self.forwarders.forwarder_runtime.lock().unwrap().clone();
        let (intents, subscriptions) = {
            let db = self.storage.db.lock().await;
            let intents = match db.load_forwarder_intents() {
                Ok(intents) => intents,
                Err(error) => {
                    warn!(error = %error, "failed to load forwarder intents for connection state");
                    // Match the sync fallback's error-time source of truth: the
                    // cache contains only explicit disconnect intents; missing
                    // endpoints still default to connect below.
                    self.forwarders
                        .disconnected_intents
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|endpoint_id| (endpoint_id.clone(), false))
                        .collect()
                }
            };
            let subscriptions = match db.load_stream_subscriptions() {
                Ok(subscriptions) => subscriptions,
                Err(error) => {
                    warn!(error = %error, "failed to load subscriptions for connections fingerprint");
                    Vec::new()
                }
            };
            (intents, subscriptions)
        };
        let any_connected = statuses
            .values()
            .any(|status| status.control_up || status.data_sessions > 0);
        let any_trying = statuses.iter().any(|(endpoint_id, status)| {
            !status.control_up
                && status.data_sessions == 0
                && *intents.get(endpoint_id).unwrap_or(&true)
        });
        let next = if any_connected {
            ConnectionState::Connected
        } else if any_trying {
            ConnectionState::Connecting
        } else {
            ConnectionState::Disconnected
        };
        let discovered = self.forwarders.discovered_forwarders.read().await.clone();
        let live_statuses = self
            .forwarders
            .forwarder_live_status
            .lock()
            .unwrap()
            .clone();
        let config_endpoints = self.forwarder_config_endpoints();
        let reader_control_endpoints = self.forwarder_reader_control_endpoints();
        let fingerprint = Self::connections_fingerprint(
            next.clone(),
            &statuses,
            &intents,
            &discovered,
            &subscriptions,
            &live_statuses,
            &config_endpoints,
            &reader_control_endpoints,
        );
        let connections_changed = {
            let mut last = self.forwarders.last_connections_fingerprint.lock().unwrap();
            if last.as_ref() == Some(&fingerprint) {
                false
            } else {
                *last = Some(fingerprint);
                true
            }
        };
        if connections_changed {
            let _ = self.ui.ui_tx.send(ReceiverUiEvent::ConnectionsChanged);
        }
        self.set_connection_state_if_changed(next).await;
    }

    #[allow(clippy::too_many_arguments)]
    fn connections_fingerprint(
        aggregate_state: ConnectionState,
        statuses: &HashMap<String, ForwarderRuntimeStatus>,
        intents: &HashMap<String, bool>,
        discovered: &DiscoveredForwarders,
        subscriptions: &[StreamSubscription],
        live_statuses: &HashMap<String, ForwarderLiveStatus>,
        config_endpoints: &[String],
        reader_control_endpoints: &[String],
    ) -> ConnectionsFingerprint {
        let mut endpoints: BTreeSet<String> = statuses.keys().cloned().collect();
        endpoints.extend(intents.keys().cloned());
        endpoints.extend(discovered.keys().cloned());
        endpoints.extend(live_statuses.keys().cloned());
        endpoints.extend(config_endpoints.iter().cloned());
        endpoints.extend(reader_control_endpoints.iter().cloned());

        let mut subscribed_counts: HashMap<String, usize> = HashMap::new();
        let local_ports = subscription_local_ports(subscriptions);
        for subscription in subscriptions {
            endpoints.insert(subscription.forwarder_endpoint_id.clone());
            *subscribed_counts
                .entry(subscription.forwarder_endpoint_id.clone())
                .or_default() += 1;
        }

        let forwarders = endpoints
            .into_iter()
            .map(|endpoint_id| {
                let runtime = statuses.get(&endpoint_id).copied().unwrap_or_default();
                let intent = *intents.get(&endpoint_id).unwrap_or(&true);
                let snapshot = derive_forwarder_state(runtime, intent);
                let discovered_forwarder = discovered.get(&endpoint_id);
                let live_status = live_statuses.get(&endpoint_id).cloned().unwrap_or_default();
                ForwarderConnectionsFingerprint {
                    endpoint_id: endpoint_id.clone(),
                    state: snapshot.state,
                    pending: snapshot.pending,
                    intent,
                    discovered: discovered_forwarder.is_some(),
                    display_name: discovered_forwarder
                        .and_then(|forwarder| forwarder.display_name.clone()),
                    remote_config_available: config_endpoints.contains(&endpoint_id),
                    reader_control_available: reader_control_endpoints.contains(&endpoint_id),
                    subscribed_count: subscribed_counts.get(&endpoint_id).copied().unwrap_or(0),
                    available_count: discovered_forwarder
                        .map_or(0, |forwarder| forwarder.streams.len()),
                    readers: fingerprint_reader_statuses(sorted_reader_statuses(
                        &live_status,
                        &local_ports,
                        &endpoint_id,
                    )),
                    ups: live_status.ups,
                    failed_stream_ids: live_status.failed_streams.into_iter().collect(),
                }
            })
            .collect();

        ConnectionsFingerprint {
            aggregate_state,
            forwarders,
        }
    }

    async fn set_connection_state_if_changed(&self, new_state: ConnectionState) {
        let changed = self.signals.connection_state.send_if_modified(|state| {
            *state != new_state && {
                *state = new_state.clone();
                true
            }
        });
        let should_emit = if changed {
            self.signals
                .pending_connection_state_side_effect
                .lock()
                .unwrap()
                .take();
            true
        } else {
            let mut pending = self
                .signals
                .pending_connection_state_side_effect
                .lock()
                .unwrap();
            if pending.as_ref() == Some(&new_state) {
                pending.take();
                true
            } else {
                false
            }
        };
        if should_emit {
            self.emit_connection_state_side_effects(new_state).await;
        }
    }

    pub async fn request_retry_connect(&self) {
        self.signals.retry_streak.fetch_add(1, Ordering::SeqCst);
        self.bump_connect_attempt(None, true);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_reconnect_if_connected(&self) -> bool {
        let was_connected = self.signals.connection_state.send_if_modified(|state| {
            if *state == ConnectionState::Connected {
                *state = ConnectionState::Connecting;
                true
            } else {
                false
            }
        });
        if !was_connected {
            return false;
        }
        self.signals.retry_streak.fetch_add(1, Ordering::SeqCst);
        self.bump_connect_attempt(None, true);
        self.emit_connection_state_side_effects(ConnectionState::Connecting)
            .await;
        true
    }

    pub(crate) async fn emit_connection_state_side_effects(&self, new_state: ConnectionState) {
        let streams_count = {
            let db = self.storage.db.lock().await;
            match db.load_stream_subscriptions() {
                Ok(s) => s.len(),
                Err(e) => {
                    warn!(error = %e, "failed to load subscriptions for status event");
                    0
                }
            }
        };
        let receiver_id = self.receiver_id.read().await.clone();
        let _ = self.ui.ui_tx.send(ReceiverUiEvent::StatusChanged {
            connection_state: new_state.clone(),
            streams_count,
            receiver_id,
        });
        let label = match &new_state {
            ConnectionState::Disconnected => "Disconnected",
            ConnectionState::Connecting => "Connecting",
            ConnectionState::Connected => "Connected",
            ConnectionState::Disconnecting => "Disconnecting",
        };
        self.ui.logger.log(label);
    }

    /// Update connection state, broadcast status change, and emit a log entry.
    pub async fn set_connection_state(&self, new_state: ConnectionState) {
        let _ = self.signals.connection_state.send(new_state.clone());
        self.signals
            .pending_connection_state_side_effect
            .lock()
            .unwrap()
            .take();
        self.emit_connection_state_side_effects(new_state).await;
    }

    /// Build the streams response from durable local subscriptions and cursors.
    pub async fn build_streams_response(&self) -> StreamsResponse {
        let counts_snapshot = self.ui.stream_counts.snapshot();
        let db = self.storage.db.lock().await;
        let subs = match db.load_stream_subscriptions() {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to load subscriptions for streams response");
                return StreamsResponse {
                    streams: vec![],
                    degraded: true,
                    upstream_error: Some(format!("failed to load subscriptions: {e}")),
                };
            }
        };
        let (cursors, cursors_degraded) = match db.load_stream_cursors() {
            Ok(c) => (c, false),
            Err(e) => {
                warn!(error = %e, "failed to load cursors");
                (vec![], true)
            }
        };
        let announcer_publish_streams = db.load_announcer_publish_streams().unwrap_or_default();
        let intents = db.load_forwarder_intents().unwrap_or_default();
        drop(db);

        let runtime_statuses = self.forwarders.forwarder_runtime.lock().unwrap().clone();
        let live_statuses = self
            .forwarders
            .forwarder_live_status
            .lock()
            .unwrap()
            .clone();

        let cursor_map: HashMap<&str, &crate::db::StreamCursorRecord> =
            cursors.iter().map(|c| (c.stream_id.as_str(), c)).collect();
        let discovered = self.forwarders.discovered_forwarders.read().await;
        let discovered_streams: HashMap<(&str, &str), (&DiscoveredForwarder, &DiscoveredStream)> =
            discovered
                .iter()
                .flat_map(|(endpoint_id, forwarder)| {
                    forwarder.streams.iter().map(move |stream| {
                        (
                            (endpoint_id.as_str(), stream.stream_id.as_str()),
                            (forwarder, stream),
                        )
                    })
                })
                .collect();

        let mut streams: Vec<StreamEntry> = Vec::new();

        for sub in &subs {
            let display_reader_ip = sub.reader_ip.clone().or_else(|| {
                crate::ports::reader_addr_if_port_mappable(&sub.stream_id)
                    .map(std::borrow::ToOwned::to_owned)
            });
            let display_forwarder_id = sub.forwarder_id.clone().or_else(|| {
                display_reader_ip
                    .as_ref()
                    .map(|_| sub.forwarder_endpoint_id.clone())
            });
            let port = sub.local_port_override.or_else(|| {
                display_reader_ip
                    .as_deref()
                    .and_then(crate::ports::default_port)
            });
            let local_stream_key = LocalStreamKey::new(&sub.forwarder_endpoint_id, &sub.stream_id);
            let counts = counts_snapshot.get(&local_stream_key);
            let cursor = cursor_map.get(local_stream_key.as_str());
            let discovered_pair = discovered_streams
                .get(&(sub.forwarder_endpoint_id.as_str(), sub.stream_id.as_str()))
                .copied();
            let runtime = runtime_statuses
                .get(&sub.forwarder_endpoint_id)
                .copied()
                .unwrap_or_default();
            let intent = *intents.get(&sub.forwarder_endpoint_id).unwrap_or(&true);
            let snapshot = derive_forwarder_state(runtime, intent);
            let online = Some(matches!(
                snapshot.state,
                ForwarderConnState::Connected | ForwarderConnState::Subscribed
            ));
            let live_reader = live_statuses
                .get(&sub.forwarder_endpoint_id)
                .and_then(|status| status.readers.get(&sub.stream_id));
            let reader_connected = live_reader
                .map(|reader| reader.connected)
                .or_else(|| (snapshot.state == ForwarderConnState::Subscribed).then_some(true));
            let current_epoch = live_reader.and_then(|reader| reader.current_epoch);
            let current_epoch_created_unix_ms =
                live_reader.and_then(|reader| reader.current_epoch_created_unix_ms);
            let override_held = live_statuses
                .get(&sub.forwarder_endpoint_id)
                .is_some_and(|status| status.override_held_streams.contains(&sub.stream_id));
            let discovered_stream = discovered_pair.map(|(_, stream)| stream);
            streams.push(StreamEntry {
                override_held,
                forwarder_endpoint_id: sub.forwarder_endpoint_id.clone(),
                stream_id: sub.stream_id.clone(),
                forwarder_id: display_forwarder_id,
                reader_ip: display_reader_ip,
                subscribed: true,
                local_port: port,
                local_port_override: sub.local_port_override,
                announcer_publish: announcer_publish_streams.contains(local_stream_key.as_str()),
                event_type: Some(sub.event_type),
                online,
                reader_connected,
                display_alias: discovered_pair
                    .and_then(|(forwarder, _)| forwarder.display_name.clone()),
                stream_epoch: current_epoch
                    .or_else(|| discovered_stream.map(|stream| stream.epoch)),
                epoch_options: merge_epoch_options(discovered_stream, live_reader),
                current_epoch_name: live_reader
                    .and_then(|reader| reader.current_epoch_name.clone()),
                current_epoch_created_unix_ms,
                reads_total: counts.as_ref().map(|c| c.total),
                reads_epoch: counts.as_ref().map(|c| c.epoch),
                cursor_epoch: cursor.and_then(|c| c.stream_epoch),
                cursor_seq: cursor.map(|c| c.last_seq),
            });
        }

        // Append discovered-but-unsubscribed streams as `subscribed = false`
        // so the UI can list streams that are available to subscribe to. A
        // (forwarder_endpoint_id, stream_id) already present as a subscription
        // takes precedence and is not duplicated.
        let mut seen: std::collections::HashSet<(String, String)> = streams
            .iter()
            .map(|s| (s.forwarder_endpoint_id.clone(), s.stream_id.clone()))
            .collect();
        for (endpoint_id, forwarder) in discovered.iter() {
            for stream in &forwarder.streams {
                let key = (endpoint_id.clone(), stream.stream_id.clone());
                if !seen.insert(key) {
                    continue;
                }
                let runtime = runtime_statuses
                    .get(endpoint_id)
                    .copied()
                    .unwrap_or_default();
                let intent = *intents.get(endpoint_id).unwrap_or(&true);
                let snapshot = derive_forwarder_state(runtime, intent);
                let online = Some(matches!(
                    snapshot.state,
                    ForwarderConnState::Connected | ForwarderConnState::Subscribed
                ));
                let live_reader = live_statuses
                    .get(endpoint_id)
                    .and_then(|status| status.readers.get(&stream.stream_id));
                let reader_connected = live_reader
                    .map(|reader| reader.connected)
                    .or_else(|| (snapshot.state == ForwarderConnState::Subscribed).then_some(true));
                let current_epoch = live_reader.and_then(|reader| reader.current_epoch);
                let current_epoch_created_unix_ms =
                    live_reader.and_then(|reader| reader.current_epoch_created_unix_ms);
                let override_held = live_statuses
                    .get(endpoint_id)
                    .is_some_and(|status| status.override_held_streams.contains(&stream.stream_id));
                streams.push(StreamEntry {
                    override_held,
                    forwarder_endpoint_id: endpoint_id.clone(),
                    stream_id: stream.stream_id.clone(),
                    forwarder_id: None,
                    reader_ip: None,
                    subscribed: false,
                    local_port: None,
                    local_port_override: None,
                    announcer_publish: false,
                    event_type: None,
                    online,
                    reader_connected,
                    display_alias: forwarder.display_name.clone(),
                    stream_epoch: current_epoch.or(Some(stream.epoch)),
                    epoch_options: merge_epoch_options(Some(stream), live_reader),
                    current_epoch_name: live_reader
                        .and_then(|reader| reader.current_epoch_name.clone()),
                    current_epoch_created_unix_ms,
                    reads_total: None,
                    reads_epoch: None,
                    cursor_epoch: None,
                    cursor_seq: None,
                });
            }
        }
        drop(discovered);

        let degraded = cursors_degraded;
        let upstream_error = cursors_degraded.then(|| "failed to load cursors".to_owned());
        StreamsResponse {
            streams,
            degraded,
            upstream_error,
        }
    }

    /// Build and broadcast a streams snapshot to UI clients.
    pub async fn emit_streams_snapshot(&self) {
        let response = self.build_streams_response().await;
        let _ = self.ui.ui_tx.send(ReceiverUiEvent::StreamsSnapshot {
            streams: response.streams,
            degraded: response.degraded,
            upstream_error: response.upstream_error,
        });
    }

    /// Ask UI clients to reload full state from the control API.
    pub fn emit_resync(&self) {
        let _ = self.ui.ui_tx.send(ReceiverUiEvent::Resync);
    }
}

/// RAII registration for a forwarder's live remote-config command channel.
/// Dropping the guard deregisters the channel even if the owning connection
/// task is aborted or panics.
pub(crate) struct ForwarderConfigRegistrationGuard {
    state: Arc<AppState>,
    endpoint_id: String,
    tx: mpsc::Sender<ConfigCommand>,
}

impl Drop for ForwarderConfigRegistrationGuard {
    fn drop(&mut self) {
        self.state
            .deregister_forwarder_config_tx(&self.endpoint_id, &self.tx);
        self.state
            .recompute_aggregate_connection_state_sync_default_trying();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.recompute_aggregate_connection_state().await;
            });
        }
    }
}

/// RAII registration for a forwarder's live reader-control command channel.
pub(crate) struct ForwarderReaderControlRegistrationGuard {
    state: Arc<AppState>,
    endpoint_id: String,
    tx: mpsc::Sender<ReaderCommand>,
}

impl Drop for ForwarderReaderControlRegistrationGuard {
    fn drop(&mut self) {
        self.state
            .deregister_forwarder_reader_control_tx(&self.endpoint_id, &self.tx);
        self.state
            .recompute_aggregate_connection_state_sync_default_trying();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.recompute_aggregate_connection_state().await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamRef {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamEntry {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarder_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader_ip: Option<String>,
    pub subscribed: bool,
    /// Resolved local proxy port: the stored override when present, otherwise
    /// the default reader port.
    pub local_port: Option<u16>,
    /// Stored explicit proxy-port override only; `None` means the stream uses
    /// the default port.
    /// This field must always serialize, including as `null`, for UI default
    /// handling.
    pub local_port_override: Option<u16>,
    /// Whether this stream is opted in to announcer publishing.
    pub announcer_publish: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<crate::db::EventType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader_connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_epoch: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub epoch_options: Vec<DiscoveredEpochSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_epoch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_epoch_created_unix_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads_epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_epoch: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_seq: Option<i64>,
    /// True while the stream's data task is held fail-closed because its
    /// earliest-epoch override cannot be resolved against the forwarder's
    /// advertised epochs. No data flows until the override resolves or is
    /// cleared.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub override_held: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamsResponse {
    pub streams: Vec<StreamEntry>,
    pub degraded: bool,
    pub upstream_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReaderLiveStatus {
    pub stream_id: String,
    pub connected: bool,
    pub state: String,
    pub last_read_unix_ms: Option<i64>,
    pub reads_session: Option<u64>,
    pub reads_total: Option<i64>,
    pub reads_epoch: Option<i64>,
    pub last_seen_secs: Option<u64>,
    pub current_epoch: Option<i64>,
    pub current_epoch_created_unix_ms: Option<i64>,
    pub current_epoch_start_seq: Option<i64>,
    pub current_epoch_name: Option<String>,
    pub hardware_reader_id: Option<String>,
    pub firmware_version: Option<String>,
    pub model: Option<String>,
    pub reader_info: Option<rt_domain::ReaderInfo>,
    pub download_progress: Option<rt_domain::DownloadProgressUpdate>,
    pub local_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpsStatusPayload {
    pub on_battery: bool,
    pub battery_percent: u32,
    pub runtime_seconds: i32,
}

// ---------------------------------------------------------------------------
// Handler functions (plain async, no Axum)
// ---------------------------------------------------------------------------

pub use crate::control::forwarders::*;
pub use crate::control::imports::*;
pub use crate::control::profile::*;
pub use crate::control::status::*;
pub use crate::control::subscriptions::*;

// ---------------------------------------------------------------------------
// Command & event registry (single source of truth)
//
// This registry enumerates every receiver control command and every UI event
// name in one place. It drives two transports:
//
//   * the Tauri IPC layer (`generate_handler!` in the receiver-tauri app), and
//   * the headless / test bridge mounted in T5.1 (`bridge_command_names`).
//
// A parity test (`tauri_and_bridge_command_sets_match`) asserts both transports
// expose an identical command set, and `event_names_match` asserts the emitted
// event names equal [`EVENT_NAMES`]. Keep all names snake_case and stable.
// ---------------------------------------------------------------------------

/// A single argument of a [`CommandSpec`], excluding the injected app state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandArg {
    /// Argument name as deserialized from the request payload (snake_case).
    pub name: &'static str,
    /// Rust type of the argument, for documentation / bridge codegen.
    pub ty: &'static str,
}

/// Describes one receiver control command: its stable name, its arguments
/// (excluding injected app state), and its success return type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// Stable, snake_case command name shared by every transport.
    pub name: &'static str,
    /// Arguments the caller supplies, excluding the injected app state.
    pub args: &'static [CommandArg],
    /// Success return type (the `T` of the command's `Result<T, _>`).
    pub return_type: &'static str,
}

/// Convenience for declaring a [`CommandArg`].
const fn arg(name: &'static str, ty: &'static str) -> CommandArg {
    CommandArg { name, ty }
}

/// Canonical, single-source list of every receiver control command.
///
/// This macro is the one place the full command surface is enumerated, with
/// each command's identifier, argument names/types, and return type. It is
/// `#[macro_export]`ed so the `receiver-tauri` crate can expand the very same
/// list into `tauri::generate_handler!` (and its test-only name list) without
/// maintaining a second copy.
///
/// The macro takes a callback macro path (captured as raw tokens so multi-
/// segment paths can be spliced before `!`) and invokes it with one entry per
/// command in the form `name(arg: "Ty", ...) -> "ReturnType",`. Each consumer
/// supplies an adapter macro that pattern-matches that shape and keeps only
/// the parts it needs (full metadata for [`COMMAND_REGISTRY`], just the
/// identifiers for `generate_handler!`).
#[macro_export]
macro_rules! receiver_command_list {
    ($($callback:tt)+) => {
        $($callback)+! {
            get_profile() -> "ProfileResponse",
            put_profile(body: "ProfileRequest") -> "()",
            get_mode() -> "ReceiverMode",
            put_mode(mode: "ReceiverMode") -> "()",
            get_streams() -> "StreamsResponse",
            get_stream_metrics() -> "Vec<StreamMetricsPayload>",
            put_earliest_epoch(body: "EarliestEpochRequest") -> "()",
            get_stream_epochs(
                forwarder_endpoint_id: "String",
                stream_id: "String"
            ) -> "StreamEpochsResponse",
            get_subscriptions() -> "SubscriptionsBody",
            put_subscriptions(body: "SubscriptionsBody") -> "()",
            get_status() -> "StatusResponse",
            get_connections() -> "ConnectionsResponse",
            reconnect_server() -> "()",
            connect_forwarder(endpoint_id: "String") -> "()",
            disconnect_forwarder(endpoint_id: "String") -> "()",
            reconnect_forwarder(endpoint_id: "String") -> "()",
            get_forwarder_config(endpoint_id: "String") -> "ForwarderConfigResponse",
            set_forwarder_config(
                endpoint_id: "String",
                config_json: "String"
            ) -> "ForwarderConfigSetResult",
            restart_forwarder(endpoint_id: "String") -> "ForwarderRestartResult",
            reader_get_info(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_sync_clock(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_set_epoch_name(
                endpoint_id: "String",
                stream_id: "String",
                name: "Option<String>"
            ) -> "ReaderControlResult",
            reader_advance_epoch(
                endpoint_id: "String",
                stream_id: "String",
                name: "Option<String>"
            ) -> "ReaderControlResult",
            reader_set_read_mode(
                endpoint_id: "String",
                stream_id: "String",
                mode: "ReadMode",
                timeout: "u8"
            ) -> "ReaderControlResult",
            reader_set_tto(
                endpoint_id: "String",
                stream_id: "String",
                enabled: "bool"
            ) -> "ReaderControlResult",
            reader_set_recording(
                endpoint_id: "String",
                stream_id: "String",
                enabled: "bool"
            ) -> "ReaderControlResult",
            reader_clear_records(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_start_download(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_stop_download(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_refresh(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_reconnect(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            get_version() -> "String",
            get_logs() -> "LogsResponse",
            admin_reset_cursor(body: "StreamRef") -> "()",
            admin_reset_all_cursors() -> "serde_json::Value",
            admin_reset_earliest_epoch(body: "StreamRef") -> "()",
            admin_reset_all_earliest_epochs() -> "serde_json::Value",
            admin_purge_subscriptions() -> "serde_json::Value",
            admin_update_port(body: "UpdatePortRequest") -> "()",
            admin_reset_profile() -> "()",
            admin_clear_data() -> "()",
            admin_factory_reset() -> "()",
            get_dbf_config() -> "DbfConfig",
            put_dbf_config(body: "DbfConfig") -> "()",
            clear_dbf() -> "()",
            update_subscription_event_type(
                forwarder_endpoint_id: "String",
                stream_id: "String",
                body: "EventTypeRequest"
            ) -> "()",
            import_participants(contents: "String") -> "ImportSummary",
            import_chips(contents: "String") -> "ImportSummary",
            import_participants_file(path: "String") -> "ImportSummary",
            import_chips_file(path: "String") -> "ImportSummary",
            import_participants_from_rd(dir: "String") -> "ImportSummary",
            get_rd_import_config() -> "RdImportConfig",
            put_rd_import_config(body: "RdImportConfig") -> "()",
            get_data_stats() -> "DataStats",
            set_announcer_enabled(enabled: "bool") -> "()",
            set_announcer_max_list_size(max_list_size: "u32") -> "()",
            set_stream_announcer_publish(
                forwarder_endpoint_id: "String",
                stream_id: "String",
                publish: "bool"
            ) -> "()",
        }
    };
}

/// Adapter that turns one [`receiver_command_list!`] entry into a
/// [`CommandSpec`] and collects them into [`COMMAND_REGISTRY`].
macro_rules! declare_command_registry {
    ($($name:ident ( $($arg:ident : $argty:literal),* $(,)? ) -> $ret:literal),* $(,)?) => {
        /// Canonical registry of every receiver control command.
        ///
        /// Built from [`receiver_command_list!`], the single source of truth
        /// shared with the Tauri `generate_handler!` list. The
        /// `tauri_and_bridge_command_sets_match` parity test asserts the two
        /// transports expose an identical command set.
        pub const COMMAND_REGISTRY: &[CommandSpec] = &[
            $(CommandSpec {
                name: stringify!($name),
                args: &[$(arg(stringify!($arg), $argty)),*],
                return_type: $ret,
            }),*
        ];
    };
}

receiver_command_list!(declare_command_registry);

/// Every command name in the canonical registry, in registry order.
pub fn command_names() -> Vec<&'static str> {
    COMMAND_REGISTRY.iter().map(|c| c.name).collect()
}

/// Command names the headless / test bridge (T5.1) mounts routes from.
///
/// Identical to [`command_names`]; named separately so bridge code reads from
/// an intent-revealing entry point and the parity test compares two
/// independently-derived surfaces rather than a list against itself.
pub fn bridge_command_names() -> Vec<&'static str> {
    command_names()
}

/// Look up a command spec by its stable name.
pub fn command_spec(name: &str) -> Option<&'static CommandSpec> {
    COMMAND_REGISTRY.iter().find(|c| c.name == name)
}

/// Canonical, snake_case names of every UI event the receiver emits.
///
/// These are the event names carried over both the Tauri IPC layer
/// (`ui_event_name`) and the headless / test bridge. [`event_name`] maps a
/// concrete [`ReceiverUiEvent`] to its entry here.
pub const EVENT_NAMES: &[&str] = &[
    "resync",
    "status_changed",
    "connections_changed",
    "streams_snapshot",
    "log_entry",
    "forwarder_reader_counts_updated",
    "mode_changed",
    "stream_deltas",
    "forwarder_ups_updated",
];

/// Canonical event name for a [`ReceiverUiEvent`].
///
/// This is the single source of truth for event naming, consumed by both the
/// Tauri bridge and the headless / test bridge. The match is exhaustive so that
/// adding a variant forces a naming decision at compile time.
pub fn event_name(event: &ReceiverUiEvent) -> &'static str {
    match event {
        ReceiverUiEvent::Resync => "resync",
        ReceiverUiEvent::StatusChanged { .. } => "status_changed",
        ReceiverUiEvent::ConnectionsChanged => "connections_changed",
        ReceiverUiEvent::StreamsSnapshot { .. } => "streams_snapshot",
        ReceiverUiEvent::LogEntry { .. } => "log_entry",
        ReceiverUiEvent::ForwarderReaderCountsUpdated(_) => "forwarder_reader_counts_updated",
        ReceiverUiEvent::ModeChanged { .. } => "mode_changed",
        ReceiverUiEvent::StreamDeltas { .. } => "stream_deltas",
        ReceiverUiEvent::ForwarderUpsUpdated { .. } => "forwarder_ups_updated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DEFAULT_UPDATE_MODE, Db, Profile, ReceivedEventInsert};
    use rt_domain::ReceiverMode;

    fn profile(url: &str, token: &str) -> Profile {
        Profile {
            server_url: url.to_owned(),
            token: token.to_owned(),
            update_mode: String::new(),
            receiver_id: Some("recv-1".to_owned()),
        }
    }

    fn assert_bad_request<T>(result: Result<T, ReceiverError>) {
        assert!(matches!(result, Err(ReceiverError::BadRequest(_))));
    }

    #[test]
    fn server_fields_to_persist_preserves_stored_when_env_active() {
        // Env override active: the echoed env values are ignored; the stored
        // (DB) server fields are preserved so the env token never lands in DB.
        let stored = profile("http://stored", "stored-token");
        let (url, token) = server_fields_to_persist(
            true,
            "http://env".to_owned(),
            "env-token".to_owned(),
            Some(&stored),
        );
        assert_eq!(url, "http://stored");
        assert_eq!(token, "stored-token");

        // No stored profile + env active: persist empty, never the env token.
        let (url, token) =
            server_fields_to_persist(true, "http://env".to_owned(), "env-token".to_owned(), None);
        assert_eq!(url, "");
        assert_eq!(token, "");
    }

    #[test]
    fn server_fields_to_persist_uses_body_when_no_env_override() {
        let stored = profile("http://stored", "stored-token");
        let (url, token) = server_fields_to_persist(
            false,
            "http://body".to_owned(),
            "body-token".to_owned(),
            Some(&stored),
        );
        assert_eq!(url, "http://body");
        assert_eq!(token, "body-token");
    }

    async fn count_connections_changed_events(
        ui_rx: &mut tokio::sync::broadcast::Receiver<ReceiverUiEvent>,
    ) -> usize {
        let mut count = 0;
        while let Ok(Ok(event)) =
            tokio::time::timeout(std::time::Duration::from_millis(25), ui_rx.recv()).await
        {
            if matches!(event, ReceiverUiEvent::ConnectionsChanged) {
                count += 1;
            }
        }
        count
    }

    #[test]
    fn derive_forwarder_state_pending_grace_expires_to_unavailable() {
        // Intent to connect, control not up, and the pending clock was started
        // more than the grace window ago: the forwarder must read as
        // Unavailable with `pending=false` (the grace has elapsed).
        let runtime = ForwarderRuntimeStatus {
            control_up: false,
            data_sessions: 0,
            pending_started_at: Some(std::time::Instant::now() - std::time::Duration::from_secs(6)),
        };
        let snapshot = derive_forwarder_state(runtime, true);
        assert_eq!(snapshot.state, ForwarderConnState::Unavailable);
        assert!(
            !snapshot.pending,
            "grace older than FORWARDER_PENDING_GRACE must clear pending"
        );
    }

    #[test]
    fn derive_forwarder_state_within_grace_stays_pending() {
        // Same as above but the pending clock just started: still within the
        // grace window, so `pending=true` (not yet treated as Unavailable to
        // the user, even though the underlying state is Unavailable).
        let runtime = ForwarderRuntimeStatus {
            control_up: false,
            data_sessions: 0,
            pending_started_at: Some(std::time::Instant::now()),
        };
        let snapshot = derive_forwarder_state(runtime, true);
        assert_eq!(snapshot.state, ForwarderConnState::Unavailable);
        assert!(
            snapshot.pending,
            "a freshly started grace clock must keep pending=true"
        );
    }

    #[tokio::test]
    async fn import_participants_and_chips_populate_lookup() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let p = import_participants(&state, ";hdr\n1,Smith,John,,,M\n2,Doe,Jane\n".to_owned())
            .await
            .unwrap();
        assert_eq!(p.imported, 2);
        let c = import_chips(&state, "BIB,CHIP\n1,0580\n2,0581\n".to_owned())
            .await
            .unwrap();
        assert_eq!(c.imported, 2);
        assert_eq!(c.resolvable_chips, 2);
        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 2);
        assert_eq!(stats.chips, 2);
        assert_eq!(stats.matched_participants, 2);
        assert_eq!(stats.participants_without_chips, 0);
        assert_eq!(stats.resolvable_chips, 2);
        let lookup = state.chip_lookup.read().await;
        let resolved = lookup
            .values()
            .find_map(|chips| chips.get("0580"))
            .expect("chip resolves");
        assert_eq!(resolved.bib, "1");
        assert_eq!(resolved.name.as_deref(), Some("John Smith"));
    }

    #[tokio::test]
    async fn rd_import_updates_stats_and_requests_ui_resync() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut ui_rx = state.ui.ui_tx.subscribe();
        let import = crate::rd_dbf::RdImport {
            participants: vec![crate::participants::Participant {
                bib: 7,
                last: "Runner".to_owned(),
                first: "Road".to_owned(),
                affiliation: String::new(),
                gender: "X".to_owned(),
                division: None,
            }],
            chips: vec![(7, "058003799177".to_owned())],
            divisions: std::collections::HashMap::new(),
        };

        apply_rd_import(&state, import).await.unwrap();

        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 1);
        assert_eq!(stats.chips, 1);
        assert_eq!(stats.resolvable_chips, 1);
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), ui_rx.recv())
            .await
            .expect("RD import should request a UI resync")
            .expect("UI event channel should stay open");
        assert!(matches!(event, ReceiverUiEvent::Resync));
    }

    #[tokio::test]
    async fn import_participants_file_reads_selected_path() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, b"1,Smith,John\n").unwrap();

        let summary = import_participants_file(&state, file.path().to_string_lossy().into_owned())
            .await
            .unwrap();

        assert_eq!(summary.imported, 1);
        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 1);
    }

    #[tokio::test]
    async fn import_participants_file_falls_back_to_windows_1252() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, b"1,Dupont,Ren\xE9\n").unwrap();

        import_participants_file(&state, file.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        import_chips(&state, "1,aaaa\n".to_owned()).await.unwrap();

        let lookup = state.chip_lookup.read().await;
        let resolved = lookup
            .values()
            .find_map(|chips| chips.get("aaaa"))
            .expect("chip resolves");
        assert_eq!(resolved.bib, "1");
        assert_eq!(resolved.name.as_deref(), Some("René Dupont"));
    }

    #[tokio::test]
    async fn data_stats_reports_participants_missing_chips() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        import_participants(&state, "1,Smith,John\n2,Doe,Jane\n3,Roe,Rich\n".to_owned())
            .await
            .unwrap();
        import_chips(&state, "1,aaa\n2,bbb\n99,deadbeef\n".to_owned())
            .await
            .unwrap();

        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 3);
        assert_eq!(stats.chips, 3);
        assert_eq!(stats.matched_participants, 2);
        assert_eq!(stats.participants_without_chips, 1);
        assert_eq!(stats.resolvable_chips, 2);
    }

    #[tokio::test]
    async fn malformed_import_is_rejected_without_mutation() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        // Seed a good participant set first.
        import_participants(&state, "1,Smith,John\n".to_owned())
            .await
            .unwrap();
        // A malformed row must reject the whole upload and not touch the table.
        let err = import_participants(&state, "2,Good,Row\nz,bad,bib\n".to_owned())
            .await
            .unwrap_err();
        assert!(matches!(err, ReceiverError::BadRequest(_)));
        // Verify bib 2 was never written: chips for bib 1 and bib 2 should leave
        // only bib 1 resolvable.
        let summary = import_chips(&state, "1,aaa\n2,bbb\n".to_owned())
            .await
            .unwrap();
        assert_eq!(summary.imported, 2);
        assert_eq!(
            summary.resolvable_chips, 1,
            "only bib 1 should resolve; the rejected import must not have added bib 2"
        );
        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 1);
        assert_eq!(stats.chips, 2);
        assert_eq!(stats.matched_participants, 1);
        assert_eq!(stats.participants_without_chips, 0);
        assert_eq!(stats.resolvable_chips, 1);
    }

    #[tokio::test]
    async fn reload_chip_lookup_populates_resolver() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        {
            let mut db = state.storage.db.lock().await;
            db.replace_participants(&[crate::participants::Participant {
                bib: 12,
                last: "Runner".to_owned(),
                first: "Fast".to_owned(),
                affiliation: String::new(),
                gender: "X".to_owned(),
                division: None,
            }])
            .unwrap();
            db.replace_bib_chips(&[(12, "chip-12".to_owned())]).unwrap();
        }
        let count = reload_chip_lookup(&state).await.unwrap();
        assert_eq!(count, 1);
        let lookup = state.chip_lookup.read().await;
        let resolved = lookup
            .values()
            .find_map(|chips| chips.get("chip-12"))
            .expect("chip resolves");
        assert_eq!(resolved.bib, "12");
        assert_eq!(resolved.name.as_deref(), Some("Fast Runner"));
    }

    #[tokio::test]
    async fn server_config_version_signal_is_observable() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut rx = state.server_config_rx();
        state.notify_server_config_changed();
        assert!(rx.changed().await.is_ok());
    }

    #[tokio::test]
    async fn put_profile_signals_server_config_change() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut rx = state.server_config_rx();
        put_profile(
            &state,
            ProfileRequest {
                server_url: "http://127.0.0.1:8080".to_owned(),
                token: "tok".to_owned(),
                receiver_id: None,
            },
        )
        .await
        .expect("put_profile ok");
        assert!(rx.changed().await.is_ok());
    }

    #[test]
    fn command_registry_names_are_unique() {
        let mut names: Vec<&str> = command_names();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            count,
            "COMMAND_REGISTRY contains duplicate names"
        );
    }

    #[test]
    fn command_registry_names_are_snake_case() {
        for spec in COMMAND_REGISTRY {
            assert!(
                spec.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "command name {:?} is not snake_case",
                spec.name
            );
        }
    }

    #[test]
    fn command_specs_have_return_type_and_unique_arg_names() {
        for spec in COMMAND_REGISTRY {
            assert!(
                !spec.return_type.is_empty(),
                "command {:?} has an empty return_type",
                spec.name
            );
            let mut arg_names: Vec<&str> = spec.args.iter().map(|a| a.name).collect();
            let total = arg_names.len();
            arg_names.sort_unstable();
            arg_names.dedup();
            assert_eq!(
                arg_names.len(),
                total,
                "command {:?} has duplicate argument names",
                spec.name
            );
            for a in spec.args {
                assert!(
                    !a.name.is_empty(),
                    "command {:?} has an empty argument name",
                    spec.name
                );
                assert!(
                    !a.ty.is_empty(),
                    "command {:?} arg {:?} has an empty type",
                    spec.name,
                    a.name
                );
            }
        }
    }

    #[test]
    fn command_spec_lookup_round_trips() {
        for spec in COMMAND_REGISTRY {
            assert_eq!(command_spec(spec.name), Some(spec));
        }
        assert_eq!(command_spec("does_not_exist"), None);
    }

    #[test]
    fn command_registry_uses_stream_identity_args_for_subscription_updates() {
        let spec = command_spec("update_subscription_event_type").unwrap();
        let arg_names: Vec<&str> = spec.args.iter().map(|arg| arg.name).collect();
        assert_eq!(
            arg_names,
            vec!["forwarder_endpoint_id", "stream_id", "body"]
        );
        assert!(!arg_names.contains(&"reader_ip"));
    }

    #[test]
    fn event_name_parity_covers_every_variant_bidirectionally() {
        use crate::ui_events::{
            ForwarderReaderCounts, LastRead, StreamDelta, StreamMetricsPayload,
        };

        let metrics = StreamMetricsPayload {
            forwarder_endpoint_id: "e".to_owned(),
            stream_id: "s".to_owned(),
            forwarder_id: "f".to_owned(),
            reader_ip: "ip".to_owned(),
            raw_count: 0,
            dedup_count: 0,
            retransmit_count: 0,
            lag_ms: None,
            epoch_raw_count: 0,
            epoch_dedup_count: 0,
            epoch_retransmit_count: 0,
            unique_chips: 0,
            epoch_last_received_at: None,
            epoch_lag_ms: None,
        };
        let last_read = LastRead {
            forwarder_id: "f".to_owned(),
            reader_ip: "ip".to_owned(),
            chip_id: "c".to_owned(),
            timestamp: "t".to_owned(),
            bib: None,
            name: None,
            division: None,
        };
        // One constructed instance of every `ReceiverUiEvent` variant.
        let samples = vec![
            ReceiverUiEvent::Resync,
            ReceiverUiEvent::StatusChanged {
                connection_state: ConnectionState::Connected,
                streams_count: 0,
                receiver_id: "recv".to_owned(),
            },
            ReceiverUiEvent::ConnectionsChanged,
            ReceiverUiEvent::StreamsSnapshot {
                streams: vec![],
                degraded: false,
                upstream_error: None,
            },
            ReceiverUiEvent::LogEntry {
                entry: "x".to_owned(),
            },
            ReceiverUiEvent::ForwarderReaderCountsUpdated(ForwarderReaderCounts {
                forwarder_id: "f".to_owned(),
                stream_id: "ip".to_owned(),
                reads_session: 0,
                reads_total: 0,
                reads_epoch: None,
                last_read_unix_ms: None,
                last_seen_secs: None,
            }),
            ReceiverUiEvent::ModeChanged {
                mode: rt_domain::ReceiverMode::Race {
                    race_id: "r".to_owned(),
                },
            },
            ReceiverUiEvent::StreamDeltas {
                updates: vec![StreamDelta {
                    forwarder_endpoint_id: "e".to_owned(),
                    stream_id: "s".to_owned(),
                    forwarder_id: "f".to_owned(),
                    reader_ip: "ip".to_owned(),
                    reads_total: 0,
                    reads_epoch: 0,
                    metrics,
                    last_read: Some(last_read),
                }],
            },
            ReceiverUiEvent::ForwarderUpsUpdated {
                forwarder_id: "f".to_owned(),
                available: false,
                status: None,
            },
        ];

        // Compile-time exhaustiveness guard: adding a `ReceiverUiEvent`
        // variant breaks this match (no wildcard arm) until it is listed
        // here, and the bidirectional set assertion below then fails until
        // both a sample above and an EVENT_NAMES entry exist for it.
        for event in &samples {
            match event {
                ReceiverUiEvent::Resync
                | ReceiverUiEvent::StatusChanged { .. }
                | ReceiverUiEvent::ConnectionsChanged
                | ReceiverUiEvent::StreamsSnapshot { .. }
                | ReceiverUiEvent::LogEntry { .. }
                | ReceiverUiEvent::ForwarderReaderCountsUpdated(_)
                | ReceiverUiEvent::ModeChanged { .. }
                | ReceiverUiEvent::StreamDeltas { .. }
                | ReceiverUiEvent::ForwarderUpsUpdated { .. } => {}
            }
        }

        let emitted: BTreeSet<&str> = samples.iter().map(event_name).collect();
        let canonical: BTreeSet<&str> = EVENT_NAMES.iter().copied().collect();
        assert_eq!(
            emitted,
            canonical,
            "event_name output and EVENT_NAMES diverged.\nEmitted-only: {:?}\nCanonical-only: {:?}",
            emitted.difference(&canonical).collect::<Vec<_>>(),
            canonical.difference(&emitted).collect::<Vec<_>>(),
        );
        // Every sample is one distinct variant; distinct variants must not
        // share a name.
        assert_eq!(
            emitted.len(),
            samples.len(),
            "distinct variants must map to distinct event names"
        );
    }

    #[test]
    fn event_names_are_unique_and_snake_case() {
        let mut names = EVENT_NAMES.to_vec();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "EVENT_NAMES contains duplicates");
        for name in EVENT_NAMES {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "event name {name:?} is not snake_case"
            );
        }
    }

    #[test]
    fn connections_changed_maps_to_expected_event_name() {
        assert_eq!(
            event_name(&ReceiverUiEvent::ConnectionsChanged),
            "connections_changed"
        );
    }

    #[test]
    fn assemble_forwarder_connection_statuses_derives_state_from_batched_intents() {
        let endpoints: BTreeSet<String> = ["endpoint-a", "endpoint-b", "endpoint-c"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let discovered = DiscoveredForwarders::new();
        let mut runtime_statuses = HashMap::new();
        runtime_statuses.insert(
            "endpoint-c".to_owned(),
            ForwarderRuntimeStatus {
                control_up: true,
                data_sessions: 0,
                pending_started_at: None,
            },
        );
        // The batch-loaded intent map is the only intent source: an explicit
        // disconnect intent yields Disconnected; endpoints absent from the map
        // default to connect.
        let mut intents = HashMap::new();
        intents.insert("endpoint-a".to_owned(), false);
        let live_statuses = HashMap::new();
        let mut subscribed_counts = HashMap::new();
        subscribed_counts.insert("endpoint-b".to_owned(), 2usize);
        let local_ports = HashMap::new();

        let forwarders = assemble_forwarder_connection_statuses(
            endpoints,
            &discovered,
            &runtime_statuses,
            &intents,
            &live_statuses,
            &subscribed_counts,
            &local_ports,
            &["endpoint-c".to_owned()],
            &[],
        );

        assert_eq!(forwarders.len(), 3);
        assert_eq!(forwarders[0].endpoint_id, "endpoint-a");
        assert_eq!(forwarders[0].state, ForwarderConnState::Disconnected);
        assert_eq!(forwarders[1].endpoint_id, "endpoint-b");
        assert_eq!(forwarders[1].state, ForwarderConnState::Unavailable);
        assert_eq!(forwarders[1].subscribed_count, 2);
        assert_eq!(forwarders[2].endpoint_id, "endpoint-c");
        assert_eq!(forwarders[2].state, ForwarderConnState::Connected);
        assert!(forwarders[2].remote_config_available);
        assert!(!forwarders[2].reader_control_available);
    }

    #[tokio::test]
    async fn get_connections_reports_disconnect_intent_from_single_batch_load() {
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-a", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state
            .forwarders
            .discovered_forwarders
            .write()
            .await
            .extend([
                (
                    "endpoint-a".to_owned(),
                    DiscoveredForwarder {
                        display_name: None,
                        direct_addrs: Vec::new(),
                        streams: Vec::new(),
                    },
                ),
                (
                    "endpoint-b".to_owned(),
                    DiscoveredForwarder {
                        display_name: None,
                        direct_addrs: Vec::new(),
                        streams: Vec::new(),
                    },
                ),
            ]);

        let response = get_connections(&state).await;

        let by_id = |id: &str| {
            response
                .forwarders
                .iter()
                .find(|f| f.endpoint_id == id)
                .unwrap_or_else(|| panic!("forwarder {id} missing from response"))
        };
        assert_eq!(by_id("endpoint-a").state, ForwarderConnState::Disconnected);
        assert_eq!(by_id("endpoint-b").state, ForwarderConnState::Unavailable);
    }

    #[tokio::test]
    async fn get_connections_reports_forwarder_reader_and_ups_live_status() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-a".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: Some(9100),
            event_type: crate::db::EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.forwarders.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );

        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-b".to_vec(),
                    connected: true,
                    state: "online".to_owned(),
                    last_read_unix_ms: 1234,
                    reads_session: 12,
                    reads_total: 120,
                    reads_epoch: Some(34),
                    last_seen_secs: Some(3),
                    current_epoch: Some(9),
                    current_epoch_created_unix_ms: Some(1_783_238_640_000),
                    current_epoch_start_seq: None,
                    current_epoch_name: Some("Race 1".to_owned()),
                },
            )
            .await;
        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: false,
                    state: "offline".to_owned(),
                    last_read_unix_ms: 0,
                    reads_session: 0,
                    reads_total: 0,
                    reads_epoch: None,
                    last_seen_secs: None,
                    current_epoch: None,
                    current_epoch_created_unix_ms: None,
                    current_epoch_start_seq: None,
                    current_epoch_name: None,
                },
            )
            .await;
        let rich_reader_info = rt_domain::ReaderInfo {
            hardware: Some(rt_domain::HardwareInfo {
                fw_version: Some("1.2.3".to_owned()),
                hw_code: Some("IPICO".to_owned()),
                reader_id: Some("reader-42".to_owned()),
            }),
            config: Some(rt_domain::Config3Info {
                mode: rt_domain::ReadMode::Event,
                timeout: 7,
            }),
            tto_enabled: Some(true),
            ..rt_domain::ReaderInfo {
                banner: None,
                hardware: None,
                config: None,
                tto_enabled: None,
                clock: None,
                estimated_stored_reads: None,
                recording: None,
                connect_failures: 0,
            }
        };
        state
            .record_forwarder_reader_info(
                "endpoint-a",
                rt_p2p_protocol::ReaderInfo {
                    stream_id: b"stream-a".to_vec(),
                    hardware_reader_id: "reader-42".to_owned(),
                    firmware_version: "1.2.3".to_owned(),
                    model: "IPICO".to_owned(),
                    reader_info_json: Some(serde_json::to_string(&rich_reader_info).unwrap()),
                },
            )
            .await;
        state
            .record_forwarder_download_progress(
                "endpoint-a",
                rt_p2p_protocol::DownloadProgress {
                    stream_id: b"stream-a".to_vec(),
                    downloaded_bytes: 0,
                    total_bytes: 0,
                    state: "downloading".to_owned(),
                    reads_received: 42,
                    progress: 42,
                    total: 100,
                    error: String::new(),
                },
            )
            .await;
        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: true,
                    state: "online".to_owned(),
                    last_read_unix_ms: 5678,
                    reads_session: 13,
                    reads_total: 121,
                    reads_epoch: Some(35),
                    last_seen_secs: Some(1),
                    current_epoch: Some(10),
                    current_epoch_created_unix_ms: Some(1_783_238_700_000),
                    current_epoch_start_seq: None,
                    current_epoch_name: Some("Race 2".to_owned()),
                },
            )
            .await;
        state
            .record_forwarder_ups_status(
                "endpoint-a",
                rt_p2p_protocol::UpsStatus {
                    on_battery: true,
                    battery_percent: 87,
                    runtime_seconds: 1200,
                },
            )
            .await;

        let response = get_connections(&state).await;

        let forwarder = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .expect("forwarder should be present");
        assert_eq!(forwarder.readers.len(), 2);
        assert_eq!(forwarder.readers[0].stream_id, "stream-a");
        assert!(forwarder.readers[0].connected);
        assert_eq!(forwarder.readers[0].state, "online");
        assert_eq!(forwarder.readers[0].last_read_unix_ms, Some(5678));
        assert_eq!(forwarder.readers[0].reads_session, Some(13));
        assert_eq!(forwarder.readers[0].reads_total, Some(121));
        assert_eq!(forwarder.readers[0].last_seen_secs, Some(1));
        assert_eq!(forwarder.readers[0].current_epoch, Some(10));
        assert_eq!(
            forwarder.readers[0].current_epoch_created_unix_ms,
            Some(1_783_238_700_000)
        );
        assert_eq!(
            forwarder.readers[0].current_epoch_name.as_deref(),
            Some("Race 2")
        );
        assert_eq!(forwarder.readers[0].local_port, Some(9100));
        assert_eq!(
            forwarder.readers[0].hardware_reader_id.as_deref(),
            Some("reader-42")
        );
        assert_eq!(
            forwarder.readers[0].firmware_version.as_deref(),
            Some("1.2.3")
        );
        assert_eq!(forwarder.readers[0].model.as_deref(), Some("IPICO"));
        assert_eq!(forwarder.readers[0].reader_info, Some(rich_reader_info));
        assert_eq!(
            forwarder.readers[0].download_progress,
            Some(rt_domain::DownloadProgressUpdate {
                reader_ip: "stream-a".to_owned(),
                state: rt_domain::DownloadState::Downloading,
                stored_reads: Some(100),
                downloaded_reads: 42,
                progress: 42,
                total: Some(100),
                last_read_at: None,
                error: None,
            })
        );
        assert_eq!(forwarder.readers[1].stream_id, "stream-b");
        assert!(forwarder.readers[1].connected);
        assert_eq!(forwarder.readers[1].last_read_unix_ms, Some(1234));
        assert_eq!(forwarder.readers[1].reads_session, Some(12));
        assert_eq!(forwarder.readers[1].reads_total, Some(120));
        assert_eq!(forwarder.readers[1].last_seen_secs, Some(3));
        assert_eq!(forwarder.readers[1].current_epoch, Some(9));
        assert_eq!(
            forwarder.readers[1].current_epoch_created_unix_ms,
            Some(1_783_238_640_000)
        );
        assert_eq!(
            forwarder.readers[1].current_epoch_name.as_deref(),
            Some("Race 1")
        );
        assert_eq!(forwarder.readers[1].local_port, None);
        let ups = forwarder
            .ups
            .as_ref()
            .expect("UPS status should be present");
        assert!(ups.on_battery);
        assert_eq!(ups.battery_percent, 87);
        assert_eq!(ups.runtime_seconds, 1200);
    }

    #[tokio::test]
    async fn clearing_forwarder_live_status_removes_stale_reader_status() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.forwarders.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );
        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: true,
                    state: "online".to_owned(),
                    last_read_unix_ms: 10,
                    reads_session: 1,
                    reads_total: 10,
                    reads_epoch: None,
                    last_seen_secs: Some(1),
                    current_epoch: None,
                    current_epoch_created_unix_ms: None,
                    current_epoch_start_seq: None,
                    current_epoch_name: None,
                },
            )
            .await;

        state.clear_forwarder_live_status("endpoint-a").await;

        let response = get_connections(&state).await;
        let forwarder = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .expect("forwarder should be present");
        assert!(forwarder.readers.is_empty());
        assert!(forwarder.ups.is_none());
    }

    #[tokio::test]
    async fn live_status_updates_emit_connections_changed_only_on_actual_change() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.forwarders.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );
        state.recompute_aggregate_connection_state().await;
        let mut ui_rx = state.ui.ui_tx.subscribe();

        state
            .record_forwarder_ups_status(
                "endpoint-a",
                rt_p2p_protocol::UpsStatus {
                    on_battery: false,
                    battery_percent: 95,
                    runtime_seconds: 3600,
                },
            )
            .await;
        state
            .record_forwarder_ups_status(
                "endpoint-a",
                rt_p2p_protocol::UpsStatus {
                    on_battery: false,
                    battery_percent: 95,
                    runtime_seconds: 3600,
                },
            )
            .await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert_eq!(
            connections_changed_count, 1,
            "unchanged live status should not keep emitting connections_changed"
        );
    }

    #[tokio::test]
    async fn get_connections_returns_sorted_discovered_forwarder_statuses() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-a".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state
            .forwarders
            .discovered_forwarders
            .write()
            .await
            .extend([
                (
                    "endpoint-b".to_owned(),
                    DiscoveredForwarder {
                        display_name: Some("Finish Line".to_owned()),
                        direct_addrs: Vec::new(),
                        streams: Vec::new(),
                    },
                ),
                (
                    "endpoint-a".to_owned(),
                    DiscoveredForwarder {
                        display_name: Some("Start Line".to_owned()),
                        direct_addrs: Vec::new(),
                        streams: vec![
                            DiscoveredStream {
                                stream_id: "stream-a".to_owned(),
                                epoch: 1,
                                next_seq: 10,
                                epoch_options: Vec::new(),
                            },
                            DiscoveredStream {
                                stream_id: "stream-b".to_owned(),
                                epoch: 2,
                                next_seq: 20,
                                epoch_options: Vec::new(),
                            },
                        ],
                    },
                ),
            ]);
        state
            .mark_forwarder_runtime("endpoint-a", |status| status.data_sessions = 1)
            .await;

        let response = get_connections(&state).await;

        assert!(!response.server.configured);
        assert_eq!(response.forwarders.len(), 2);
        assert_eq!(response.forwarders[0].endpoint_id, "endpoint-a");
        assert_eq!(
            response.forwarders[0].display_name.as_deref(),
            Some("Start Line")
        );
        assert_eq!(response.forwarders[0].state, ForwarderConnState::Subscribed);
        assert!(!response.forwarders[0].pending);
        assert_eq!(response.forwarders[0].subscribed_count, 1);
        assert_eq!(response.forwarders[0].available_count, 2);
        assert!(response.forwarders[0].readers.is_empty());
        assert!(response.forwarders[0].ups.is_none());
        assert_eq!(response.forwarders[0].restart_needed, None);
        assert_eq!(response.forwarders[1].endpoint_id, "endpoint-b");
        assert_eq!(
            response.forwarders[1].display_name.as_deref(),
            Some("Finish Line")
        );
        assert_eq!(
            response.forwarders[1].state,
            ForwarderConnState::Unavailable
        );
        assert!(!response.forwarders[1].pending);
        assert_eq!(response.forwarders[1].subscribed_count, 0);
        assert_eq!(response.forwarders[1].available_count, 0);
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_when_forwarder_state_changes() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut ui_rx = state.ui.ui_tx.subscribe();

        state
            .mark_forwarder_runtime("endpoint-a", |status| status.control_up = true)
            .await;

        let mut saw_connections_changed = false;
        for _ in 0..4 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(100), ui_rx.recv())
                .await
                .expect("forwarder state recompute should emit UI events")
                .expect("UI event channel should stay open");
            if matches!(event, ReceiverUiEvent::ConnectionsChanged) {
                saw_connections_changed = true;
                break;
            }
        }
        assert!(
            saw_connections_changed,
            "forwarder state change should emit ConnectionsChanged"
        );
    }

    #[tokio::test]
    async fn reader_count_only_updates_emit_targeted_event_without_connections_changed() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        let status =
            |reads_session: u64, reads_total: i64, connected: bool| rt_p2p_protocol::ReaderStatus {
                stream_id: b"stream-a".to_vec(),
                connected,
                state: if connected { "online" } else { "offline" }.to_owned(),
                last_read_unix_ms: 1234,
                reads_session,
                reads_total,
                reads_epoch: None,
                last_seen_secs: Some(reads_session),
                current_epoch: None,
                current_epoch_created_unix_ms: None,
                current_epoch_start_seq: None,
                current_epoch_name: None,
            };

        // Initial status: the reader appearing is a structural change.
        state
            .record_forwarder_reader_status("endpoint-a", status(1, 10, true))
            .await;

        let mut ui_rx = state.ui.ui_tx.subscribe();

        // Count-only update: counters advance, everything structural is equal.
        state
            .record_forwarder_reader_status("endpoint-a", status(2, 11, true))
            .await;

        let mut saw_counts_update = false;
        let mut connections_changed = 0;
        while let Ok(Ok(event)) =
            tokio::time::timeout(std::time::Duration::from_millis(25), ui_rx.recv()).await
        {
            match event {
                ReceiverUiEvent::ForwarderReaderCountsUpdated(update) => {
                    assert_eq!(update.forwarder_id, "endpoint-a");
                    assert_eq!(update.stream_id, "stream-a");
                    assert_eq!(update.reads_session, 2);
                    assert_eq!(update.reads_total, 11);
                    assert_eq!(update.last_read_unix_ms, Some(1234));
                    assert_eq!(update.last_seen_secs, Some(2));
                    saw_counts_update = true;
                }
                ReceiverUiEvent::ConnectionsChanged => connections_changed += 1,
                _ => {}
            }
        }
        assert!(
            saw_counts_update,
            "count-only reader status should emit ForwarderReaderCountsUpdated"
        );
        assert_eq!(
            connections_changed, 0,
            "count-only reader status must not emit ConnectionsChanged"
        );

        // A state transition is structural and still triggers ConnectionsChanged.
        state
            .record_forwarder_reader_status("endpoint-a", status(2, 11, false))
            .await;
        let connections_changed = count_connections_changed_events(&mut ui_rx).await;
        assert_eq!(
            connections_changed, 1,
            "reader state transition should emit ConnectionsChanged"
        );
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_at_most_once_when_view_is_unchanged() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.forwarders.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: vec![DiscoveredStream {
                    stream_id: "stream-a".to_owned(),
                    epoch: 1,
                    next_seq: 10,
                    epoch_options: Vec::new(),
                }],
            },
        );
        let mut ui_rx = state.ui.ui_tx.subscribe();

        state.recompute_aggregate_connection_state().await;
        state.recompute_aggregate_connection_state().await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert!(
            connections_changed_count <= 1,
            "unchanged connections view should emit at most one ConnectionsChanged event, got {connections_changed_count}"
        );
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_when_remote_config_availability_changes() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state
            .mark_forwarder_runtime("endpoint-a", |status| status.control_up = true)
            .await;
        let mut ui_rx = state.ui.ui_tx.subscribe();
        let _ = count_connections_changed_events(&mut ui_rx).await;

        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let _guard = state.register_forwarder_config_tx("endpoint-a", tx);
        state.recompute_aggregate_connection_state().await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert_eq!(
            connections_changed_count, 1,
            "remote_config_available changing should emit ConnectionsChanged"
        );
        let response = get_connections(&state).await;
        let forwarder = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .expect("forwarder should be present");
        assert!(forwarder.remote_config_available);
    }

    #[tokio::test]
    async fn config_command_fast_fails_when_session_queue_is_full() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let (held_resp_tx, _held_resp_rx) = tokio::sync::oneshot::channel();
        tx.try_send(ConfigCommand::Get { resp: held_resp_tx })
            .expect("test setup should fill the one-slot queue");
        let _guard = state.register_forwarder_config_tx("endpoint-a", tx);

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            get_forwarder_config(&state, "endpoint-a".to_owned()),
        )
        .await;

        assert!(
            result.is_ok(),
            "full config command queue should fail fast, not wait for enqueue capacity"
        );
        assert!(result.unwrap().is_err());
    }

    #[tokio::test]
    async fn reader_control_command_routes_request_and_stores_reader_info() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let _guard = state.register_forwarder_reader_control_tx("endpoint-a", tx);
        let expected_info = rt_domain::ReaderInfo {
            clock: Some(rt_domain::ClockInfo {
                reader_clock: "2026-06-22T12:00:00Z".to_owned(),
                drift_ms: 12,
            }),
            tto_enabled: Some(true),
            ..rt_domain::ReaderInfo {
                banner: None,
                hardware: None,
                config: None,
                tto_enabled: None,
                clock: None,
                estimated_stored_reads: None,
                recording: None,
                connect_failures: 0,
            }
        };
        state
            .record_forwarder_reader_info(
                "endpoint-a",
                rt_p2p_protocol::ReaderInfo {
                    stream_id: b"stream-a".to_vec(),
                    hardware_reader_id: "reader-42".to_owned(),
                    firmware_version: "1.2.3".to_owned(),
                    model: "IPICO".to_owned(),
                    reader_info_json: None,
                },
            )
            .await;

        let response_info = expected_info.clone();
        tokio::spawn(async move {
            let ReaderCommand::Request {
                stream_id,
                action,
                resp,
            } = rx.recv().await.expect("reader command");
            assert_eq!(stream_id, "stream-a");
            assert_eq!(action, rt_domain::ReaderControlAction::SyncClock);
            resp.send(ReaderControlResponse {
                stream_id: b"stream-a".to_vec(),
                request_id: "1".to_owned(),
                success: true,
                message: String::new(),
                reader_info_json: Some(serde_json::to_string(&response_info).unwrap()),
                current_epoch: None,
                current_epoch_created_unix_ms: None,
                current_epoch_name: None,
            })
            .expect("send reader response");
        });

        let result = reader_sync_clock(&state, "endpoint-a".to_owned(), "stream-a".to_owned())
            .await
            .expect("reader sync clock");

        assert!(result.success);
        assert_eq!(result.reader_info, Some(expected_info.clone()));
        let response = get_connections(&state).await;
        let reader = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .and_then(|forwarder| forwarder.readers.first())
            .expect("reader status should be populated from response reader_info_json");
        assert_eq!(reader.reader_info, Some(expected_info));
        assert_eq!(reader.hardware_reader_id.as_deref(), Some("reader-42"));
        assert_eq!(reader.firmware_version.as_deref(), Some("1.2.3"));
        assert_eq!(reader.model.as_deref(), Some("IPICO"));
    }

    #[tokio::test]
    async fn reader_control_command_stores_epoch_metadata_from_response() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let _guard = state.register_forwarder_reader_control_tx("endpoint-a", tx);

        tokio::spawn(async move {
            let ReaderCommand::Request {
                stream_id,
                action,
                resp,
            } = rx.recv().await.expect("reader command");
            assert_eq!(stream_id, "stream-a");
            assert_eq!(
                action,
                rt_domain::ReaderControlAction::AdvanceEpoch {
                    name: Some("Race 2".to_owned())
                }
            );
            resp.send(ReaderControlResponse {
                stream_id: b"stream-a".to_vec(),
                request_id: "1".to_owned(),
                success: true,
                message: String::new(),
                reader_info_json: None,
                current_epoch: Some(5),
                current_epoch_created_unix_ms: Some(1_783_238_640_000),
                current_epoch_name: Some("Race 2".to_owned()),
            })
            .expect("send reader response");
        });

        let result = reader_advance_epoch(
            &state,
            "endpoint-a".to_owned(),
            "stream-a".to_owned(),
            Some("Race 2".to_owned()),
        )
        .await
        .expect("reader advance epoch");

        assert!(result.success);
        assert_eq!(result.current_epoch, Some(5));
        assert_eq!(
            result.current_epoch_created_unix_ms,
            Some(1_783_238_640_000)
        );
        assert_eq!(result.current_epoch_name.as_deref(), Some("Race 2"));
        let response = get_connections(&state).await;
        let reader = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .and_then(|forwarder| forwarder.readers.first())
            .expect("reader status should be populated from response epoch metadata");
        assert_eq!(reader.current_epoch, Some(5));
        assert_eq!(
            reader.current_epoch_created_unix_ms,
            Some(1_783_238_640_000)
        );
        assert_eq!(reader.current_epoch_name.as_deref(), Some("Race 2"));
    }

    #[tokio::test]
    async fn reader_control_command_rejects_invalid_reader_info_json() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let _guard = state.register_forwarder_reader_control_tx("endpoint-a", tx);
        tokio::spawn(async move {
            let ReaderCommand::Request { resp, .. } = rx.recv().await.expect("reader command");
            resp.send(ReaderControlResponse {
                stream_id: b"stream-a".to_vec(),
                request_id: "1".to_owned(),
                success: true,
                message: String::new(),
                reader_info_json: Some("{".to_owned()),
                current_epoch: None,
                current_epoch_created_unix_ms: None,
                current_epoch_name: None,
            })
            .expect("send reader response");
        });

        let result = reader_refresh(&state, "endpoint-a".to_owned(), "stream-a".to_owned()).await;

        assert!(matches!(result, Err(ReceiverError::UpstreamError(_))));
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_again_when_view_changes() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.forwarders.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );
        let mut ui_rx = state.ui.ui_tx.subscribe();

        state.recompute_aggregate_connection_state().await;
        let _ = count_connections_changed_events(&mut ui_rx).await;

        state.update_forwarder_runtime_sync("endpoint-a", |status| status.control_up = true);
        state.recompute_aggregate_connection_state().await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert_eq!(
            connections_changed_count, 1,
            "changed connections view should emit another ConnectionsChanged event"
        );
    }

    #[tokio::test]
    async fn store_forwarder_catalog_exposes_epoch_options_for_streams() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        state
            .store_forwarder_catalog(
                "endpoint-1",
                &StreamCatalog {
                    generation: 1,
                    entries: vec![rt_p2p_protocol::StreamEntry {
                        stream_id: b"stream-a".to_vec(),
                        display_name: "Finish".to_owned(),
                        network_addr: "10.0.0.1:10000".to_owned(),
                        reader_connected: true,
                        hardware_reader_id: "R1".to_owned(),
                        epoch_summaries: vec![
                            rt_p2p_protocol::StreamEpochSummary {
                                epoch: 2,
                                created_unix_ms: Some(1_783_238_640_000),
                                start_seq: 42,
                                end_seq: None,
                                name: Some("Race 2".to_owned()),
                            },
                            rt_p2p_protocol::StreamEpochSummary {
                                epoch: 1,
                                created_unix_ms: Some(1_783_235_000_000),
                                start_seq: 1,
                                end_seq: Some(41),
                                name: None,
                            },
                        ],
                    }],
                },
            )
            .await;

        let response = state.build_streams_response().await;

        assert_eq!(response.streams.len(), 1);
        assert_eq!(response.streams[0].stream_epoch, Some(2));
        assert_eq!(
            response.streams[0].epoch_options,
            vec![
                DiscoveredEpochSummary {
                    stream_epoch: 2,
                    created_unix_ms: Some(1_783_238_640_000),
                    start_seq: Some(42),
                    name: Some("Race 2".to_owned()),
                },
                DiscoveredEpochSummary {
                    stream_epoch: 1,
                    created_unix_ms: Some(1_783_235_000_000),
                    start_seq: Some(1),
                    name: None,
                },
            ]
        );
    }

    #[tokio::test]
    async fn epoch_sync_clear_name_is_authoritative_when_epoch_present() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        // A named epoch arrives (e.g. set_epoch_name response).
        state.store_forwarder_reader_epoch_sync(
            "endpoint-1",
            "stream-a",
            Some(3),
            Some(1_783_238_640_000),
            Some("Race 1".to_owned()),
        );
        // The name is then cleared: same epoch, absent name. Absent is
        // authoritative — the cached name must not survive.
        state.store_forwarder_reader_epoch_sync(
            "endpoint-1",
            "stream-a",
            Some(3),
            Some(1_783_238_640_000),
            None,
        );
        {
            let live = state.forwarders.forwarder_live_status.lock().unwrap();
            let reader = &live["endpoint-1"].readers["stream-a"];
            assert_eq!(reader.current_epoch, Some(3));
            assert_eq!(
                reader.current_epoch_name, None,
                "cleared name must not persist"
            );
        }

        // An epoch advance resets the epoch read counter and drops the stale
        // start_seq until the next authoritative status frame.
        state.store_forwarder_reader_epoch_sync(
            "endpoint-1",
            "stream-a",
            Some(4),
            Some(1_783_238_700_000),
            Some("Race 2".to_owned()),
        );
        {
            let live = state.forwarders.forwarder_live_status.lock().unwrap();
            let reader = &live["endpoint-1"].readers["stream-a"];
            assert_eq!(reader.current_epoch, Some(4));
            assert_eq!(reader.reads_epoch, Some(0));
            assert_eq!(reader.current_epoch_start_seq, None);
            assert_eq!(reader.current_epoch_name.as_deref(), Some("Race 2"));
        }
    }

    #[tokio::test]
    async fn store_forwarder_catalog_rejects_invalid_start_seq_for_overrides() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        state
            .store_forwarder_catalog(
                "endpoint-1",
                &StreamCatalog {
                    generation: 1,
                    entries: vec![rt_p2p_protocol::StreamEntry {
                        stream_id: b"stream-a".to_vec(),
                        display_name: "Finish".to_owned(),
                        network_addr: "10.0.0.1:10000".to_owned(),
                        reader_connected: true,
                        hardware_reader_id: "R1".to_owned(),
                        epoch_summaries: vec![
                            // start_seq 0 (not advertised / invalid).
                            rt_p2p_protocol::StreamEpochSummary {
                                epoch: 2,
                                created_unix_ms: None,
                                start_seq: 0,
                                end_seq: None,
                                name: None,
                            },
                            // start_seq beyond i64::MAX.
                            rt_p2p_protocol::StreamEpochSummary {
                                epoch: 1,
                                created_unix_ms: None,
                                start_seq: u64::MAX,
                                end_seq: None,
                                name: None,
                            },
                        ],
                    }],
                },
            )
            .await;

        let response = state.build_streams_response().await;
        let options = &response.streams[0].epoch_options;
        assert_eq!(options.len(), 2, "epochs still listed for display");
        assert!(
            options.iter().all(|option| option.start_seq.is_none()),
            "invalid start_seq must never be usable for override resolution"
        );
    }

    #[tokio::test]
    async fn build_streams_response_uses_discovered_epoch_for_subscription() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.forwarders.discovered_forwarders.write().await.insert(
            "endpoint-1".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Start Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: vec![DiscoveredStream {
                    stream_id: "stream-a".to_owned(),
                    epoch: 7,
                    next_seq: 42,
                    epoch_options: Vec::new(),
                }],
            },
        );

        let response = state.build_streams_response().await;

        assert_eq!(response.streams.len(), 1);
        assert!(response.streams[0].subscribed);
        assert_eq!(response.streams[0].stream_epoch, Some(7));
        assert_eq!(
            response.streams[0].display_alias.as_deref(),
            Some("Start Line")
        );
    }

    #[tokio::test]
    async fn build_streams_response_marks_live_subscribed_stream_online() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("fwd-1".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.forwarders.discovered_forwarders.write().await.insert(
            "endpoint-1".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Start Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: vec![DiscoveredStream {
                    stream_id: "stream-a".to_owned(),
                    epoch: 7,
                    next_seq: 42,
                    epoch_options: Vec::new(),
                }],
            },
        );
        state
            .mark_forwarder_runtime("endpoint-1", |status| {
                status.control_up = true;
                status.data_sessions = 1;
            })
            .await;
        state
            .record_forwarder_reader_status(
                "endpoint-1",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: true,
                    state: "connected".to_owned(),
                    last_read_unix_ms: 1234,
                    reads_session: 1,
                    reads_total: 1,
                    reads_epoch: Some(1),
                    last_seen_secs: Some(1),
                    current_epoch: Some(7),
                    current_epoch_created_unix_ms: Some(1_783_238_640_000),
                    current_epoch_start_seq: None,
                    current_epoch_name: Some("Race Morning".to_owned()),
                },
            )
            .await;

        let response = state.build_streams_response().await;

        assert_eq!(response.streams.len(), 1);
        assert_eq!(response.streams[0].online, Some(true));
        assert_eq!(response.streams[0].reader_connected, Some(true));
        assert_eq!(
            response.streams[0].current_epoch_name.as_deref(),
            Some("Race Morning")
        );
        assert_eq!(
            response.streams[0].current_epoch_created_unix_ms,
            Some(1_783_238_640_000)
        );
    }

    #[tokio::test]
    async fn reconnect_server_notifies_connect_watchers() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        reconnect_server(&state).await.unwrap();

        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(connect_rx.borrow().endpoint_id, None);
        assert!(connect_rx.borrow().restart);
        assert_eq!(state.current_connect_attempt(), 1);
        assert_eq!(
            state.signals.connection_state.borrow().clone(),
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn targeted_forwarder_reconnect_notifies_connect_watchers() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        state.request_forwarder_reconnect("fwd-1".to_owned()).await;

        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(connect_rx.borrow().endpoint_id.as_deref(), Some("fwd-1"));
        assert!(connect_rx.borrow().restart);
        assert_eq!(state.current_connect_attempt(), 1);
    }

    #[tokio::test]
    async fn disconnect_forwarder_sets_intent_false_preserves_subscriptions_and_notifies_ui() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "stream-1".to_owned(),
            local_port_override: Some(9100),
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut ui_rx = state.ui.ui_tx.subscribe();

        disconnect_forwarder(&state, "endpoint-1".to_owned())
            .await
            .unwrap();

        {
            let db = state.storage.db.lock().await;
            assert!(!db.forwarder_should_connect("endpoint-1").unwrap());
            assert_eq!(
                db.load_stream_subscriptions().unwrap(),
                vec![crate::db::StreamSubscription {
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    stream_id: "stream-1".to_owned(),
                    local_port_override: Some(9100),
                    event_type: crate::db::EventType::Finish,
                    forwarder_id: Some("legacy-fwd".to_owned()),
                    reader_ip: Some("10.0.0.1:10000".to_owned()),
                }]
            );
        }

        let mut saw_connections_changed = false;
        for _ in 0..4 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(100), ui_rx.recv())
                .await
                .expect("disconnect forwarder should emit UI events")
                .expect("UI event channel should stay open");
            if matches!(event, ReceiverUiEvent::ConnectionsChanged) {
                saw_connections_changed = true;
                break;
            }
        }
        assert!(
            saw_connections_changed,
            "disconnect_forwarder should emit ConnectionsChanged"
        );
    }

    #[tokio::test]
    async fn connect_forwarder_sets_intent_true_and_wakes_reconcile() {
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        connect_forwarder(&state, "endpoint-1".to_owned())
            .await
            .unwrap();

        {
            let db = state.storage.db.lock().await;
            assert!(db.forwarder_should_connect("endpoint-1").unwrap());
        }
        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(connect_rx.borrow().endpoint_id, None);
        assert!(!connect_rx.borrow().restart);
    }

    #[tokio::test]
    async fn reconnect_forwarder_sets_intent_true_and_triggers_targeted_reconnect() {
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        reconnect_forwarder(&state, "endpoint-1".to_owned())
            .await
            .unwrap();

        {
            let db = state.storage.db.lock().await;
            assert!(db.forwarder_should_connect("endpoint-1").unwrap());
        }
        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(
            connect_rx.borrow().endpoint_id.as_deref(),
            Some("endpoint-1")
        );
        assert!(connect_rx.borrow().restart);
    }

    #[tokio::test]
    async fn guard_drop_disconnect_emits_status_side_effects_once_after_sync_fallback() {
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.update_forwarder_runtime_sync("endpoint-1", |status| {
            status.control_up = true;
            status.data_sessions = 0;
        });
        state.recompute_aggregate_connection_state().await;
        assert_eq!(
            *state.signals.connection_state.borrow(),
            ConnectionState::Connected
        );

        let mut ui_rx = state.ui.ui_tx.subscribe();
        state.update_forwarder_runtime_sync("endpoint-1", |status| {
            status.control_up = false;
            status.data_sessions = 0;
            status.pending_started_at = Some(std::time::Instant::now());
        });
        state.recompute_aggregate_connection_state_sync_default_trying();
        assert_eq!(
            *state.signals.connection_state.borrow(),
            ConnectionState::Disconnected
        );

        state.recompute_aggregate_connection_state().await;

        let mut status_changed = 0;
        let mut log_entry = 0;
        while let Ok(Ok(event)) =
            tokio::time::timeout(std::time::Duration::from_millis(25), ui_rx.recv()).await
        {
            match event {
                ReceiverUiEvent::StatusChanged {
                    connection_state: ConnectionState::Disconnected,
                    ..
                } => status_changed += 1,
                ReceiverUiEvent::LogEntry { entry } if entry.contains("Disconnected") => {
                    log_entry += 1;
                }
                _ => {}
            }
        }

        assert_eq!(
            status_changed, 1,
            "guard-drop Connected→Disconnected should emit exactly one StatusChanged"
        );
        assert_eq!(
            log_entry, 1,
            "guard-drop Connected→Disconnected should emit exactly one Disconnected log entry"
        );
    }

    #[tokio::test]
    async fn sync_fallback_seeds_disconnect_intents_from_db_at_startup() {
        // Restart-shaped: the intent cache must be seeded from persisted
        // intents during AppState construction, so a forwarder disconnected
        // before a restart never shows Connecting through the sync fallback.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("receiver-test.sqlite3");
        {
            let db = Db::open(&db_path).unwrap();
            db.set_forwarder_intent("endpoint-1", false).unwrap();
        }
        let db = Db::open(&db_path).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        // Runtime status entry with no live sessions, as the reconcile loop
        // would leave behind for a known forwarder.
        state.update_forwarder_runtime_sync("endpoint-1", |_status| {});
        state.recompute_aggregate_connection_state_sync_default_trying();

        assert_eq!(
            *state.signals.connection_state.borrow(),
            ConnectionState::Disconnected
        );
    }

    #[tokio::test]
    async fn async_recompute_uses_intent_cache_when_intent_load_fails() {
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.update_forwarder_runtime_sync("endpoint-1", |_status| {});
        {
            let db = state.storage.db.lock().await;
            db.raw_execute_for_test("DROP TABLE forwarder_intent", rusqlite::params![])
                .unwrap();
        }

        state.recompute_aggregate_connection_state().await;

        assert_eq!(
            *state.signals.connection_state.borrow(),
            ConnectionState::Disconnected
        );
    }

    #[tokio::test]
    async fn connect_intent_write_restores_sync_fallback_trying() {
        // A successful connect-intent write must update the sync-fallback
        // cache so the forwarder counts as trying again.
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.update_forwarder_runtime_sync("endpoint-1", |_status| {});

        connect_forwarder(&state, "endpoint-1".to_owned())
            .await
            .unwrap();
        state.recompute_aggregate_connection_state_sync_default_trying();

        assert_eq!(
            *state.signals.connection_state.borrow(),
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn factory_reset_clears_intent_cache_for_sync_fallback() {
        // Factory reset deletes all forwarder_intent rows, restoring the
        // default-connect contract; the sync-fallback cache must follow.
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.update_forwarder_runtime_sync("endpoint-1", |_status| {});

        admin_factory_reset(&state).await.unwrap();
        state.recompute_aggregate_connection_state_sync_default_trying();

        assert_eq!(
            *state.signals.connection_state.borrow(),
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn put_subscriptions_uses_stream_identity() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        put_subscriptions(
            &state,
            SubscriptionsBody {
                subscriptions: vec![SubscriptionRequest {
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
                    local_port_override: Some(9100),
                    event_type: Some(crate::db::EventType::Start),
                    forwarder_id: None,
                    reader_ip: None,
                }],
            },
        )
        .await
        .unwrap();

        let db = state.storage.db.lock().await;
        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].forwarder_endpoint_id, "endpoint-1");
        assert_eq!(subs[0].stream_id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(subs[0].forwarder_id, None);
        assert_eq!(subs[0].reader_ip, None);
        assert_eq!(subs[0].local_port_override, Some(9100));
        assert_eq!(subs[0].event_type, crate::db::EventType::Start);
    }

    #[tokio::test]
    async fn put_subscriptions_signals_subscription_change() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let rx = state.subscriptions_rx();

        put_subscriptions(
            &state,
            SubscriptionsBody {
                subscriptions: vec![SubscriptionRequest {
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    stream_id: "stream-1".to_owned(),
                    local_port_override: None,
                    event_type: None,
                    forwarder_id: None,
                    reader_ip: None,
                }],
            },
        )
        .await
        .unwrap();

        assert!(
            rx.has_changed().unwrap(),
            "replacing subscriptions must signal the DBF worker to regenerate"
        );
    }

    #[tokio::test]
    async fn update_subscription_event_type_signals_subscription_change() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        put_subscriptions(
            &state,
            SubscriptionsBody {
                subscriptions: vec![SubscriptionRequest {
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    stream_id: "stream-1".to_owned(),
                    local_port_override: None,
                    event_type: Some(crate::db::EventType::Finish),
                    forwarder_id: None,
                    reader_ip: None,
                }],
            },
        )
        .await
        .unwrap();
        let rx = state.subscriptions_rx();

        update_subscription_event_type(
            &state,
            "endpoint-1",
            "stream-1",
            EventTypeRequest {
                event_type: crate::db::EventType::Start,
            },
        )
        .await
        .unwrap();

        assert!(
            rx.has_changed().unwrap(),
            "changing a subscription event type must signal the DBF worker to regenerate"
        );

        // A same-value update is a no-op and must NOT signal: SQLite counts
        // identity updates as changed rows, and a spurious signal resets the
        // DBF worker's pass state (full cross-stream regenerate).
        let rx = state.subscriptions_rx();
        update_subscription_event_type(
            &state,
            "endpoint-1",
            "stream-1",
            EventTypeRequest {
                event_type: crate::db::EventType::Start,
            },
        )
        .await
        .unwrap();
        assert!(
            !rx.has_changed().unwrap(),
            "a same-value event-type update must not signal the DBF worker"
        );
    }

    #[tokio::test]
    async fn put_subscriptions_rejects_blank_stream_identity_fields_without_persisting() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        for (forwarder_endpoint_id, stream_id) in [
            ("", "stream-1"),
            ("  ", "stream-1"),
            ("endpoint-1", ""),
            ("endpoint-1", "  "),
        ] {
            assert_bad_request(
                put_subscriptions(
                    &state,
                    SubscriptionsBody {
                        subscriptions: vec![SubscriptionRequest {
                            forwarder_endpoint_id: forwarder_endpoint_id.to_owned(),
                            stream_id: stream_id.to_owned(),
                            local_port_override: None,
                            event_type: Some(crate::db::EventType::Finish),
                            forwarder_id: None,
                            reader_ip: None,
                        }],
                    },
                )
                .await,
            );
        }

        let db = state.storage.db.lock().await;
        assert!(db.load_stream_subscriptions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn put_subscriptions_rejects_endpoint_separator_without_persisting() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        assert_bad_request(
            put_subscriptions(
                &state,
                SubscriptionsBody {
                    subscriptions: vec![SubscriptionRequest {
                        forwarder_endpoint_id: "endpoint\u{1f}bad".to_owned(),
                        stream_id: "stream-1".to_owned(),
                        local_port_override: None,
                        event_type: Some(crate::db::EventType::Finish),
                        forwarder_id: None,
                        reader_ip: None,
                    }],
                },
            )
            .await,
        );

        let db = state.storage.db.lock().await;
        assert!(db.load_stream_subscriptions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn put_subscriptions_rejects_zero_port_override_without_persisting() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        let result = put_subscriptions(
            &state,
            SubscriptionsBody {
                subscriptions: vec![SubscriptionRequest {
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    stream_id: "stream-1".to_owned(),
                    local_port_override: Some(0),
                    event_type: Some(crate::db::EventType::Finish),
                    forwarder_id: None,
                    reader_ip: None,
                }],
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(ReceiverError::BadRequest(message)) if message == "port must be 1-65535"
        ));
        let db = state.storage.db.lock().await;
        assert!(db.load_stream_subscriptions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_stream_epochs_uses_local_stream_key() {
        let db = Db::open_in_memory().unwrap();
        let local_stream_key = LocalStreamKey::new("endpoint-1", "stream-canonical");
        db.insert_received_event(&ReceivedEventInsert {
            stream_id: local_stream_key.as_str(),
            seq: 1,
            epoch: 1,
            raw_frame: b"raw-1",
            read_kind: "live",
            reader_timestamp: Some("2026-07-05T09:40:00Z"),
            received_unix_ms: 1_783_237_600_000,
            dbf_delivered_unix_ms: None,
            chip_id: Some("chip-1"),
        })
        .unwrap();
        db.insert_received_event(&ReceivedEventInsert {
            stream_id: local_stream_key.as_str(),
            seq: 2,
            epoch: 2,
            raw_frame: b"raw-2",
            read_kind: "live",
            reader_timestamp: Some("2026-07-05T09:51:11Z"),
            received_unix_ms: 1_783_238_271_000,
            dbf_delivered_unix_ms: None,
            chip_id: Some("chip-2"),
        })
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        let result = get_stream_epochs(
            &state,
            "endpoint-1".to_owned(),
            "stream-canonical".to_owned(),
        )
        .await
        .unwrap();

        assert_eq!(result.epochs.len(), 2);
        assert_eq!(result.epochs[0].stream_epoch, 2);
        assert_eq!(
            result.epochs[0].first_seen_at.as_deref(),
            Some("2026-07-05T09:51:11Z")
        );
        assert!(
            !result.epochs[0].selectable,
            "local-only epochs (not advertised) must not be selectable"
        );
        assert_eq!(result.epochs[1].stream_epoch, 1);
    }

    #[tokio::test]
    async fn stream_identity_handlers_reject_blank_fields() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        for (forwarder_endpoint_id, stream_id) in [
            ("", "stream-1"),
            ("  ", "stream-1"),
            ("endpoint-1", ""),
            ("endpoint-1", "  "),
        ] {
            assert_bad_request(
                get_stream_epochs(
                    &state,
                    forwarder_endpoint_id.to_owned(),
                    stream_id.to_owned(),
                )
                .await,
            );
            assert_bad_request(
                set_stream_announcer_publish(&state, forwarder_endpoint_id, stream_id, true).await,
            );
            assert_bad_request(
                admin_reset_cursor(
                    &state,
                    StreamRef {
                        forwarder_endpoint_id: forwarder_endpoint_id.to_owned(),
                        stream_id: stream_id.to_owned(),
                    },
                )
                .await,
            );
            assert_bad_request(
                admin_reset_earliest_epoch(
                    &state,
                    StreamRef {
                        forwarder_endpoint_id: forwarder_endpoint_id.to_owned(),
                        stream_id: stream_id.to_owned(),
                    },
                )
                .await,
            );
            assert_bad_request(
                put_earliest_epoch(
                    &state,
                    EarliestEpochRequest {
                        forwarder_endpoint_id: forwarder_endpoint_id.to_owned(),
                        stream_id: stream_id.to_owned(),
                        earliest_epoch: 7,
                    },
                )
                .await,
            );
        }
    }

    #[tokio::test]
    async fn admin_reset_cursor_uses_stream_id() {
        let stream_id = "127.0.0.1:10000";
        let local_stream_key = LocalStreamKey::new("forwarder-a", stream_id);
        let db = Db::open_in_memory().unwrap();
        db.jump_stream_cursor(local_stream_key.as_str(), 42)
            .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        admin_reset_cursor(
            &state,
            StreamRef {
                forwarder_endpoint_id: "forwarder-a".to_owned(),
                stream_id: stream_id.to_owned(),
            },
        )
        .await
        .unwrap();

        let db = state.storage.db.lock().await;
        assert_eq!(db.load_stream_cursor(local_stream_key.as_str()).unwrap(), 0);
    }

    #[tokio::test]
    async fn earliest_epoch_uses_stream_identity() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        put_earliest_epoch(
            &state,
            EarliestEpochRequest {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
                earliest_epoch: 7,
            },
        )
        .await
        .unwrap();

        {
            let db = state.storage.db.lock().await;
            // Canonical row is keyed by stream_id with the forwarder endpoint id.
            assert_eq!(
                db.load_stream_earliest_epochs().unwrap(),
                vec![crate::db::StreamEarliestEpoch {
                    stream_id: LocalStreamKey::new(
                        "endpoint-1",
                        "22222222-2222-2222-2222-222222222222",
                    )
                    .as_str()
                    .to_owned(),
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    earliest_epoch: 7,
                }]
            );
        }

        admin_reset_earliest_epoch(
            &state,
            StreamRef {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
            },
        )
        .await
        .unwrap();

        let db = state.storage.db.lock().await;
        assert!(db.load_stream_earliest_epochs().unwrap().is_empty());
    }

    #[tokio::test]
    async fn admin_update_port_uses_stream_identity() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "33333333-3333-3333-3333-333333333333".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        admin_update_port(
            &state,
            UpdatePortRequest {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: "33333333-3333-3333-3333-333333333333".to_owned(),
                local_port_override: Some(9200),
            },
        )
        .await
        .unwrap();

        let db = state.storage.db.lock().await;
        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs[0].local_port_override, Some(9200));
    }

    #[tokio::test]
    async fn update_subscription_event_type_uses_stream_identity() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "44444444-4444-4444-4444-444444444444".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        update_subscription_event_type(
            &state,
            "endpoint-1",
            "44444444-4444-4444-4444-444444444444",
            EventTypeRequest {
                event_type: crate::db::EventType::Start,
            },
        )
        .await
        .unwrap();

        let db = state.storage.db.lock().await;
        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs[0].event_type, crate::db::EventType::Start);
    }

    #[tokio::test]
    async fn app_state_emits_distinct_disconnect_and_terminate_shutdown_signals() {
        let db = Db::open_in_memory().unwrap();
        let (state, mut shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        state.request_disconnect_shutdown();
        shutdown_rx.changed().await.unwrap();
        assert_eq!(*shutdown_rx.borrow(), ShutdownSignal::Disconnect);

        state.request_process_shutdown();
        shutdown_rx.changed().await.unwrap();
        assert_eq!(*shutdown_rx.borrow(), ShutdownSignal::Terminate);
    }

    #[tokio::test]
    async fn admin_clear_data_notifies_dbf_watchers_and_requests_ui_resync() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile(
            "https://server.example.com",
            "tok",
            DEFAULT_UPDATE_MODE,
            Some("recv-1"),
        )
        .unwrap();
        db.save_dbf_config(&crate::db::DbfConfig {
            enabled: true,
            flush_interval_ms: crate::db::DEFAULT_DBF_FLUSH_INTERVAL_MS,
        })
        .unwrap();
        db.save_receiver_mode(&ReceiverMode::Race {
            race_id: "11111111-1111-1111-1111-111111111111".to_owned(),
        })
        .unwrap();

        let (state, _shutdown_rx) = AppState::new(db, "recv-1".to_owned());
        let mut dbf_config_rx = state.dbf_config_rx();
        let mut ui_rx = state.ui.ui_tx.subscribe();

        admin_clear_data(&state).await.unwrap();

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            dbf_config_rx.changed(),
        )
        .await
        .expect("clear data should notify DBF config subscribers")
        .expect("DBF config channel should stay open");

        let mut saw_resync = false;
        for _ in 0..3 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(100), ui_rx.recv())
                .await
                .expect("clear data should emit UI events")
                .expect("UI event channel should stay open");
            if matches!(event, ReceiverUiEvent::Resync) {
                saw_resync = true;
                break;
            }
        }
        assert!(saw_resync, "clear data should request a UI resync");
    }
}

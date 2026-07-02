//! Headless P2P receiver runtime wiring.
//!
//! Drives the real loopback/headless P2P lane that T5.4 process orchestration
//! needs. This is intentionally separate from the local UI/control runtime
//! ([`crate::runtime::run`]): it is enabled only when explicit P2P config is
//! present and never alters default behavior.
//!
//! For each canonical stream subscription
//! ([`Db::load_stream_subscriptions`]) whose `forwarder_endpoint_id` matches the
//! configured forwarder peer, the runtime starts:
//!
//! * a reconnecting [`p2p_session`](crate::p2p_session) that persists
//!   `received_events` (insert-before-ack) and broadcasts a post-commit durable
//!   hint;
//! * a durable [`LocalProxy`] that replays `received_events` and follows live
//!   hints (when a local port can be resolved);
//! * a DBF feed (when DBF config is enabled) that rebuilds the DBF file from
//!   `received_events` after each durable hint and on startup/retry;
//! * a server announcer push worker (when a server client is configured)
//!   that pushes not-yet-pushed rows under a fenced generation after each
//!   durable hint and on startup/retry.
//!
//! Subscriptions are reconciled on an interval keyed by
//! `(forwarder_endpoint_id, stream_id)`. Rows that disappear stop their
//! workers; rows whose effective worker config changes (e.g. a different local
//! proxy port via `local_port_override`/`reader_ip`, or any other subscription
//! field that affects worker behavior) have their worker torn down and rebuilt;
//! and a worker whose session task has exited non-retryably is rebuilt on the
//! next pass so it cannot linger dead in the map forever.
//!
//! ## Caveats (minimal real lane)
//!
//! * The DBF feed locks the durable store while writing the (small) DBF file.
//! * Announcer pushes run on a blocking task and resolve participants from a
//!   snapshot of the in-memory chip lookup, searching across all forwarders.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rt_iroh::{
    Endpoint, EndpointBuilder, NodeAddr, NodeId, RelayMode, SecretKey, load_or_create_secret_key,
};
use rt_p2p_protocol::{
    CAP_CONTROL_EVENTS, CAP_READER_CONTROL, CAP_REMOTE_CONFIG, Hello, MAX_FRAME_BYTES,
    SubscribeMode,
};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::announcer_push::{
    self, AnnouncerPushClient, ParticipantResolver, ResolvedParticipant, ServerAnnouncerClient,
};
use crate::cache::StreamKey;
use crate::control_api::ConnectionState;
use crate::control_api::{
    AppState, DiscoveredForwarder, DiscoveredForwarders, DiscoveredStream,
    server_device_status_for_url,
};
use crate::db::{Db, StreamSubscription};
use crate::local_proxy::LocalProxy;
use crate::p2p_forwarder::{ForwarderConnection, ForwarderDataStream};
use crate::p2p_session::{BackoffConfig, DurableBatch, SessionStatusReporter};
use crate::ports::{default_port, reader_addr_if_port_mappable};
use crate::projection::StreamProjection;
use crate::ui_events::ReceiverUiEvent;

/// Capacity of each per-stream durable-hint broadcast channel.
const HINT_CHANNEL_CAPACITY: usize = 1024;

/// Minimum allowed reconcile interval. Intervals below this are rejected by the
/// parser and by [`start_receiver_p2p`] to avoid hot-polling subscriptions. The
/// per-stream delivery retry timer (DBF + announcer) reuses the reconcile
/// interval as its bounded retry cadence, so this minimum also bounds retries.
pub const MIN_RECONCILE_INTERVAL: Duration = Duration::from_millis(50);

/// Configuration for the forwarder peer this receiver dials.
#[derive(Clone, Debug)]
pub struct ForwarderPeerConfig {
    /// The forwarder's iroh endpoint id (string node id). Also the
    /// `forwarder_endpoint_id` used to filter canonical subscriptions.
    pub node_id: String,
    /// A direct socket address where the forwarder peer can be reached.
    pub direct_addr: SocketAddr,
}

/// Configuration for the optional server client (register / takeover /
/// announcer rows).
///
/// `Debug` is implemented by hand (not derived) so the bearer token is never
/// leaked through debug logs; it is rendered as `<redacted>`.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerClientConfig {
    /// Base URL, e.g. `http://127.0.0.1:8080`.
    pub url: String,
    /// Per-device bearer token. Never logged.
    pub token: String,
}

impl std::fmt::Debug for ServerClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerClientConfig")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The receiver's iroh identity source. Decoupled from transport so a
/// persistent production identity can run with or without relays/discovery,
/// mirroring the forwarder (`services/forwarder/src/p2p`).
#[derive(Clone, Debug)]
pub enum ReceiverIdentity {
    /// Persistent production identity: load-or-create a secret key at this path.
    KeyPath(std::path::PathBuf),
    /// Deterministic loopback/dev seed.
    Seed([u8; 32]),
}

/// Full P2P receiver configuration. Presence of this in
/// [`HeadlessConfig`](crate::headless::HeadlessConfig) enables the P2P lane.
#[derive(Clone, Debug)]
pub struct P2pReceiverConfig {
    /// How this receiver's iroh secret key is sourced (persistent key path for
    /// production, or a deterministic seed for loopback/dev).
    pub identity: ReceiverIdentity,
    /// Disable iroh relays (loopback/LAN/air-gapped deployments).
    pub relay_disabled: bool,
    /// Disable iroh discovery services.
    pub discovery_disabled: bool,
    /// Optional explicit IPv4 bind address for the endpoint.
    pub bind_addr_v4: Option<std::net::SocketAddrV4>,
    /// An optional explicit forwarder peer to dial. When present it is seeded
    /// into the discovered-forwarders map at startup so the loopback/dev path
    /// works without a server. When absent, forwarders are learned entirely
    /// from the server discovery feed.
    pub forwarder: Option<ForwarderPeerConfig>,
    /// Optional server client for announcer push and forwarder discovery.
    pub server: Option<ServerClientConfig>,
    /// The raw server override (env vars for the desktop app, CLI flags for
    /// headless) captured at construction, as `(url, token)`. The reconcile
    /// loop re-resolves the effective server from `(profile, server_override)`
    /// on a config-change signal, so a headless CLI override is not silently
    /// lost when the profile is saved. Empty when there is no override source.
    pub server_override: (Option<String>, Option<String>),
    /// How often to reconcile canonical subscriptions. Must be at least
    /// [`MIN_RECONCILE_INTERVAL`]; also used as the delivery retry cadence.
    pub reconcile_interval: Duration,
}

impl P2pReceiverConfig {
    /// A bare production config: persistent key-path identity, relays and
    /// discovery enabled, no explicit forwarder, and no server. The caller
    /// sets `server` from the profile. Used for cold start so a fresh install
    /// always has a live endpoint that a later profile save can reconfigure.
    #[must_use]
    pub fn production_default(key_path: std::path::PathBuf) -> Self {
        Self {
            identity: ReceiverIdentity::KeyPath(key_path),
            relay_disabled: false,
            discovery_disabled: false,
            bind_addr_v4: None,
            forwarder: None,
            server: None,
            server_override: (None, None),
            reconcile_interval: Duration::from_millis(1000),
        }
    }
}

#[cfg(test)]
impl P2pReceiverConfig {
    /// Loopback/dev config from a seed, replicating `EndpointBuilder::test`
    /// transport (relay off, discovery off, loopback bind). `relay_disabled`
    /// is overridable so tests can exercise the transport knob independently.
    pub(crate) fn for_test_seed(seed: [u8; 32], relay_disabled: bool) -> Self {
        Self {
            identity: ReceiverIdentity::Seed(seed),
            relay_disabled,
            discovery_disabled: true,
            bind_addr_v4: Some(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::LOCALHOST,
                0,
            )),
            forwarder: None,
            server: None,
            server_override: (None, None),
            reconcile_interval: Duration::from_millis(1000),
        }
    }

    /// Production-style config using a persistent key path; relays/discovery
    /// enabled by default (transport defaults are independent of identity).
    pub(crate) fn for_test_keypath(path: std::path::PathBuf) -> Self {
        Self {
            identity: ReceiverIdentity::KeyPath(path),
            relay_disabled: false,
            discovery_disabled: false,
            bind_addr_v4: None,
            forwarder: None,
            server: None,
            server_override: (None, None),
            reconcile_interval: Duration::from_millis(1000),
        }
    }
}

/// Build the client `Hello` presented during control-plane negotiation.
fn client_hello() -> Hello {
    Hello {
        min_minor: 1,
        max_minor: 1,
        capabilities: vec![
            "data".to_owned(),
            CAP_CONTROL_EVENTS.to_owned(),
            CAP_REMOTE_CONFIG.to_owned(),
            CAP_READER_CONTROL.to_owned(),
        ],
        max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap_or(u32::MAX),
        catalog_generation: 0,
    }
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// Extra reconnect delay after server approval. Approval releases the receiver
/// to dial immediately, but the forwarder may still be applying the updated
/// receiver allow-list. One bounded follow-up reconnect resets the receiver's
/// exponential dial backoff once that allow-list has usually propagated.
const APPROVAL_FOLLOW_UP_RECONNECT_DELAY: Duration = Duration::from_millis(1_000);

/// Returns true iff this is the rising edge into "active".
fn approval_became_active(previous: Option<&str>, current: Option<&str>) -> bool {
    previous != Some("active") && current == Some("active")
}

fn receiver_pending_approval(status: &crate::control_api::ServerDeviceStatus) -> bool {
    status.reachable == Some(true) && status.approval_state.as_deref() == Some("pending")
}

fn schedule_approval_follow_up_reconnect(state: Arc<AppState>, delay: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if *state.connection_state.borrow() != ConnectionState::Connected {
            info!("server approval follow-up reconnect requested");
            state.request_connect().await;
            state.emit_resync();
        }
    });
}

/// A running P2P receiver runtime. Dropping or [`shutdown`](Self::shutdown)ing
/// it cancels all sessions, proxies, and workers.
pub struct P2pReceiverRuntime {
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl P2pReceiverRuntime {
    /// Signal shutdown and await the runtime task to completion.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.task.await;
    }
}

/// Start the headless P2P receiver runtime. Returns immediately; the endpoint
/// is bound and `p2p_node_id=<id>` is printed to stdout once ready.
pub async fn start_receiver_p2p(
    state: Arc<AppState>,
    config: P2pReceiverConfig,
) -> Result<P2pReceiverRuntime, String> {
    if config.reconcile_interval < MIN_RECONCILE_INTERVAL {
        return Err(format!(
            "reconcile interval {:?} is below the minimum {:?}",
            config.reconcile_interval, MIN_RECONCILE_INTERVAL
        ));
    }
    let secret_key = match &config.identity {
        ReceiverIdentity::KeyPath(path) => load_or_create_secret_key(path)
            .map_err(|e| format!("failed to load/create p2p key at {}: {e}", path.display()))?,
        ReceiverIdentity::Seed(seed) => SecretKey::from_bytes(seed),
    };
    let mut builder = EndpointBuilder::default().secret_key(secret_key);
    if config.relay_disabled {
        builder = builder.relay_mode(RelayMode::Disabled);
    }
    if config.discovery_disabled {
        builder = builder.clear_discovery();
    }
    if let Some(addr) = config.bind_addr_v4 {
        builder = builder.bind_addr_v4(addr);
    }
    let endpoint = builder
        .bind()
        .await
        .map_err(|e| format!("failed to bind p2p endpoint: {e}"))?;
    // Stdout line consumed by T5.4 orchestration to learn this receiver's id.
    println!("p2p_node_id={}", endpoint.node_id());
    state
        .set_p2p_endpoint_id(endpoint.node_id().to_string())
        .await;
    let local_addr = endpoint.node_addr().await;
    info!(
        p2p_node_id = %endpoint.node_id(),
        direct_addresses = ?local_addr.direct_addresses,
        "receiver p2p endpoint bound"
    );

    // Seed the discovered-forwarders map from the optional explicit forwarder
    // so the loopback/dev path (and tests) dial it without a server. The
    // discovery task (when a server is configured) refreshes the map but
    // preserves this seed if the server hasn't advertised the same endpoint.
    // An already-present entry (e.g. injected by a test) is left untouched.
    if let Some(forwarder) = &config.forwarder {
        forwarder
            .node_id
            .parse::<NodeId>()
            .map_err(|e| format!("invalid forwarder node id: {e}"))?;
        let mut discovered = state.discovered_forwarders.write().await;
        discovered
            .entry(forwarder.node_id.clone())
            .or_insert_with(|| DiscoveredForwarder {
                display_name: None,
                direct_addrs: vec![forwarder.direct_addr],
                streams: Vec::new(),
            });
    }

    // P2P is configured and the runtime is attempting to reach the forwarder:
    // surface that as Connecting until a session actually connects. A shared
    // live-session counter, threaded into every stream worker via a
    // `SessionStatusReporter`, then drives the aggregate state to Connected
    // while at least one session is up and back to Connecting when the last one
    // drops. Shutdown (below) restores Disconnected.
    state
        .set_connection_state(ConnectionState::Connecting)
        .await;
    let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run_reconcile_loop(
        state,
        config,
        endpoint,
        reporter,
        shutdown_rx,
    ));

    Ok(P2pReceiverRuntime { shutdown_tx, task })
}

/// Replace the discovered-forwarders map with just the optional explicit
/// (loopback/dev) forwarder seed. Called when the server config changes so
/// forwarders learned from the previous server are not re-dialed; the new
/// server's discovery loop (if any) then repopulates the map. An invalid seed
/// node id is dropped because it cannot be dialed.
async fn reseed_discovered_forwarders(
    state: &Arc<AppState>,
    forwarder: Option<&ForwarderPeerConfig>,
) {
    let mut discovered = state.discovered_forwarders.write().await;
    discovered.clear();
    if let Some(forwarder) = forwarder
        && forwarder.node_id.parse::<NodeId>().is_ok()
    {
        discovered.insert(
            forwarder.node_id.clone(),
            DiscoveredForwarder {
                display_name: None,
                direct_addrs: vec![forwarder.direct_addr],
                streams: Vec::new(),
            },
        );
    }
}

/// Per-stream auxiliary worker bundle (local proxy, UI projection, DBF, announcer).
struct StreamWorker {
    /// The canonical subscription this worker was built from. Reconciliation
    /// compares the desired subscription against this snapshot and rebuilds the
    /// worker if any field that affects worker behavior changed.
    sub: StreamSubscription,
    /// Cancels the session task and DBF/announcer workers.
    shutdown_tx: watch::Sender<bool>,
    /// Durable hint channel shared with the forwarder data subscription.
    hint_tx: broadcast::Sender<DurableBatch>,
    /// The durable local proxy, if a local port could be resolved.
    proxy: Option<LocalProxy>,
    /// UI projection, DBF, and announcer task handles.
    tasks: Vec<JoinHandle<()>>,
    /// Whether an announcer push worker was spawned for this worker. When the
    /// server was unavailable at worker-build time (no fenced generation
    /// yet) this is `false`, and reconciliation rebuilds the worker once a
    /// generation becomes available so announcer push can start.
    announcer_active: bool,
}

impl StreamWorker {
    async fn stop(self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(proxy) = self.proxy {
            proxy.shutdown();
        }
        let tasks = self.tasks;
        for mut task in tasks {
            // Give each task a bounded window to observe the shutdown signal,
            // then abort and drain so a wedged task cannot delay shutdown.
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

async fn run_reconcile_loop(
    state: Arc<AppState>,
    mut config: P2pReceiverConfig,
    endpoint: Endpoint,
    reporter: Arc<SessionStatusReporter>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let endpoint = Arc::new(endpoint);
    let endpoint_id = endpoint.node_id().to_string();
    let mut workers: HashMap<String, ForwarderConnection> = HashMap::new();
    let mut stream_workers: HashMap<String, StreamWorker> = HashMap::new();
    let mut connect_attempt_rx = state.connect_attempt_rx();
    let mut server_config_rx = state.server_config_rx();
    let mut force_reconnect: Option<Option<String>> = None;

    // `server_baseline` is the voucher/provisioning config that
    // `resolve_server_config` produces; it is the stable key for detecting a
    // real server-config change. `config.server` instead carries the effective
    // credential: the server-minted per-device token once one is available
    // (loaded from the profile, or freshly bootstrapped via the voucher). All
    // server-bound tasks use `config.server`. A pending receiver's `takeover` /
    // discovery 401 is tolerated and retried until an admin approves it.
    let mut server_baseline = config.server.clone();
    if let Some(thin) = config.server.clone()
        && let Some(minted) = resolve_receiver_device_token(&state, &thin, &endpoint_id).await
    {
        config.server = Some(ServerClientConfig {
            url: thin.url,
            token: minted,
        });
    }

    // When a server is configured, periodically refresh the discovered
    // forwarders map from its `GET /forwarders` feed. The task observes the
    // same shutdown signal and is awaited/aborted below. Re-spawned when the
    // server config changes (see the reconfigure branch below). The closure
    // captures local clones (not `config`) so the reconfigure branch can still
    // mutate `config.server`.
    let discovery_seed = config.forwarder.clone();
    let discovery_interval = config.reconcile_interval;
    let spawn_discovery = |thin: ServerClientConfig, shutdown: watch::Receiver<bool>| {
        tokio::spawn(run_discovery_loop(
            Arc::clone(&state),
            thin,
            discovery_seed.clone(),
            discovery_interval,
            shutdown,
            state.connect_attempt_rx(),
        ))
    };
    let mut discovery_task = config
        .server
        .clone()
        .map(|thin| spawn_discovery(thin, shutdown_rx.clone()));
    // The approval-watch task is also server-bound, so build it from a closure
    // and keep it rebindable (`let mut`) the same way as discovery, so the
    // reconfigure branch can restart it against a new server.
    let spawn_approval_watch = |thin: ServerClientConfig, shutdown: watch::Receiver<bool>| {
        tokio::spawn(run_approval_watch_loop(
            Arc::clone(&state),
            thin.url.clone(),
            discovery_interval,
            shutdown,
        ))
    };
    let mut approval_watch_task = config
        .server
        .clone()
        .map(|thin| spawn_approval_watch(thin, shutdown_rx.clone()));

    // Single cross-stream DBF worker (one per DBF file; see
    // run_shared_dbf_worker). Lives for the whole runtime; it re-resolves
    // config/subscriptions every pass, so stream worker rebuilds don't touch
    // it.
    let dbf_worker_task = tokio::spawn(run_shared_dbf_worker(
        Arc::clone(&state),
        shutdown_rx.clone(),
    ));

    // Server announcer generation, acquired by registering this endpoint and
    // taking over the announcer generation. When the server is unavailable
    // at startup the takeover is retried every reconcile pass (bounded by the
    // reconcile interval, racing the shutdown signal) until it succeeds rather
    // than permanently disabling announcer push. Workers are rebuilt once a
    // generation becomes available so they begin pushing pending rows. The HTTP
    // calls are bounded by the blocking client's connect/request timeouts.
    let mut announcer_generation: Option<i64> = None;

    loop {
        if let Some(target) = force_reconnect.take() {
            match target {
                Some(endpoint_id) => {
                    info!(%endpoint_id, "receiver p2p reconnect requested; restarting forwarder worker");
                    if let Some(worker) = workers.remove(&endpoint_id) {
                        worker.stop().await;
                    }
                }
                None => {
                    info!("receiver p2p reconnect requested; restarting forwarder workers");
                    for (_endpoint_id, worker) in workers.drain() {
                        worker.stop().await;
                    }
                    state.clear_stream_metrics_cache().await;
                    state.emit_streams_snapshot().await;
                }
            }
        }

        if announcer_generation.is_none()
            && let Some(thin) = config.server.clone()
        {
            let receiver_id = state.receiver_id.read().await.clone();
            // If the startup overlay could not mint a token (e.g. the server was
            // unreachable at boot), `config.server` still holds the bootstrap
            // voucher, which `takeover`/discovery reject. Re-attempt the
            // mint+persist here each pass until it succeeds, then adopt the
            // minted token and rebind discovery so it stops 401-ing. This is a
            // cheap no-op once a token is held (the comparison fails and the
            // persisted token short-circuits resolution).
            let thin = if config.server == server_baseline {
                match resolve_receiver_device_token(&state, &thin, &endpoint_id).await {
                    Some(minted) => {
                        let minted_server = ServerClientConfig {
                            url: thin.url.clone(),
                            token: minted,
                        };
                        config.server = Some(minted_server.clone());
                        if let Some(task) = discovery_task.take() {
                            task.abort();
                            let _ = task.await;
                        }
                        discovery_task =
                            Some(spawn_discovery(minted_server.clone(), shutdown_rx.clone()));
                        minted_server
                    }
                    None => thin,
                }
            } else {
                thin
            };
            let status = server_device_status_for_url(&state, &thin.url).await;
            if receiver_pending_approval(&status) {
                tracing::trace!("server announcer startup waiting for receiver approval");
            } else {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() { break; }
                    }
                    result = server_startup(thin, endpoint_id.clone(), receiver_id) => match result {
                        Ok(generation) => {
                            info!(generation, "server announcer startup succeeded");
                            announcer_generation = Some(generation);
                        }
                        Err(e) => {
                            warn!(error = %e, "server announcer startup failed; will retry");
                        }
                    }
                }
            }
        }

        reconcile_once(
            &state,
            &config,
            &endpoint,
            &reporter,
            announcer_generation,
            &mut workers,
            &mut stream_workers,
        )
        .await;

        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            changed = connect_attempt_rx.changed() => {
                if changed.is_ok() {
                    let attempt = connect_attempt_rx.borrow().clone();
                    if attempt.restart {
                        force_reconnect = Some(attempt.endpoint_id);
                    }
                }
            }
            changed = server_config_rx.changed() => {
                if changed.is_ok() {
                    // Re-resolve the effective server config (profile is the
                    // source of truth; env vars override). On a real change,
                    // rebind every server-bound task: restart discovery, re-run
                    // register/takeover, and rebuild stream workers so their
                    // announcer clients pick up the new server. This causes a
                    // brief session reconnect, which is acceptable and bounded.
                    let profile = state.db.lock().await.load_profile().ok().flatten();
                    // Re-resolve using the override captured at construction
                    // (env vars for desktop, CLI flags for headless) so a
                    // headless CLI override survives a profile save instead of
                    // silently falling back to the stored profile.
                    let new_server = crate::runtime::resolve_server_config(
                        profile.as_ref(),
                        config.server_override.clone(),
                    );
                    if new_server != server_baseline {
                        info!("receiver server config changed; rebinding server-bound tasks");
                        server_baseline = new_server.clone();
                        config.server = new_server;
                        // Stop all existing stream workers FIRST. Each holds an
                        // announcer push client bound to the previous server and
                        // its (higher) generation. Draining them before the
                        // fence reset below is required for correctness: were an
                        // old worker still alive, it could race the reset, push
                        // to the old server with the old generation, and re-raise
                        // the fence so the new (possibly lower) generation is
                        // permanently staled. Drain both the forwarder
                        // connections and the per-stream (announcer) workers.
                        for (_endpoint_id, worker) in workers.drain() {
                            worker.stop().await;
                        }
                        for (_stream_id, worker) in stream_workers.drain() {
                            worker.stop().await;
                        }
                        // Abort the server-bound discovery + approval-watch
                        // tasks here; both are respawned after the reseed +
                        // fence reset below so they bind to the new server.
                        if let Some(task) = discovery_task.take() {
                            task.abort();
                            let _ = task.await;
                        }
                        if let Some(task) = approval_watch_task.take() {
                            task.abort();
                            let _ = task.await;
                        }
                        // Drop forwarders learned from the old server so stale
                        // peers are not re-dialed; re-seed only the explicit
                        // (loopback/dev) forwarder. Done before respawning
                        // discovery so the new server's feed repopulates the
                        // freshly-cleared map; when the server was cleared the
                        // map stays empty so no old-server workers are rebuilt.
                        reseed_discovered_forwarders(&state, config.forwarder.as_ref()).await;
                        // Announcer generation fencing is per-stream and
                        // monotonic; a replacement server may start at a lower
                        // generation, which would otherwise permanently stale
                        // every push. With all old workers now stopped, reset
                        // the fences so the new server's generation is accepted
                        // fresh without an old worker re-raising them.
                        if let Err(e) = {
                            let db = state.db.lock().await;
                            db.reset_announcer_fences()
                        } {
                            warn!(error = %e, "failed to reset announcer fences on server change");
                        }
                        // The persisted device token (if any) belonged to the
                        // previous server; drop it and re-bootstrap against the
                        // new server from the voucher before respawning tasks.
                        if let Some(thin) = config.server.clone() {
                            let _ = state.db.lock().await.clear_device_token();
                            if let Some(minted) =
                                resolve_receiver_device_token(&state, &thin, &endpoint_id).await
                            {
                                config.server = Some(ServerClientConfig {
                                    url: thin.url,
                                    token: minted,
                                });
                            }
                        }
                        // Respawn discovery + approval-watch against the new
                        // server, after the reseed so they repopulate the
                        // freshly-cleared discovered-forwarders map.
                        discovery_task = config
                            .server
                            .clone()
                            .map(|thin| spawn_discovery(thin, shutdown_rx.clone()));
                        approval_watch_task = config
                            .server
                            .clone()
                            .map(|thin| spawn_approval_watch(thin, shutdown_rx.clone()));
                        // Force re-running register/takeover and rebuilding all
                        // forwarder + stream workers against the new server.
                        announcer_generation = None;
                        force_reconnect = Some(None);
                    }
                }
            }
            () = tokio::time::sleep(config.reconcile_interval) => {}
        }
    }

    for (_endpoint_id, worker) in workers.drain() {
        worker.stop().await;
    }
    for (_stream_id, worker) in stream_workers.drain() {
        worker.stop().await;
    }
    if let Some(task) = discovery_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = approval_watch_task {
        task.abort();
        let _ = task.await;
    }
    dbf_worker_task.abort();
    let _ = dbf_worker_task.await;
    endpoint.close().await;
    // The runtime is shutting down: no sessions remain and none will be
    // reattempted, so report a clean Disconnected. `P2pReceiverRuntime::shutdown`
    // awaits this task, so the state is settled before shutdown returns.
    state
        .set_connection_state(ConnectionState::Disconnected)
        .await;
}

async fn server_startup(
    thin: ServerClientConfig,
    endpoint_id: String,
    receiver_id: String,
) -> Result<i64, String> {
    tokio::task::spawn_blocking(move || {
        // Registration/mint already happened at runtime startup; this is an
        // idempotent re-register (mints nothing) followed by the generation
        // takeover, which requires the receiver to be approved (active). Before
        // approval the takeover returns 401 and this whole call errors, so the
        // reconcile loop simply retries until an admin approves the receiver.
        announcer_push::register_receiver_with_server(
            &thin.url,
            &thin.token,
            &endpoint_id,
            &receiver_id,
        )
        .map_err(|e| e.to_string())?;
        announcer_push::takeover_announcer_generation(&thin.url, &thin.token)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("server startup task failed: {e}"))?
}

/// Resolve the receiver's effective server credential.
///
/// Prefers a persisted server-minted device token; otherwise bootstraps by
/// registering with the configured token (an enrollment voucher, or the
/// provisioning token during migration) to mint one, persisting it for reuse.
/// Returns `None` when no token could be obtained (transient failure, or the
/// server minted nothing), in which case the caller keeps using the configured
/// token. The bearer token is never logged.
async fn resolve_receiver_device_token(
    state: &Arc<AppState>,
    thin: &ServerClientConfig,
    endpoint_id: &str,
) -> Option<String> {
    match state.db.lock().await.load_device_token() {
        Ok(Some(token)) => return Some(token),
        Ok(None) => {}
        Err(e) => warn!(error = %e, "failed to load persisted device token"),
    }

    let url = thin.url.clone();
    let voucher = thin.token.clone();
    let eid = endpoint_id.to_owned();
    let receiver_id = state.receiver_id.read().await.clone();
    let minted = tokio::task::spawn_blocking(move || {
        announcer_push::register_receiver_with_server(&url, &voucher, &eid, &receiver_id)
    })
    .await;

    match minted {
        Ok(Ok(Some(token))) => {
            if let Err(e) = state.db.lock().await.set_device_token(&token) {
                warn!(error = %e, "failed to persist minted device token; will re-bootstrap on next start");
            }
            Some(token)
        }
        Ok(Ok(None)) => {
            warn!("server /register minted no device token; using configured token");
            None
        }
        Ok(Err(e)) => {
            warn!(error = %e, "receiver bootstrap registration failed; using configured token");
            None
        }
        Err(e) => {
            warn!(error = %e, "receiver bootstrap task failed");
            None
        }
    }
}

/// Periodically refresh [`AppState::discovered_forwarders`] from the server
/// `GET /forwarders` feed. Failures are logged and retried on the next interval;
/// the task never crashes. The optional explicit `seed` forwarder is preserved
/// in the refreshed map when the server has not advertised that endpoint, so
/// the loopback/dev path keeps working alongside discovery.
///
/// When a refresh changes the discovered map, a streams snapshot is broadcast
/// to UI clients so server-side renames (and address/stream changes) surface
/// without a manual reconnect. The snapshot is SSE-only and never restarts
/// stream workers, so active streams are not interrupted.
async fn run_discovery_loop(
    state: Arc<AppState>,
    thin: ServerClientConfig,
    seed: Option<ForwarderPeerConfig>,
    interval: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
    mut connect_attempt_rx: watch::Receiver<crate::control_api::ConnectAttempt>,
) {
    loop {
        match fetch_forwarders(&thin).await {
            Ok(entries) => {
                let map = build_discovered_forwarders(entries, seed.as_ref());
                let changed = {
                    let mut current = state.discovered_forwarders.write().await;
                    if *current == map {
                        false
                    } else {
                        *current = map;
                        true
                    }
                };
                // When discovery metadata changes (e.g. a forwarder was
                // renamed on the server, or its addresses/streams changed),
                // push a streams snapshot so the UI reflects the new
                // `display_alias` promptly. This is purely an SSE broadcast and
                // does not restart stream workers, so live streams keep flowing
                // uninterrupted. The write guard is dropped before emitting
                // because `emit_streams_snapshot` re-reads the same map.
                if changed {
                    state.emit_streams_snapshot().await;
                }
            }
            Err(e) => {
                let status = server_device_status_for_url(&state, &thin.url).await;
                if receiver_pending_approval(&status) {
                    tracing::trace!(error = %e, "forwarder discovery waiting for receiver approval");
                } else {
                    warn!(error = %e, "forwarder discovery fetch failed; will retry");
                }
            }
        }

        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            changed = connect_attempt_rx.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            () = tokio::time::sleep(interval) => {}
        }
    }
}

async fn run_approval_watch_loop(
    state: Arc<AppState>,
    server_url: String,
    interval: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut previous_approval: Option<String> = None;
    loop {
        let status = server_device_status_for_url(&state, &server_url).await;
        if approval_became_active(
            previous_approval.as_deref(),
            status.approval_state.as_deref(),
        ) {
            info!(
                endpoint_id = status.endpoint_id.as_deref().unwrap_or("<unknown>"),
                "server approval became active; reconnecting receiver"
            );
            // A transient active→unknown→active flap intentionally re-requests
            // a connect so server recovery resets backoff and re-dials.
            state.request_connect().await;
            state.emit_resync();
            schedule_approval_follow_up_reconnect(
                Arc::clone(&state),
                APPROVAL_FOLLOW_UP_RECONNECT_DELAY,
            );
        }
        previous_approval = status.approval_state;

        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            () = tokio::time::sleep(interval) => {}
        }
    }
}

/// Build the discovered-forwarders snapshot from a server discovery feed.
///
/// Each entry's endpoint id is validated as a dialable node id exactly once
/// here, at discovery cadence, so a malformed id is dropped (with a single
/// warning) before it can reach the map. That keeps `resolve_forwarder_addr` —
/// which runs over every subscription on each reconcile pass — free of
/// per-reconcile log spam. The optional `seed` (a pre-validated explicit
/// forwarder for the loopback/dev path) is added only if the feed did not
/// already advertise the same endpoint id.
fn build_discovered_forwarders(
    entries: Vec<announcer_push::ForwarderDiscoveryEntry>,
    seed: Option<&ForwarderPeerConfig>,
) -> DiscoveredForwarders {
    let mut map = DiscoveredForwarders::new();
    for entry in entries {
        if let Err(e) = entry.endpoint_id.parse::<NodeId>() {
            warn!(
                endpoint_id = %entry.endpoint_id,
                error = %e,
                "discovered forwarder has invalid node id; skipping"
            );
            continue;
        }
        let direct_addrs = entry
            .direct_addrs
            .iter()
            .filter_map(|addr| addr.parse::<SocketAddr>().ok())
            .collect::<Vec<_>>();
        let streams = entry
            .streams
            .into_iter()
            .map(|stream| DiscoveredStream {
                stream_id: stream.stream_id,
                epoch: stream.epoch,
                next_seq: stream.next_seq,
            })
            .collect::<Vec<_>>();
        map.insert(
            entry.endpoint_id,
            DiscoveredForwarder {
                display_name: entry.display_name,
                direct_addrs,
                streams,
            },
        );
    }
    if let Some(forwarder) = seed {
        map.entry(forwarder.node_id.clone())
            .or_insert_with(|| DiscoveredForwarder {
                display_name: None,
                direct_addrs: vec![forwarder.direct_addr],
                streams: Vec::new(),
            });
    }
    map
}

async fn fetch_forwarders(
    thin: &ServerClientConfig,
) -> Result<Vec<announcer_push::ForwarderDiscoveryEntry>, String> {
    let url = thin.url.clone();
    let token = thin.token.clone();
    tokio::task::spawn_blocking(move || {
        announcer_push::fetch_approved_forwarders(&url, &token).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("forwarder discovery task failed: {e}"))?
}

/// Resolve a forwarder endpoint id to a dialable [`NodeAddr`] from the
/// discovered-forwarders snapshot. Returns `None` when the forwarder is not yet
/// discovered or its endpoint id is not a valid node id.
///
/// Endpoint ids are validated once when they enter the map (the discovery loop
/// and the startup seed both reject invalid node ids), so the parse here is a
/// cheap, non-logging fallback that cannot spam at reconcile cadence.
fn resolve_forwarder_addr(
    endpoint_id: &str,
    discovered: &DiscoveredForwarders,
) -> Option<NodeAddr> {
    let forwarder = discovered.get(endpoint_id)?;
    let node_id = endpoint_id.parse::<NodeId>().ok()?;
    Some(NodeAddr::new(node_id).with_direct_addresses(forwarder.direct_addrs.iter().copied()))
}

fn desired_forwarder_subscriptions(
    discovered: &DiscoveredForwarders,
    subs: &[StreamSubscription],
    intents: &HashMap<String, bool>,
) -> HashMap<String, Vec<StreamSubscription>> {
    let mut desired = HashMap::new();
    for endpoint_id in discovered.keys() {
        if *intents.get(endpoint_id).unwrap_or(&true) {
            desired.insert(endpoint_id.clone(), Vec::new());
        }
    }
    for sub in subs {
        if !*intents.get(&sub.forwarder_endpoint_id).unwrap_or(&true) {
            continue;
        }
        desired
            .entry(sub.forwarder_endpoint_id.clone())
            .or_insert_with(Vec::new)
            .push(sub.clone());
    }
    desired
}

async fn reconcile_once(
    state: &Arc<AppState>,
    config: &P2pReceiverConfig,
    endpoint: &Arc<Endpoint>,
    reporter: &Arc<SessionStatusReporter>,
    announcer_generation: Option<i64>,
    workers: &mut HashMap<String, ForwarderConnection>,
    stream_workers: &mut HashMap<String, StreamWorker>,
) {
    let (subs, intents, announcer_enabled, announcer_publish_streams) = {
        let db = state.db.lock().await;
        let subs = match db.load_stream_subscriptions() {
            Ok(subs) => subs,
            Err(e) => {
                warn!(error = %e, "failed to load stream subscriptions; skipping reconcile pass");
                return;
            }
        };
        let intents = match db.load_forwarder_intents() {
            Ok(intents) => intents,
            Err(e) => {
                warn!(error = %e, "failed to load forwarder intents; using default connect intent");
                HashMap::new()
            }
        };
        // Announcer gating inputs (global toggle + per-stream opt-in). Failures
        // are treated as "disabled" so a transient DB error never publishes.
        let enabled = db.load_announcer_enabled().unwrap_or(false);
        let publish = db.load_announcer_publish_streams().unwrap_or_default();
        (subs, intents, enabled, publish)
    };

    let discovered = state.discovered_forwarders.read().await.clone();
    let desired_forwarders = desired_forwarder_subscriptions(&discovered, &subs, &intents);

    let stale_forwarders = workers
        .keys()
        .filter(|endpoint_id| !desired_forwarders.contains_key(*endpoint_id))
        .cloned()
        .collect::<Vec<_>>();
    for endpoint_id in stale_forwarders {
        if let Some(worker) = workers.remove(&endpoint_id) {
            info!(%endpoint_id, "stopping p2p forwarder worker");
            worker.stop().await;
        }
    }

    for endpoint_id in desired_forwarders.keys() {
        // The pending grace clock is driven by the `ForwarderConnection` state
        // machine (set-once on the first dial attempt, cleared on connect,
        // re-armed on disconnect), not by the reconcile cadence. Marking it here
        // on every pass would reset it every reconcile interval so it could
        // never elapse to `Unavailable`.
        if workers.contains_key(endpoint_id) {
            continue;
        }
        let Some(forwarder_addr) = resolve_forwarder_addr(endpoint_id, &discovered) else {
            continue;
        };
        info!(%endpoint_id, "starting p2p forwarder worker");
        let worker = ForwarderConnection::start(
            endpoint_id.clone(),
            Arc::clone(endpoint),
            forwarder_addr,
            state.writer.clone(),
            client_hello(),
            Arc::clone(reporter),
            BackoffConfig::default(),
        );
        workers.insert(endpoint_id.clone(), worker);
    }

    let mut desired_streams: HashMap<String, StreamSubscription> = HashMap::new();
    for sub in subs {
        desired_streams.insert(sub.stream_id.clone(), sub);
    }

    let stale_streams = stream_workers
        .keys()
        .filter(|stream_id| !desired_streams.contains_key(*stream_id))
        .cloned()
        .collect::<Vec<_>>();
    for stream_id in stale_streams {
        if let Some(worker) = stream_workers.remove(&stream_id) {
            info!(%stream_id, "stopping p2p stream worker (subscription removed)");
            worker.stop().await;
        }
    }

    // Whether announcer push may run at all this pass: a server is configured,
    // a fenced generation was acquired, and the global toggle is on. Per-stream
    // opt-in is applied below via `announcer_publish_streams`.
    let announcer_available =
        config.server.is_some() && announcer_generation.is_some() && announcer_enabled;
    let should_announce =
        |stream_id: &str| announcer_available && announcer_publish_streams.contains(stream_id);
    for (stream_id, sub) in desired_streams {
        let want_announce = should_announce(&stream_id);
        if let Some(existing) = stream_workers.get(&stream_id) {
            let config_changed = existing.sub != sub;
            // Rebuild if the announcer state for this stream needs to change
            // (turned on or off).
            let announcer_mismatch = existing.announcer_active != want_announce;
            if !config_changed && !announcer_mismatch {
                continue;
            }
            if let Some(worker) = stream_workers.remove(&stream_id) {
                if config_changed {
                    info!(%stream_id, "rebuilding p2p stream worker (subscription config changed)");
                } else {
                    info!(%stream_id, announce = want_announce, "rebuilding p2p stream worker (announcer state changed)");
                }
                worker.stop().await;
            }
        }
        let worker =
            start_stream_worker(state, config, announcer_generation, want_announce, &sub).await;
        stream_workers.insert(stream_id, worker);
    }

    for (endpoint_id, worker) in workers.iter() {
        let streams = desired_forwarders
            .get(endpoint_id)
            .into_iter()
            .flatten()
            .filter_map(|sub| {
                stream_workers
                    .get(&sub.stream_id)
                    .map(|stream_worker| ForwarderDataStream {
                        stream_id: sub.stream_id.clone(),
                        mode: SubscribeMode::Replay,
                        durable_hint_tx: Some(stream_worker.hint_tx.clone()),
                    })
            })
            .collect::<Vec<_>>();
        worker.set_desired_streams(streams);
    }
    state.recompute_aggregate_connection_state().await;
}

async fn start_stream_worker(
    state: &Arc<AppState>,
    config: &P2pReceiverConfig,
    announcer_generation: Option<i64>,
    announce: bool,
    sub: &StreamSubscription,
) -> StreamWorker {
    let stream_id = sub.stream_id.clone();
    info!(%stream_id, "starting p2p stream worker");

    let (hint_tx, _hint_rx) = broadcast::channel::<DurableBatch>(HINT_CHANNEL_CAPACITY);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut tasks: Vec<JoinHandle<()>> = Vec::new();

    // Durable local proxy (only when a port can be resolved).
    let port = resolve_local_port(sub);
    let proxy = match port {
        Some(port) => {
            match LocalProxy::bind_durable(
                port,
                stream_id.clone(),
                state.read_source.clone(),
                hint_tx.clone(),
            )
            .await
            {
                Ok(proxy) => Some(proxy),
                Err(e) => {
                    warn!(error = %e, %stream_id, port, "failed to bind durable local proxy; skipping proxy");
                    None
                }
            }
        }
        None => {
            warn!(%stream_id, "no local port could be resolved; running session without local proxy");
            None
        }
    };

    // UI projection for P2P-delivered events. Canonical streams discovered from
    // the server may not carry legacy `(forwarder_id, reader_ip)` metadata, but
    // stream IDs that are reader network addresses can still drive the existing
    // UI count/last-read/metrics model without storing fabricated metadata.
    if let Some(ui_key) = ui_stream_key(sub) {
        tasks.push(tokio::spawn(run_ui_projection_worker(
            Arc::clone(state),
            stream_id.clone(),
            ui_key,
            hint_tx.subscribe(),
            shutdown_rx.clone(),
        )));
    }

    // DBF delivery is NOT per-stream: all streams share one IPICO.DBF file,
    // so a single cross-stream worker (spawned once in the reconcile loop,
    // see run_shared_dbf_worker) owns it. Per-stream workers used to clobber
    // each other's rows via full-file rebuilds.

    // Announcer push. Gated by the per-stream + global opt-in resolved by the
    // reconcile loop (`announce`).
    let mut announcer_active = false;
    if let (true, Some(thin), Some(generation)) =
        (announce, config.server.clone(), announcer_generation)
    {
        match ServerAnnouncerClient::new(&thin.url, thin.token.clone()) {
            Ok(client) => {
                let client: Arc<dyn AnnouncerPushClient + Send + Sync> = Arc::new(client);
                let db = Arc::clone(&state.db);
                let chip_lookup = Arc::clone(&state.chip_lookup);
                let stream_id = stream_id.clone();
                let hint_rx = hint_tx.subscribe();
                let ann_shutdown = shutdown_rx.clone();
                tasks.push(tokio::spawn(run_announcer_worker(
                    db,
                    chip_lookup,
                    stream_id,
                    client,
                    generation,
                    config.reconcile_interval,
                    hint_rx,
                    ann_shutdown,
                )));
                announcer_active = true;
            }
            Err(e) => {
                warn!(error = %e, %stream_id, "failed to build announcer client; skipping announcer push");
            }
        }
    }

    StreamWorker {
        sub: sub.clone(),
        shutdown_tx,
        hint_tx,
        proxy,
        tasks,
        announcer_active,
    }
}

/// Resolve the reader address to use for compatibility UI/proxy paths.
///
/// Prefer explicit legacy metadata. When it is absent, use the canonical stream
/// id only if it is a reader network address that can be mapped to a local port.
fn ui_reader_ip(sub: &StreamSubscription) -> Option<String> {
    sub.reader_ip.clone().or_else(|| {
        reader_addr_if_port_mappable(&sub.stream_id).map(std::borrow::ToOwned::to_owned)
    })
}

/// Resolve the compatibility stream key used by existing UI count, last-read,
/// and metrics events.
fn ui_stream_key(sub: &StreamSubscription) -> Option<StreamKey> {
    let reader_ip = ui_reader_ip(sub)?;
    let forwarder_id = sub
        .forwarder_id
        .clone()
        .unwrap_or_else(|| sub.forwarder_endpoint_id.clone());
    Some(StreamKey::new(forwarder_id, reader_ip))
}

/// Resolve the local TCP port for a subscription: explicit override first, then
/// the default mapping from reader metadata, falling back to a canonical stream
/// id that is itself a reader network address. Returns `None` if none yields a
/// port.
fn resolve_local_port(sub: &StreamSubscription) -> Option<u16> {
    sub.local_port_override
        .or_else(|| ui_reader_ip(sub).as_deref().and_then(default_port))
}

/// How often the UI projection flushes dirty state to the UI event channel.
const UI_PROJECTION_EMIT_INTERVAL: Duration = Duration::from_millis(250);

async fn run_ui_projection_worker(
    state: Arc<AppState>,
    stream_id: String,
    ui_key: StreamKey,
    mut hint_rx: broadcast::Receiver<DurableBatch>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    // One-time O(N) seed from the durable store; the hot path below never
    // touches the DB.
    let mut proj = rebuild_stream_projection(&state, &stream_id, &ui_key)
        .await
        .unwrap_or_default();
    let mut dirty = proj.total > 0;
    let mut needs_rebuild = false;
    // (epoch → seqs) folded since the last tick, mirrored into the shared
    // `state.stream_counts` cache on emit.
    let mut pending_counts: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut tick = tokio::time::interval(UI_PROJECTION_EMIT_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            recv = hint_rx.recv() => {
                match recv {
                    Ok(batch) => {
                        // O(batch) fold; no DB access on the hot path.
                        for fact in batch.inserted.iter() {
                            proj.apply(fact);
                            pending_counts.entry(fact.epoch).or_default().push(fact.seq);
                        }
                        if !batch.inserted.is_empty() {
                            dirty = true;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Hint channel overflowed; facts were lost. Re-seed
                        // from the durable store on the next tick.
                        needs_rebuild = true;
                        dirty = true;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = tick.tick() => {
                if needs_rebuild {
                    if let Some(rebuilt) =
                        rebuild_stream_projection(&state, &stream_id, &ui_key).await
                    {
                        proj = rebuilt;
                        // The rebuild replaced the shared counts wholesale;
                        // drop deltas already covered by the durable store.
                        pending_counts.clear();
                        needs_rebuild = false;
                    }
                }
                if !dirty {
                    continue;
                }
                for (epoch, seqs) in pending_counts.drain() {
                    state.stream_counts.record_batch(&ui_key, epoch, seqs);
                }
                emit_stream_projection(&state, &ui_key, &proj).await;
                dirty = false;
            }
        }
    }
}

/// One-time projection rebuild from the durable store: seeds the in-memory
/// [`StreamProjection`] and replaces the shared `state.stream_counts` entry so
/// status/UI totals survive a restart. Returns `None` on a DB error (the
/// worker keeps its current state and retries on the next overflow).
async fn rebuild_stream_projection(
    state: &Arc<AppState>,
    stream_id: &str,
    ui_key: &StreamKey,
) -> Option<StreamProjection> {
    let stream_id_owned = stream_id.to_owned();
    let rows = match state
        .read_source
        .run(move |db| db.load_stream_projection_summary(&stream_id_owned))
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, %stream_id, "failed to load stream projection summary");
            return None;
        }
    };
    if rows.is_empty() {
        return Some(StreamProjection::default());
    }
    info!(%stream_id, epochs = rows.len(), "rebuilding stream projection");

    // Until the chip_id column exists (Phase 4), seed the live epoch's unique
    // chips by parsing that epoch's raw frames — bounded to one epoch, once.
    let live_epoch = rows.last().map_or(0, |row| row.epoch);
    let mut chips = HashSet::new();
    let mut last_chip_id = None;
    let stream_id_owned = stream_id.to_owned();
    match state
        .read_source
        .run(move |db| db.load_epoch_chip_ids(&stream_id_owned, live_epoch))
        .await
    {
        Ok(chip_ids) => {
            last_chip_id = chip_ids.last().map(|(_, chip_id)| chip_id.clone());
            for (_seq, chip_id) in chip_ids {
                let _ = chips.insert(chip_id);
            }
        }
        Err(e) => {
            warn!(error = %e, %stream_id, "failed to load live-epoch chip ids for projection seed");
        }
    }

    state.stream_counts.seed_from_epoch_summaries(
        ui_key,
        rows.iter().map(|row| (row.epoch, row.count, row.max_seq)),
    );
    Some(StreamProjection::seed_from_summary(
        &rows,
        chips,
        last_chip_id,
    ))
}

/// Emit the throttled UI events for one stream projection: counts, last read,
/// metrics, and the streams snapshot.
async fn emit_stream_projection(
    state: &Arc<AppState>,
    ui_key: &StreamKey,
    proj: &StreamProjection,
) {
    if let Some(counts) = state.stream_counts.get(ui_key) {
        let _ = state.ui_tx.send(ReceiverUiEvent::StreamCountsUpdated {
            updates: vec![crate::ui_events::StreamCountUpdate {
                forwarder_id: ui_key.forwarder_id.clone(),
                reader_ip: ui_key.reader_ip.clone(),
                reads_total: counts.total,
                reads_epoch: counts.epoch,
            }],
        });
    }

    if let Some(chip_id) = proj.last_chip_id.clone() {
        let resolved = {
            let snapshot = state.chip_lookup.read().await.clone();
            SnapshotResolver { snapshot }.resolve(&chip_id)
        };
        let _ = state
            .ui_tx
            .send(ReceiverUiEvent::LastRead(crate::ui_events::LastRead {
                forwarder_id: ui_key.forwarder_id.clone(),
                reader_ip: ui_key.reader_ip.clone(),
                chip_id,
                timestamp: crate::ui_events::unix_ms_to_rfc3339(proj.max_received_unix_ms)
                    .unwrap_or_else(|| proj.max_received_unix_ms.to_string()),
                bib: resolved.as_ref().map(|participant| participant.bib.clone()),
                name: resolved
                    .as_ref()
                    .and_then(|participant| participant.name.clone()),
                division: resolved.and_then(|participant| participant.division),
            }));
    }

    let metrics = proj.metrics(ui_key, now_unix_ms());
    state.cache_stream_metrics(&metrics).await;
    let _ = state
        .ui_tx
        .send(ReceiverUiEvent::StreamMetricsUpdated(metrics));
    state.emit_streams_snapshot().await;
}

/// Single cross-stream DBF worker: one worker per DBF *file*, not per stream.
///
/// All subscribed streams deliver into one `IPICO.DBF`, so per-stream workers
/// (the previous design) clobbered each other's rows on every full rebuild.
/// This worker runs **at most one delivery pass per flush interval**
/// (interval-coalesced: per-hint passes would mean a file open — and a
/// Defender scan on Windows — per group commit). Nothing real-time reads the
/// DBF: Race Director polls it on a multi-second cadence.
///
/// Instead of subscribing to every stream's hint channel, each tick probes
/// `min_undelivered_dbf_seq` per subscribed stream — O(1) via the partial
/// undelivered index and near-free when idle — which also retries failed
/// passes automatically on the next tick.
async fn run_shared_dbf_worker(state: Arc<AppState>, mut shutdown_rx: watch::Receiver<bool>) {
    let mut pass_state = crate::dbf_writer::DbfPassState::default();
    let mut was_enabled = false;
    let mut interval_ms = u64::from(crate::db::DEFAULT_DBF_FLUSH_INTERVAL_MS);
    let mut tick = tokio::time::interval(Duration::from_millis(interval_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            _ = tick.tick() => {
                let (enabled, configured_ms) =
                    run_shared_dbf_pass(&state, &mut pass_state, was_enabled).await;
                was_enabled = enabled;
                // Apply a changed flush interval without restart. Each pass
                // re-reads the persisted config, so a UI save takes effect on
                // the next tick.
                if configured_ms != interval_ms {
                    interval_ms = configured_ms;
                    tick = tokio::time::interval(Duration::from_millis(interval_ms));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    tick.reset();
                }
            }
        }
    }
}

/// One tick of the shared DBF worker. Returns `(dbf enabled, configured
/// flush interval in ms)`.
async fn run_shared_dbf_pass(
    state: &Arc<AppState>,
    pass_state: &mut crate::dbf_writer::DbfPassState,
    was_enabled: bool,
) -> (bool, u64) {
    // Resolve config + subscriptions on the cold connection (tiny queries).
    let (enabled, interval_ms, dbf_path, specs) = {
        let db = state.db.lock().await;
        let config = db.load_dbf_config().unwrap_or(crate::db::DbfConfig {
            enabled: false,
            flush_interval_ms: crate::db::DEFAULT_DBF_FLUSH_INTERVAL_MS,
        });
        let interval_ms = u64::from(config.flush_interval_ms);
        if !config.enabled {
            (false, interval_ms, String::new(), Vec::new())
        } else {
            let dbf_path = db
                .load_rd_import_config()
                .ok()
                .map(|cfg| std::path::Path::new(&cfg.dir).join("IPICO.DBF"))
                .unwrap_or_else(|| {
                    std::path::Path::new(crate::db::DEFAULT_RD_IMPORT_DIR).join("IPICO.DBF")
                })
                .to_string_lossy()
                .into_owned();
            let mut specs = Vec::new();
            match db.load_stream_subscriptions() {
                Ok(subs) => {
                    for sub in subs {
                        match db.load_subscription_dbf_details(
                            &sub.forwarder_endpoint_id,
                            &sub.stream_id,
                        ) {
                            Ok(Some((idx, event_type))) => match u8::try_from(idx) {
                                Ok(reader_index) if reader_index <= 9 => {
                                    specs.push(crate::dbf_writer::DbfStreamSpec {
                                        stream_id: sub.stream_id,
                                        event_type,
                                        reader_index,
                                    });
                                }
                                _ => {
                                    warn!(
                                        stream_id = %sub.stream_id,
                                        idx,
                                        "subscription index exceeds DBF reader range; skipping stream"
                                    );
                                }
                            },
                            Ok(None) => {}
                            Err(e) => {
                                warn!(error = %e, stream_id = %sub.stream_id, "failed to load DBF subscription details");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "failed to load subscriptions for DBF delivery");
                    return (was_enabled, interval_ms);
                }
            }
            (true, interval_ms, dbf_path, specs)
        }
    };
    if !enabled {
        // Re-enabling later must reconcile the file against the durable
        // store, exactly like a restart.
        if was_enabled {
            *pass_state = crate::dbf_writer::DbfPassState::default();
        }
        return (false, interval_ms);
    }

    // DBF delivery does synchronous disk + flock I/O: run on a blocking
    // thread with the cold connection (blocking_lock), like the announcer.
    let db = Arc::clone(&state.db);
    let mut moved_state = std::mem::take(pass_state);
    let result = tokio::task::spawn_blocking(move || {
        let mut guard = db.blocking_lock();
        let result = crate::dbf_writer::run_dbf_delivery_pass(
            &mut guard,
            &specs,
            std::path::Path::new(&dbf_path),
            &mut moved_state,
            now_unix_ms(),
        );
        (moved_state, result)
    })
    .await;
    match result {
        Ok((state_back, Ok(()))) => *pass_state = state_back,
        Ok((state_back, Err(e))) => {
            warn!(error = %e, "DBF delivery pass failed; will retry next tick");
            *pass_state = state_back;
        }
        Err(e) => {
            warn!(error = %e, "DBF delivery task failed");
        }
    }
    (true, interval_ms)
}

#[allow(clippy::too_many_arguments)]
async fn run_announcer_worker(
    db: Arc<Mutex<Db>>,
    chip_lookup: Arc<tokio::sync::RwLock<crate::control_api::ChipLookup>>,
    stream_id: String,
    client: Arc<dyn AnnouncerPushClient + Send + Sync>,
    generation: i64,
    retry_interval: Duration,
    mut hint_rx: broadcast::Receiver<DurableBatch>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    // `needs_retry` is set whenever a push attempt fails to reach the announcer
    // sink. While set, the worker retries on `retry_interval` even if no new
    // durable hint arrives, so pending rows are not stranded after the last
    // hint. A successful (or stale-generation) attempt clears it.
    let mut needs_retry = !push_announcer(&db, &chip_lookup, &stream_id, &client, generation).await;
    loop {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            recv = hint_rx.recv() => {
                match recv {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        needs_retry =
                            !push_announcer(&db, &chip_lookup, &stream_id, &client, generation).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            () = tokio::time::sleep(retry_interval), if needs_retry => {
                needs_retry =
                    !push_announcer(&db, &chip_lookup, &stream_id, &client, generation).await;
            }
        }
    }
}

/// Run one announcer push pass. Returns `true` when no retry is needed (the
/// push was accepted, there was nothing pending, or the generation was stale
/// and retrying cannot help), and `false` when the caller should schedule a
/// retry (transport failure or task error left rows unpushed).
async fn push_announcer(
    db: &Arc<Mutex<Db>>,
    chip_lookup: &Arc<tokio::sync::RwLock<crate::control_api::ChipLookup>>,
    stream_id: &str,
    client: &Arc<dyn AnnouncerPushClient + Send + Sync>,
    generation: i64,
) -> bool {
    // Snapshot the chip lookup so the blocking task owns Send + 'static data.
    let snapshot = chip_lookup.read().await.clone();
    let db = Arc::clone(db);
    let client = Arc::clone(client);
    let stream_id_owned = stream_id.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        let resolver = SnapshotResolver { snapshot };
        let mut guard = db.blocking_lock();
        announcer_push::push_announcer_rows(
            &mut guard,
            client.as_ref(),
            &resolver,
            &stream_id_owned,
            generation,
            now_unix_ms(),
        )
    })
    .await;

    match result {
        Ok(Ok(_outcome)) => true,
        Ok(Err(e)) => {
            warn!(error = %e, %stream_id, "announcer push failed; will retry");
            false
        }
        Err(e) => {
            warn!(error = %e, %stream_id, "announcer push task failed");
            false
        }
    }
}

/// Resolves a chip id against a snapshot of the in-memory chip lookup, searching
/// across all forwarders' maps.
struct SnapshotResolver {
    snapshot: crate::control_api::ChipLookup,
}

impl ParticipantResolver for SnapshotResolver {
    fn resolve(&self, chip_id: &str) -> Option<ResolvedParticipant> {
        for chips in self.snapshot.values() {
            if let Some(entry) = chips.get(chip_id) {
                return Some(ResolvedParticipant {
                    bib: entry.bib.clone(),
                    name: entry.name.clone(),
                    division: entry.division.clone(),
                });
            }
        }
        None
    }
}

/// Parse a 64-hex-character string into a 32-byte secret-key seed.
pub fn parse_secret_key_seed_hex(hex: &str) -> Result<[u8; 32], String> {
    // Require ASCII hex up front. Length and slicing below operate on bytes, so
    // a multibyte string (e.g. 64 UTF-8 bytes that are fewer than 64 chars, or
    // 64 chars that don't fall on byte boundaries) must be rejected rather than
    // panicking on a non-char-boundary slice.
    if !hex.is_ascii() {
        return Err("secret key seed must be ASCII hex characters".to_owned());
    }
    if hex.len() != 64 {
        return Err(format!(
            "secret key seed must be 64 hex characters, got {}",
            hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let slice = &hex[i * 2..i * 2 + 2];
        *byte = u8::from_str_radix(slice, 16)
            .map_err(|e| format!("invalid hex in secret key seed: {e}"))?;
    }
    Ok(out)
}

/// Derive the deterministic loopback node id for a given seed, useful for tests
/// and orchestration that need the receiver/forwarder id before binding.
pub fn node_id_for_seed(seed: [u8; 32]) -> String {
    SecretKey::from_bytes(&seed).public().to_string()
}

// ---------------------------------------------------------------------------
// Local/dev P2P config (env var) builder
//
// Shared by the desktop (Tauri) receiver app to start the same P2P lane as
// `receiver-headless` without CLI flags. This is an explicit local-config
// affordance for development and the manual dev stack; production forwarder
// discovery via the server is a separate concern. P2P stays disabled unless
// at least one of these keys is present.
// ---------------------------------------------------------------------------

/// Env var naming the forwarder's iroh endpoint (string node id) to dial.
pub const ENV_P2P_FORWARDER_NODE_ID: &str = "RT_P2P_FORWARDER_NODE_ID";
/// Env var giving a direct `ip:port` socket address for the forwarder peer.
pub const ENV_P2P_FORWARDER_DIRECT_ADDR: &str = "RT_P2P_FORWARDER_DIRECT_ADDR";
/// Env var holding the receiver's 64-hex-character secret-key seed.
pub const ENV_P2P_SECRET_KEY_SEED_HEX: &str = "RT_P2P_SECRET_KEY_SEED_HEX";
/// Env var for the optional server base URL (set with the token).
pub const ENV_P2P_SERVER_URL: &str = "RT_P2P_SERVER_URL";
/// Env var for the optional server bearer token (set with the URL).
pub const ENV_P2P_SERVER_TOKEN: &str = "RT_P2P_SERVER_TOKEN";
/// Env var overriding the subscription reconcile interval, in milliseconds.
pub const ENV_P2P_RECONCILE_MS: &str = "RT_P2P_RECONCILE_MS";
/// Env var for an explicit persistent secret-key file path (production
/// identity). Mutually exclusive with [`ENV_P2P_SECRET_KEY_SEED_HEX`].
pub const ENV_P2P_SECRET_KEY_PATH: &str = "RT_P2P_SECRET_KEY_PATH";
/// Env var disabling iroh relays (truthy value = disabled).
pub const ENV_P2P_RELAY_DISABLED: &str = "RT_P2P_RELAY_DISABLED";
/// Env var disabling iroh discovery services (truthy value = disabled).
pub const ENV_P2P_DISCOVERY_DISABLED: &str = "RT_P2P_DISCOVERY_DISABLED";

/// Parse a boolean-ish env flag: empty/absent is `false`; `1/true/yes/on`
/// (case-insensitive) is `true`; `0/false/no/off` is `false`; anything else is
/// an error so typos surface loudly.
fn parse_env_flag(value: Option<String>, key: &str) -> Result<bool, String> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(false),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => Err(format!("invalid {key}: expected a boolean, got `{other}`")),
        },
    }
}

/// Build an optional [`P2pReceiverConfig`] from a key->value lookup (e.g. env).
///
/// Mirrors the `receiver-headless` CLI validation: P2P is enabled only when at
/// least one key is present; the forwarder node id and direct address must be
/// supplied together (both or neither); the server URL and token must be
/// supplied together; and the reconcile interval defaults to 1000ms and must be
/// at least [`MIN_RECONCILE_INTERVAL`]. Empty/whitespace-only values are treated
/// as absent. A config with only identity/transport/reconcile knobs (no
/// forwarder, no server) is valid and returns `server: None`; the caller then
/// attaches the resolved profile/override server.
///
/// Identity: a seed and an explicit key path are mutually exclusive. When
/// neither is given the receiver uses a persistent key at `default_key_path`
/// (production identity). A seed implies the loopback/dev transport (relays and
/// discovery off, loopback bind) unless overridden; otherwise transport follows
/// the relay/discovery flags (default: enabled, i.e. production).
pub fn p2p_config_from_lookup(
    get: impl Fn(&str) -> Option<String>,
    default_key_path: std::path::PathBuf,
) -> Result<Option<P2pReceiverConfig>, String> {
    let trimmed = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };

    let forwarder_node_id = trimmed(ENV_P2P_FORWARDER_NODE_ID);
    let forwarder_direct_addr = trimmed(ENV_P2P_FORWARDER_DIRECT_ADDR);
    let secret_key_seed_hex = trimmed(ENV_P2P_SECRET_KEY_SEED_HEX);
    let secret_key_path = trimmed(ENV_P2P_SECRET_KEY_PATH);
    let server_url = trimmed(ENV_P2P_SERVER_URL);
    let server_token = trimmed(ENV_P2P_SERVER_TOKEN);
    let reconcile_ms_raw = trimmed(ENV_P2P_RECONCILE_MS);
    let relay_disabled_raw = trimmed(ENV_P2P_RELAY_DISABLED);
    let discovery_disabled_raw = trimmed(ENV_P2P_DISCOVERY_DISABLED);

    let any_present = forwarder_node_id.is_some()
        || forwarder_direct_addr.is_some()
        || secret_key_seed_hex.is_some()
        || secret_key_path.is_some()
        || server_url.is_some()
        || server_token.is_some()
        || reconcile_ms_raw.is_some()
        || relay_disabled_raw.is_some()
        || discovery_disabled_raw.is_some();
    if !any_present {
        return Ok(None);
    }

    let forwarder = match (forwarder_node_id, forwarder_direct_addr) {
        (Some(node_id), Some(direct_addr_raw)) => {
            let direct_addr: SocketAddr = direct_addr_raw
                .parse()
                .map_err(|e| format!("invalid {ENV_P2P_FORWARDER_DIRECT_ADDR}: {e}"))?;
            Some(ForwarderPeerConfig {
                node_id,
                direct_addr,
            })
        }
        (None, None) => None,
        (Some(_), None) => {
            return Err(format!(
                "{ENV_P2P_FORWARDER_DIRECT_ADDR} is required when {ENV_P2P_FORWARDER_NODE_ID} is set"
            ));
        }
        (None, Some(_)) => {
            return Err(format!(
                "{ENV_P2P_FORWARDER_NODE_ID} is required when {ENV_P2P_FORWARDER_DIRECT_ADDR} is set"
            ));
        }
    };

    let server_override = (server_url.clone(), server_token.clone());
    let server = match (server_url, server_token) {
        (Some(url), Some(token)) => Some(ServerClientConfig { url, token }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "{ENV_P2P_SERVER_URL} and {ENV_P2P_SERVER_TOKEN} must be set together"
            ));
        }
    };

    // Identity: seed XOR explicit key path; fall back to the persistent default
    // key path when neither is given.
    let identity = match (secret_key_seed_hex, secret_key_path) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{ENV_P2P_SECRET_KEY_SEED_HEX} and {ENV_P2P_SECRET_KEY_PATH} are mutually exclusive"
            ));
        }
        (Some(seed_hex), None) => ReceiverIdentity::Seed(parse_secret_key_seed_hex(&seed_hex)?),
        (None, Some(path)) => ReceiverIdentity::KeyPath(std::path::PathBuf::from(path)),
        (None, None) => ReceiverIdentity::KeyPath(default_key_path),
    };

    // A seed identity defaults to the loopback/dev transport (relays + discovery
    // off, loopback bind) to preserve deterministic local behavior; explicit
    // flags still override. A key-path identity defaults to production
    // transport (relays + discovery on, OS-chosen bind).
    let seed_identity = matches!(identity, ReceiverIdentity::Seed(_));
    let relay_disabled = match relay_disabled_raw {
        Some(v) => parse_env_flag(Some(v), ENV_P2P_RELAY_DISABLED)?,
        None => seed_identity,
    };
    let discovery_disabled = match discovery_disabled_raw {
        Some(v) => parse_env_flag(Some(v), ENV_P2P_DISCOVERY_DISABLED)?,
        None => seed_identity,
    };
    let bind_addr_v4 = if seed_identity {
        Some(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::LOCALHOST,
            0,
        ))
    } else {
        None
    };

    let reconcile_ms = match reconcile_ms_raw {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|e| format!("invalid {ENV_P2P_RECONCILE_MS}: {e}"))?,
        None => 1000,
    };
    let reconcile_interval = Duration::from_millis(reconcile_ms);
    if reconcile_interval < MIN_RECONCILE_INTERVAL {
        return Err(format!(
            "{ENV_P2P_RECONCILE_MS} must be at least {} ms",
            MIN_RECONCILE_INTERVAL.as_millis()
        ));
    }

    Ok(Some(P2pReceiverConfig {
        identity,
        relay_disabled,
        discovery_disabled,
        bind_addr_v4,
        forwarder,
        server,
        server_override,
        reconcile_interval,
    }))
}

/// Build an optional [`P2pReceiverConfig`] from process environment variables.
/// See [`p2p_config_from_lookup`] for the validation rules. `default_key_path`
/// is the persistent secret-key path used when no seed/key-path env is set.
pub fn p2p_config_from_env(
    default_key_path: std::path::PathBuf,
) -> Result<Option<P2pReceiverConfig>, String> {
    p2p_config_from_lookup(|key| std::env::var(key).ok(), default_key_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announcer_push::{AnnouncerPushError, AnnouncerRow};
    use crate::control_api::ChipLookup;
    use crate::db::{EventType, ReceivedEventInsert};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A valid IPICO chip-read frame (chip id `000000012345`).
    const SAMPLE_FRAME: &[u8] = b"aa400000000123450a2a01123018455927a7";

    #[test]
    fn config_supports_keypath_and_seed_identities_independent_of_transport() {
        let p = std::path::PathBuf::from("/tmp/k.key");
        let c1 = P2pReceiverConfig::for_test_keypath(p.clone());
        assert!(matches!(c1.identity, ReceiverIdentity::KeyPath(ref x) if *x == p));
        assert!(!c1.relay_disabled);
        let c2 = P2pReceiverConfig::for_test_seed([7u8; 32], /*relay_disabled*/ true);
        assert!(matches!(c2.identity, ReceiverIdentity::Seed(s) if s == [7u8; 32]));
        assert!(c2.relay_disabled);
    }

    #[tokio::test]
    async fn keypath_identity_persists_node_id_across_starts() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("p2p_secret.key");
        let id1 = bind_node_id(ReceiverIdentity::KeyPath(key.clone())).await;
        let id2 = bind_node_id(ReceiverIdentity::KeyPath(key)).await;
        assert_eq!(id1, id2);
    }

    #[test]
    fn production_default_has_keypath_identity_and_production_transport() {
        let p = std::path::PathBuf::from("/tmp/k.key");
        let cfg = P2pReceiverConfig::production_default(p.clone());
        assert!(matches!(cfg.identity, ReceiverIdentity::KeyPath(ref x) if *x == p));
        assert!(!cfg.relay_disabled);
        assert!(!cfg.discovery_disabled);
        assert!(cfg.bind_addr_v4.is_none());
        assert!(cfg.forwarder.is_none());
        assert!(cfg.server.is_none());
    }

    #[tokio::test]
    async fn bare_runtime_binds_and_idles_without_server_or_forwarder() {
        use crate::control_api::AppState;
        use crate::db::Db;
        // server=None, forwarder=None, loopback transport: the runtime must
        // bind and run its reconcile loop without panicking (no discovery task,
        // no announcer workers, generation stays None).
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let cfg = P2pReceiverConfig::for_test_seed([5u8; 32], true);
        let rt = start_receiver_p2p(Arc::clone(&state), cfg)
            .await
            .expect("bare runtime starts");
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn reseed_discovered_forwarders_clears_old_and_keeps_valid_seed() {
        use crate::control_api::AppState;
        use crate::db::Db;
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        // Populate with forwarders learned from a previous server.
        {
            let mut d = state.discovered_forwarders.write().await;
            for id in ["old-a", "old-b"] {
                d.insert(
                    id.to_owned(),
                    DiscoveredForwarder {
                        display_name: None,
                        direct_addrs: vec!["127.0.0.1:1".parse().unwrap()],
                        streams: Vec::new(),
                    },
                );
            }
        }

        // Clearing with no explicit seed (server cleared) empties the map, so
        // no old-server forwarder can be re-dialed.
        reseed_discovered_forwarders(&state, None).await;
        assert!(state.discovered_forwarders.read().await.is_empty());

        // Re-populate, then reseed with a valid explicit (loopback/dev)
        // forwarder: only that entry survives.
        {
            let mut d = state.discovered_forwarders.write().await;
            d.insert(
                "old-a".to_owned(),
                DiscoveredForwarder {
                    display_name: None,
                    direct_addrs: vec!["127.0.0.1:1".parse().unwrap()],
                    streams: Vec::new(),
                },
            );
        }
        let seed_id = SecretKey::from_bytes(&[7u8; 32]).public().to_string();
        let seed = ForwarderPeerConfig {
            node_id: seed_id.clone(),
            direct_addr: "127.0.0.1:9000".parse().unwrap(),
        };
        reseed_discovered_forwarders(&state, Some(&seed)).await;
        let map = state.discovered_forwarders.read().await;
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&seed_id));
        assert!(!map.contains_key("old-a"));
    }

    /// Bind a minimal loopback runtime with the given identity, capture the
    /// resulting p2p node id, and shut it down.
    #[cfg(test)]
    async fn bind_node_id(identity: ReceiverIdentity) -> String {
        use crate::control_api::AppState;
        use crate::db::Db;
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let cfg = P2pReceiverConfig {
            identity,
            relay_disabled: true,
            discovery_disabled: true,
            bind_addr_v4: Some(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::LOCALHOST,
                0,
            )),
            forwarder: None,
            server: None,
            server_override: (None, None),
            reconcile_interval: Duration::from_millis(1000),
        };
        let rt = start_receiver_p2p(Arc::clone(&state), cfg)
            .await
            .expect("runtime starts");
        let id = state
            .p2p_endpoint_id
            .read()
            .await
            .clone()
            .expect("endpoint id set after start");
        rt.shutdown().await;
        id
    }

    /// A valid 64-hex-character secret-key seed for config-builder tests.
    const TEST_SEED_HEX: &str = "abababababababababababababababababababababababababababababababab";

    /// Build a `get(key)` closure from `(key, value)` pairs for the lookup-based
    /// P2P config builder, so tests need not mutate process-global env vars.
    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    /// Default key path for config-builder tests; never written because these
    /// tests do not bind an endpoint.
    fn test_default_key_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/rt-p2p-test-default.key")
    }

    /// Wrapper over [`p2p_config_from_lookup`] supplying a test default key path.
    fn cfg_from(pairs: &[(&str, &str)]) -> Result<Option<P2pReceiverConfig>, String> {
        p2p_config_from_lookup(lookup(pairs), test_default_key_path())
    }

    #[test]
    fn client_hello_advertises_control_events_capability() {
        let hello = client_hello();

        assert!(rt_p2p_protocol::has_capability(
            &hello.capabilities,
            rt_p2p_protocol::CAP_CONTROL_EVENTS
        ));
    }

    #[test]
    fn client_hello_advertises_remote_config_capability() {
        let hello = client_hello();

        assert!(rt_p2p_protocol::has_capability(
            &hello.capabilities,
            rt_p2p_protocol::CAP_REMOTE_CONFIG
        ));
    }

    #[test]
    fn client_hello_advertises_reader_control_capability() {
        let hello = client_hello();

        assert!(rt_p2p_protocol::has_capability(
            &hello.capabilities,
            rt_p2p_protocol::CAP_READER_CONTROL
        ));
    }

    #[test]
    fn p2p_config_from_lookup_none_when_no_keys() {
        assert!(cfg_from(&[]).unwrap().is_none());
    }

    #[test]
    fn lookup_defaults_to_keypath_when_no_seed_and_rejects_both() {
        // Server set, no seed/key-path env -> persistent default key path.
        let cfg = cfg_from(&[
            (ENV_P2P_SERVER_URL, "http://x"),
            (ENV_P2P_SERVER_TOKEN, "t"),
        ])
        .unwrap()
        .expect("config present");
        assert!(matches!(cfg.identity, ReceiverIdentity::KeyPath(_)));
        // Key-path identity defaults to production transport.
        assert!(!cfg.relay_disabled);
        assert!(!cfg.discovery_disabled);
        assert!(cfg.bind_addr_v4.is_none());

        // Seed AND explicit key path -> mutually exclusive error.
        let seed_hex = "ab".repeat(32);
        let err = cfg_from(&[
            (ENV_P2P_SERVER_URL, "http://x"),
            (ENV_P2P_SERVER_TOKEN, "t"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, seed_hex.as_str()),
            (ENV_P2P_SECRET_KEY_PATH, "/k"),
        ])
        .unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }

    #[test]
    fn lookup_relay_and_discovery_flags_parse() {
        let cfg = cfg_from(&[
            (ENV_P2P_SERVER_URL, "http://x"),
            (ENV_P2P_SERVER_TOKEN, "t"),
            (ENV_P2P_RELAY_DISABLED, "1"),
            (ENV_P2P_DISCOVERY_DISABLED, "true"),
        ])
        .unwrap()
        .expect("config present");
        assert!(cfg.relay_disabled);
        assert!(cfg.discovery_disabled);
    }

    #[test]
    fn approval_became_active_detects_only_rising_edge() {
        assert!(approval_became_active(None, Some("active")));
        assert!(approval_became_active(Some("pending"), Some("active")));
        assert!(!approval_became_active(Some("active"), Some("active")));
        assert!(!approval_became_active(Some("active"), Some("pending")));
        assert!(!approval_became_active(Some("pending"), Some("pending")));
    }

    #[test]
    fn receiver_pending_approval_detects_only_registered_pending_receiver() {
        let pending = crate::control_api::ServerDeviceStatus {
            configured: true,
            endpoint_id: Some("receiver-ep".to_owned()),
            reachable: Some(true),
            approval_state: Some("pending".to_owned()),
            waiting_for_approval: true,
            message: None,
        };
        assert!(receiver_pending_approval(&pending));

        let unregistered = crate::control_api::ServerDeviceStatus {
            configured: true,
            endpoint_id: Some("receiver-ep".to_owned()),
            reachable: Some(true),
            approval_state: None,
            waiting_for_approval: true,
            message: None,
        };
        assert!(!receiver_pending_approval(&unregistered));

        let active = crate::control_api::ServerDeviceStatus {
            configured: true,
            endpoint_id: Some("receiver-ep".to_owned()),
            reachable: Some(true),
            approval_state: Some("active".to_owned()),
            waiting_for_approval: false,
            message: None,
        };
        assert!(!receiver_pending_approval(&active));
    }

    #[tokio::test]
    async fn approval_follow_up_reconnect_resets_backoff_when_still_not_connected() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _shutdown_rx) = crate::runtime::init_with_data_dir(None, dir.path())
            .await
            .expect("init receiver state");
        let mut connect_rx = state.connect_attempt_rx();

        schedule_approval_follow_up_reconnect(Arc::clone(&state), Duration::from_millis(10));

        tokio::time::timeout(Duration::from_secs(2), connect_rx.changed())
            .await
            .expect("follow-up reconnect should fire while disconnected")
            .expect("connect attempt sender should remain open");
        assert_eq!(state.current_connect_attempt(), 1);
    }

    #[tokio::test]
    async fn approval_follow_up_reconnect_does_not_restart_when_already_connected() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _shutdown_rx) = crate::runtime::init_with_data_dir(None, dir.path())
            .await
            .expect("init receiver state");
        state.set_connection_state(ConnectionState::Connected).await;
        let mut connect_rx = state.connect_attempt_rx();

        schedule_approval_follow_up_reconnect(Arc::clone(&state), Duration::from_millis(10));

        assert!(
            tokio::time::timeout(Duration::from_millis(100), connect_rx.changed())
                .await
                .is_err(),
            "follow-up reconnect should not fire after the receiver is connected"
        );
        assert_eq!(state.current_connect_attempt(), 0);
    }

    #[tokio::test]
    async fn approval_watch_uses_configured_server_url_without_profile() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _shutdown_rx) = crate::runtime::init_with_data_dir(None, dir.path())
            .await
            .expect("init receiver state");
        state.set_p2p_endpoint_id("receiver-ep".to_owned()).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/status",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "devices": [{
                        "endpoint_id": "receiver-ep",
                        "approval_state": "active"
                    }]
                }))
            }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut connect_rx = state.connect_attempt_rx();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(run_approval_watch_loop(
            Arc::clone(&state),
            format!("http://{addr}"),
            Duration::from_millis(50),
            shutdown_rx,
        ));

        tokio::time::timeout(Duration::from_secs(2), connect_rx.changed())
            .await
            .expect("approval watch should request reconnect from configured server url")
            .expect("connect attempt sender should remain open");
        assert_eq!(state.current_connect_attempt(), 1);

        let _ = shutdown_tx.send(true);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("approval watch should stop")
            .expect("approval watch task should not panic");
    }

    #[test]
    fn p2p_config_from_lookup_builds_minimal_config() {
        let cfg = cfg_from(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
        ])
        .unwrap()
        .expect("config present");
        let fwd = cfg.forwarder.as_ref().expect("forwarder present");
        assert_eq!(fwd.node_id, "endpoint-x");
        assert_eq!(fwd.direct_addr, "127.0.0.1:5000".parse().unwrap());
        assert!(matches!(cfg.identity, ReceiverIdentity::Seed(s) if s == [0xab; 32]));
        assert!(cfg.server.is_none());
        assert_eq!(cfg.reconcile_interval, Duration::from_millis(1000));
    }

    #[test]
    fn p2p_config_from_lookup_accepts_server_only_without_forwarder() {
        let cfg = cfg_from(&[
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_SERVER_URL, "http://127.0.0.1:8080"),
            (ENV_P2P_SERVER_TOKEN, "tok"),
        ])
        .unwrap()
        .expect("config present");
        assert!(
            cfg.forwarder.is_none(),
            "server-only config must not require an explicit forwarder"
        );
        assert!(cfg.server.is_some());
    }

    #[test]
    fn p2p_config_from_lookup_allows_transport_only_without_forwarder_or_server() {
        // Only identity/transport knobs set (no forwarder, no server). This is
        // valid: the caller attaches the resolved profile/override server, so a
        // dev override like RT_P2P_RELAY_DISABLED=1 with a stored-profile server
        // must not fail at startup.
        let cfg = cfg_from(&[
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_RELAY_DISABLED, "1"),
        ])
        .unwrap()
        .expect("transport-only config is valid");
        assert!(cfg.forwarder.is_none());
        assert!(cfg.server.is_none());
        assert_eq!(cfg.server_override, (None, None));
        assert!(cfg.relay_disabled);
    }

    #[test]
    fn p2p_config_from_lookup_captures_server_override() {
        let cfg = cfg_from(&[
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_SERVER_URL, "http://127.0.0.1:8080"),
            (ENV_P2P_SERVER_TOKEN, "tok"),
        ])
        .unwrap()
        .expect("config present");
        assert_eq!(
            cfg.server_override,
            (
                Some("http://127.0.0.1:8080".to_owned()),
                Some("tok".to_owned())
            )
        );
    }

    #[test]
    fn p2p_config_from_lookup_errors_on_partial_required_keys() {
        let err = cfg_from(&[(ENV_P2P_FORWARDER_NODE_ID, "endpoint-x")]).unwrap_err();
        assert!(err.contains(ENV_P2P_FORWARDER_DIRECT_ADDR), "got: {err}");
    }

    #[test]
    fn p2p_config_from_lookup_server_requires_both() {
        let err = cfg_from(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_SERVER_URL, "http://127.0.0.1:8080"),
        ])
        .unwrap_err();
        assert!(err.contains(ENV_P2P_SERVER_TOKEN), "got: {err}");
    }

    #[test]
    fn p2p_config_from_lookup_accepts_server_pair_and_reconcile_override() {
        let cfg = cfg_from(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_SERVER_URL, "http://127.0.0.1:8080"),
            (ENV_P2P_SERVER_TOKEN, "tok"),
            (ENV_P2P_RECONCILE_MS, "200"),
        ])
        .unwrap()
        .expect("config present");
        let thin = cfg.server.expect("server configured");
        assert_eq!(thin.url, "http://127.0.0.1:8080");
        assert_eq!(thin.token, "tok");
        assert_eq!(cfg.reconcile_interval, Duration::from_millis(200));
    }

    #[test]
    fn p2p_config_from_lookup_rejects_below_min_reconcile() {
        let err = cfg_from(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_RECONCILE_MS, "10"),
        ])
        .unwrap_err();
        assert!(err.contains(ENV_P2P_RECONCILE_MS), "got: {err}");
    }

    fn insert_chip_event(db: &Db, stream_id: &str, seq: i64, received_unix_ms: i64) {
        insert_chip_event_in_epoch(db, stream_id, seq, 1, received_unix_ms);
    }

    fn insert_chip_event_in_epoch(
        db: &Db,
        stream_id: &str,
        seq: i64,
        epoch: i64,
        received_unix_ms: i64,
    ) {
        db.insert_received_event(&ReceivedEventInsert {
            stream_id,
            seq,
            epoch,
            raw_frame: SAMPLE_FRAME,
            read_kind: "chip",
            reader_timestamp: None,
            received_unix_ms,
            dbf_delivered_unix_ms: None,
            chip_id: None,
        })
        .unwrap();
    }

    #[tokio::test]
    async fn startup_rebuild_seeds_projection_and_stream_counts() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let stream_id = "rebuild-stream";
        {
            let db = state.db.lock().await;
            insert_chip_event_in_epoch(&db, stream_id, 1, 1, 1_700_000_000_100);
            insert_chip_event_in_epoch(&db, stream_id, 2, 1, 1_700_000_000_200);
            insert_chip_event_in_epoch(&db, stream_id, 3, 2, 1_700_000_000_300);
        }
        let ui_key = StreamKey::new("fwd-1", "10.0.0.1:10000");

        let proj = rebuild_stream_projection(&state, stream_id, &ui_key)
            .await
            .expect("rebuild succeeds");

        assert_eq!(proj.total, 3);
        assert_eq!(proj.epoch, 2);
        assert_eq!(
            proj.epoch_count, 1,
            "epoch count covers the live epoch only"
        );
        assert_eq!(proj.unique_chips.len(), 1);
        assert_eq!(proj.last_seq, 3);
        assert_eq!(proj.last_chip_id.as_deref(), Some("000000012345"));
        assert_eq!(proj.max_received_unix_ms, 1_700_000_000_300);

        // The shared counts cache is seeded so status/UI totals survive restart.
        let counts = state.stream_counts.get(&ui_key).expect("counts seeded");
        assert_eq!(counts.total, 3);
        assert_eq!(counts.epoch, 1);
        assert_eq!(counts.current_epoch, 2);
    }

    #[tokio::test]
    async fn startup_rebuild_with_empty_stream_is_default() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let ui_key = StreamKey::new("fwd-1", "10.0.0.1:10000");
        let proj = rebuild_stream_projection(&state, "missing", &ui_key)
            .await
            .expect("rebuild succeeds");
        assert_eq!(proj.total, 0);
        assert!(
            state.stream_counts.get(&ui_key).is_none(),
            "an empty stream must not fabricate a counts entry"
        );
    }

    /// Feed facts through a hint channel into the projection worker and wait
    /// for the throttled LastRead emit.
    async fn projected_last_read(
        state: &Arc<AppState>,
        stream_id: &str,
        facts: Vec<crate::p2p_session::EventFact>,
    ) -> crate::ui_events::LastRead {
        let mut ui_rx = state.ui_tx.subscribe();
        let through_seq = facts.iter().map(|fact| fact.seq).max().unwrap_or(0);
        let (hint_tx, hint_rx) = broadcast::channel::<DurableBatch>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let worker = tokio::spawn(run_ui_projection_worker(
            Arc::clone(state),
            stream_id.to_owned(),
            StreamKey::new("fwd-1", "10.0.0.1:10000"),
            hint_rx,
            shutdown_rx,
        ));
        hint_tx
            .send(DurableBatch {
                through_seq,
                inserted: std::sync::Arc::new(facts),
            })
            .unwrap();

        let mut last_read = None;
        while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_secs(2), ui_rx.recv()).await {
            if let ReceiverUiEvent::LastRead(read) = event {
                last_read = Some(read);
                break;
            }
        }
        let _ = shutdown_tx.send(true);
        let _ = worker.await;
        last_read.expect("last read event")
    }

    fn chip_fact(seq: i64, received_unix_ms: i64) -> crate::p2p_session::EventFact {
        crate::p2p_session::EventFact {
            seq,
            epoch: 1,
            received_unix_ms,
            chip_id: "000000012345".to_owned(),
        }
    }

    #[tokio::test]
    async fn stream_ui_projection_resolves_last_read_participant_names() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        crate::control_api::import_participants(&state, "42,Lovelace,Ada\n".to_owned())
            .await
            .unwrap();
        crate::control_api::import_chips(&state, "42,000000012345\n".to_owned())
            .await
            .unwrap();

        let read =
            projected_last_read(&state, "stream-a", vec![chip_fact(1, 1_700_000_000_123)]).await;
        assert_eq!(read.chip_id, "000000012345");
        assert_eq!(read.bib.as_deref(), Some("42"));
        assert_eq!(read.name.as_deref(), Some("Ada Lovelace"));
    }

    #[tokio::test]
    async fn stream_ui_projection_includes_bib_when_chip_has_no_participant() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        crate::control_api::import_chips(&state, "1488,000000012345\n".to_owned())
            .await
            .unwrap();

        let read =
            projected_last_read(&state, "stream-a", vec![chip_fact(1, 1_700_000_000_123)]).await;
        assert_eq!(read.chip_id, "000000012345");
        assert_eq!(read.bib.as_deref(), Some("1488"));
        assert_eq!(read.name, None);
    }

    #[test]
    fn resolve_local_port_falls_back_to_canonical_stream_address() {
        let sub = StreamSubscription {
            forwarder_endpoint_id: "endpoint".to_owned(),
            stream_id: "127.0.0.1:50057".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        };

        assert_eq!(resolve_local_port(&sub), default_port(&sub.stream_id));
    }

    /// Poll an async predicate until it returns `true` or `timeout` elapses.
    async fn poll_async<F, Fut>(timeout: Duration, mut f: F) -> bool
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let start = std::time::Instant::now();
        loop {
            if f().await {
                return true;
            }
            if start.elapsed() > timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// An announcer push client that fails while `fail` is set, and records the
    /// `(stream_id, seq)` of every row it successfully pushes otherwise.
    struct FlakyAnnouncerClient {
        fail: AtomicBool,
        pushed: std::sync::Mutex<Vec<(String, i64)>>,
    }

    impl AnnouncerPushClient for FlakyAnnouncerClient {
        fn push(
            &self,
            rows: &[AnnouncerRow],
            _max_list_size: u32,
        ) -> Result<(), AnnouncerPushError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(AnnouncerPushError::Transport("simulated outage".to_owned()));
            }
            let mut pushed = self.pushed.lock().unwrap();
            for row in rows {
                pushed.push((row.stream_id.clone(), row.seq));
            }
            Ok(())
        }
    }

    #[test]
    fn parse_secret_key_seed_hex_roundtrip() {
        let hex = "".to_owned() + &"ab".repeat(32);
        let seed = parse_secret_key_seed_hex(hex.trim()).unwrap();
        assert_eq!(seed, [0xab; 32]);
    }

    #[test]
    fn parse_secret_key_seed_hex_rejects_wrong_length() {
        assert!(parse_secret_key_seed_hex("abcd").is_err());
    }

    #[test]
    fn parse_secret_key_seed_hex_rejects_multibyte_without_panic() {
        // A leading 3-byte '€' (U+20AC) plus 61 ASCII bytes is exactly 64 UTF-8
        // bytes, but the first `[0..2]` slice lands inside the multibyte char.
        // The parser must return an error instead of panicking on a
        // non-char-boundary slice.
        let multibyte = format!("€{}", "a".repeat(61));
        assert_eq!(multibyte.len(), 64, "fixture must be 64 bytes");
        assert!(parse_secret_key_seed_hex(&multibyte).is_err());
    }

    #[test]
    fn resolve_local_port_prefers_override() {
        let sub = StreamSubscription {
            forwarder_endpoint_id: "fwd".to_owned(),
            stream_id: "127.0.0.1:10000".to_owned(),
            local_port_override: Some(9999),
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: Some("10.0.0.5:10000".to_owned()),
        };
        assert_eq!(resolve_local_port(&sub), Some(9999));
    }

    #[test]
    fn resolve_local_port_falls_back_to_reader_ip() {
        let sub = StreamSubscription {
            forwarder_endpoint_id: "fwd".to_owned(),
            stream_id: "127.0.0.1:10000".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: Some("10.0.0.5:10000".to_owned()),
        };
        assert_eq!(resolve_local_port(&sub), Some(10005));
    }

    #[test]
    fn resolve_local_port_none_when_unresolvable() {
        let sub = StreamSubscription {
            forwarder_endpoint_id: "fwd".to_owned(),
            stream_id: "stream-x".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        };
        assert_eq!(resolve_local_port(&sub), None);
    }

    #[test]
    fn server_client_config_debug_redacts_token() {
        let cfg = ServerClientConfig {
            url: "http://127.0.0.1:8080".to_owned(),
            token: "super-secret-token".to_owned(),
        };
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "token must not appear in debug output: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "debug output must mark the token as redacted: {rendered}"
        );
        assert!(
            rendered.contains("http://127.0.0.1:8080"),
            "url should still appear: {rendered}"
        );

        // The outer config derives Debug and must inherit the redaction.
        let outer = P2pReceiverConfig {
            identity: ReceiverIdentity::Seed([0u8; 32]),
            relay_disabled: true,
            discovery_disabled: true,
            bind_addr_v4: None,
            forwarder: Some(ForwarderPeerConfig {
                node_id: "node".to_owned(),
                direct_addr: "127.0.0.1:1".parse().unwrap(),
            }),
            server: Some(cfg),
            server_override: (None, None),
            reconcile_interval: Duration::from_millis(50),
        };
        let outer_rendered = format!("{outer:?}");
        assert!(
            !outer_rendered.contains("super-secret-token"),
            "token must not leak via outer config debug: {outer_rendered}"
        );
        assert!(
            outer_rendered.contains("<redacted>"),
            "outer config debug must show redaction: {outer_rendered}"
        );
    }

    /// The shared cross-stream DBF worker delivers pending rows on its
    /// interval tick (no hints involved), retries after failures (missing
    /// directory) on later ticks, and appends incrementally afterwards.
    #[tokio::test]
    async fn shared_dbf_worker_delivers_and_retries_on_interval() {
        let stream_id = "127.0.0.1:11000";
        let fwd = "fwd-dbf-retry";

        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let tmp = tempfile::tempdir().unwrap();
        let missing_dir = tmp.path().join("not-yet");
        {
            let mut db = state.db.lock().await;
            db.save_profile("http://server", "tok", "check-and-download", None)
                .unwrap();
            db.save_dbf_config(&crate::db::DbfConfig {
                enabled: true,
                flush_interval_ms: crate::db::DEFAULT_DBF_FLUSH_INTERVAL_MS,
            })
            .unwrap();
            db.save_rd_import_config(&crate::db::RdImportConfig {
                enabled: false,
                dir: missing_dir.to_string_lossy().into_owned(),
                interval_secs: 15,
            })
            .unwrap();
            db.replace_stream_subscriptions(&[StreamSubscription {
                forwarder_endpoint_id: fwd.to_owned(),
                stream_id: stream_id.to_owned(),
                local_port_override: None,
                event_type: EventType::Finish,
                forwarder_id: None,
                reader_ip: Some(stream_id.to_owned()),
            }])
            .unwrap();
            insert_chip_event(&db, stream_id, 1, 1_700_000_000_100);
            insert_chip_event(&db, stream_id, 2, 1_700_000_000_200);
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_shared_dbf_worker(Arc::clone(&state), shutdown_rx));

        // While the RD directory is missing every pass fails; rows stay
        // undelivered.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        {
            let guard = state.db.lock().await;
            assert_eq!(
                guard
                    .load_undelivered_received_events(stream_id)
                    .unwrap()
                    .len(),
                2,
                "events must remain undelivered while the DBF dir is missing"
            );
        }

        // Recover by creating the directory: the next tick regenerates.
        std::fs::create_dir_all(&missing_dir).unwrap();
        let delivered = poll_async(Duration::from_secs(10), || {
            let state = Arc::clone(&state);
            async move {
                state
                    .db
                    .lock()
                    .await
                    .load_undelivered_received_events(stream_id)
                    .unwrap()
                    .is_empty()
            }
        })
        .await;
        assert!(delivered, "interval tick must deliver once the dir exists");
        assert!(missing_dir.join("IPICO.DBF").exists());

        // New rows are appended incrementally on later ticks.
        {
            let guard = state.db.lock().await;
            insert_chip_event(&guard, stream_id, 3, 1_700_000_000_300);
        }
        let appended = poll_async(Duration::from_secs(10), || {
            let state = Arc::clone(&state);
            async move {
                state
                    .db
                    .lock()
                    .await
                    .load_undelivered_received_events(stream_id)
                    .unwrap()
                    .is_empty()
            }
        })
        .await;
        assert!(appended, "incremental append must deliver new rows");

        let _ = shutdown_tx.send(true);
        let _ = handle.await;
    }

    /// An announcer push that fails (simulated transport outage) must be retried
    /// by the worker's retry timer once the sink recovers, even though no new
    /// durable hint arrives after the failure.
    #[tokio::test]
    async fn announcer_worker_retries_after_push_failure_without_new_hint() {
        let stream_id = "ann-retry-stream";

        let db = Db::open_in_memory().unwrap();
        insert_chip_event(&db, stream_id, 1, 1_700_000_000_100);
        insert_chip_event(&db, stream_id, 2, 1_700_000_000_200);
        let db = Arc::new(Mutex::new(db));
        let chip_lookup = Arc::new(tokio::sync::RwLock::new(ChipLookup::new()));

        let client = Arc::new(FlakyAnnouncerClient {
            fail: AtomicBool::new(true),
            pushed: std::sync::Mutex::new(Vec::new()),
        });
        let client_dyn: Arc<dyn AnnouncerPushClient + Send + Sync> = Arc::clone(&client) as _;

        let (hint_tx, hint_rx) = broadcast::channel::<DurableBatch>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_announcer_worker(
            Arc::clone(&db),
            chip_lookup,
            stream_id.to_owned(),
            client_dyn,
            1,
            Duration::from_millis(50),
            hint_rx,
            shutdown_rx,
        ));

        // Startup attempt and an explicit hint both fail while the sink is down.
        let _ = hint_tx.send(DurableBatch {
            through_seq: 1,
            inserted: std::sync::Arc::new(Vec::new()),
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            client.pushed.lock().unwrap().is_empty(),
            "nothing must be pushed while the announcer sink is failing"
        );
        {
            let guard = db.lock().await;
            assert_eq!(
                guard
                    .load_unpushed_announcer_events(stream_id)
                    .unwrap()
                    .len(),
                2,
                "rows must stay unpushed while the sink is failing"
            );
        }

        // Recover by clearing the failure. Send NO new hint.
        client.fail.store(false, Ordering::SeqCst);

        let pushed = poll_async(Duration::from_secs(5), || {
            let client = Arc::clone(&client);
            async move { client.pushed.lock().unwrap().len() >= 2 }
        })
        .await;
        assert!(
            pushed,
            "retry timer must push pending announcer rows after recovery without a new hint"
        );

        {
            let guard = db.lock().await;
            assert!(
                guard
                    .load_unpushed_announcer_events(stream_id)
                    .unwrap()
                    .is_empty(),
                "rows must be marked pushed after a successful retry"
            );
        }

        let _ = shutdown_tx.send(true);
        let _ = handle.await;
        drop(hint_tx);
    }

    #[test]
    fn desired_forwarders_include_discovered_and_subscribed_unless_disconnected() {
        let discovered_only = node_id_for_seed([21u8; 32]);
        let subscribed = node_id_for_seed([22u8; 32]);
        let disconnected = node_id_for_seed([23u8; 32]);
        let mut discovered = DiscoveredForwarders::new();
        discovered.insert(
            discovered_only.clone(),
            DiscoveredForwarder {
                display_name: None,
                direct_addrs: vec!["127.0.0.1:7001".parse().unwrap()],
                streams: Vec::new(),
            },
        );
        discovered.insert(
            disconnected.clone(),
            DiscoveredForwarder {
                display_name: None,
                direct_addrs: vec!["127.0.0.1:7003".parse().unwrap()],
                streams: Vec::new(),
            },
        );
        let subs = vec![
            StreamSubscription {
                forwarder_endpoint_id: subscribed.clone(),
                stream_id: "stream-a".to_owned(),
                local_port_override: None,
                event_type: EventType::Finish,
                forwarder_id: None,
                reader_ip: None,
            },
            StreamSubscription {
                forwarder_endpoint_id: disconnected.clone(),
                stream_id: "stream-b".to_owned(),
                local_port_override: None,
                event_type: EventType::Finish,
                forwarder_id: None,
                reader_ip: None,
            },
        ];
        let intents = HashMap::from([(disconnected.clone(), false)]);

        let desired = desired_forwarder_subscriptions(&discovered, &subs, &intents);

        assert!(desired.contains_key(&discovered_only));
        assert_eq!(desired.get(&subscribed).unwrap().len(), 1);
        assert!(!desired.contains_key(&disconnected));
    }

    #[test]
    fn build_discovered_forwarders_skips_invalid_node_ids() {
        use crate::announcer_push::{ForwarderDiscoveryEntry, ForwarderDiscoveryStream};

        let valid = node_id_for_seed([7u8; 32]);
        let entries = vec![
            ForwarderDiscoveryEntry {
                endpoint_id: valid.clone(),
                display_name: Some("Start".to_owned()),
                direct_addrs: vec!["127.0.0.1:5000".to_owned(), "bad-addr".to_owned()],
                streams: vec![ForwarderDiscoveryStream {
                    stream_id: "reader-a".to_owned(),
                    epoch: 2,
                    next_seq: 9,
                }],
            },
            ForwarderDiscoveryEntry {
                endpoint_id: "not-a-node-id".to_owned(),
                display_name: None,
                direct_addrs: vec!["127.0.0.1:6000".to_owned()],
                streams: vec![],
            },
        ];

        let map = build_discovered_forwarders(entries, None);

        // Only the valid entry survives; the malformed id is dropped here, once.
        assert_eq!(map.len(), 1);
        let fwd = map.get(&valid).expect("valid forwarder present");
        assert_eq!(fwd.display_name.as_deref(), Some("Start"));
        // Unparseable direct addr is filtered out, the valid one kept.
        assert_eq!(fwd.direct_addrs, vec!["127.0.0.1:5000".parse().unwrap()]);
        assert_eq!(fwd.streams.len(), 1);
        assert_eq!(fwd.streams[0].stream_id, "reader-a");
        assert_eq!(fwd.streams[0].epoch, 2);
        assert_eq!(fwd.streams[0].next_seq, 9);

        // The dropped id never reaches the map, so resolve_forwarder_addr is
        // never asked to parse (and warn about) it at reconcile cadence.
        assert!(resolve_forwarder_addr("not-a-node-id", &map).is_none());
        assert!(resolve_forwarder_addr(&valid, &map).is_some());
    }

    #[test]
    fn build_discovered_forwarders_seed_added_only_when_absent() {
        use crate::announcer_push::ForwarderDiscoveryEntry;

        let seed_id = node_id_for_seed([3u8; 32]);
        let seed = ForwarderPeerConfig {
            node_id: seed_id.clone(),
            direct_addr: "127.0.0.1:7000".parse().unwrap(),
        };

        // Seed is inserted when the feed does not advertise it.
        let map = build_discovered_forwarders(vec![], Some(&seed));
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&seed_id).unwrap().direct_addrs,
            vec!["127.0.0.1:7000".parse().unwrap()]
        );

        // Seed does NOT override a discovered entry for the same endpoint id.
        let entries = vec![ForwarderDiscoveryEntry {
            endpoint_id: seed_id.clone(),
            display_name: Some("Discovered".to_owned()),
            direct_addrs: vec!["10.0.0.1:8000".to_owned()],
            streams: vec![],
        }];
        let map = build_discovered_forwarders(entries, Some(&seed));
        assert_eq!(map.len(), 1);
        let fwd = map.get(&seed_id).unwrap();
        assert_eq!(fwd.display_name.as_deref(), Some("Discovered"));
        assert_eq!(fwd.direct_addrs, vec!["10.0.0.1:8000".parse().unwrap()]);
    }
}

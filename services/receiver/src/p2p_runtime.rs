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
//! * a thin-node announcer push worker (when a thin-node client is configured)
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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use rt_iroh::{Endpoint, EndpointBuilder, NodeAddr, NodeId, SecretKey};
use rt_p2p_protocol::{Hello, MAX_FRAME_BYTES, SubscribeMode};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::announcer_push::{
    self, AnnouncerPushClient, ParticipantResolver, ResolvedParticipant, ThinNodeAnnouncerClient,
};
use crate::control_api::AppState;
use crate::control_api::ConnectionState;
use crate::control_api::{DiscoveredForwarder, DiscoveredForwarders, DiscoveredStream};
use crate::db::{Db, StreamSubscription};
use crate::local_proxy::LocalProxy;
use crate::p2p_session::{
    BackoffConfig, SessionParams, SessionStatusReporter, run_session_with_reconnect,
};
use crate::ports::default_port;

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

/// Configuration for the optional thin-node client (register / takeover /
/// announcer rows).
///
/// `Debug` is implemented by hand (not derived) so the bearer token is never
/// leaked through debug logs; it is rendered as `<redacted>`.
#[derive(Clone)]
pub struct ThinNodeClientConfig {
    /// Base URL, e.g. `http://127.0.0.1:8080`.
    pub url: String,
    /// Per-device bearer token. Never logged.
    pub token: String,
}

impl std::fmt::Debug for ThinNodeClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThinNodeClientConfig")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Full P2P receiver configuration. Presence of this in
/// [`HeadlessConfig`](crate::headless::HeadlessConfig) enables the P2P lane.
#[derive(Clone, Debug)]
pub struct P2pReceiverConfig {
    /// Deterministic loopback secret-key seed (see
    /// [`EndpointBuilder::test`]).
    pub secret_key_seed: [u8; 32],
    /// An optional explicit forwarder peer to dial. When present it is seeded
    /// into the discovered-forwarders map at startup so the loopback/dev path
    /// works without a thin node. When absent, forwarders are learned entirely
    /// from the thin-node discovery feed.
    pub forwarder: Option<ForwarderPeerConfig>,
    /// Optional thin-node client for announcer push and forwarder discovery.
    pub thin_node: Option<ThinNodeClientConfig>,
    /// How often to reconcile canonical subscriptions. Must be at least
    /// [`MIN_RECONCILE_INTERVAL`]; also used as the delivery retry cadence.
    pub reconcile_interval: Duration,
}

/// Build the client `Hello` presented during control-plane negotiation.
fn client_hello() -> Hello {
    Hello {
        min_minor: 1,
        max_minor: 1,
        capabilities: vec!["data".to_owned()],
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
    let endpoint = EndpointBuilder::test(config.secret_key_seed)
        .bind()
        .await
        .map_err(|e| format!("failed to bind p2p endpoint: {e}"))?;
    // Stdout line consumed by T5.4 orchestration to learn this receiver's id.
    println!("p2p_node_id={}", endpoint.node_id());
    let local_addr = endpoint.node_addr().await;
    info!(
        p2p_node_id = %endpoint.node_id(),
        direct_addresses = ?local_addr.direct_addresses,
        "receiver p2p endpoint bound"
    );

    // Seed the discovered-forwarders map from the optional explicit forwarder
    // so the loopback/dev path (and tests) dial it without a thin node. The
    // discovery task (when a thin node is configured) refreshes the map but
    // preserves this seed if the thin node hasn't advertised the same endpoint.
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
    let live_sessions = Arc::new(AtomicUsize::new(0));
    let reporter = Arc::new(SessionStatusReporter::new(
        Arc::clone(&state),
        live_sessions,
    ));

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

/// Per-stream worker bundle.
struct StreamWorker {
    /// The canonical subscription this worker was built from. Reconciliation
    /// compares the desired subscription against this snapshot and rebuilds the
    /// worker if any field that affects worker behavior changed.
    sub: StreamSubscription,
    /// Cancels the session task and DBF/announcer workers.
    shutdown_tx: watch::Sender<bool>,
    /// The durable local proxy, if a local port could be resolved.
    proxy: Option<LocalProxy>,
    /// The reconnecting session task. Tracked separately so reconciliation can
    /// detect a non-retryable session exit and rebuild the worker.
    session_task: JoinHandle<()>,
    /// DBF + announcer task handles.
    tasks: Vec<JoinHandle<()>>,
    /// Whether an announcer push worker was spawned for this worker. When the
    /// thin-node was unavailable at worker-build time (no fenced generation
    /// yet) this is `false`, and reconciliation rebuilds the worker once a
    /// generation becomes available so announcer push can start.
    announcer_active: bool,
}

impl StreamWorker {
    /// Whether the reconnecting session task has exited. A non-retryable session
    /// error completes this task; reconciliation treats a finished session as a
    /// dead worker to be rebuilt.
    fn session_finished(&self) -> bool {
        self.session_task.is_finished()
    }

    async fn stop(self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(proxy) = self.proxy {
            proxy.shutdown();
        }
        let mut tasks = self.tasks;
        tasks.push(self.session_task);
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
    config: P2pReceiverConfig,
    endpoint: Endpoint,
    reporter: Arc<SessionStatusReporter>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let endpoint = Arc::new(endpoint);
    let mut workers: HashMap<String, StreamWorker> = HashMap::new();

    // When a thin node is configured, periodically refresh the discovered
    // forwarders map from its `GET /forwarders` feed. The task observes the
    // same shutdown signal and is awaited/aborted below.
    let discovery_task = config.thin_node.clone().map(|thin| {
        tokio::spawn(run_discovery_loop(
            Arc::clone(&state),
            thin,
            config.forwarder.clone(),
            config.reconcile_interval,
            shutdown_rx.clone(),
        ))
    });

    // Thin-node announcer generation, acquired by registering this endpoint and
    // taking over the announcer generation. When the thin-node is unavailable
    // at startup the takeover is retried every reconcile pass (bounded by the
    // reconcile interval, racing the shutdown signal) until it succeeds rather
    // than permanently disabling announcer push. Workers are rebuilt once a
    // generation becomes available so they begin pushing pending rows. The HTTP
    // calls are bounded by the blocking client's connect/request timeouts.
    let mut announcer_generation: Option<i64> = None;

    loop {
        if announcer_generation.is_none()
            && let Some(thin) = config.thin_node.clone()
        {
            let endpoint_id = endpoint.node_id().to_string();
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() { break; }
                }
                result = thin_node_startup(thin, endpoint_id) => match result {
                    Ok(generation) => {
                        info!(generation, "thin-node announcer startup succeeded");
                        announcer_generation = Some(generation);
                    }
                    Err(e) => {
                        warn!(error = %e, "thin-node announcer startup failed; will retry");
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
        )
        .await;

        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            () = tokio::time::sleep(config.reconcile_interval) => {}
        }
    }

    for (_stream_id, worker) in workers.drain() {
        worker.stop().await;
    }
    if let Some(task) = discovery_task {
        task.abort();
        let _ = task.await;
    }
    endpoint.close().await;
    // The runtime is shutting down: no sessions remain and none will be
    // reattempted, so report a clean Disconnected. `P2pReceiverRuntime::shutdown`
    // awaits this task, so the state is settled before shutdown returns.
    state
        .set_connection_state(ConnectionState::Disconnected)
        .await;
}

async fn thin_node_startup(thin: ThinNodeClientConfig, endpoint_id: String) -> Result<i64, String> {
    tokio::task::spawn_blocking(move || {
        announcer_push::register_receiver_with_thin_node(&thin.url, &thin.token, &endpoint_id)
            .map_err(|e| e.to_string())?;
        announcer_push::takeover_announcer_generation(&thin.url, &thin.token)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("thin-node startup task failed: {e}"))?
}

/// Periodically refresh [`AppState::discovered_forwarders`] from the thin-node
/// `GET /forwarders` feed. Failures are logged and retried on the next interval;
/// the task never crashes. The optional explicit `seed` forwarder is preserved
/// in the refreshed map when the thin node has not advertised that endpoint, so
/// the loopback/dev path keeps working alongside discovery.
async fn run_discovery_loop(
    state: Arc<AppState>,
    thin: ThinNodeClientConfig,
    seed: Option<ForwarderPeerConfig>,
    interval: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        match fetch_forwarders(&thin).await {
            Ok(entries) => {
                let mut map = DiscoveredForwarders::new();
                for entry in entries {
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
                if let Some(forwarder) = &seed {
                    map.entry(forwarder.node_id.clone())
                        .or_insert_with(|| DiscoveredForwarder {
                            display_name: None,
                            direct_addrs: vec![forwarder.direct_addr],
                            streams: Vec::new(),
                        });
                }
                *state.discovered_forwarders.write().await = map;
            }
            Err(e) => {
                warn!(error = %e, "forwarder discovery fetch failed; will retry");
            }
        }

        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            () = tokio::time::sleep(interval) => {}
        }
    }
}

async fn fetch_forwarders(
    thin: &ThinNodeClientConfig,
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
fn resolve_forwarder_addr(
    endpoint_id: &str,
    discovered: &DiscoveredForwarders,
) -> Option<NodeAddr> {
    let forwarder = discovered.get(endpoint_id)?;
    let node_id = match endpoint_id.parse::<NodeId>() {
        Ok(node_id) => node_id,
        Err(e) => {
            warn!(%endpoint_id, error = %e, "discovered forwarder has invalid node id; skipping");
            return None;
        }
    };
    Some(NodeAddr::new(node_id).with_direct_addresses(forwarder.direct_addrs.iter().copied()))
}

async fn reconcile_once(
    state: &Arc<AppState>,
    config: &P2pReceiverConfig,
    endpoint: &Arc<Endpoint>,
    reporter: &Arc<SessionStatusReporter>,
    announcer_generation: Option<i64>,
    workers: &mut HashMap<String, StreamWorker>,
) {
    let subs: Vec<StreamSubscription> = {
        let db = state.db.lock().await;
        match db.load_stream_subscriptions() {
            Ok(subs) => subs,
            Err(e) => {
                warn!(error = %e, "failed to load stream subscriptions; skipping reconcile pass");
                return;
            }
        }
    };

    // All subscriptions are desired, keyed by stream id (globally unique in the
    // receiver's durable model). The forwarder that serves each stream is
    // resolved per-subscription from the discovered-forwarders map below.
    let mut desired: HashMap<String, StreamSubscription> = HashMap::new();
    for sub in subs {
        desired.insert(sub.stream_id.clone(), sub);
    }

    // Stop workers whose subscription disappeared.
    let stale: Vec<String> = workers
        .keys()
        .filter(|stream_id| !desired.contains_key(*stream_id))
        .cloned()
        .collect();
    for stream_id in stale {
        if let Some(worker) = workers.remove(&stream_id) {
            info!(%stream_id, "stopping p2p stream worker (subscription removed)");
            worker.stop().await;
        }
    }

    // Whether announcer push should be running: a thin-node is configured and a
    // fenced generation has been acquired.
    let announcer_desired = config.thin_node.is_some() && announcer_generation.is_some();

    // Snapshot discovered forwarders for per-subscription address resolution.
    let discovered = state.discovered_forwarders.read().await.clone();

    // Start workers for newly-seen subscriptions, and rebuild workers whose
    // effective config changed, whose session task has exited non-retryably, or
    // that need an announcer push worker now that a generation is available.
    for (stream_id, sub) in desired {
        // Resolve the forwarder that serves this subscription. If it isn't
        // discovered yet, skip this pass (no worker started); an existing worker
        // is left running so it keeps reconnecting. The worker starts once
        // discovery learns the forwarder.
        let Some(forwarder_addr) = resolve_forwarder_addr(&sub.forwarder_endpoint_id, &discovered)
        else {
            continue;
        };
        if let Some(existing) = workers.get(&stream_id) {
            let config_changed = existing.sub != sub;
            let session_dead = existing.session_finished();
            let announcer_missing = announcer_desired && !existing.announcer_active;
            if !config_changed && !session_dead && !announcer_missing {
                continue;
            }
            if let Some(worker) = workers.remove(&stream_id) {
                if config_changed {
                    info!(%stream_id, "rebuilding p2p stream worker (subscription config changed)");
                } else if session_dead {
                    info!(%stream_id, "rebuilding p2p stream worker (session exited)");
                } else {
                    info!(%stream_id, "rebuilding p2p stream worker (announcer now available)");
                }
                worker.stop().await;
            }
        }
        let worker = start_stream_worker(
            state,
            config,
            endpoint,
            &forwarder_addr,
            reporter,
            announcer_generation,
            &sub,
        )
        .await;
        workers.insert(stream_id, worker);
    }
}

async fn start_stream_worker(
    state: &Arc<AppState>,
    config: &P2pReceiverConfig,
    endpoint: &Arc<Endpoint>,
    forwarder_addr: &NodeAddr,
    reporter: &Arc<SessionStatusReporter>,
    announcer_generation: Option<i64>,
    sub: &StreamSubscription,
) -> StreamWorker {
    let stream_id = sub.stream_id.clone();
    info!(%stream_id, "starting p2p stream worker");

    let (hint_tx, _hint_rx) = broadcast::channel::<i64>(HINT_CHANNEL_CAPACITY);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut tasks: Vec<JoinHandle<()>> = Vec::new();

    // Reconnecting session.
    let session_task = {
        let endpoint = Arc::clone(endpoint);
        let forwarder_addr = forwarder_addr.clone();
        let db = Arc::clone(&state.db);
        let params = SessionParams {
            stream_id: stream_id.clone(),
            client_hello: client_hello(),
            mode: SubscribeMode::Replay,
            backoff: BackoffConfig::default(),
            durable_hint_tx: Some(hint_tx.clone()),
            reporter: Some(Arc::clone(reporter)),
        };
        let session_shutdown = shutdown_rx.clone();
        let session_stream_id = stream_id.clone();
        tokio::spawn(async move {
            if let Err(e) = run_session_with_reconnect(
                &endpoint,
                forwarder_addr,
                &db,
                &params,
                session_shutdown,
            )
            .await
            {
                warn!(error = %e, stream_id = %session_stream_id, "p2p session ended with error");
            }
        })
    };

    // Durable local proxy (only when a port can be resolved).
    let port = resolve_local_port(sub);
    let proxy = match port {
        Some(port) => {
            match LocalProxy::bind_durable(
                port,
                stream_id.clone(),
                Arc::clone(&state.db),
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

    // DBF feed.
    let dbf_config = {
        let db = state.db.lock().await;
        db.load_dbf_config().ok()
    };
    if let Some(dbf) = dbf_config.filter(|c| c.enabled) {
        let db = Arc::clone(&state.db);
        let dbf_path = dbf.path.clone();
        let stream_id = stream_id.clone();
        let forwarder_endpoint_id = sub.forwarder_endpoint_id.clone();
        let hint_rx = hint_tx.subscribe();
        let dbf_shutdown = shutdown_rx.clone();
        tasks.push(tokio::spawn(run_dbf_worker(
            db,
            stream_id,
            forwarder_endpoint_id,
            dbf_path,
            config.reconcile_interval,
            hint_rx,
            dbf_shutdown,
        )));
    }

    // Announcer push.
    let mut announcer_active = false;
    if let (Some(thin), Some(generation)) = (config.thin_node.clone(), announcer_generation) {
        match ThinNodeAnnouncerClient::new(&thin.url, thin.token.clone()) {
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
        proxy,
        session_task,
        tasks,
        announcer_active,
    }
}

/// Resolve the local TCP port for a subscription: explicit override first, then
/// the default mapping from `reader_ip`. Returns `None` if neither yields a
/// port.
fn resolve_local_port(sub: &StreamSubscription) -> Option<u16> {
    sub.local_port_override
        .or_else(|| sub.reader_ip.as_deref().and_then(default_port))
}

async fn run_dbf_worker(
    db: Arc<Mutex<Db>>,
    stream_id: String,
    forwarder_endpoint_id: String,
    dbf_path: String,
    retry_interval: Duration,
    mut hint_rx: broadcast::Receiver<i64>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    // `needs_retry` is set whenever a delivery attempt fails (DBF write or a
    // transient DB error). While set, the worker retries on `retry_interval`
    // even if no new durable hint arrives, so pending rows are not stranded
    // after the last hint. A successful attempt clears it.
    let mut needs_retry = !deliver_dbf(&db, &stream_id, &forwarder_endpoint_id, &dbf_path).await;
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
                            !deliver_dbf(&db, &stream_id, &forwarder_endpoint_id, &dbf_path).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            () = tokio::time::sleep(retry_interval), if needs_retry => {
                needs_retry =
                    !deliver_dbf(&db, &stream_id, &forwarder_endpoint_id, &dbf_path).await;
            }
        }
    }
}

/// Run one DBF delivery pass. Returns `true` when no retry is needed (delivery
/// succeeded, there is nothing to deliver, or the failure is permanent and
/// cannot be fixed by retrying), and `false` when the caller should schedule a
/// retry (DBF write failed or a transient DB error occurred).
async fn deliver_dbf(
    db: &Arc<Mutex<Db>>,
    stream_id: &str,
    forwarder_endpoint_id: &str,
    dbf_path: &str,
) -> bool {
    // DBF delivery does synchronous disk + flock I/O, so run it on a blocking
    // thread (acquiring the durable store via `blocking_lock`, like the
    // announcer path) instead of blocking an async worker thread while holding
    // the async mutex.
    let db = Arc::clone(db);
    let stream_id = stream_id.to_owned();
    let forwarder_endpoint_id = forwarder_endpoint_id.to_owned();
    let dbf_path = dbf_path.to_owned();
    let result = tokio::task::spawn_blocking(move || -> bool {
        let guard = db.blocking_lock();
        let details =
            match guard.load_subscription_dbf_details(&forwarder_endpoint_id, &stream_id) {
                Ok(Some(details)) => details,
                Ok(None) => return true,
                Err(e) => {
                    warn!(error = %e, %stream_id, "failed to load DBF subscription details");
                    return false;
                }
            };
        let (idx, event_type) = details;
        let reader_index = match u8::try_from(idx) {
            Ok(idx) => idx,
            Err(_) => {
                warn!(%stream_id, idx, "subscription index exceeds DBF reader range; skipping DBF delivery");
                return true;
            }
        };
        match crate::dbf_writer::deliver_durable_events_to_dbf(
            &guard,
            &stream_id,
            std::path::Path::new(&dbf_path),
            event_type,
            reader_index,
            now_unix_ms(),
        ) {
            Ok(_) => true,
            Err(e) => {
                warn!(error = %e, %stream_id, "DBF delivery failed; will retry");
                false
            }
        }
    })
    .await;
    match result {
        Ok(no_retry) => no_retry,
        Err(e) => {
            warn!(error = %e, "DBF delivery task failed");
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_announcer_worker(
    db: Arc<Mutex<Db>>,
    chip_lookup: Arc<tokio::sync::RwLock<crate::control_api::ChipLookup>>,
    stream_id: String,
    client: Arc<dyn AnnouncerPushClient + Send + Sync>,
    generation: i64,
    retry_interval: Duration,
    mut hint_rx: broadcast::Receiver<i64>,
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
        let guard = db.blocking_lock();
        announcer_push::push_announcer_rows(
            &guard,
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
            if let Some((bib, name)) = chips.get(chip_id) {
                return Some(ResolvedParticipant {
                    bib: bib.clone(),
                    name: name.clone(),
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
// discovery via the thin node is a separate concern. P2P stays disabled unless
// at least one of these keys is present.
// ---------------------------------------------------------------------------

/// Env var naming the forwarder's iroh endpoint (string node id) to dial.
pub const ENV_P2P_FORWARDER_NODE_ID: &str = "RT_P2P_FORWARDER_NODE_ID";
/// Env var giving a direct `ip:port` socket address for the forwarder peer.
pub const ENV_P2P_FORWARDER_DIRECT_ADDR: &str = "RT_P2P_FORWARDER_DIRECT_ADDR";
/// Env var holding the receiver's 64-hex-character secret-key seed.
pub const ENV_P2P_SECRET_KEY_SEED_HEX: &str = "RT_P2P_SECRET_KEY_SEED_HEX";
/// Env var for the optional thin-node base URL (set with the token).
pub const ENV_P2P_THIN_NODE_URL: &str = "RT_P2P_THIN_NODE_URL";
/// Env var for the optional thin-node bearer token (set with the URL).
pub const ENV_P2P_THIN_NODE_TOKEN: &str = "RT_P2P_THIN_NODE_TOKEN";
/// Env var overriding the subscription reconcile interval, in milliseconds.
pub const ENV_P2P_RECONCILE_MS: &str = "RT_P2P_RECONCILE_MS";

/// Build an optional [`P2pReceiverConfig`] from a key->value lookup (e.g. env).
///
/// Mirrors the `receiver-headless` CLI validation: P2P is enabled only when at
/// least one key is present; the secret-key seed is then required; the forwarder
/// node id and direct address must be supplied together (both or neither); the
/// thin-node URL and token must be supplied together; at least one of an
/// explicit forwarder or a thin node must be configured; and the reconcile
/// interval defaults to 1000ms and must be at least [`MIN_RECONCILE_INTERVAL`].
/// Empty/whitespace-only values are treated as absent.
pub fn p2p_config_from_lookup(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<P2pReceiverConfig>, String> {
    let trimmed = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };

    let forwarder_node_id = trimmed(ENV_P2P_FORWARDER_NODE_ID);
    let forwarder_direct_addr = trimmed(ENV_P2P_FORWARDER_DIRECT_ADDR);
    let secret_key_seed_hex = trimmed(ENV_P2P_SECRET_KEY_SEED_HEX);
    let thin_node_url = trimmed(ENV_P2P_THIN_NODE_URL);
    let thin_node_token = trimmed(ENV_P2P_THIN_NODE_TOKEN);
    let reconcile_ms_raw = trimmed(ENV_P2P_RECONCILE_MS);

    let any_present = forwarder_node_id.is_some()
        || forwarder_direct_addr.is_some()
        || secret_key_seed_hex.is_some()
        || thin_node_url.is_some()
        || thin_node_token.is_some()
        || reconcile_ms_raw.is_some();
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

    let thin_node = match (thin_node_url, thin_node_token) {
        (Some(url), Some(token)) => Some(ThinNodeClientConfig { url, token }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "{ENV_P2P_THIN_NODE_URL} and {ENV_P2P_THIN_NODE_TOKEN} must be set together"
            ));
        }
    };

    if forwarder.is_none() && thin_node.is_none() {
        return Err(format!(
            "P2P requires either an explicit forwarder ({ENV_P2P_FORWARDER_NODE_ID} + \
             {ENV_P2P_FORWARDER_DIRECT_ADDR}) or a thin node ({ENV_P2P_THIN_NODE_URL} + \
             {ENV_P2P_THIN_NODE_TOKEN})"
        ));
    }

    let secret_key_seed_hex = secret_key_seed_hex.ok_or_else(|| {
        format!("{ENV_P2P_SECRET_KEY_SEED_HEX} is required when any P2P env var is set")
    })?;
    let secret_key_seed = parse_secret_key_seed_hex(&secret_key_seed_hex)?;

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
        secret_key_seed,
        forwarder,
        thin_node,
        reconcile_interval,
    }))
}

/// Build an optional [`P2pReceiverConfig`] from process environment variables.
/// See [`p2p_config_from_lookup`] for the validation rules.
pub fn p2p_config_from_env() -> Result<Option<P2pReceiverConfig>, String> {
    p2p_config_from_lookup(|key| std::env::var(key).ok())
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

    #[test]
    fn p2p_config_from_lookup_none_when_no_keys() {
        assert!(p2p_config_from_lookup(lookup(&[])).unwrap().is_none());
    }

    #[test]
    fn p2p_config_from_lookup_builds_minimal_config() {
        let cfg = p2p_config_from_lookup(lookup(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
        ]))
        .unwrap()
        .expect("config present");
        let fwd = cfg.forwarder.as_ref().expect("forwarder present");
        assert_eq!(fwd.node_id, "endpoint-x");
        assert_eq!(fwd.direct_addr, "127.0.0.1:5000".parse().unwrap());
        assert_eq!(cfg.secret_key_seed, [0xab; 32]);
        assert!(cfg.thin_node.is_none());
        assert_eq!(cfg.reconcile_interval, Duration::from_millis(1000));
    }

    #[test]
    fn p2p_config_from_lookup_accepts_thin_node_only_without_forwarder() {
        let cfg = p2p_config_from_lookup(lookup(&[
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_THIN_NODE_URL, "http://127.0.0.1:8080"),
            (ENV_P2P_THIN_NODE_TOKEN, "tok"),
        ]))
        .unwrap()
        .expect("config present");
        assert!(
            cfg.forwarder.is_none(),
            "thin-node-only config must not require an explicit forwarder"
        );
        assert!(cfg.thin_node.is_some());
    }

    #[test]
    fn p2p_config_from_lookup_requires_forwarder_or_thin_node() {
        let err = p2p_config_from_lookup(lookup(&[(ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX)]))
            .unwrap_err();
        assert!(err.contains("either an explicit forwarder"), "got: {err}");
    }

    #[test]
    fn p2p_config_from_lookup_errors_on_partial_required_keys() {
        let err = p2p_config_from_lookup(lookup(&[(ENV_P2P_FORWARDER_NODE_ID, "endpoint-x")]))
            .unwrap_err();
        assert!(err.contains(ENV_P2P_FORWARDER_DIRECT_ADDR), "got: {err}");
    }

    #[test]
    fn p2p_config_from_lookup_thin_node_requires_both() {
        let err = p2p_config_from_lookup(lookup(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_THIN_NODE_URL, "http://127.0.0.1:8080"),
        ]))
        .unwrap_err();
        assert!(err.contains(ENV_P2P_THIN_NODE_TOKEN), "got: {err}");
    }

    #[test]
    fn p2p_config_from_lookup_accepts_thin_node_pair_and_reconcile_override() {
        let cfg = p2p_config_from_lookup(lookup(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_THIN_NODE_URL, "http://127.0.0.1:8080"),
            (ENV_P2P_THIN_NODE_TOKEN, "tok"),
            (ENV_P2P_RECONCILE_MS, "200"),
        ]))
        .unwrap()
        .expect("config present");
        let thin = cfg.thin_node.expect("thin node configured");
        assert_eq!(thin.url, "http://127.0.0.1:8080");
        assert_eq!(thin.token, "tok");
        assert_eq!(cfg.reconcile_interval, Duration::from_millis(200));
    }

    #[test]
    fn p2p_config_from_lookup_rejects_below_min_reconcile() {
        let err = p2p_config_from_lookup(lookup(&[
            (ENV_P2P_FORWARDER_NODE_ID, "endpoint-x"),
            (ENV_P2P_FORWARDER_DIRECT_ADDR, "127.0.0.1:5000"),
            (ENV_P2P_SECRET_KEY_SEED_HEX, TEST_SEED_HEX),
            (ENV_P2P_RECONCILE_MS, "10"),
        ]))
        .unwrap_err();
        assert!(err.contains(ENV_P2P_RECONCILE_MS), "got: {err}");
    }

    fn insert_chip_event(db: &Db, stream_id: &str, seq: i64, received_unix_ms: i64) {
        db.insert_received_event(&ReceivedEventInsert {
            stream_id,
            seq,
            epoch: 1,
            raw_frame: SAMPLE_FRAME,
            read_kind: "chip",
            reader_timestamp: None,
            received_unix_ms,
            dbf_delivered_unix_ms: None,
        })
        .unwrap();
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
        fn push(&self, rows: &[AnnouncerRow]) -> Result<(), AnnouncerPushError> {
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
    fn thin_node_client_config_debug_redacts_token() {
        let cfg = ThinNodeClientConfig {
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
            secret_key_seed: [0u8; 32],
            forwarder: Some(ForwarderPeerConfig {
                node_id: "node".to_owned(),
                direct_addr: "127.0.0.1:1".parse().unwrap(),
            }),
            thin_node: Some(cfg),
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

    /// A DBF write that fails (its parent directory does not yet exist) must be
    /// retried by the worker's retry timer once the directory appears, even
    /// though no new durable hint arrives after the failure.
    #[tokio::test]
    async fn dbf_worker_retries_after_failure_without_new_hint() {
        let stream_id = "127.0.0.1:11000";
        let fwd = "fwd-dbf-retry";

        let mut db = Db::open_in_memory().unwrap();
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
        let db = Arc::new(Mutex::new(db));

        let tmp = tempfile::tempdir().unwrap();
        let missing_dir = tmp.path().join("not-yet");
        let dbf_path = missing_dir.join("out.dbf").to_string_lossy().into_owned();

        let (hint_tx, hint_rx) = broadcast::channel::<i64>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_dbf_worker(
            Arc::clone(&db),
            stream_id.to_owned(),
            fwd.to_owned(),
            dbf_path,
            Duration::from_millis(50),
            hint_rx,
            shutdown_rx,
        ));

        // Startup attempt and an explicit hint both fail while the dir is gone.
        let _ = hint_tx.send(1);
        tokio::time::sleep(Duration::from_millis(200)).await;
        {
            let guard = db.lock().await;
            assert_eq!(
                guard
                    .load_undelivered_received_events(stream_id)
                    .unwrap()
                    .len(),
                2,
                "events must remain undelivered while the DBF dir is missing"
            );
        }

        // Recover by creating the directory. Send NO new hint.
        std::fs::create_dir_all(&missing_dir).unwrap();

        let delivered = poll_async(Duration::from_secs(5), || {
            let db = Arc::clone(&db);
            async move {
                db.lock()
                    .await
                    .load_undelivered_received_events(stream_id)
                    .unwrap()
                    .is_empty()
            }
        })
        .await;
        assert!(
            delivered,
            "retry timer must deliver pending DBF rows after recovery without a new hint"
        );

        let _ = shutdown_tx.send(true);
        let _ = handle.await;
        drop(hint_tx);
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

        let (hint_tx, hint_rx) = broadcast::channel::<i64>(16);
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
        let _ = hint_tx.send(1);
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
}

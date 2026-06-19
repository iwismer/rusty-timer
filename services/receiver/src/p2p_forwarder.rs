//! Per-forwarder P2P connection manager.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use rt_iroh::{Endpoint, NodeAddr, RecvStream, SendStream};
use rt_p2p_protocol::{
    ControlC2F, ControlF2C, Hello, Pong, SubscribeMode, control_c2f, control_f2c,
};
use tokio::sync::{Mutex, Notify, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::db::Db;
use crate::p2p_session::{
    BackoffConfig, P2pSessionError, SessionStatusReporter, connect_and_hello, read_frame,
    run_data_subscription_with_hint, write_frame,
};

#[derive(Clone, Debug)]
pub struct ForwarderDataStream {
    pub stream_id: String,
    pub mode: SubscribeMode,
    pub durable_hint_tx: Option<broadcast::Sender<i64>>,
}

/// Owns one live control session to a forwarder and opens one data bi-stream per
/// desired stream on that same QUIC connection.
#[derive(Debug)]
pub struct ForwarderConnection {
    desired_tx: watch::Sender<HashMap<String, ForwarderDataStream>>,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl ForwarderConnection {
    #[must_use]
    pub fn start(
        endpoint_id: String,
        endpoint: Arc<Endpoint>,
        forwarder_addr: NodeAddr,
        db: Arc<Mutex<Db>>,
        client_hello: Hello,
        reporter: Arc<SessionStatusReporter>,
        backoff: BackoffConfig,
    ) -> Self {
        let (desired_tx, desired_rx) = watch::channel(HashMap::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(run_forwarder_connection(
            endpoint_id,
            endpoint,
            forwarder_addr,
            db,
            client_hello,
            reporter,
            backoff,
            desired_rx,
            shutdown_rx,
        ));
        Self {
            desired_tx,
            shutdown_tx,
            task,
        }
    }

    pub fn set_desired_streams(&self, streams: Vec<ForwarderDataStream>) {
        let desired = streams
            .into_iter()
            .map(|stream| (stream.stream_id.clone(), stream))
            .collect();
        let _ = self.desired_tx.send(desired);
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub async fn stop(self) {
        let _ = self.shutdown_tx.send(true);
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_forwarder_connection(
    endpoint_id: String,
    endpoint: Arc<Endpoint>,
    forwarder_addr: NodeAddr,
    db: Arc<Mutex<Db>>,
    client_hello: Hello,
    reporter: Arc<SessionStatusReporter>,
    backoff: BackoffConfig,
    mut desired_rx: watch::Receiver<HashMap<String, ForwarderDataStream>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut next_delay = backoff.initial;
    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        reporter
            .app_state()
            .mark_forwarder_dial_started(&endpoint_id)
            .await;
        let session = tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return;
                }
                continue;
            }
            result = connect_and_hello(&endpoint, forwarder_addr.clone(), client_hello.clone()) => result,
        };

        match session {
            Ok(session) => {
                next_delay = backoff.initial;
                run_connected_forwarder(
                    &endpoint_id,
                    session,
                    &db,
                    &reporter,
                    &mut desired_rx,
                    &mut shutdown_rx,
                )
                .await;
            }
            Err(error) => {
                warn!(%endpoint_id, %error, "forwarder control connection failed; retrying");
            }
        }

        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return;
                }
            }
            () = tokio::time::sleep(next_delay) => {}
        }
        next_delay = next_backoff(next_delay, backoff.max);
    }
}

/// Compute the next reconnect backoff delay: double `current`, capped at `max`.
///
/// Pure helper so the doubling/capping contract can be unit-tested without
/// spinning up a connection. The saturating multiply guards against overflow
/// once the delay grows large.
fn next_backoff(current: Duration, max: Duration) -> Duration {
    current.saturating_mul(2).min(max)
}

async fn run_connected_forwarder(
    endpoint_id: &str,
    session: crate::p2p_session::ControlSession,
    db: &Arc<Mutex<Db>>,
    reporter: &Arc<SessionStatusReporter>,
    desired_rx: &mut watch::Receiver<HashMap<String, ForwarderDataStream>>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    let _control_guard = reporter.on_control_connected(endpoint_id).await;
    let mut data_tasks = HashMap::new();

    let crate::p2p_session::ControlSession {
        connection,
        mut control_send,
        control_recv,
        ..
    } = session;

    let desired = desired_rx.borrow().clone();
    sync_data_tasks(
        endpoint_id,
        &connection,
        db,
        reporter,
        &desired,
        &mut data_tasks,
    )
    .await;

    // BUG 1 fix: cancel-safe control reads. A dedicated task exclusively owns
    // the control `RecvStream` and loops `read_frame` (which uses `read_exact`
    // and is therefore cancel-UNSAFE), forwarding each parsed frame over an
    // mpsc. The main `select!` only ever consumes already-parsed frames from
    // that channel, so a `desired_rx.changed()` wake (every reconcile pass) can
    // never cancel a partial wire read and desync the length-prefixed frame
    // stream — which would otherwise break Ping/Pong and get us dropped.
    let (frame_tx, mut frame_rx) = mpsc::channel::<ControlF2C>(64);
    let reader_task = tokio::spawn(control_reader_loop(
        endpoint_id.to_owned(),
        control_recv,
        frame_tx,
    ));

    // BUG 2 fix: keep the aggregate recompute (which awaits DB + discovered
    // locks) off the frame-handling path. Status frames only do a quick
    // in-memory store and signal this coalescing task, so a queued heartbeat
    // `Ping` is never delayed behind a lock-bound recompute. Each notify
    // guarantees at least one subsequent recompute that observes the store.
    let recompute_notify = Arc::new(Notify::new());
    let recompute_task = tokio::spawn({
        let reporter = Arc::clone(reporter);
        let recompute_notify = Arc::clone(&recompute_notify);
        async move {
            loop {
                recompute_notify.notified().await;
                reporter
                    .app_state()
                    .recompute_aggregate_connection_state()
                    .await;
            }
        }
    });

    loop {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
            changed = desired_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let desired = desired_rx.borrow().clone();
                sync_data_tasks(
                    endpoint_id,
                    &connection,
                    db,
                    reporter,
                    &desired,
                    &mut data_tasks,
                ).await;
            }
            frame = frame_rx.recv() => {
                match frame {
                    Some(frame) => {
                        if let Err(error) = handle_control_frame(
                            endpoint_id,
                            frame,
                            &mut control_send,
                            reporter,
                            &recompute_notify,
                        ).await {
                            warn!(%endpoint_id, %error, "failed to handle forwarder control frame");
                            break;
                        }
                    }
                    // The reader task ended: clean disconnect/EOF or a decode/
                    // protocol error (already logged by the reader task).
                    None => break,
                }
            }
        }
    }

    // Tear down the owned tasks first so no further frames are parsed and no
    // offloaded recompute can re-add status after we clear it below. Clearing
    // live status on disconnect must still happen, so it runs after the abort
    // (and does its own synchronous recompute), with no task left racing it.
    reader_task.abort();
    recompute_task.abort();
    reporter
        .app_state()
        .clear_forwarder_live_status(endpoint_id)
        .await;
    stop_data_tasks(data_tasks).await;
}

/// Cancel-safe owner of the control `RecvStream`: loops `read_frame` and
/// forwards each parsed frame to the main loop over `frame_tx`. Returns (which
/// drops the sender, signalling the main loop) on disconnect/EOF, on a decode/
/// protocol error, or once the main loop has gone away.
async fn control_reader_loop(
    endpoint_id: String,
    mut recv: RecvStream,
    frame_tx: mpsc::Sender<ControlF2C>,
) {
    loop {
        match read_frame::<ControlF2C>(&mut recv).await {
            Ok(frame) => {
                if frame_tx.send(frame).await.is_err() {
                    return;
                }
            }
            // A read error is the expected clean disconnect/EOF signal.
            Err(P2pSessionError::Read(_)) => return,
            Err(error) => {
                warn!(%endpoint_id, %error, "forwarder control stream ended with error");
                return;
            }
        }
    }
}

async fn handle_control_frame(
    endpoint_id: &str,
    frame: ControlF2C,
    send: &mut SendStream,
    reporter: &Arc<SessionStatusReporter>,
    recompute_notify: &Notify,
) -> Result<(), P2pSessionError> {
    match frame.msg {
        Some(control_f2c::Msg::ReaderStatus(status)) => {
            reporter
                .app_state()
                .store_forwarder_reader_status_sync(endpoint_id, status);
            recompute_notify.notify_one();
        }
        Some(control_f2c::Msg::ReaderInfo(info)) => {
            reporter
                .app_state()
                .store_forwarder_reader_info_sync(endpoint_id, info);
            recompute_notify.notify_one();
        }
        Some(control_f2c::Msg::UpsStatus(status)) => {
            reporter
                .app_state()
                .store_forwarder_ups_status_sync(endpoint_id, status);
            recompute_notify.notify_one();
        }
        Some(control_f2c::Msg::Ping(ping)) => {
            write_frame(
                send,
                &ControlC2F {
                    msg: Some(control_c2f::Msg::Pong(Pong { nonce: ping.nonce })),
                },
            )
            .await?;
        }
        Some(control_f2c::Msg::ProtocolError(error)) => {
            warn!(%endpoint_id, code = error.code, message = %error.message, "forwarder sent protocol error");
        }
        Some(
            control_f2c::Msg::Pong(_)
            | control_f2c::Msg::DownloadProgress(_)
            | control_f2c::Msg::SyncClock(_)
            | control_f2c::Msg::ReaderControlResponse(_)
            | control_f2c::Msg::HelloOk(_)
            | control_f2c::Msg::StreamCatalog(_),
        )
        | None => {}
    }
    Ok(())
}

async fn sync_data_tasks(
    endpoint_id: &str,
    connection: &rt_iroh::Connection,
    db: &Arc<Mutex<Db>>,
    reporter: &Arc<SessionStatusReporter>,
    desired: &HashMap<String, ForwarderDataStream>,
    tasks: &mut HashMap<String, JoinHandle<()>>,
) {
    let finished = tasks
        .iter()
        .filter(|(_, task)| task.is_finished())
        .map(|(stream_id, _)| stream_id.clone())
        .collect::<Vec<_>>();
    for stream_id in finished {
        if let Some(task) = tasks.remove(&stream_id) {
            let _ = task.await;
        }
    }

    let desired_ids = desired.keys().cloned().collect::<HashSet<_>>();
    let stale = tasks
        .keys()
        .filter(|stream_id| !desired_ids.contains(*stream_id))
        .cloned()
        .collect::<Vec<_>>();
    for stream_id in stale {
        if let Some(task) = tasks.remove(&stream_id) {
            stop_data_task(task).await;
        }
    }

    for (stream_id, stream) in desired {
        if tasks.contains_key(stream_id) {
            continue;
        }
        let endpoint_id = endpoint_id.to_owned();
        let connection = connection.clone();
        let db = Arc::clone(db);
        let reporter = Arc::clone(reporter);
        let stream = stream.clone();
        tasks.insert(
            stream_id.clone(),
            tokio::spawn(async move {
                let _data_guard = reporter.on_data_session(&endpoint_id).await;
                let result = run_data_subscription_with_hint(
                    &connection,
                    &db,
                    &stream.stream_id,
                    stream.mode,
                    stream.durable_hint_tx.as_ref(),
                )
                .await;
                if let Err(error) = result
                    && !matches!(error, P2pSessionError::Read(_))
                {
                    warn!(%endpoint_id, stream_id = %stream.stream_id, %error, "forwarder data subscription ended with error");
                }
            }),
        );
    }
}

async fn stop_data_tasks(tasks: HashMap<String, JoinHandle<()>>) {
    for task in tasks.into_values() {
        stop_data_task(task).await;
    }
}

async fn stop_data_task(task: JoinHandle<()>) {
    task.abort();
    let _ = task.await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use rt_iroh::{Endpoint, EndpointBuilder};
    use rt_p2p_protocol::{
        ControlF2C, DownloadProgress, EventBatch, Hello, MAX_FRAME_BYTES, ReadRecord, ReaderStatus,
        StreamCatalog, SubscribeMode, SubscribeOk, control_f2c,
    };
    use rt_test_utils::p2p::{ConnectivityFault, ForwarderScript, MockForwarderPeer};
    use rt_test_utils::poll_until;
    use tokio::sync::{Mutex, broadcast};

    use crate::control_api::{AppState, ForwarderConnState, get_connections};
    use crate::db::Db;
    use crate::p2p_session::{BackoffConfig, SessionStatusReporter};

    use super::{ForwarderConnection, ForwarderDataStream};

    const STREAM_ID: &str = "127.0.0.1:10000";

    fn test_hello() -> Hello {
        Hello {
            min_minor: 1,
            max_minor: 1,
            capabilities: vec!["data".to_owned()],
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
            catalog_generation: 0,
        }
    }

    fn base_script() -> ForwarderScript {
        ForwarderScript {
            server_hello: test_hello(),
            catalog: StreamCatalog {
                generation: 1,
                entries: Vec::new(),
            },
            subscribe_ok: SubscribeOk {
                stream_id: STREAM_ID.as_bytes().to_vec(),
                earliest_available_seq: 1,
                latest_seq_at_open: 0,
            },
            gap_notice: None,
            batches: Vec::new(),
            caught_up_through: None,
            data_fault: ConnectivityFault::delayed(Duration::from_secs(2)),
            echo_subscribed_stream_id: false,
            close_connection_after_data: false,
            control_events: Vec::new(),
            control_pings: 0,
            control_ping_interval: Duration::from_millis(50),
        }
    }

    async fn test_endpoint(seed: u8) -> Endpoint {
        EndpointBuilder::test([seed; 32]).bind().await.unwrap()
    }

    #[tokio::test]
    async fn forwarder_connection_tracks_control_and_data_states() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let forwarder = MockForwarderPeer::start([40; 32], base_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.node_addr().node_id.to_string();
            let endpoint = Arc::new(test_endpoint(41).await);
            let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
            let (state, _shutdown_rx) =
                AppState::new(Db::open_in_memory().unwrap(), "recv-test".to_owned());
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.node_addr(),
                Arc::clone(&db),
                test_hello(),
                Arc::clone(&reporter),
                BackoffConfig {
                    initial: Duration::from_millis(50),
                    max: Duration::from_millis(50),
                },
            );

            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move {
                        state.forwarder_state(&endpoint_id).await.state
                            == ForwarderConnState::Connected
                    }
                },
                Duration::from_secs(5),
            )
            .await;

            let (hint_tx, _hint_rx) = broadcast::channel(16);
            connection.set_desired_streams(vec![ForwarderDataStream {
                stream_id: STREAM_ID.to_owned(),
                mode: SubscribeMode::Replay,
                durable_hint_tx: Some(hint_tx),
            }]);

            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move {
                        state.forwarder_state(&endpoint_id).await.state
                            == ForwarderConnState::Subscribed
                    }
                },
                Duration::from_secs(5),
            )
            .await;

            forwarder.shutdown().await;

            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move {
                        state.forwarder_state(&endpoint_id).await.state
                            == ForwarderConnState::Unavailable
                    }
                },
                Duration::from_secs(5),
            )
            .await;

            connection.stop().await;
            endpoint.close().await;
        })
        .await
        .expect("forwarder connection state test timed out");
    }

    #[test]
    fn backoff_doubles_and_caps_at_max() {
        let max = Duration::from_secs(30);
        // Doubling from the initial delay.
        assert_eq!(
            super::next_backoff(Duration::from_secs(1), max),
            Duration::from_secs(2)
        );
        assert_eq!(
            super::next_backoff(Duration::from_secs(2), max),
            Duration::from_secs(4)
        );
        assert_eq!(
            super::next_backoff(Duration::from_secs(8), max),
            Duration::from_secs(16)
        );
        // Doubling past the cap clamps to max.
        assert_eq!(super::next_backoff(Duration::from_secs(16), max), max);
        assert_eq!(super::next_backoff(max, max), max);
        // A delay already above max stays at max (never grows unbounded).
        assert_eq!(super::next_backoff(Duration::from_secs(60), max), max);
        // Saturating multiply guards against overflow at huge delays.
        assert_eq!(super::next_backoff(Duration::MAX, max), max);
    }

    fn record(seq: u64) -> ReadRecord {
        ReadRecord {
            stream_id: STREAM_ID.as_bytes().to_vec(),
            seq,
            epoch: 1,
            raw_frame: format!("frame-{seq}").into_bytes(),
            read_kind: "chip".to_owned(),
            reader_timestamp: 0,
            received_unix_ms: 0,
        }
    }

    /// A script that delivers `[1, 2]` then drops the whole connection after the
    /// first data stream is acked, forcing the per-forwarder connection to
    /// reconnect. Each reconnect is served from scratch (re-sending `[1, 2]`),
    /// so the receiver must dedup on resume.
    fn reconnect_script() -> ForwarderScript {
        let mut script = base_script();
        script.subscribe_ok = SubscribeOk {
            stream_id: STREAM_ID.as_bytes().to_vec(),
            earliest_available_seq: 1,
            latest_seq_at_open: 2,
        };
        script.batches = vec![EventBatch {
            records: vec![record(1), record(2)],
            replay: false,
        }];
        script.caught_up_through = Some(2);
        script.data_fault = ConnectivityFault::healthy();
        script.close_connection_after_data = true;
        script
    }

    #[tokio::test]
    async fn control_reconnect_resumes_from_cursor_without_duplicates() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let forwarder = MockForwarderPeer::start([42; 32], reconnect_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.node_addr().node_id.to_string();
            let endpoint = Arc::new(test_endpoint(43).await);
            let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
            let (state, _shutdown_rx) =
                AppState::new(Db::open_in_memory().unwrap(), "recv-test".to_owned());
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.node_addr(),
                Arc::clone(&db),
                test_hello(),
                Arc::clone(&reporter),
                BackoffConfig {
                    initial: Duration::from_millis(50),
                    max: Duration::from_millis(50),
                },
            );

            let (hint_tx, _hint_rx) = broadcast::channel(16);
            connection.set_desired_streams(vec![ForwarderDataStream {
                stream_id: STREAM_ID.to_owned(),
                mode: SubscribeMode::Replay,
                durable_hint_tx: Some(hint_tx),
            }]);

            // The forwarder drops the control connection after the first data
            // stream is acked; the per-forwarder connection must reconnect and
            // resubscribe, proving the control reconnect loop is live.
            poll_until(
                || async { forwarder.subscribes().len() >= 2 },
                Duration::from_secs(10),
            )
            .await;

            // The resubscribe resumes from the persisted cursor, not from 0.
            let subscribes = forwarder.subscribes();
            assert_eq!(
                subscribes[0].after_seq, 0,
                "first subscribe starts at the empty cursor"
            );
            assert_eq!(
                subscribes[1].after_seq, 2,
                "reconnect must resume from the persisted contiguous cursor"
            );

            // Resuming re-delivers [1, 2]; dedup keeps exactly one durable row
            // per seq, with no duplicate seqs in the final durable set.
            let guard = db.lock().await;
            let seqs: Vec<i64> = guard
                .load_received_events(STREAM_ID)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect();
            assert_eq!(
                seqs,
                vec![1, 2],
                "resume must not produce duplicate durable rows"
            );
            assert_eq!(guard.load_stream_cursor(STREAM_ID).unwrap(), 2);
            drop(guard);

            connection.stop().await;
            forwarder.shutdown().await;
            endpoint.close().await;
        })
        .await
        .expect("control reconnect resume test timed out");
    }

    /// A script that, after the handshake, pushes an ignored control variant
    /// (`DownloadProgress`) followed by a live `ReaderStatus`, then issues a
    /// short burst of heartbeat `Ping`s. Exercises the receiver's ongoing
    /// control read loop over a real stream.
    fn live_status_heartbeat_script() -> ForwarderScript {
        let mut script = base_script();
        script.control_events = vec![
            // An ignored F2C variant must NOT desync the frame stream or
            // disconnect the receiver.
            ControlF2C {
                msg: Some(control_f2c::Msg::DownloadProgress(DownloadProgress {
                    stream_id: STREAM_ID.as_bytes().to_vec(),
                    downloaded_bytes: 0,
                    total_bytes: 0,
                })),
            },
            // Live reader status the receiver must reflect in get_connections.
            ControlF2C {
                msg: Some(control_f2c::Msg::ReaderStatus(ReaderStatus {
                    stream_id: STREAM_ID.as_bytes().to_vec(),
                    connected: false,
                    state: "disconnected".to_owned(),
                    last_read_unix_ms: 0,
                })),
            },
        ];
        script.control_pings = 3;
        script.control_ping_interval = Duration::from_millis(50);
        script
    }

    /// Over a REAL control stream the receiver must: consume a `ReaderStatus`
    /// (reflected in `get_connections`), answer the forwarder's heartbeat
    /// `Ping`s with `Pong`s (proving the cancel-safe reader keeps the frame
    /// stream in sync even while desired-stream updates churn the reconcile
    /// path), tolerate an ignored control variant without disconnecting, and
    /// clear the forwarder's live status on control disconnect.
    #[tokio::test]
    async fn control_live_status_and_heartbeat_over_real_stream() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let forwarder = MockForwarderPeer::start([44; 32], live_status_heartbeat_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.node_addr().node_id.to_string();
            let endpoint = Arc::new(test_endpoint(45).await);
            let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
            let (state, _shutdown_rx) =
                AppState::new(Db::open_in_memory().unwrap(), "recv-test".to_owned());
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.node_addr(),
                Arc::clone(&db),
                test_hello(),
                Arc::clone(&reporter),
                BackoffConfig {
                    initial: Duration::from_millis(50),
                    max: Duration::from_millis(50),
                },
            );

            // The ReaderStatus must surface in get_connections. Churn desired
            // updates meanwhile (each one wakes the reconcile select arm, the
            // cadence that previously cancelled an in-flight control read).
            let mut reflected = false;
            for _ in 0..200 {
                connection.set_desired_streams(Vec::new());
                let conns = get_connections(&state).await;
                reflected = conns.forwarders.iter().any(|forwarder| {
                    forwarder.endpoint_id == endpoint_id
                        && forwarder.readers.iter().any(|reader| {
                            reader.stream_id == STREAM_ID
                                && !reader.connected
                                && reader.state == "disconnected"
                        })
                });
                if reflected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(
                reflected,
                "receiver must reflect the forwarder ReaderStatus in get_connections"
            );

            // Heartbeat maintained: every Ping was answered with a Pong despite
            // the ignored variant and the desired-stream churn. If the frame
            // stream had desynced, pongs would stall here.
            poll_until(
                || async { forwarder.pongs().len() >= 3 },
                Duration::from_secs(10),
            )
            .await;

            // The ignored control variant did not tear the session down.
            assert_eq!(
                state.forwarder_state(&endpoint_id).await.state,
                ForwarderConnState::Connected,
                "an ignored control variant must not disconnect the control session"
            );

            // Control disconnect must clear the forwarder's live status.
            forwarder.shutdown().await;
            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move {
                        let conns = get_connections(&state).await;
                        !conns.forwarders.iter().any(|forwarder| {
                            forwarder.endpoint_id == endpoint_id && !forwarder.readers.is_empty()
                        })
                    }
                },
                Duration::from_secs(10),
            )
            .await;

            connection.stop().await;
            endpoint.close().await;
        })
        .await
        .expect("control live status + heartbeat test timed out");
    }
}

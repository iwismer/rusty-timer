//! Per-forwarder P2P connection manager.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use rt_iroh::{Endpoint, NodeAddr};
use rt_p2p_protocol::{Hello, SubscribeMode};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::db::Db;
use crate::p2p_session::{
    BackoffConfig, P2pSessionError, SessionStatusReporter, connect_and_hello,
    run_data_subscription_with_hint, wait_control_stream_closed,
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
        next_delay = next_delay.saturating_mul(2).min(backoff.max);
    }
}

async fn run_connected_forwarder(
    endpoint_id: &str,
    mut session: crate::p2p_session::ControlSession,
    db: &Arc<Mutex<Db>>,
    reporter: &Arc<SessionStatusReporter>,
    desired_rx: &mut watch::Receiver<HashMap<String, ForwarderDataStream>>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    let _control_guard = reporter.on_control_connected(endpoint_id).await;
    let mut data_tasks = HashMap::new();
    let desired = desired_rx.borrow().clone();
    sync_data_tasks(
        endpoint_id,
        &session.connection,
        db,
        reporter,
        &desired,
        &mut data_tasks,
    )
    .await;

    loop {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    stop_data_tasks(data_tasks).await;
                    return;
                }
            }
            changed = desired_rx.changed() => {
                if changed.is_err() {
                    stop_data_tasks(data_tasks).await;
                    return;
                }
                let desired = desired_rx.borrow().clone();
                sync_data_tasks(
                    endpoint_id,
                    &session.connection,
                    db,
                    reporter,
                    &desired,
                    &mut data_tasks,
                ).await;
            }
            result = wait_control_stream_closed(&mut session.control_recv) => {
                if let Err(error) = result {
                    warn!(%endpoint_id, %error, "forwarder control stream ended with error");
                }
                stop_data_tasks(data_tasks).await;
                return;
            }
        }
    }
}

async fn sync_data_tasks(
    endpoint_id: &str,
    connection: &rt_iroh::Connection,
    db: &Arc<Mutex<Db>>,
    reporter: &Arc<SessionStatusReporter>,
    desired: &HashMap<String, ForwarderDataStream>,
    tasks: &mut HashMap<String, JoinHandle<()>>,
) {
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
    use rt_p2p_protocol::{Hello, MAX_FRAME_BYTES, StreamCatalog, SubscribeMode, SubscribeOk};
    use rt_test_utils::p2p::{ConnectivityFault, ForwarderScript, MockForwarderPeer};
    use rt_test_utils::poll_until;
    use tokio::sync::{Mutex, broadcast};

    use crate::control_api::{AppState, ForwarderConnState};
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
}

//! Per-forwarder P2P connection manager.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use rt_iroh::{Endpoint, EndpointAddr, RecvStream, SendStream};
use rt_p2p_protocol::{
    CAP_READER_CONTROL, CAP_REMOTE_CONFIG, ConfigGetRequest, ConfigGetResponse, ConfigSetRequest,
    ConfigSetResponse, ControlC2F, ControlF2C, Hello, Pong, ReaderControlRequest,
    ReaderControlResponse, RestartRequest, RestartResponse, SubscribeMode, control_c2f,
    control_f2c, has_capability,
};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::warn;

use crate::control_api::{ConfigCommand, FORWARDER_CONFIG_TIMEOUT, ReaderCommand};
use crate::p2p_session::{
    BackoffConfig, DurableBatch, P2pSessionError, SessionStatusReporter, connect_and_hello,
    read_frame, run_data_subscription_with_hint, write_frame,
};
use crate::stream_key::LocalStreamKey;
use crate::writer::WriterHandle;

#[derive(Clone, Debug)]
pub struct ForwarderDataStream {
    pub stream_id: String,
    pub local_stream_key: LocalStreamKey,
    pub mode: SubscribeMode,
    pub durable_hint_tx: Option<broadcast::Sender<DurableBatch>>,
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
        forwarder_addr: EndpointAddr,
        writer: WriterHandle,
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
            writer,
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
            .map(|stream| (stream.local_stream_key.as_str().to_owned(), stream))
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
    forwarder_addr: EndpointAddr,
    writer: WriterHandle,
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
                    &writer,
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
    writer: &WriterHandle,
    reporter: &Arc<SessionStatusReporter>,
    desired_rx: &mut watch::Receiver<HashMap<String, ForwarderDataStream>>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    let _control_guard = reporter.on_control_connected(endpoint_id).await;
    let mut data_tasks = HashMap::new();

    let crate::p2p_session::ControlSession {
        connection,
        hello_ok,
        mut control_send,
        control_recv,
        ..
    } = session;

    // Remote-config bridge: when the negotiated session advertises
    // `CAP_REMOTE_CONFIG`, register an mpsc sender so `&AppState` control-API
    // commands can reach this live session. The request write happens in the
    // main loop below (serialized with the heartbeat `Pong`), and responses are
    // routed back by `request_id` to the matching pending `oneshot` — a map
    // lookup plus a non-blocking send, so neither path can stall the heartbeat.
    let remote_config = has_capability(&hello_ok.capabilities, CAP_REMOTE_CONFIG);
    let (config_tx, mut config_rx) = mpsc::channel::<ConfigCommand>(32);
    let _config_registration_guard = if remote_config {
        let guard = reporter
            .app_state()
            .register_forwarder_config_tx(endpoint_id, config_tx.clone());
        reporter
            .app_state()
            .recompute_aggregate_connection_state()
            .await;
        Some(guard)
    } else {
        None
    };
    // Hold the original sender for the session's lifetime so `config_rx.recv()`
    // never returns `None` while connected (only the registered clone is handed
    // to commands; an incapable session simply never receives any).
    let _config_tx_keepalive = config_tx;

    let reader_control = has_capability(&hello_ok.capabilities, CAP_READER_CONTROL);
    let (reader_tx, mut reader_rx) = mpsc::channel::<ReaderCommand>(32);
    let _reader_registration_guard = if reader_control {
        let guard = reporter
            .app_state()
            .register_forwarder_reader_control_tx(endpoint_id, reader_tx.clone());
        reporter
            .app_state()
            .recompute_aggregate_connection_state()
            .await;
        Some(guard)
    } else {
        None
    };
    let _reader_tx_keepalive = reader_tx;

    let mut pending_config: HashMap<String, PendingConfigRequest> = HashMap::new();
    let mut pending_reader: HashMap<String, PendingReaderRequest> = HashMap::new();
    let mut next_config_request_id: u64 = 0;
    let mut next_reader_request_id: u64 = 0;
    let prune_interval = FORWARDER_CONFIG_TIMEOUT.min(Duration::from_secs(1));
    let mut pending_config_prune_tick =
        tokio::time::interval_at(Instant::now() + prune_interval, prune_interval);
    pending_config_prune_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let desired = desired_rx.borrow().clone();
    sync_data_tasks(
        endpoint_id,
        &connection,
        writer,
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
                    writer,
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
                            &mut pending_config,
                            &mut pending_reader,
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
            command = reader_rx.recv() => {
                if let Some(command) = command
                    && let Err(error) = handle_reader_command(
                        command,
                        &mut control_send,
                        &mut pending_reader,
                        &mut next_reader_request_id,
                    ).await
                {
                    warn!(%endpoint_id, %error, "failed to send reader control request");
                    break;
                }
            }
            command = config_rx.recv() => {
                // A config command is consumed here in the main loop (NOT the
                // reader task): we generate a request_id, register the pending
                // responder, and write the request frame on `control_send` —
                // the same serialized send path as `Pong`, so a config exchange
                // never delays a heartbeat answer.
                if let Some(command) = command
                    && let Err(error) = handle_config_command(
                        command,
                        &mut control_send,
                        &mut pending_config,
                        &mut next_config_request_id,
                    ).await
                {
                    warn!(%endpoint_id, %error, "failed to send forwarder config request");
                    break;
                }
            }
            _ = pending_config_prune_tick.tick() => {
                let now = Instant::now();
                prune_expired_pending_config(&mut pending_config, now);
                prune_expired_pending_reader(&mut pending_reader, now);
            }
        }
    }

    // Tear down the owned tasks first so no further frames are parsed and no
    // offloaded recompute can re-add status after we clear it below. Clearing
    // live status on disconnect must still happen, so it runs after the abort
    // (and does its own synchronous recompute), with no task left racing it.
    reader_task.abort();
    recompute_task.abort();
    // Dropping `pending_config` drops every pending `oneshot` sender, so any
    // in-flight command awaiting a response is woken with an error rather than
    // hanging until its timeout. The remote-config channel is deregistered by
    // `_config_registration_guard` on normal exit, panic, or task abort.
    drop(pending_config);
    drop(pending_reader);
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

/// The awaiting side of an in-flight remote-config request, keyed in the
/// per-connection `pending_config` map by `request_id`. When the reader task
/// delivers the matching response the main loop routes it here with a single
/// non-blocking `oneshot` send.
enum PendingConfigResponder {
    Get(oneshot::Sender<ConfigGetResponse>),
    Set(oneshot::Sender<ConfigSetResponse>),
    Restart(oneshot::Sender<RestartResponse>),
}

struct PendingConfigRequest {
    deadline: Instant,
    responder: PendingConfigResponder,
}

struct PendingReaderRequest {
    deadline: Instant,
    responder: oneshot::Sender<ReaderControlResponse>,
}

fn prune_expired_pending_config(pending: &mut HashMap<String, PendingConfigRequest>, now: Instant) {
    pending.retain(|_, request| request.deadline > now);
}

fn prune_expired_pending_reader(pending: &mut HashMap<String, PendingReaderRequest>, now: Instant) {
    pending.retain(|_, request| request.deadline > now);
}

/// Translate a [`ConfigCommand`] into its wire request, register the pending
/// responder under a fresh per-connection `request_id`, and write the request
/// frame. Runs in the main control loop (same serialized send path as `Pong`),
/// so issuing a config request can never delay a heartbeat answer.
async fn handle_config_command(
    command: ConfigCommand,
    send: &mut SendStream,
    pending: &mut HashMap<String, PendingConfigRequest>,
    next_request_id: &mut u64,
) -> Result<(), P2pSessionError> {
    prune_expired_pending_config(pending, Instant::now());
    *next_request_id += 1;
    let request_id = next_request_id.to_string();
    let (frame, responder) = match command {
        ConfigCommand::Get { resp } => (
            ControlC2F {
                msg: Some(control_c2f::Msg::ConfigGetRequest(ConfigGetRequest {
                    request_id: request_id.clone(),
                })),
            },
            PendingConfigResponder::Get(resp),
        ),
        ConfigCommand::Set { config_json, resp } => (
            ControlC2F {
                msg: Some(control_c2f::Msg::ConfigSetRequest(ConfigSetRequest {
                    request_id: request_id.clone(),
                    config_json,
                })),
            },
            PendingConfigResponder::Set(resp),
        ),
        ConfigCommand::Restart { resp } => (
            ControlC2F {
                msg: Some(control_c2f::Msg::RestartRequest(RestartRequest {
                    request_id: request_id.clone(),
                })),
            },
            PendingConfigResponder::Restart(resp),
        ),
    };
    pending.insert(
        request_id.clone(),
        PendingConfigRequest {
            deadline: Instant::now() + FORWARDER_CONFIG_TIMEOUT,
            responder,
        },
    );
    let result = write_frame(send, &frame).await;
    if result.is_err() {
        pending.remove(&request_id);
    }
    result
}

async fn handle_reader_command(
    command: ReaderCommand,
    send: &mut SendStream,
    pending: &mut HashMap<String, PendingReaderRequest>,
    next_request_id: &mut u64,
) -> Result<(), P2pSessionError> {
    prune_expired_pending_reader(pending, Instant::now());
    *next_request_id += 1;
    let request_id = next_request_id.to_string();
    let ReaderCommand::Request {
        stream_id,
        action,
        resp,
    } = command;
    let request = action_to_request(stream_id, action, request_id.clone());
    let frame = ControlC2F {
        msg: Some(control_c2f::Msg::ReaderControlRequest(request)),
    };
    pending.insert(
        request_id.clone(),
        PendingReaderRequest {
            deadline: Instant::now() + FORWARDER_CONFIG_TIMEOUT,
            responder: resp,
        },
    );
    let result = write_frame(send, &frame).await;
    if result.is_err() {
        pending.remove(&request_id);
    }
    result
}

fn action_to_request(
    stream_id: String,
    action: rt_domain::ReaderControlAction,
    request_id: String,
) -> ReaderControlRequest {
    let mut request = ReaderControlRequest {
        stream_id: stream_id.into_bytes(),
        command: String::new(),
        request_id,
        mode: None,
        timeout: None,
        enabled: None,
        epoch_name: None,
    };
    match action {
        rt_domain::ReaderControlAction::GetInfo => request.command = "get_info".to_owned(),
        rt_domain::ReaderControlAction::SyncClock => request.command = "sync_clock".to_owned(),
        rt_domain::ReaderControlAction::SetReadMode { mode, timeout } => {
            request.command = "set_read_mode".to_owned();
            request.mode = Some(match mode {
                rt_domain::ReadMode::Raw => "raw".to_owned(),
                rt_domain::ReadMode::Event => "event".to_owned(),
                rt_domain::ReadMode::FirstLastSeen => "fsls".to_owned(),
            });
            request.timeout = Some(u32::from(timeout));
        }
        rt_domain::ReaderControlAction::SetTto { enabled } => {
            request.command = "set_tto".to_owned();
            request.enabled = Some(enabled);
        }
        rt_domain::ReaderControlAction::SetRecording { enabled } => {
            request.command = "set_recording".to_owned();
            request.enabled = Some(enabled);
        }
        rt_domain::ReaderControlAction::ClearRecords => {
            request.command = "clear_records".to_owned()
        }
        rt_domain::ReaderControlAction::StartDownload => {
            request.command = "start_download".to_owned()
        }
        rt_domain::ReaderControlAction::StopDownload => {
            request.command = "stop_download".to_owned()
        }
        rt_domain::ReaderControlAction::Refresh => request.command = "refresh".to_owned(),
        rt_domain::ReaderControlAction::Reconnect => request.command = "reconnect".to_owned(),
        rt_domain::ReaderControlAction::SetEpochName { name } => {
            request.command = "set_epoch_name".to_owned();
            request.epoch_name = name;
        }
        rt_domain::ReaderControlAction::AdvanceEpoch => {
            request.command = "advance_epoch".to_owned()
        }
    }
    request
}

async fn handle_control_frame(
    endpoint_id: &str,
    frame: ControlF2C,
    send: &mut SendStream,
    reporter: &Arc<SessionStatusReporter>,
    recompute_notify: &Notify,
    pending_config: &mut HashMap<String, PendingConfigRequest>,
    pending_reader: &mut HashMap<String, PendingReaderRequest>,
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
        Some(control_f2c::Msg::DownloadProgress(progress)) => {
            reporter
                .app_state()
                .store_forwarder_download_progress_sync(endpoint_id, progress);
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
        // Route config responses by request_id to the awaiting command. A pure
        // map lookup plus a non-blocking `oneshot` send — never blocks the
        // reader-fed frame path (and thus never the heartbeat). An unknown
        // request_id (timed-out/cancelled command) is dropped.
        Some(control_f2c::Msg::ConfigGetResponse(response)) => {
            if let Some(request) = pending_config.remove(&response.request_id)
                && let PendingConfigResponder::Get(tx) = request.responder
            {
                let _ = tx.send(response);
            }
        }
        Some(control_f2c::Msg::ConfigSetResponse(response)) => {
            if let Some(request) = pending_config.remove(&response.request_id)
                && let PendingConfigResponder::Set(tx) = request.responder
            {
                let _ = tx.send(response);
            }
        }
        Some(control_f2c::Msg::RestartResponse(response)) => {
            if let Some(request) = pending_config.remove(&response.request_id)
                && let PendingConfigResponder::Restart(tx) = request.responder
            {
                let _ = tx.send(response);
            }
        }
        Some(control_f2c::Msg::ReaderControlResponse(response)) => {
            if let Some(request) = pending_reader.remove(&response.request_id) {
                let _ = request.responder.send(response);
            }
        }
        Some(
            control_f2c::Msg::Pong(_)
            | control_f2c::Msg::SyncClock(_)
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
    writer: &WriterHandle,
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
        let writer = writer.clone();
        let reporter = Arc::clone(reporter);
        let stream = stream.clone();
        tasks.insert(
            stream_id.clone(),
            tokio::spawn(async move {
                let _data_guard = reporter.on_data_session(&endpoint_id).await;
                let result = run_data_subscription_with_hint(
                    &connection,
                    &writer,
                    &stream.stream_id,
                    &stream.local_stream_key,
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
        CAP_READER_CONTROL, CAP_REMOTE_CONFIG, ControlF2C, DownloadProgress, EventBatch, Hello,
        MAX_FRAME_BYTES, ReadRecord, ReaderStatus, StreamCatalog, SubscribeMode, SubscribeOk,
        control_f2c,
    };
    use rt_test_utils::p2p::{ConnectivityFault, ForwarderScript, MockForwarderPeer};
    use rt_test_utils::poll_until;
    use tokio::sync::{broadcast, oneshot};

    use crate::control_api::{
        AppState, ConfigCommand, ForwarderConnState, ReaderCommand, get_connections,
        get_forwarder_config, restart_forwarder, set_forwarder_config,
    };
    use crate::p2p_session::{BackoffConfig, SessionStatusReporter};
    use crate::stream_key::LocalStreamKey;

    use super::{
        FORWARDER_CONFIG_TIMEOUT, ForwarderConnection, ForwarderDataStream, PendingConfigRequest,
        PendingConfigResponder, prune_expired_pending_config,
    };

    const STREAM_ID: &str = "127.0.0.1:10000";

    fn local_stream_key(endpoint_id: &str) -> LocalStreamKey {
        LocalStreamKey::new(endpoint_id, STREAM_ID)
    }

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
            config_get_json: String::new(),
            config_restart_needed: false,
            respond_to_config_requests: true,
            reader_control_info_json: None,
            respond_to_reader_control_requests: true,
        }
    }

    /// Writer + Db pair sharing one temp-file DB for data-stream tests.
    struct TestStore {
        writer: crate::writer::WriterHandle,
        db: Arc<tokio::sync::Mutex<crate::db::Db>>,
        _dir: tempfile::TempDir,
    }

    fn test_store() -> TestStore {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fwd-test.sqlite3");
        let db = crate::db::Db::open(&path).unwrap();
        let (writer, _thread) =
            crate::writer::spawn_writer(&path, crate::writer::WriterConfig::default()).unwrap();
        TestStore {
            writer,
            db: Arc::new(tokio::sync::Mutex::new(db)),
            _dir: dir,
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
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(41).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
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
                local_stream_key: local_stream_key(&endpoint_id),
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
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(43).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
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
                local_stream_key: local_stream_key(&endpoint_id),
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
            let guard = store.db.lock().await;
            let seqs: Vec<i64> = guard
                .load_received_events(local_stream_key(&endpoint_id).as_str())
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect();
            assert_eq!(
                seqs,
                vec![1, 2],
                "resume must not produce duplicate durable rows"
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key(&endpoint_id).as_str())
                    .unwrap(),
                2
            );
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
                    state: String::new(),
                    reads_received: 0,
                    progress: 0,
                    total: 0,
                    error: String::new(),
                })),
            },
            // Live reader status the receiver must reflect in get_connections.
            ControlF2C {
                msg: Some(control_f2c::Msg::ReaderStatus(ReaderStatus {
                    stream_id: STREAM_ID.as_bytes().to_vec(),
                    connected: false,
                    state: "disconnected".to_owned(),
                    last_read_unix_ms: 0,
                    reads_session: 0,
                    reads_total: 0,
                    last_seen_secs: None,
                    current_epoch_name: None,
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
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(45).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
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

    /// A client `Hello` advertising `CAP_REMOTE_CONFIG` (alongside data), so the
    /// negotiated session intersects to remote-config support when the
    /// forwarder also advertises it.
    fn remote_config_hello() -> Hello {
        Hello {
            min_minor: 1,
            max_minor: 1,
            capabilities: vec!["data".to_owned(), CAP_REMOTE_CONFIG.to_owned()],
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
            catalog_generation: 0,
        }
    }

    /// A forwarder script that advertises `CAP_REMOTE_CONFIG`, returns a canned
    /// config document for `ConfigGet`, acks `ConfigSet` with
    /// `restart_needed = true`, accepts `Restart`, and keeps issuing heartbeat
    /// pings so a config exchange can be shown not to disrupt the heartbeat.
    fn remote_config_script() -> ForwarderScript {
        let mut script = base_script();
        script.server_hello = remote_config_hello();
        script.config_get_json = "{\"sample\":true}".to_owned();
        script.config_restart_needed = true;
        script.control_pings = 4;
        script.control_ping_interval = Duration::from_millis(40);
        script
    }

    fn remote_config_no_response_script() -> ForwarderScript {
        let mut script = remote_config_script();
        script.respond_to_config_requests = false;
        script
    }

    fn reader_control_hello() -> Hello {
        Hello {
            min_minor: 1,
            max_minor: 1,
            capabilities: vec!["data".to_owned(), CAP_READER_CONTROL.to_owned()],
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
            catalog_generation: 0,
        }
    }

    fn reader_control_script() -> ForwarderScript {
        let mut script = base_script();
        script.server_hello = reader_control_hello();
        script.reader_control_info_json = Some("{\"tto_enabled\":true}".to_owned());
        script.control_pings = 2;
        script.control_ping_interval = Duration::from_millis(40);
        script
    }

    /// Over a REAL control session that negotiated `CAP_REMOTE_CONFIG`, the
    /// receiver must round-trip config get/set and restart, surface
    /// `remote_config_available = true`, and keep the heartbeat alive
    /// throughout the exchange.
    #[tokio::test]
    async fn remote_config_get_set_restart_over_real_session() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let forwarder = MockForwarderPeer::start([46; 32], remote_config_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(47).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let mut ui_rx = state.ui_tx.subscribe();
            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
                remote_config_hello(),
                Arc::clone(&reporter),
                BackoffConfig {
                    initial: Duration::from_millis(50),
                    max: Duration::from_millis(50),
                },
            );

            // The remote-config channel is registered only once the session is
            // up and `CAP_REMOTE_CONFIG` was negotiated, and the registration
            // emits a follow-up ConnectionsChanged event so UI refetches can
            // observe `remote_config_available = true`.
            let mut saw_remote_config_event = false;
            for _ in 0..8 {
                let event = tokio::time::timeout(Duration::from_secs(2), ui_rx.recv())
                    .await
                    .expect("remote-config connection should emit connection events")
                    .expect("UI event channel should stay open");
                if matches!(event, crate::ui_events::ReceiverUiEvent::ConnectionsChanged)
                    && get_connections(&state)
                        .await
                        .forwarders
                        .iter()
                        .any(|f| f.endpoint_id == endpoint_id && f.remote_config_available)
                {
                    saw_remote_config_event = true;
                    break;
                }
            }
            assert!(
                saw_remote_config_event,
                "remote_config_available=true should be visible after a ConnectionsChanged event"
            );

            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move {
                        get_connections(&state)
                            .await
                            .forwarders
                            .iter()
                            .any(|f| f.endpoint_id == endpoint_id && f.remote_config_available)
                    }
                },
                Duration::from_secs(5),
            )
            .await;

            // get → the forwarder's canned config document.
            let config = get_forwarder_config(&state, endpoint_id.clone())
                .await
                .expect("get_forwarder_config over a live remote-config session");
            assert_eq!(config.config_json, "{\"sample\":true}");
            assert!(config.restart_needed);

            // set → round-trips the full document and returns ok + restart_needed.
            let set =
                set_forwarder_config(&state, endpoint_id.clone(), "{\"updated\":1}".to_owned())
                    .await
                    .expect("set_forwarder_config over a live remote-config session");
            assert!(set.ok);
            assert!(set.restart_needed);
            assert!(set.error.is_none());
            let sets = forwarder.config_sets();
            assert_eq!(
                sets.len(),
                1,
                "forwarder must observe exactly one ConfigSet"
            );
            assert_eq!(sets[0].config_json, "{\"updated\":1}");

            // restart → accepted.
            let restart = restart_forwarder(&state, endpoint_id.clone())
                .await
                .expect("restart_forwarder over a live remote-config session");
            assert!(restart.accepted);
            assert!(restart.error.is_none());

            // Heartbeat maintained throughout: pongs continued to flow during
            // the config exchange. If the request writes/response routing had
            // blocked the heartbeat, pongs would stall here.
            poll_until(
                || async { forwarder.pongs().len() >= 4 },
                Duration::from_secs(10),
            )
            .await;
            assert_eq!(
                state.forwarder_state(&endpoint_id).await.state,
                ForwarderConnState::Connected,
                "the config exchange must not disconnect the control session"
            );

            connection.stop().await;
            forwarder.shutdown().await;
            endpoint.close().await;
        })
        .await
        .expect("remote config get/set/restart test timed out");
    }

    /// Over a REAL control session that negotiated `CAP_READER_CONTROL`, the
    /// receiver must expose reader-control availability, send a typed request,
    /// route the response by request id, and keep the heartbeat alive.
    #[tokio::test]
    async fn reader_control_request_over_real_session() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let forwarder = MockForwarderPeer::start([52; 32], reader_control_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(53).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
                reader_control_hello(),
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
                        get_connections(&state)
                            .await
                            .forwarders
                            .iter()
                            .any(|f| f.endpoint_id == endpoint_id && f.reader_control_available)
                    }
                },
                Duration::from_secs(5),
            )
            .await;

            let tx = state
                .forwarder_reader_control_tx(&endpoint_id)
                .expect("reader-control sender should be registered");
            let (resp_tx, resp_rx) = oneshot::channel();
            tx.send(ReaderCommand::Request {
                stream_id: STREAM_ID.to_owned(),
                action: rt_domain::ReaderControlAction::SyncClock,
                resp: resp_tx,
            })
            .await
            .expect("send reader-control command");

            let response = tokio::time::timeout(FORWARDER_CONFIG_TIMEOUT, resp_rx)
                .await
                .expect("reader-control response should arrive before timeout")
                .expect("reader-control responder should stay open");
            assert!(response.success);
            assert_eq!(response.message, "");
            assert_eq!(
                response.reader_info_json.as_deref(),
                Some("{\"tto_enabled\":true}")
            );

            let requests = forwarder.reader_control_requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].command, "sync_clock");
            assert_eq!(requests[0].stream_id, STREAM_ID.as_bytes());

            poll_until(
                || async { forwarder.pongs().len() >= 2 },
                Duration::from_secs(10),
            )
            .await;
            assert_eq!(
                state.forwarder_state(&endpoint_id).await.state,
                ForwarderConnState::Connected,
                "the reader-control exchange must not disconnect the control session"
            );

            connection.stop().await;
            forwarder.shutdown().await;
            endpoint.close().await;
        })
        .await
        .expect("reader-control request test timed out");
    }

    #[test]
    fn prune_expired_pending_config_drops_only_expired_responders() {
        let now = tokio::time::Instant::now();
        let (expired_tx, mut expired_rx) = oneshot::channel();
        let (fresh_tx, mut fresh_rx) = oneshot::channel();
        let mut pending = std::collections::HashMap::from([
            (
                "expired".to_owned(),
                PendingConfigRequest {
                    deadline: now - Duration::from_millis(1),
                    responder: PendingConfigResponder::Get(expired_tx),
                },
            ),
            (
                "fresh".to_owned(),
                PendingConfigRequest {
                    deadline: now + Duration::from_millis(1),
                    responder: PendingConfigResponder::Get(fresh_tx),
                },
            ),
        ]);

        prune_expired_pending_config(&mut pending, now);

        assert!(!pending.contains_key("expired"));
        assert!(pending.contains_key("fresh"));
        assert!(matches!(
            expired_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ));
        assert!(matches!(
            fresh_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn unanswered_raw_config_requests_are_pruned_without_breaking_heartbeat() {
        tokio::time::timeout(Duration::from_secs(25), async {
            let forwarder = MockForwarderPeer::start([50; 32], remote_config_no_response_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(51).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
                remote_config_hello(),
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
                    async move { state.forwarder_config_tx(&endpoint_id).is_some() }
                },
                Duration::from_secs(5),
            )
            .await;

            let tx = state
                .forwarder_config_tx(&endpoint_id)
                .expect("remote-config tx should be registered");
            let mut receivers = Vec::new();
            for _ in 0..3 {
                let (resp_tx, resp_rx) = oneshot::channel();
                tx.try_send(ConfigCommand::Get { resp: resp_tx })
                    .expect("test request should enqueue");
                receivers.push(resp_rx);
            }

            for rx in receivers {
                tokio::time::timeout(FORWARDER_CONFIG_TIMEOUT + Duration::from_secs(2), rx)
                    .await
                    .expect("expired pending config request should be pruned")
                    .expect_err("pruning should drop the pending responder");
            }

            poll_until(
                || async { forwarder.pongs().len() >= 4 },
                Duration::from_secs(10),
            )
            .await;

            connection.stop().await;
            forwarder.shutdown().await;
            endpoint.close().await;
        })
        .await
        .expect("unanswered config request prune test timed out");
    }

    #[tokio::test]
    async fn config_registration_deregisters_on_control_disconnect_and_task_abort() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let forwarder = MockForwarderPeer::start([52; 32], remote_config_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(53).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
                remote_config_hello(),
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
                    async move { state.forwarder_remote_config_available(&endpoint_id) }
                },
                Duration::from_secs(5),
            )
            .await;

            forwarder.shutdown().await;
            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move { !state.forwarder_remote_config_available(&endpoint_id) }
                },
                Duration::from_secs(5),
            )
            .await;
            tokio::time::timeout(
                Duration::from_secs(2),
                get_forwarder_config(&state, endpoint_id.clone()),
            )
            .await
            .expect("command after disconnect should fail fast")
            .expect_err("command after disconnect should error");

            let ForwarderConnection { task, .. } = connection;
            task.abort();
            let _ = task.await;
            endpoint.close().await;
        })
        .await
        .expect("config deregister on disconnect test timed out");

        tokio::time::timeout(Duration::from_secs(20), async {
            let forwarder = MockForwarderPeer::start([54; 32], remote_config_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(55).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
                remote_config_hello(),
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
                    async move { state.forwarder_remote_config_available(&endpoint_id) }
                },
                Duration::from_secs(5),
            )
            .await;

            let ForwarderConnection { task, .. } = connection;
            task.abort();
            let _ = task.await;
            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move { !state.forwarder_remote_config_available(&endpoint_id) }
                },
                Duration::from_secs(5),
            )
            .await;
            tokio::time::timeout(
                Duration::from_secs(2),
                get_forwarder_config(&state, endpoint_id.clone()),
            )
            .await
            .expect("command after task abort should fail fast")
            .expect_err("command after task abort should error");

            forwarder.shutdown().await;
            endpoint.close().await;
        })
        .await
        .expect("config deregister on task abort test timed out");
    }

    #[tokio::test]
    async fn config_registration_is_restored_after_reconnect() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut script = remote_config_script();
            script.close_connection_after_data = true;
            let forwarder = MockForwarderPeer::start([56; 32], script).await.unwrap();
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(57).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
                remote_config_hello(),
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
                    async move { state.forwarder_remote_config_available(&endpoint_id) }
                },
                Duration::from_secs(5),
            )
            .await;

            let (hint_tx, _hint_rx) = broadcast::channel(16);
            connection.set_desired_streams(vec![ForwarderDataStream {
                stream_id: STREAM_ID.to_owned(),
                local_stream_key: local_stream_key(&endpoint_id),
                mode: SubscribeMode::Replay,
                durable_hint_tx: Some(hint_tx),
            }]);
            poll_until(
                || async { forwarder.connection_count() >= 2 },
                Duration::from_secs(10),
            )
            .await;
            connection.set_desired_streams(Vec::new());

            poll_until(
                || {
                    let state = Arc::clone(&state);
                    let endpoint_id = endpoint_id.clone();
                    async move { state.forwarder_remote_config_available(&endpoint_id) }
                },
                Duration::from_secs(5),
            )
            .await;
            let config = get_forwarder_config(&state, endpoint_id.clone())
                .await
                .expect("remote config should work after reconnect");
            assert_eq!(config.config_json, "{\"sample\":true}");

            connection.stop().await;
            forwarder.shutdown().await;
            endpoint.close().await;
        })
        .await
        .expect("config registration reconnect test timed out");
    }

    /// A forwarder that does NOT advertise `CAP_REMOTE_CONFIG` must leave
    /// `remote_config_available = false`, and config commands must fail fast
    /// (no registered channel) rather than hang.
    #[tokio::test]
    async fn config_command_without_negotiated_capability_errors_fast() {
        tokio::time::timeout(Duration::from_secs(20), async {
            // base_script's server_hello advertises only "data".
            let forwarder = MockForwarderPeer::start([48; 32], base_script())
                .await
                .unwrap();
            let endpoint_id = forwarder.endpoint_addr().id.to_string();
            let endpoint = Arc::new(test_endpoint(49).await);
            let store = test_store();
            let (state, _shutdown_rx) = AppState::new(
                crate::db::Db::open_in_memory().unwrap(),
                "recv-test".to_owned(),
            );
            let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));

            let connection = ForwarderConnection::start(
                endpoint_id.clone(),
                Arc::clone(&endpoint),
                forwarder.endpoint_addr(),
                store.writer.clone(),
                // Client advertises remote-config, but the forwarder does not,
                // so the negotiated session has no remote-config capability.
                remote_config_hello(),
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

            assert!(
                !get_connections(&state)
                    .await
                    .forwarders
                    .iter()
                    .any(|f| f.endpoint_id == endpoint_id && f.remote_config_available),
                "remote_config_available must be false without a negotiated capability"
            );

            // Must error fast (well under the 10s command timeout), not hang.
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                get_forwarder_config(&state, endpoint_id.clone()),
            )
            .await
            .expect("config command must fail fast, not hang");
            assert!(
                result.is_err(),
                "config command to an incapable forwarder must error"
            );

            connection.stop().await;
            forwarder.shutdown().await;
            endpoint.close().await;
        })
        .await
        .expect("incapable config command test timed out");
    }

    /// With no live session at all, a config command errors immediately.
    #[tokio::test]
    async fn config_command_without_live_session_errors() {
        let (state, _shutdown_rx) = AppState::new(
            crate::db::Db::open_in_memory().unwrap(),
            "recv-test".to_owned(),
        );
        let result = get_forwarder_config(&state, "no-such-forwarder".to_owned()).await;
        assert!(
            result.is_err(),
            "config command without a live session must error"
        );
    }
}

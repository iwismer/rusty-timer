//! Scripted forwarder peer for loopback P2P tests.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use prost::Message;
use rt_iroh::{Connection, Endpoint, EndpointAddr, EndpointBuilder, RecvStream, SendStream};
use rt_p2p_protocol::{
    Ack, CaughtUp, ConfigGetResponse, ConfigSetRequest, ConfigSetResponse, ControlC2F, ControlF2C,
    DataC2F, DataF2C, DataSubscribe, EventBatch, GapNotice, Hello, Ping, Pong,
    ReaderControlRequest, ReaderControlResponse, RestartResponse, StreamCatalog, SubscribeOk,
    control_c2f, control_f2c, data_c2f, data_f2c, negotiate,
};
use tokio::task::JoinHandle;

use super::{ConnectivityFault, HarnessResult, read_frame, write_frame};

/// Deterministically holds back the tail of a script's event batches until a
/// test releases them.
///
/// Batches at index `>= after_batches` (and the trailing `CaughtUp`, which is
/// only sent once every batch has been sent) wait until the watch value
/// becomes `true`. This gives tests an unbounded, race-free window to act
/// between "early batches delivered" and "late batches exist anywhere" —
/// e.g. forcing a reconnect mid-stream — instead of relying on injected frame
/// delays and hoping the runner is fast enough.
///
/// The gate is shared across connections: a reconnecting peer served from
/// scratch also blocks at the same batch index until the release.
#[derive(Clone, Debug)]
pub struct BatchGate {
    /// Number of leading batches served immediately (ungated).
    pub after_batches: usize,
    /// Gated batches are sent once this observes `true`. If the test drops
    /// the sender without releasing, gated batches are never sent.
    pub release: tokio::sync::watch::Receiver<bool>,
}

/// The scripted responses a [`MockForwarderPeer`] serves to a connecting peer.
#[derive(Clone, Debug)]
pub struct ForwarderScript {
    /// The forwarder's own `Hello`, used to negotiate against the client's.
    pub server_hello: Hello,
    /// Catalog delivered on the control plane after `HelloOk`.
    pub catalog: StreamCatalog,
    /// `SubscribeOk` delivered on the data plane after a `DataSubscribe`.
    pub subscribe_ok: SubscribeOk,
    /// If set, a `GapNotice` delivered on the data plane immediately after
    /// `SubscribeOk` (before any batches), simulating unavailable history.
    pub gap_notice: Option<GapNotice>,
    /// Event batches delivered on the data plane after `SubscribeOk`.
    pub batches: Vec<EventBatch>,
    /// If set, a `CaughtUp` notice (with this `through_seq`) is sent after the
    /// batches.
    pub caught_up_through: Option<u64>,
    /// Fault injected into outbound data-plane frames after a subscription.
    pub data_fault: ConnectivityFault,
    /// If set, holds back event batches at index `>= after_batches` until the
    /// test releases the gate. See [`BatchGate`]. Defaults to `None` (all
    /// batches sent immediately).
    pub batch_gate: Option<BatchGate>,
    /// When `true`, every outbound data-plane frame's `stream_id` (the
    /// `SubscribeOk` and each `EventBatch` record, plus any `GapNotice`) is
    /// rewritten to the `stream_id` carried by the inbound `DataSubscribe`, so a
    /// single script can serve *any* subscribed stream. This lets one connection
    /// multiplex several distinct streams (each gets records tagged with its own
    /// id) without a per-stream script. Defaults to `false`, which serves the
    /// script's verbatim `stream_id`s (so stream-id-mismatch tests still work).
    pub echo_subscribed_stream_id: bool,
    /// When `true`, the mock closes the whole QUIC connection immediately after
    /// the first data stream is served (through its ack), instead of keeping the
    /// control session open for further data streams. This forces a connecting
    /// peer's per-forwarder connection to observe a control disconnect and
    /// reconnect. Each reconnect is accepted as a fresh connection and served
    /// from scratch (re-sending the scripted batches), so it exercises
    /// resume-from-cursor dedup. Defaults to `false`.
    pub close_connection_after_data: bool,
    /// Control-plane `F2C` frames sent (in order) on the control stream right
    /// after the `HelloOk`/`StreamCatalog` handshake. Lets a test forwarder push
    /// live status (`ReaderStatus`, `ReaderInfo`, `UpsStatus`) and exercise the
    /// receiver's control read loop over a real stream. Defaults to empty.
    pub control_events: Vec<ControlF2C>,
    /// Number of heartbeat `Ping` frames the mock issues on the control stream
    /// after `control_events`, each spaced by [`Self::control_ping_interval`].
    /// The mock records every `Pong` it reads back (see
    /// [`MockForwarderPeer::pongs`]) so a test can assert the receiver keeps the
    /// heartbeat alive. Defaults to `0` (no pings).
    pub control_pings: u32,
    /// Delay between successive heartbeat pings (see [`Self::control_pings`]).
    pub control_ping_interval: Duration,
    /// Canned config document returned in a `ConfigGetResponse` for any
    /// inbound `ConfigGetRequest` (remote-config support). Only exercised when
    /// the negotiated [`Self::server_hello`] advertises `CAP_REMOTE_CONFIG` so
    /// the receiver actually issues config requests. Defaults to empty.
    pub config_get_json: String,
    /// `restart_needed` flag echoed in both `ConfigGetResponse` and
    /// `ConfigSetResponse`. Defaults to `false`.
    pub config_restart_needed: bool,
    /// When false, inbound remote-config requests are recorded where relevant
    /// but deliberately left unanswered. This lets receiver tests exercise
    /// timeout/prune behavior against a forwarder that heartbeats but never
    /// answers config commands.
    pub respond_to_config_requests: bool,
    /// Optional rich reader-info JSON echoed in successful reader-control
    /// responses.
    pub reader_control_info_json: Option<String>,
    /// When false, inbound reader-control requests are recorded but left
    /// unanswered so receiver tests can exercise timeout behavior.
    pub respond_to_reader_control_requests: bool,
}

/// A scripted forwarder peer bound to a loopback iroh endpoint.
///
/// Spawns a background accept loop that, for each inbound connection, performs
/// the control-plane `Hello` negotiation, serves the scripted catalog, then
/// serves the scripted data-plane subscription and records the client `Ack`.
#[derive(Debug)]
pub struct MockForwarderPeer {
    endpoint: Endpoint,
    endpoint_addr: EndpointAddr,
    accept_task: JoinHandle<()>,
    acks: Arc<Mutex<Vec<Ack>>>,
    subscribes: Arc<Mutex<Vec<DataSubscribe>>>,
    connections: Arc<Mutex<usize>>,
    pongs: Arc<Mutex<Vec<Pong>>>,
    config_sets: Arc<Mutex<Vec<ConfigSetRequest>>>,
    reader_control_requests: Arc<Mutex<Vec<ReaderControlRequest>>>,
}

impl MockForwarderPeer {
    /// Binds a loopback endpoint seeded with `seed` and starts serving `script`.
    pub async fn start(seed: [u8; 32], script: ForwarderScript) -> HarnessResult<Self> {
        let endpoint = EndpointBuilder::test(seed).bind().await?;
        let endpoint_addr = endpoint.endpoint_addr().await;

        let acks = Arc::new(Mutex::new(Vec::new()));
        let subscribes = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(Mutex::new(0usize));
        let pongs = Arc::new(Mutex::new(Vec::new()));
        let config_sets = Arc::new(Mutex::new(Vec::new()));
        let reader_control_requests = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(script);

        let accept_endpoint = endpoint.clone();
        let accept_acks = Arc::clone(&acks);
        let accept_subscribes = Arc::clone(&subscribes);
        let accept_connections = Arc::clone(&connections);
        let accept_pongs = Arc::clone(&pongs);
        let accept_config_sets = Arc::clone(&config_sets);
        let accept_reader_control_requests = Arc::clone(&reader_control_requests);
        let accept_task = tokio::spawn(async move {
            accept_loop(
                accept_endpoint,
                script,
                accept_acks,
                accept_subscribes,
                accept_connections,
                accept_pongs,
                accept_config_sets,
                accept_reader_control_requests,
            )
            .await;
        });

        Ok(Self {
            endpoint,
            endpoint_addr,
            accept_task,
            acks,
            subscribes,
            connections,
            pongs,
            config_sets,
            reader_control_requests,
        })
    }

    /// The dialable address of this forwarder peer.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.endpoint_addr.clone()
    }

    /// A snapshot of the acks received from connected receivers.
    pub fn acks(&self) -> Vec<Ack> {
        self.acks.lock().expect("acks mutex poisoned").clone()
    }

    /// A snapshot of the `DataSubscribe` requests received, in arrival order.
    /// Useful for asserting that a reconnecting receiver resumes from its
    /// persisted cursor (`after_seq`).
    pub fn subscribes(&self) -> Vec<DataSubscribe> {
        self.subscribes
            .lock()
            .expect("subscribes mutex poisoned")
            .clone()
    }

    /// A snapshot of the `Pong` frames received on the control stream, in
    /// arrival order. Used to assert the receiver answered the mock's heartbeat
    /// `Ping`s (see [`ForwarderScript::control_pings`]).
    pub fn pongs(&self) -> Vec<Pong> {
        self.pongs.lock().expect("pongs mutex poisoned").clone()
    }

    /// A snapshot of the `ConfigSetRequest`s received on the control stream, in
    /// arrival order. Used to assert a `set_forwarder_config` round-trip
    /// delivered the exact config document.
    pub fn config_sets(&self) -> Vec<ConfigSetRequest> {
        self.config_sets
            .lock()
            .expect("config_sets mutex poisoned")
            .clone()
    }

    /// A snapshot of the `ReaderControlRequest`s received on the control stream.
    pub fn reader_control_requests(&self) -> Vec<ReaderControlRequest> {
        self.reader_control_requests
            .lock()
            .expect("reader_control_requests mutex poisoned")
            .clone()
    }

    /// The number of inbound QUIC connections accepted so far. A receiver that
    /// multiplexes several data streams over one control session opens exactly
    /// one connection, so this stays `1` while many streams are subscribed.
    pub fn connection_count(&self) -> usize {
        *self.connections.lock().expect("connections mutex poisoned")
    }

    /// Stops the accept loop and closes the endpoint.
    pub async fn shutdown(self) {
        self.accept_task.abort();
        self.endpoint.close().await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn accept_loop(
    endpoint: Endpoint,
    script: Arc<ForwarderScript>,
    acks: Arc<Mutex<Vec<Ack>>>,
    subscribes: Arc<Mutex<Vec<DataSubscribe>>>,
    connections: Arc<Mutex<usize>>,
    pongs: Arc<Mutex<Vec<Pong>>>,
    config_sets: Arc<Mutex<Vec<ConfigSetRequest>>>,
    reader_control_requests: Arc<Mutex<Vec<ReaderControlRequest>>>,
) {
    while let Ok(Some(connection)) = endpoint.accept().await {
        *connections.lock().expect("connections mutex poisoned") += 1;
        let script = Arc::clone(&script);
        let acks = Arc::clone(&acks);
        let subscribes = Arc::clone(&subscribes);
        let pongs = Arc::clone(&pongs);
        let config_sets = Arc::clone(&config_sets);
        let reader_control_requests = Arc::clone(&reader_control_requests);
        tokio::spawn(async move {
            // Errors here are surfaced via missing acks / failed reads on the
            // receiver side; the harness self-test asserts on those.
            let _ = handle_connection(
                connection,
                script,
                acks,
                subscribes,
                pongs,
                config_sets,
                reader_control_requests,
            )
            .await;
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    connection: Connection,
    script: Arc<ForwarderScript>,
    acks: Arc<Mutex<Vec<Ack>>>,
    subscribes: Arc<Mutex<Vec<DataSubscribe>>>,
    pongs: Arc<Mutex<Vec<Pong>>>,
    config_sets: Arc<Mutex<Vec<ConfigSetRequest>>>,
    reader_control_requests: Arc<Mutex<Vec<ReaderControlRequest>>>,
) -> HarnessResult {
    let (control_send, control_recv) = serve_control(&connection, &script).await?;
    // Drive live control-plane events (status pushes) and heartbeat pings on a
    // dedicated task that exclusively owns the control streams, mirroring a real
    // forwarder that keeps the control session active alongside data streams.
    let control_task = tokio::spawn(serve_control_loop(
        control_send,
        control_recv,
        Arc::clone(&script),
        pongs,
        config_sets,
        reader_control_requests,
    ));
    serve_data_loop(&connection, &script, &acks, &subscribes).await;
    control_task.abort();
    Ok(())
}

/// After the handshake, send the scripted control events, issue the scripted
/// heartbeat pings, and record every `Pong` the receiver sends back. Runs until
/// the control stream closes (receiver disconnect) or the task is aborted.
async fn serve_control_loop(
    mut send: SendStream,
    mut recv: RecvStream,
    script: Arc<ForwarderScript>,
    pongs: Arc<Mutex<Vec<Pong>>>,
    config_sets: Arc<Mutex<Vec<ConfigSetRequest>>>,
    reader_control_requests: Arc<Mutex<Vec<ReaderControlRequest>>>,
) {
    for event in &script.control_events {
        if write_frame(&mut send, event).await.is_err() {
            return;
        }
    }
    for nonce in 0..u64::from(script.control_pings) {
        if nonce > 0 {
            tokio::time::sleep(script.control_ping_interval).await;
        }
        let ping = ControlF2C {
            msg: Some(control_f2c::Msg::Ping(Ping { nonce })),
        };
        if write_frame(&mut send, &ping).await.is_err() {
            return;
        }
    }
    // Keep reading C2F frames until the control stream closes. Records every
    // Pong (heartbeat answer) and responds to remote-config requests, mirroring
    // a forwarder that advertised `CAP_REMOTE_CONFIG`. Config responses are
    // written on the same control send stream, interleaved with heartbeat pings
    // exactly as a real forwarder would.
    loop {
        match read_frame::<ControlC2F>(&mut recv).await {
            Ok(frame) => match frame.msg {
                Some(control_c2f::Msg::Pong(pong)) => {
                    pongs.lock().expect("pongs mutex poisoned").push(pong);
                }
                Some(control_c2f::Msg::ConfigGetRequest(request)) => {
                    if !script.respond_to_config_requests {
                        continue;
                    }
                    let response = ControlF2C {
                        msg: Some(control_f2c::Msg::ConfigGetResponse(ConfigGetResponse {
                            request_id: request.request_id,
                            config_json: script.config_get_json.clone(),
                            restart_needed: script.config_restart_needed,
                        })),
                    };
                    if write_frame(&mut send, &response).await.is_err() {
                        return;
                    }
                }
                Some(control_c2f::Msg::ConfigSetRequest(request)) => {
                    config_sets
                        .lock()
                        .expect("config_sets mutex poisoned")
                        .push(request.clone());
                    if !script.respond_to_config_requests {
                        continue;
                    }
                    let response = ControlF2C {
                        msg: Some(control_f2c::Msg::ConfigSetResponse(ConfigSetResponse {
                            request_id: request.request_id,
                            ok: true,
                            restart_needed: script.config_restart_needed,
                            error: String::new(),
                        })),
                    };
                    if write_frame(&mut send, &response).await.is_err() {
                        return;
                    }
                }
                Some(control_c2f::Msg::RestartRequest(request)) => {
                    if !script.respond_to_config_requests {
                        continue;
                    }
                    let response = ControlF2C {
                        msg: Some(control_f2c::Msg::RestartResponse(RestartResponse {
                            request_id: request.request_id,
                            accepted: true,
                            error: String::new(),
                        })),
                    };
                    if write_frame(&mut send, &response).await.is_err() {
                        return;
                    }
                }
                Some(control_c2f::Msg::ReaderControlRequest(request)) => {
                    reader_control_requests
                        .lock()
                        .expect("reader_control_requests mutex poisoned")
                        .push(request.clone());
                    if !script.respond_to_reader_control_requests {
                        continue;
                    }
                    let response = ControlF2C {
                        msg: Some(control_f2c::Msg::ReaderControlResponse(
                            ReaderControlResponse {
                                stream_id: request.stream_id,
                                request_id: request.request_id,
                                success: true,
                                message: String::new(),
                                reader_info_json: script.reader_control_info_json.clone(),
                            },
                        )),
                    };
                    if write_frame(&mut send, &response).await.is_err() {
                        return;
                    }
                }
                _ => {}
            },
            Err(_) => return,
        }
    }
}

/// Accept data bi-streams on a single connection for as long as it stays open,
/// serving each accepted stream concurrently in its own task. This is what lets
/// one control session multiplex several data subscriptions over the same
/// connection. The loop ends when the connection closes (`accept_bi` errors).
async fn serve_data_loop(
    connection: &Connection,
    script: &Arc<ForwarderScript>,
    acks: &Arc<Mutex<Vec<Ack>>>,
    subscribes: &Arc<Mutex<Vec<DataSubscribe>>>,
) {
    loop {
        let (send, recv) = match connection.accept_bi().await {
            Ok(pair) => pair,
            // Connection closed (receiver disconnected / shutdown): stop.
            Err(_) => return,
        };
        if script.close_connection_after_data {
            // Serve this one stream inline (so the ack is recorded), then close
            // the whole connection to force the receiver to reconnect and
            // resume from its persisted cursor.
            let _ = serve_one_data_stream(send, recv, script, acks, subscribes).await;
            connection.close(0u32.into(), b"mock-reconnect");
            return;
        }
        let script = Arc::clone(script);
        let acks = Arc::clone(acks);
        let subscribes = Arc::clone(subscribes);
        tokio::spawn(async move {
            let _ = serve_one_data_stream(send, recv, &script, &acks, &subscribes).await;
        });
    }
}

async fn serve_control(
    connection: &Connection,
    script: &ForwarderScript,
) -> Result<(SendStream, RecvStream), Box<dyn std::error::Error + Send + Sync>> {
    let (mut send, mut recv) = connection.accept_bi().await?;

    let control = read_frame::<ControlC2F>(&mut recv).await?;
    let client_hello = match control.msg {
        Some(control_c2f::Msg::Hello(hello)) => hello,
        other => return Err(format!("expected control Hello, got {other:?}").into()),
    };

    let hello_ok = negotiate(&client_hello, &script.server_hello)?;
    write_frame(
        &mut send,
        &ControlF2C {
            msg: Some(control_f2c::Msg::HelloOk(hello_ok)),
        },
    )
    .await?;
    write_frame(
        &mut send,
        &ControlF2C {
            msg: Some(control_f2c::Msg::StreamCatalog(script.catalog.clone())),
        },
    )
    .await?;

    Ok((send, recv))
}

async fn serve_one_data_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    script: &ForwarderScript,
    acks: &Arc<Mutex<Vec<Ack>>>,
    subscribes: &Arc<Mutex<Vec<DataSubscribe>>>,
) -> HarnessResult {
    let data = read_frame::<DataC2F>(&mut recv).await?;
    let subscribe = match data.msg {
        Some(data_c2f::Msg::DataSubscribe(subscribe)) => subscribe,
        other => return Err(format!("expected DataSubscribe, got {other:?}").into()),
    };
    subscribes
        .lock()
        .expect("subscribes mutex poisoned")
        .push(subscribe.clone());

    // When echoing, every outbound frame is tagged with the subscribed
    // stream_id so one script can serve any stream. Otherwise the script's
    // verbatim stream_ids are used.
    let stream_id_for = |script_default: &[u8]| -> Vec<u8> {
        if script.echo_subscribed_stream_id {
            subscribe.stream_id.clone()
        } else {
            script_default.to_vec()
        }
    };

    let mut subscribe_ok = script.subscribe_ok.clone();
    subscribe_ok.stream_id = stream_id_for(&script.subscribe_ok.stream_id);
    write_faulted_data_frame(
        &mut send,
        &script.data_fault,
        &DataF2C {
            msg: Some(data_f2c::Msg::SubscribeOk(subscribe_ok)),
        },
    )
    .await?;

    if let Some(gap_notice) = &script.gap_notice {
        let mut gap_notice = gap_notice.clone();
        gap_notice.stream_id = stream_id_for(&gap_notice.stream_id);
        write_faulted_data_frame(
            &mut send,
            &script.data_fault,
            &DataF2C {
                msg: Some(data_f2c::Msg::GapNotice(gap_notice)),
            },
        )
        .await?;
    }

    for (index, batch) in script.batches.iter().enumerate() {
        if let Some(gate) = &script.batch_gate
            && index >= gate.after_batches
        {
            let mut release = gate.release.clone();
            // A dropped sender means the test abandoned the gate; stop
            // serving rather than sending the gated batches.
            if release.wait_for(|open| *open).await.is_err() {
                return Ok(());
            }
        }
        let mut batch = batch.clone();
        if script.echo_subscribed_stream_id {
            for record in &mut batch.records {
                record.stream_id = subscribe.stream_id.clone();
            }
        }
        write_faulted_data_frame(
            &mut send,
            &script.data_fault,
            &DataF2C {
                msg: Some(data_f2c::Msg::EventBatch(batch)),
            },
        )
        .await?;
    }

    if let Some(through_seq) = script.caught_up_through {
        write_faulted_data_frame(
            &mut send,
            &script.data_fault,
            &DataF2C {
                msg: Some(data_f2c::Msg::CaughtUp(CaughtUp {
                    stream_id: subscribe.stream_id.clone(),
                    through_seq,
                })),
            },
        )
        .await?;
    }

    if script.data_fault.partitioned {
        return Ok(());
    }

    // Only block on an inbound ack when the receiver was given something to
    // acknowledge (events or a gap notice). A truly empty script expects no
    // ack, so returning here closes the connection and lets the receiver
    // observe EOF without deadlocking on a read that never arrives.
    if script.batches.is_empty() && script.gap_notice.is_none() {
        return Ok(());
    }

    let ack_message = read_frame::<DataC2F>(&mut recv).await?;
    if let Some(data_c2f::Msg::Ack(ack)) = ack_message.msg {
        acks.lock().expect("acks mutex poisoned").push(ack);
    }

    Ok(())
}

async fn write_faulted_data_frame(
    send: &mut SendStream,
    fault: &ConnectivityFault,
    message: &impl Message,
) -> HarnessResult {
    fault.apply_delay().await;
    if fault.should_drop() {
        return Ok(());
    }

    write_frame(send, message).await
}

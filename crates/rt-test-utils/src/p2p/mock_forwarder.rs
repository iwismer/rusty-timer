//! Scripted forwarder peer for loopback P2P tests.

use std::sync::{Arc, Mutex};

use prost::Message;
use rt_iroh::{Connection, Endpoint, EndpointBuilder, NodeAddr, SendStream};
use rt_p2p_protocol::{
    Ack, CaughtUp, ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, EventBatch, GapNotice,
    Hello, StreamCatalog, SubscribeOk, control_c2f, control_f2c, data_c2f, data_f2c, negotiate,
};
use tokio::task::JoinHandle;

use super::{ConnectivityFault, HarnessResult, read_frame, write_frame};

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
}

/// A scripted forwarder peer bound to a loopback iroh endpoint.
///
/// Spawns a background accept loop that, for each inbound connection, performs
/// the control-plane `Hello` negotiation, serves the scripted catalog, then
/// serves the scripted data-plane subscription and records the client `Ack`.
#[derive(Debug)]
pub struct MockForwarderPeer {
    endpoint: Endpoint,
    node_addr: NodeAddr,
    accept_task: JoinHandle<()>,
    acks: Arc<Mutex<Vec<Ack>>>,
    subscribes: Arc<Mutex<Vec<DataSubscribe>>>,
}

impl MockForwarderPeer {
    /// Binds a loopback endpoint seeded with `seed` and starts serving `script`.
    pub async fn start(seed: [u8; 32], script: ForwarderScript) -> HarnessResult<Self> {
        let endpoint = EndpointBuilder::test(seed).bind().await?;
        let node_addr = endpoint.node_addr().await;

        let acks = Arc::new(Mutex::new(Vec::new()));
        let subscribes = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(script);

        let accept_endpoint = endpoint.clone();
        let accept_acks = Arc::clone(&acks);
        let accept_subscribes = Arc::clone(&subscribes);
        let accept_task = tokio::spawn(async move {
            accept_loop(accept_endpoint, script, accept_acks, accept_subscribes).await;
        });

        Ok(Self {
            endpoint,
            node_addr,
            accept_task,
            acks,
            subscribes,
        })
    }

    /// The dialable address of this forwarder peer.
    pub fn node_addr(&self) -> NodeAddr {
        self.node_addr.clone()
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

    /// Stops the accept loop and closes the endpoint.
    pub async fn shutdown(self) {
        self.accept_task.abort();
        self.endpoint.close().await;
    }
}

async fn accept_loop(
    endpoint: Endpoint,
    script: Arc<ForwarderScript>,
    acks: Arc<Mutex<Vec<Ack>>>,
    subscribes: Arc<Mutex<Vec<DataSubscribe>>>,
) {
    while let Ok(Some(connection)) = endpoint.accept().await {
        let script = Arc::clone(&script);
        let acks = Arc::clone(&acks);
        let subscribes = Arc::clone(&subscribes);
        tokio::spawn(async move {
            // Errors here are surfaced via missing acks / failed reads on the
            // receiver side; the harness self-test asserts on those.
            let _ = handle_connection(connection, script, acks, subscribes).await;
        });
    }
}

async fn handle_connection(
    connection: Connection,
    script: Arc<ForwarderScript>,
    acks: Arc<Mutex<Vec<Ack>>>,
    subscribes: Arc<Mutex<Vec<DataSubscribe>>>,
) -> HarnessResult {
    serve_control(&connection, &script).await?;
    serve_data(&connection, &script, &acks, &subscribes).await?;
    Ok(())
}

async fn serve_control(connection: &Connection, script: &ForwarderScript) -> HarnessResult {
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

    Ok(())
}

async fn serve_data(
    connection: &Connection,
    script: &ForwarderScript,
    acks: &Arc<Mutex<Vec<Ack>>>,
    subscribes: &Arc<Mutex<Vec<DataSubscribe>>>,
) -> HarnessResult {
    let (mut send, mut recv) = connection.accept_bi().await?;

    let data = read_frame::<DataC2F>(&mut recv).await?;
    let subscribe = match data.msg {
        Some(data_c2f::Msg::DataSubscribe(subscribe)) => subscribe,
        other => return Err(format!("expected DataSubscribe, got {other:?}").into()),
    };
    subscribes
        .lock()
        .expect("subscribes mutex poisoned")
        .push(subscribe.clone());

    write_faulted_data_frame(
        &mut send,
        &script.data_fault,
        &DataF2C {
            msg: Some(data_f2c::Msg::SubscribeOk(script.subscribe_ok.clone())),
        },
    )
    .await?;

    if let Some(gap_notice) = &script.gap_notice {
        write_faulted_data_frame(
            &mut send,
            &script.data_fault,
            &DataF2C {
                msg: Some(data_f2c::Msg::GapNotice(gap_notice.clone())),
            },
        )
        .await?;
    }

    for batch in &script.batches {
        write_faulted_data_frame(
            &mut send,
            &script.data_fault,
            &DataF2C {
                msg: Some(data_f2c::Msg::EventBatch(batch.clone())),
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

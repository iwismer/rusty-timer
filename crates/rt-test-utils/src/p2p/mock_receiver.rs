//! Receiver peer for loopback P2P tests.

use rt_iroh::{Connection, Endpoint, EndpointAddr, EndpointBuilder, RecvStream, SendStream};
use rt_p2p_protocol::{
    Ack, ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, EventBatch, Hello, HelloOk,
    StreamCatalog, SubscribeOk, control_c2f, control_f2c, data_c2f, data_f2c,
};

use super::{HarnessResult, read_frame, write_frame};

/// A receiver peer bound to a loopback iroh endpoint.
///
/// Dials a [`MockForwarderPeer`](super::MockForwarderPeer), performs the control
/// plane `Hello` negotiation, and yields a [`ReceiverSession`] for data-plane
/// subscriptions.
#[derive(Debug)]
pub struct MockReceiverPeer {
    endpoint: Endpoint,
    endpoint_addr: EndpointAddr,
}

impl MockReceiverPeer {
    /// Binds a loopback endpoint seeded with `seed`.
    pub async fn start(seed: [u8; 32]) -> HarnessResult<Self> {
        let endpoint = EndpointBuilder::test(seed).bind().await?;
        let endpoint_addr = endpoint.endpoint_addr().await;
        Ok(Self {
            endpoint,
            endpoint_addr,
        })
    }

    /// The dialable address of this receiver peer.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.endpoint_addr.clone()
    }

    /// Dials `forwarder_addr`, opens the control stream, sends `client_hello`,
    /// and reads back the negotiated `HelloOk` plus the `StreamCatalog`.
    pub async fn hello(
        &self,
        forwarder_addr: EndpointAddr,
        client_hello: Hello,
    ) -> HarnessResult<ReceiverSession> {
        let connection = self.endpoint.connect(forwarder_addr).await?;

        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(client_hello)),
            },
        )
        .await?;

        let hello_ok = match read_frame::<ControlF2C>(&mut recv).await?.msg {
            Some(control_f2c::Msg::HelloOk(hello_ok)) => hello_ok,
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        };
        let catalog = match read_frame::<ControlF2C>(&mut recv).await?.msg {
            Some(control_f2c::Msg::StreamCatalog(catalog)) => catalog,
            other => return Err(format!("expected StreamCatalog, got {other:?}").into()),
        };

        Ok(ReceiverSession {
            connection,
            control_send: send,
            control_recv: recv,
            hello_ok,
            catalog,
        })
    }
}

/// An established control-plane session with a forwarder peer.
#[derive(Debug)]
pub struct ReceiverSession {
    connection: Connection,
    #[allow(dead_code)]
    control_send: SendStream,
    #[allow(dead_code)]
    control_recv: RecvStream,
    /// The negotiated handshake acknowledgement.
    pub hello_ok: HelloOk,
    /// The catalog delivered immediately after the handshake.
    pub catalog: StreamCatalog,
}

impl ReceiverSession {
    /// Opens a data-plane stream, sends `subscribe`, and reads back `SubscribeOk`.
    pub async fn subscribe(&self, subscribe: DataSubscribe) -> HarnessResult<DataSubscription> {
        let (mut send, mut recv) = self.connection.open_bi().await?;
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(subscribe)),
            },
        )
        .await?;

        let subscribe_ok = match read_frame::<DataF2C>(&mut recv).await?.msg {
            Some(data_f2c::Msg::SubscribeOk(subscribe_ok)) => subscribe_ok,
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        };

        Ok(DataSubscription {
            _connection: self.connection.clone(),
            send,
            recv,
            subscribe_ok,
        })
    }
}

/// An open data-plane subscription.
#[derive(Debug)]
pub struct DataSubscription {
    // Held to keep the connection alive independently of the parent session.
    _connection: Connection,
    send: SendStream,
    recv: RecvStream,
    /// The acknowledgement returned when the subscription opened.
    pub subscribe_ok: SubscribeOk,
}

impl DataSubscription {
    /// Reads the next data-plane message from the forwarder.
    pub async fn next_message(&mut self) -> HarnessResult<DataF2C> {
        read_frame::<DataF2C>(&mut self.recv).await
    }

    /// Reads data-plane messages until `count` [`EventBatch`] frames have been
    /// collected, ignoring other message kinds (e.g. `CaughtUp`).
    pub async fn collect_batches(&mut self, count: usize) -> HarnessResult<Vec<EventBatch>> {
        let mut batches = Vec::with_capacity(count);
        while batches.len() < count {
            if let Some(data_f2c::Msg::EventBatch(batch)) = self.next_message().await?.msg {
                batches.push(batch);
            }
        }
        Ok(batches)
    }

    /// Sends an [`Ack`] to the forwarder over the data stream.
    pub async fn ack(&mut self, ack: Ack) -> HarnessResult {
        write_frame(
            &mut self.send,
            &DataC2F {
                msg: Some(data_c2f::Msg::Ack(ack)),
            },
        )
        .await
    }
}

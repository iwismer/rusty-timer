//! Forwarder iroh endpoint and inbound accept loop.
//!
//! [`P2pEndpoint`] wraps an [`rt_iroh::Endpoint`] and serves inbound receiver
//! connections. For each accepted connection the remote node id is read from
//! the transport handshake and checked against an [`AllowList`]; unknown peers
//! are closed immediately. Admitted peers are handed to the control-stream
//! handler ([`crate::p2p::control`]), which performs the `Hello`/`HelloOk`
//! negotiation, serves the [`StreamCatalog`](rt_p2p_protocol::StreamCatalog),
//! and runs the heartbeat until the peer disconnects or is declared dead.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rt_iroh::{Connection, Endpoint, EndpointBuilder, NodeAddr, NodeId, load_or_create_secret_key};
use tokio::sync::Mutex;

use crate::storage::journal::Journal;

use super::allowlist::AllowList;
use super::control::{
    CatalogProvider, HeartbeatConfig, negotiate_control_stream, run_control_stream_loop,
};
use super::data::{DataConfig, serve_data_streams};

/// QUIC application error code used when closing a rejected connection.
const REJECT_ERROR_CODE: u32 = 1;

/// QUIC application error code used when closing a connection whose control
/// stream failed, timed out, or whose peer was declared dead by the heartbeat.
const CONTROL_ERROR_CODE: u32 = 2;

/// Maximum time an admitted peer is given to complete the control-plane
/// `Hello` handshake before the connection is closed. Bounds the lifetime of
/// broken or malicious allow-listed peers that connect but never make progress.
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The forwarder's P2P endpoint and accept loop.
#[derive(Clone)]
pub struct P2pEndpoint {
    endpoint: Endpoint,
    allow_list: AllowList,
    catalog: Arc<dyn CatalogProvider>,
    journal: Arc<Mutex<Journal>>,
    data_config: DataConfig,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
    #[cfg(test)]
    _test_tempdir: Option<Arc<tempfile::TempDir>>,
}

impl std::fmt::Debug for P2pEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P2pEndpoint")
            .field("endpoint", &self.endpoint)
            .field("allow_list", &self.allow_list)
            .field("catalog", &self.catalog)
            .field("data_config", &self.data_config)
            .field("handshake_timeout", &self.handshake_timeout)
            .field("heartbeat", &self.heartbeat)
            .finish_non_exhaustive()
    }
}

impl P2pEndpoint {
    /// Binds the endpoint, loading or creating the persistent secret key at
    /// `secret_key_path`. Admitted peers are served the catalog from `catalog`.
    pub async fn bind(
        secret_key_path: impl AsRef<Path>,
        allow_list: AllowList,
        catalog: Arc<dyn CatalogProvider>,
        journal: Arc<Mutex<Journal>>,
    ) -> Result<Self, rt_iroh::Error> {
        let secret_key = load_or_create_secret_key(secret_key_path)?;
        Self::bind_with_builder(
            EndpointBuilder::default().secret_key(secret_key),
            allow_list,
            catalog,
            journal,
            DataConfig::default(),
        )
        .await
    }

    /// Binds an endpoint from an explicit builder. Production startup uses this
    /// to apply loopback/test transport knobs from config without exposing raw
    /// iroh APIs to the binary.
    pub async fn bind_with_builder(
        builder: EndpointBuilder,
        allow_list: AllowList,
        catalog: Arc<dyn CatalogProvider>,
        journal: Arc<Mutex<Journal>>,
        data_config: DataConfig,
    ) -> Result<Self, rt_iroh::Error> {
        let endpoint = builder.bind().await?;
        Ok(Self {
            endpoint,
            allow_list,
            catalog,
            journal,
            data_config,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            heartbeat: HeartbeatConfig::default(),
            #[cfg(test)]
            _test_tempdir: None,
        })
    }

    /// The underlying iroh endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// This endpoint's dialable address.
    pub async fn node_addr(&self) -> NodeAddr {
        self.endpoint.node_addr().await
    }

    /// This endpoint's node id.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.endpoint.node_id()
    }

    /// Runs the accept loop until the endpoint is closed.
    ///
    /// Each inbound connection is handled on its own task so a slow or
    /// misbehaving peer cannot stall admission of others.
    pub async fn run(&self) {
        loop {
            match self.endpoint.accept().await {
                Ok(Some(connection)) => {
                    let allow_list = self.allow_list.clone();
                    let catalog = Arc::clone(&self.catalog);
                    let journal = Arc::clone(&self.journal);
                    let data_config = self.data_config;
                    let handshake_timeout = self.handshake_timeout;
                    let heartbeat = self.heartbeat;
                    tokio::spawn(async move {
                        handle_connection(
                            connection,
                            allow_list,
                            catalog,
                            journal,
                            data_config,
                            handshake_timeout,
                            heartbeat,
                        )
                        .await;
                    });
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "p2p: failed to accept inbound connection");
                }
            }
        }
    }
}

/// Admits or rejects a single inbound connection based on the allow-list, then
/// negotiates the control stream and only afterwards serves data streams.
///
/// Stream ordering contract: the receiver must open the control stream as the
/// first bidirectional stream on the connection. The forwarder accepts that
/// first stream as control, completes `Hello`/catalog negotiation, and only
/// then begins accepting subsequent bidirectional streams as data subscriptions.
/// Data is never served before negotiation succeeds.
///
/// Lifecycle: any control-loop termination (clean close, handshake/heartbeat
/// failure) closes the QUIC connection, which ends the data accept loop and
/// every in-flight data stream. The data task is awaited before returning so no
/// data stream outlives the allow-list connection guard (`_guard`).
async fn handle_connection(
    connection: Connection,
    allow_list: AllowList,
    catalog: Arc<dyn CatalogProvider>,
    journal: Arc<Mutex<Journal>>,
    data_config: DataConfig,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
) {
    let Ok(node_id) = connection.remote_node_id() else {
        tracing::warn!("p2p: rejecting connection without a remote node id");
        connection.close(REJECT_ERROR_CODE.into(), b"missing remote node id");
        return;
    };

    let Some(_guard) = allow_list.try_register_connection(node_id, connection.clone()) else {
        tracing::warn!(%node_id, "p2p: rejecting peer not on allow-list");
        connection.close(REJECT_ERROR_CODE.into(), b"unauthorized peer");
        return;
    };

    tracing::info!(%node_id, "p2p: admitted allow-listed peer");
    let (control_send, control_recv) =
        match tokio::time::timeout(handshake_timeout, connection.accept_bi()).await {
            Ok(Ok(streams)) => streams,
            Ok(Err(error)) => {
                tracing::warn!(%node_id, %error, "p2p: failed to accept control stream");
                connection.close(CONTROL_ERROR_CODE.into(), b"control stream failed");
                return;
            }
            Err(_elapsed) => {
                tracing::warn!(%node_id, "p2p: control stream accept timed out");
                connection.close(CONTROL_ERROR_CODE.into(), b"control stream timed out");
                return;
            }
        };

    // Gate data delivery on a completed control handshake: negotiate Hello and
    // serve the catalog before any data stream is accepted.
    let (control_send, control_recv) = match negotiate_control_stream(
        control_send,
        control_recv,
        catalog.as_ref(),
        handshake_timeout,
        heartbeat,
    )
    .await
    {
        Ok(streams) => streams,
        Err(error) => {
            tracing::warn!(%node_id, %error, "p2p: control handshake failed");
            connection.close(CONTROL_ERROR_CODE.into(), b"control handshake failed");
            return;
        }
    };

    // Handshake succeeded: now safe to serve data subscriptions concurrently
    // with the heartbeat/control loop.
    let data_connection = connection.clone();
    let data_task = tokio::spawn(async move {
        if let Err(error) =
            serve_data_streams(data_connection, journal, node_id.to_string(), data_config).await
        {
            tracing::debug!(%node_id, %error, "p2p: data stream accept loop ended");
        }
    });

    let control_result = run_control_stream_loop(control_send, control_recv, heartbeat).await;
    match &control_result {
        Ok(()) => tracing::info!(%node_id, "p2p: control stream closed by peer"),
        Err(error) => tracing::warn!(%node_id, %error, "p2p: control stream failed"),
    }

    // Any control-loop exit closes the connection, which ends the data accept
    // loop and all in-flight data streams. Await the data task so no data stream
    // outlives the control lifecycle or the allow-list guard dropped below.
    connection.close(CONTROL_ERROR_CODE.into(), b"control stream closed");
    let _ = data_task.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::p2p::control::{
        PROTOCOL_MINOR, StaticCatalog, forwarder_hello, read_frame, write_frame,
    };
    use rt_p2p_protocol::{
        ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, HelloOk, StreamCatalog,
        SubscribeMode, control_c2f, control_f2c, data_c2f, data_f2c,
    };

    type BoxError = Box<dyn std::error::Error + Send + Sync>;
    type TestResult = Result<(), BoxError>;

    /// Reader-address journal key seeded into every test endpoint's journal so
    /// data subscriptions have stream state to resolve against.
    const DATA_STREAM_KEY: &str = "10.0.0.7:10000";

    fn empty_catalog() -> Arc<dyn CatalogProvider> {
        Arc::new(StaticCatalog::new(StreamCatalog::default()))
    }

    impl P2pEndpoint {
        /// Binds a hermetic loopback endpoint seeded with `seed` for tests.
        async fn bind_test(seed: [u8; 32], allow_list: AllowList) -> Result<Self, rt_iroh::Error> {
            let endpoint = EndpointBuilder::test(seed).bind().await?;
            let tempdir = Arc::new(tempfile::tempdir().expect("tempdir"));
            let journal_path = tempdir.path().join("journal.sqlite3");
            let mut journal = Journal::open(&journal_path).expect("open journal");
            journal
                .ensure_stream_state(DATA_STREAM_KEY, 1)
                .expect("seed data stream state");
            Ok(Self {
                endpoint,
                allow_list,
                catalog: empty_catalog(),
                journal: Arc::new(Mutex::new(journal)),
                data_config: DataConfig::default(),
                handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
                heartbeat: HeartbeatConfig::default(),
                _test_tempdir: Some(tempdir),
            })
        }

        /// Overrides the handshake timeout, keeping stalled-peer tests fast and
        /// deterministic.
        fn with_handshake_timeout(mut self, handshake_timeout: Duration) -> Self {
            self.handshake_timeout = handshake_timeout;
            self
        }
    }

    /// Dials `forwarder_addr`, opens the control stream, sends `Hello`, and
    /// returns the negotiated `HelloOk`.
    async fn dial_hello(
        receiver: &Endpoint,
        forwarder_addr: NodeAddr,
    ) -> Result<HelloOk, BoxError> {
        let (_connection, hello_ok, _control_send, _control_recv) =
            dial_hello_connection(receiver, forwarder_addr).await?;
        Ok(hello_ok)
    }

    async fn dial_hello_connection(
        receiver: &Endpoint,
        forwarder_addr: NodeAddr,
    ) -> Result<
        (
            Connection,
            HelloOk,
            rt_iroh::SendStream,
            rt_iroh::RecvStream,
        ),
        BoxError,
    > {
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;

        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(forwarder_hello())),
            },
        )
        .await?;

        let response = read_frame::<ControlF2C>(&mut recv).await?;
        let hello_ok = match response.msg {
            Some(control_f2c::Msg::HelloOk(hello_ok)) => hello_ok,
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        };
        let catalog = read_frame::<ControlF2C>(&mut recv).await?;
        match catalog.msg {
            Some(control_f2c::Msg::StreamCatalog(_)) => Ok((connection, hello_ok, send, recv)),
            other => Err(format!("expected StreamCatalog, got {other:?}").into()),
        }
    }

    async fn subscribe_data(connection: &Connection) -> Result<DataF2C, BoxError> {
        let (send, mut recv) = open_data_subscribe(connection).await?;
        drop(send);
        tokio::time::timeout(Duration::from_secs(1), read_frame::<DataF2C>(&mut recv)).await?
    }

    /// Opens a data stream and sends a `DataSubscribe` for the seeded stream,
    /// returning the stream halves so callers can keep them open.
    async fn open_data_subscribe(
        connection: &Connection,
    ) -> Result<(rt_iroh::SendStream, rt_iroh::RecvStream), BoxError> {
        let (mut send, recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id: DATA_STREAM_KEY.as_bytes().to_vec(),
                    after_seq: 0,
                    mode: SubscribeMode::Replay as i32,
                })),
            },
        )
        .await?;
        Ok((send, recv))
    }

    #[tokio::test]
    async fn accepts_allowlisted_peer() -> TestResult {
        let receiver = EndpointBuilder::test([20; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([21; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let hello_ok = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello(&receiver, forwarder_addr),
        )
        .await??;
        assert_eq!(hello_ok.protocol_minor, PROTOCOL_MINOR);

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn serves_data_streams_after_control_handshake_on_same_connection() -> TestResult {
        let receiver = EndpointBuilder::test([26; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([27; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (connection, hello_ok, _control_send, _control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection(&receiver, forwarder_addr),
        )
        .await??;
        assert_eq!(hello_ok.protocol_minor, PROTOCOL_MINOR);

        let frame = subscribe_data(&connection).await?;
        match frame.msg {
            Some(data_f2c::Msg::SubscribeOk(_)) => {}
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        }

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn data_not_served_before_control_handshake() -> TestResult {
        let receiver = EndpointBuilder::test([28; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([29; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;

        // Open the control stream first (per the ordering contract) but do NOT
        // send Hello yet, so negotiation cannot complete.
        let (mut control_send, mut control_recv) = connection.open_bi().await?;

        // Open a data stream and subscribe before negotiating control.
        let (_data_send, mut data_recv) = open_data_subscribe(&connection).await?;

        // The forwarder must not serve data before the handshake: no SubscribeOk
        // (or any data frame) should arrive while control is un-negotiated.
        let premature = tokio::time::timeout(
            Duration::from_millis(750),
            read_frame::<DataF2C>(&mut data_recv),
        )
        .await;
        assert!(
            premature.is_err(),
            "data must not be served before the control handshake completes, got {premature:?}"
        );

        // Now negotiate control: send Hello, read HelloOk + catalog.
        write_frame(
            &mut control_send,
            &ControlC2F {
                msg: Some(control_c2f::Msg::Hello(forwarder_hello())),
            },
        )
        .await?;
        match read_frame::<ControlF2C>(&mut control_recv).await?.msg {
            Some(control_f2c::Msg::HelloOk(_)) => {}
            other => return Err(format!("expected HelloOk, got {other:?}").into()),
        }
        match read_frame::<ControlF2C>(&mut control_recv).await?.msg {
            Some(control_f2c::Msg::StreamCatalog(_)) => {}
            other => return Err(format!("expected StreamCatalog, got {other:?}").into()),
        }

        // Post-negotiation, the already-open data subscription must now be served.
        let frame = tokio::time::timeout(
            Duration::from_secs(5),
            read_frame::<DataF2C>(&mut data_recv),
        )
        .await??;
        match frame.msg {
            Some(data_f2c::Msg::SubscribeOk(_)) => {}
            other => {
                return Err(format!("expected SubscribeOk after handshake, got {other:?}").into());
            }
        }

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn data_stream_terminates_when_control_stream_closes() -> TestResult {
        let receiver = EndpointBuilder::test([30; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([31; 32], allow_list).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let (connection, hello_ok, control_send, control_recv) = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello_connection(&receiver, forwarder_addr),
        )
        .await??;
        assert_eq!(hello_ok.protocol_minor, PROTOCOL_MINOR);

        // Establish a live data subscription.
        let (_data_send, mut data_recv) = open_data_subscribe(&connection).await?;
        match tokio::time::timeout(
            Duration::from_secs(5),
            read_frame::<DataF2C>(&mut data_recv),
        )
        .await??
        .msg
        {
            Some(data_f2c::Msg::SubscribeOk(_)) => {}
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        }

        // Close the control stream cleanly. The forwarder's control loop should
        // observe the close, tear down the connection, and end the data stream.
        drop(control_send);
        drop(control_recv);

        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if read_frame::<DataF2C>(&mut data_recv).await.is_err() {
                    break;
                }
            }
        })
        .await;
        assert!(
            closed.is_ok(),
            "data stream must terminate once the control stream closes"
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn stalled_handshake_is_timed_out() -> TestResult {
        let receiver = EndpointBuilder::test([24; 32]).bind().await?;
        let allow_list = AllowList::new([receiver.node_id()]);
        let forwarder = P2pEndpoint::bind_test([25; 32], allow_list)
            .await?
            .with_handshake_timeout(Duration::from_millis(200));
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        // Connect and open the control stream as an allow-listed peer, but never
        // send `Hello`. The forwarder must close the connection on its own once
        // the handshake deadline elapses rather than leaking the task/connection.
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;
        let (mut _send, mut recv) = connection.open_bi().await?;

        // A read that would otherwise block forever must observe the connection
        // being closed promptly after the (short) handshake timeout.
        let mut buf = [0u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(5), recv.read_exact(&mut buf)).await;
        assert!(
            closed.is_ok(),
            "stalled handshake must be closed before the outer timeout elapses"
        );
        assert!(
            closed.unwrap().is_err(),
            "forwarder must close the stream after the handshake timeout, not deliver data"
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unknown_peer() -> TestResult {
        let receiver = EndpointBuilder::test([22; 32]).bind().await?;
        let forwarder = P2pEndpoint::bind_test([23; 32], AllowList::default()).await?;
        let forwarder_addr = forwarder.node_addr().await;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.run().await })
        };

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            dial_hello(&receiver, forwarder_addr),
        )
        .await?;
        assert!(
            result.is_err(),
            "unknown peer must be rejected, got {result:?}"
        );

        accept.abort();
        receiver.close().await;
        forwarder.endpoint().close().await;
        Ok(())
    }
}

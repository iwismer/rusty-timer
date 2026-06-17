//! Forwarder iroh endpoint and inbound accept loop.
//!
//! [`P2pEndpoint`] wraps an [`rt_iroh::Endpoint`] and serves inbound receiver
//! connections. For each accepted connection the remote node id is read from
//! the transport handshake and checked against an [`AllowList`]; unknown peers
//! are closed immediately. Admitted peers are handed to the control-stream
//! handler ([`crate::p2p::control`]), which performs the `Hello`/`HelloOk`
//! negotiation, serves the [`StreamCatalog`](rt_p2p_protocol::StreamCatalog),
//! and runs the heartbeat until the peer disconnects or is declared dead.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rt_iroh::{Connection, Endpoint, EndpointBuilder, NodeAddr, NodeId, load_or_create_secret_key};

use super::control::{CatalogProvider, HeartbeatConfig, serve_control};

/// QUIC application error code used when closing a rejected connection.
const REJECT_ERROR_CODE: u32 = 1;

/// QUIC application error code used when closing a connection whose control
/// stream failed, timed out, or whose peer was declared dead by the heartbeat.
const CONTROL_ERROR_CODE: u32 = 2;

/// Maximum time an admitted peer is given to complete the control-plane
/// `Hello` handshake before the connection is closed. Bounds the lifetime of
/// broken or malicious allow-listed peers that connect but never make progress.
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// In-memory set of peer node ids allowed to connect.
///
/// This is deliberately minimal for the accept-loop task; the persistent
/// allow-list and revocation are implemented separately.
#[derive(Clone, Debug, Default)]
pub struct AllowList {
    allowed: Arc<HashSet<NodeId>>,
}

impl AllowList {
    /// Builds an allow-list from the given peer node ids.
    #[must_use]
    pub fn new(allowed: impl IntoIterator<Item = NodeId>) -> Self {
        Self {
            allowed: Arc::new(allowed.into_iter().collect()),
        }
    }

    /// Returns whether `node_id` is permitted to connect.
    #[must_use]
    pub fn contains(&self, node_id: &NodeId) -> bool {
        self.allowed.contains(node_id)
    }
}

/// The forwarder's P2P endpoint and accept loop.
#[derive(Clone, Debug)]
pub struct P2pEndpoint {
    endpoint: Endpoint,
    allow_list: AllowList,
    catalog: Arc<dyn CatalogProvider>,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
}

impl P2pEndpoint {
    /// Binds the endpoint, loading or creating the persistent secret key at
    /// `secret_key_path`. Admitted peers are served the catalog from `catalog`.
    pub async fn bind(
        secret_key_path: impl AsRef<Path>,
        allow_list: AllowList,
        catalog: Arc<dyn CatalogProvider>,
    ) -> Result<Self, rt_iroh::Error> {
        let secret_key = load_or_create_secret_key(secret_key_path)?;
        let endpoint = EndpointBuilder::default()
            .secret_key(secret_key)
            .bind()
            .await?;
        Ok(Self {
            endpoint,
            allow_list,
            catalog,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            heartbeat: HeartbeatConfig::default(),
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
                    let handshake_timeout = self.handshake_timeout;
                    let heartbeat = self.heartbeat;
                    tokio::spawn(async move {
                        handle_connection(
                            connection,
                            allow_list,
                            catalog,
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
/// serves the control stream for admitted peers.
async fn handle_connection(
    connection: Connection,
    allow_list: AllowList,
    catalog: Arc<dyn CatalogProvider>,
    handshake_timeout: Duration,
    heartbeat: HeartbeatConfig,
) {
    let Ok(node_id) = connection.remote_node_id() else {
        tracing::warn!("p2p: rejecting connection without a remote node id");
        connection.close(REJECT_ERROR_CODE.into(), b"missing remote node id");
        return;
    };

    if !allow_list.contains(&node_id) {
        tracing::warn!(%node_id, "p2p: rejecting peer not on allow-list");
        connection.close(REJECT_ERROR_CODE.into(), b"unauthorized peer");
        return;
    }

    tracing::info!(%node_id, "p2p: admitted allow-listed peer");
    match serve_control(&connection, catalog.as_ref(), handshake_timeout, heartbeat).await {
        Ok(()) => {
            tracing::info!(%node_id, "p2p: control stream closed by peer");
        }
        Err(error) => {
            tracing::warn!(%node_id, %error, "p2p: control stream failed");
            connection.close(CONTROL_ERROR_CODE.into(), b"control stream failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::p2p::control::{
        PROTOCOL_MINOR, StaticCatalog, forwarder_hello, read_frame, write_frame,
    };
    use rt_p2p_protocol::{
        ControlC2F, ControlF2C, HelloOk, StreamCatalog, control_c2f, control_f2c,
    };

    type BoxError = Box<dyn std::error::Error + Send + Sync>;
    type TestResult = Result<(), BoxError>;

    fn empty_catalog() -> Arc<dyn CatalogProvider> {
        Arc::new(StaticCatalog::new(StreamCatalog::default()))
    }

    impl P2pEndpoint {
        /// Binds a hermetic loopback endpoint seeded with `seed` for tests.
        async fn bind_test(seed: [u8; 32], allow_list: AllowList) -> Result<Self, rt_iroh::Error> {
            let endpoint = EndpointBuilder::test(seed).bind().await?;
            Ok(Self {
                endpoint,
                allow_list,
                catalog: empty_catalog(),
                handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
                heartbeat: HeartbeatConfig::default(),
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
        match response.msg {
            Some(control_f2c::Msg::HelloOk(hello_ok)) => Ok(hello_ok),
            other => Err(format!("expected HelloOk, got {other:?}").into()),
        }
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

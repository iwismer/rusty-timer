use std::{
    fs,
    io::{self, ErrorKind},
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
};

pub use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{
    Endpoint as IrohEndpoint, Watcher,
    endpoint::{QuicTransportConfig, presets},
};
// iroh 1.0 renamed `NodeId`/`NodeAddr` to `EndpointId`/`EndpointAddr`. The
// rest of the workspace keeps the node-oriented names via these re-exports.
pub use iroh::{
    EndpointAddr as NodeAddr, EndpointId as NodeId, RelayMode, SecretKey, TransportAddr,
};

pub const ALPN: &[u8] = b"rusty-timer/fwd-rcv/1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("secret key file must contain exactly 32 bytes, found {len}")]
    InvalidSecretKeyLength { len: usize },
    #[error("io error")]
    Io(#[from] io::Error),
    #[error("failed to bind iroh endpoint")]
    Bind(#[source] Box<iroh::endpoint::BindError>),
    #[error("failed to connect iroh endpoint")]
    Connect(#[source] Box<iroh::endpoint::ConnectError>),
    #[error("failed to accept incoming connection")]
    Accept(#[source] Box<iroh::endpoint::ConnectingError>),
}

impl From<iroh::endpoint::BindError> for Error {
    fn from(source: iroh::endpoint::BindError) -> Self {
        Self::Bind(Box::new(source))
    }
}

impl From<iroh::endpoint::ConnectError> for Error {
    fn from(source: iroh::endpoint::ConnectError) -> Self {
        Self::Connect(Box::new(source))
    }
}

impl From<iroh::endpoint::ConnectingError> for Error {
    fn from(source: iroh::endpoint::ConnectingError) -> Self {
        Self::Accept(Box::new(source))
    }
}

pub fn load_or_create_secret_key(path: impl AsRef<Path>) -> Result<SecretKey, Error> {
    let path = path.as_ref();
    match fs::read(path) {
        Ok(bytes) => {
            // Harden an existing key file that is group/world-accessible (e.g.
            // created before 0600 enforcement, restored from backup, or copied
            // with loose permissions). Best-effort: failing to tighten does not
            // block loading the key.
            #[cfg(unix)]
            tighten_secret_key_permissions(path);
            secret_key_from_bytes(&bytes)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => create_secret_key(path),
        Err(error) => Err(error.into()),
    }
}

/// Restrict an existing key file to owner-only read/write (`0600`) when any
/// group/other permission bit is set.
#[cfg(unix)]
fn tighten_secret_key_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.permissions().mode() & 0o077 != 0 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

fn secret_key_from_bytes(bytes: &[u8]) -> Result<SecretKey, Error> {
    let key_bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::InvalidSecretKeyLength { len: bytes.len() })?;
    Ok(SecretKey::from_bytes(&key_bytes))
}

fn create_secret_key(path: &Path) -> Result<SecretKey, Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let key = SecretKey::generate();
    let bytes = key.to_bytes();

    match write_new_secret_key(path, &bytes) {
        Ok(()) => Ok(key),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let bytes = fs::read(path)?;
            secret_key_from_bytes(&bytes)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn write_new_secret_key(path: &Path, bytes: &[u8; 32]) -> io::Result<()> {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_new_secret_key(path: &Path, bytes: &[u8; 32]) -> io::Result<()> {
    use std::{fs::OpenOptions, io::Write};

    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)
}

#[derive(Clone, Debug)]
pub struct Endpoint {
    inner: IrohEndpoint,
}

impl Endpoint {
    pub fn builder() -> EndpointBuilder {
        EndpointBuilder::default()
    }

    pub async fn connect(&self, node_addr: impl Into<NodeAddr>) -> Result<Connection, Error> {
        Ok(self.inner.connect(node_addr, ALPN).await?)
    }

    pub async fn accept(&self) -> Result<Option<Connection>, Error> {
        match self.inner.accept().await {
            Some(incoming) => Ok(Some(incoming.await?)),
            None => Ok(None),
        }
    }

    /// Returns the endpoint's address, waiting until it contains at least one
    /// transport address (iroh populates bound socket addresses shortly after
    /// binding).
    pub async fn node_addr(&self) -> NodeAddr {
        let mut watcher = self.inner.watch_addr();
        loop {
            let addr = watcher.get();
            if !addr.addrs.is_empty() {
                return addr;
            }
            if watcher.updated().await.is_err() {
                return self.inner.addr();
            }
        }
    }

    pub fn node_id(&self) -> NodeId {
        self.inner.id()
    }

    pub async fn close(&self) {
        self.inner.close().await;
    }
}

#[derive(Debug)]
pub struct EndpointBuilder {
    inner: iroh::endpoint::Builder,
}

impl Default for EndpointBuilder {
    fn default() -> Self {
        Self {
            inner: IrohEndpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]),
        }
    }
}

impl EndpointBuilder {
    pub fn test(seed: [u8; 32]) -> Self {
        Self::default()
            .secret_key(SecretKey::from_bytes(&seed))
            .relay_mode(RelayMode::Disabled)
            .clear_discovery()
            .bind_addr_v4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .max_concurrent_bidi_streams(256)
    }

    #[must_use]
    pub fn secret_key(mut self, secret_key: SecretKey) -> Self {
        self.inner = self.inner.secret_key(secret_key);
        self
    }

    #[must_use]
    pub fn bind_addr_v4(mut self, bind_addr: SocketAddrV4) -> Self {
        self.inner = self
            .inner
            .clear_ip_transports()
            .bind_addr(bind_addr)
            .expect("a SocketAddrV4 is always a valid bind address");
        self
    }

    #[must_use]
    pub fn relay_mode(mut self, relay_mode: RelayMode) -> Self {
        self.inner = self.inner.relay_mode(relay_mode);
        self
    }

    #[must_use]
    pub fn clear_discovery(mut self) -> Self {
        self.inner = self.inner.clear_address_lookup();
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn insecure_skip_relay_cert_verify(mut self, skip_verify: bool) -> Self {
        if skip_verify {
            self.inner = self
                .inner
                .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify());
        }
        self
    }

    #[must_use]
    pub fn max_concurrent_bidi_streams(mut self, max_streams: u32) -> Self {
        let transport_config = QuicTransportConfig::builder()
            .max_concurrent_bidi_streams(max_streams.into())
            .build();
        self.inner = self.inner.transport_config(transport_config);
        self
    }

    pub async fn bind(self) -> Result<Endpoint, Error> {
        Ok(Endpoint {
            inner: self.inner.bind().await?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_key_persists_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");

        let first = load_or_create_secret_key(&path).unwrap();
        let second = load_or_create_secret_key(&path).unwrap();

        assert_eq!(first.to_bytes(), second.to_bytes());
    }

    #[tokio::test]
    async fn test_builder_two_endpoints_connect_loopback() {
        let endpoint_a = EndpointBuilder::test([1; 32]).bind().await.unwrap();
        let endpoint_b = EndpointBuilder::test([2; 32]).bind().await.unwrap();
        let endpoint_b_addr = endpoint_b.node_addr().await;

        let (connected, accepted) = tokio::join!(
            endpoint_a.connect(endpoint_b_addr.clone()),
            endpoint_b.accept(),
        );

        let connected = connected.unwrap();
        let accepted = accepted.unwrap().unwrap();

        assert_eq!(connected.remote_id(), endpoint_b_addr.id);
        assert_eq!(accepted.remote_id(), endpoint_a.node_id());

        endpoint_a.close().await;
        endpoint_b.close().await;
    }

    #[tokio::test]
    #[ignore = "T6.2 non-PR connectivity chaos lane: starts a local iroh relay"]
    async fn local_relay_ping_non_pr_lane() {
        let (relay_map, relay_url, _relay_guard) =
            iroh::test_utils::run_relay_server().await.unwrap();

        let endpoint_a = EndpointBuilder::default()
            .secret_key(SecretKey::from_bytes(&[3; 32]))
            .relay_mode(RelayMode::Custom(relay_map.clone()))
            .insecure_skip_relay_cert_verify(true)
            .clear_discovery()
            .bind_addr_v4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .max_concurrent_bidi_streams(256)
            .bind()
            .await
            .unwrap();
        let endpoint_b = EndpointBuilder::default()
            .secret_key(SecretKey::from_bytes(&[4; 32]))
            .relay_mode(RelayMode::Custom(relay_map))
            .insecure_skip_relay_cert_verify(true)
            .clear_discovery()
            .bind_addr_v4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .max_concurrent_bidi_streams(256)
            .bind()
            .await
            .unwrap();

        endpoint_b.node_addr().await;
        let relay_only_addr = NodeAddr::new(endpoint_b.node_id()).with_relay_url(relay_url);

        let (connected, accepted) =
            tokio::join!(endpoint_a.connect(relay_only_addr), endpoint_b.accept(),);

        let connected = connected.unwrap();
        let accepted = accepted.unwrap().unwrap();
        assert_eq!(connected.remote_id(), endpoint_b.node_id());
        assert_eq!(accepted.remote_id(), endpoint_a.node_id());

        endpoint_a.close().await;
        endpoint_b.close().await;
    }

    #[tokio::test]
    #[ignore = "T6.2 non-PR connectivity chaos lane: starts a local iroh relay"]
    async fn relay_loss_uses_last_known_direct_addr() {
        let (relay_map, _relay_url, relay_guard) =
            iroh::test_utils::run_relay_server().await.unwrap();

        let endpoint_a = EndpointBuilder::default()
            .secret_key(SecretKey::from_bytes(&[5; 32]))
            .relay_mode(RelayMode::Custom(relay_map.clone()))
            .insecure_skip_relay_cert_verify(true)
            .clear_discovery()
            .bind_addr_v4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .max_concurrent_bidi_streams(256)
            .bind()
            .await
            .unwrap();
        let endpoint_b = EndpointBuilder::default()
            .secret_key(SecretKey::from_bytes(&[6; 32]))
            .relay_mode(RelayMode::Custom(relay_map))
            .insecure_skip_relay_cert_verify(true)
            .clear_discovery()
            .bind_addr_v4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .max_concurrent_bidi_streams(256)
            .bind()
            .await
            .unwrap();

        let last_known_addr = endpoint_b.node_addr().await;
        let direct_addrs: Vec<TransportAddr> = last_known_addr
            .addrs
            .iter()
            .filter(|addr| addr.is_ip())
            .cloned()
            .collect();
        assert!(
            !direct_addrs.is_empty(),
            "last-known address must include a direct loopback fallback: {last_known_addr:?}"
        );
        let direct_fallback_addr = NodeAddr::from_parts(endpoint_b.node_id(), direct_addrs);
        drop(relay_guard);

        let (connected, accepted) = tokio::join!(
            endpoint_a.connect(direct_fallback_addr),
            endpoint_b.accept(),
        );

        let connected = connected.unwrap();
        let accepted = accepted.unwrap().unwrap();
        assert_eq!(connected.remote_id(), endpoint_b.node_id());
        assert_eq!(accepted.remote_id(), endpoint_a.node_id());

        endpoint_a.close().await;
        endpoint_b.close().await;
    }
}

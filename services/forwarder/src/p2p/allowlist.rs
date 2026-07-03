//! Persistent receiver allow-list with revocation for the forwarder P2P endpoint.
//!
//! [`AllowList`] gates inbound receiver connections by their iroh endpoint id (the
//! transport-layer `EndpointId`). It keeps three concerns together so the
//! accept loop can enforce them at the connection boundary:
//!
//! 1. **Authoritative in-memory set.** [`AllowList::try_register_connection`]
//!    admits and tracks a connection under one lock, before any control or data
//!    stream work happens.
//! 2. **Last-known persistence.** The server-snapshot portion of the allowed
//!    set is cached on disk (pinned/static receivers are re-derived from
//!    config at startup, never persisted) so a refresh failure (e.g. an
//!    offline server) falls back to the previously persisted list rather than
//!    failing open or failing closed-empty. Updates persist *before* the
//!    in-memory swap, so a write failure leaves the last-known set in force
//!    ([`AllowList::apply_update`]).
//! 3. **Revocation / force-close.** Admitted connections are tracked by remote
//!    endpoint id ([`AllowList::try_register_connection`]); when an update removes a
//!    peer, its open connections are force-closed immediately.
//!
//! Server refresh and catalog HTTP clients live in the sibling
//! `server_client` module; this module owns only the admission state machine and
//! last-known cache.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rt_iroh::{Connection, EndpointId};

/// QUIC application error code used when force-closing a revoked peer's
/// connection after it is removed from the allow-list.
const REVOKED_ERROR_CODE: u32 = 3;

/// Shared, mutable allow-list state guarded by a single mutex.
#[derive(Debug, Default)]
struct State {
    /// Endpoint ids currently permitted to connect.
    allowed: HashSet<EndpointId>,
    /// Operator-pinned (static config) receivers. Always allowed; never
    /// revoked or persisted by server snapshots.
    pinned: HashSet<EndpointId>,
    /// Open admitted connections, keyed by remote endpoint id then a per-connection
    /// registration id so multiple connections from one peer are tracked
    /// independently.
    connections: HashMap<EndpointId, HashMap<u64, Connection>>,
    /// Monotonic id allocator for connection registrations.
    next_conn_id: u64,
}

/// Persistent, revocable allow-list of receiver endpoint ids.
#[derive(Clone, Debug, Default)]
pub struct AllowList {
    state: Arc<Mutex<State>>,
    /// On-disk cache of the last-known allowed set, if persistence is enabled.
    cache_path: Option<Arc<PathBuf>>,
}

impl AllowList {
    /// Builds an in-memory allow-list (no persistence) from the given endpoint ids.
    #[must_use]
    pub fn new(allowed: impl IntoIterator<Item = EndpointId>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                allowed: allowed.into_iter().collect(),
                ..State::default()
            })),
            cache_path: None,
        }
    }

    /// Loads the allow-list from the on-disk cache at `path`, binding future
    /// updates to persist there.
    ///
    /// A missing cache file yields an empty allow-list; this is the only
    /// fail-closed case (first boot before any list has been distributed). When
    /// a later refresh fails, callers keep using the loaded (last-known) set
    /// rather than reloading, so an offline source never empties the list.
    ///
    /// # Errors
    ///
    /// Returns an error if the cache file exists but cannot be read or contains
    /// a line that is not a valid endpoint id.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let allowed = match fs::read_to_string(&path) {
            Ok(contents) => parse_endpoint_ids(&contents)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => HashSet::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                allowed,
                ..State::default()
            })),
            cache_path: Some(Arc::new(path)),
        })
    }

    /// Returns whether `endpoint_id` is currently permitted to connect.
    #[must_use]
    pub fn contains(&self, endpoint_id: &EndpointId) -> bool {
        self.lock().allowed.contains(endpoint_id)
    }

    /// Pins operator-configured receivers: always allowed, never revoked by
    /// [`AllowList::apply_update`], never persisted to the server-snapshot
    /// cache. Intended to be called once at startup; the pinned set itself is
    /// replaced immediately, but endpoints removed from it remain allowed
    /// until the next `apply_update`.
    pub fn set_pinned(&self, pinned: impl IntoIterator<Item = EndpointId>) {
        let mut state = self.lock();
        let state = &mut *state;
        state.pinned = pinned.into_iter().collect();
        state.allowed.extend(state.pinned.iter().copied());
    }

    /// Atomically replaces the allowed set with `allowed` ∪ the pinned set,
    /// persisting the server snapshot (`allowed` only, never pinned) and
    /// force-closing every open connection whose endpoint id was removed.
    ///
    /// Returns the revoked endpoint ids; pinned endpoints are never revoked.
    /// Persistence happens before the in-memory swap: if the cache cannot be
    /// written the in-memory state is left at its last-known value and the
    /// error is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence is enabled and the cache file cannot be
    /// written.
    pub fn apply_update(
        &self,
        allowed: impl IntoIterator<Item = EndpointId>,
    ) -> io::Result<Vec<EndpointId>> {
        let new_allowed: HashSet<EndpointId> = allowed.into_iter().collect();
        let (revoked, connections_to_close) = {
            let mut state = self.lock();
            // Persist before swapping in memory while holding the state lock so
            // concurrent updates cannot interleave disk and memory changes.
            // If the cache write fails, the last-known set stays in force.
            if let Some(path) = &self.cache_path {
                persist(path, &new_allowed)?;
            }

            let effective: HashSet<EndpointId> =
                new_allowed.union(&state.pinned).copied().collect();
            let revoked: Vec<EndpointId> =
                state.allowed.difference(&effective).copied().collect();
            state.allowed = effective;
            let mut connections_to_close = Vec::new();
            for endpoint_id in &revoked {
                if let Some(connections) = state.connections.remove(endpoint_id) {
                    connections_to_close.extend(connections.into_values());
                }
            }
            (revoked, connections_to_close)
        };

        for connection in connections_to_close {
            connection.close(REVOKED_ERROR_CODE.into(), b"revoked");
        }
        Ok(revoked)
    }

    /// Registers `connection` only if `endpoint_id` is still allowed, returning a
    /// guard that deregisters the connection when dropped.
    #[must_use]
    pub(crate) fn try_register_connection(
        &self,
        endpoint_id: EndpointId,
        connection: Connection,
    ) -> Option<ConnectionGuard> {
        let mut state = self.lock();
        if !state.allowed.contains(&endpoint_id) {
            return None;
        }
        let id = state.next_conn_id;
        state.next_conn_id += 1;
        state
            .connections
            .entry(endpoint_id)
            .or_default()
            .insert(id, connection);
        Some(ConnectionGuard {
            allow_list: self.clone(),
            endpoint_id,
            id,
        })
    }

    fn deregister(&self, endpoint_id: EndpointId, id: u64) {
        let mut state = self.lock();
        if let Some(connections) = state.connections.get_mut(&endpoint_id) {
            connections.remove(&id);
            if connections.is_empty() {
                state.connections.remove(&endpoint_id);
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock means another thread panicked mid-update; the
        // allow-list set itself is still consistent, so recover rather than
        // cascade the panic into every admission check.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Deregisters a tracked connection when dropped, keeping the connection
/// registry bounded to currently-open admitted connections.
#[must_use = "dropping the guard immediately deregisters the tracked connection"]
pub(crate) struct ConnectionGuard {
    allow_list: AllowList,
    endpoint_id: EndpointId,
    id: u64,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.allow_list.deregister(self.endpoint_id, self.id);
    }
}

/// Parses a newline-delimited list of endpoint ids, ignoring blank lines.
fn parse_endpoint_ids(contents: &str) -> io::Result<HashSet<EndpointId>> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            EndpointId::from_str(line)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
        })
        .collect()
}

/// Writes `allowed` to `path` via a write-then-rename so a crash never leaves a
/// partially written cache. Ids are sorted for a deterministic on-disk form.
fn persist(path: &Path, allowed: &HashSet<EndpointId>) -> io::Result<()> {
    let mut ids: Vec<String> = allowed.iter().map(ToString::to_string).collect();
    ids.sort();
    let mut contents = ids.join("\n");
    if !contents.is_empty() {
        contents.push('\n');
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use rt_iroh::{Endpoint, EndpointBuilder, EndpointAddr};

    type BoxError = Box<dyn std::error::Error + Send + Sync>;
    type TestResult = Result<(), BoxError>;

    /// Probe bytes a test "receiver" sends to open a subscription.
    const SUBSCRIBE: &[u8; 4] = b"SUB?";
    /// Bytes the admission server returns once a subscription is accepted.
    const SUBSCRIBE_OK: &[u8; 4] = b"SUBK";

    /// Minimal forwarder accept loop that mirrors production admission: reject
    /// peers absent from `allow`, register admitted connections for revocation,
    /// and answer a subscription probe so a successful subscribe is observable.
    async fn admission_server(forwarder: Endpoint, allow: AllowList) {
        loop {
            let connection = match forwarder.accept().await {
                Ok(Some(connection)) => connection,
                _ => return,
            };
            let allow = allow.clone();
            tokio::spawn(async move {
                let endpoint_id = connection.remote_id();
                let Some(_guard) = allow.try_register_connection(endpoint_id, connection.clone())
                else {
                    connection.close(1u32.into(), b"denied");
                    return;
                };
                while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                    let mut buf = [0u8; SUBSCRIBE.len()];
                    if recv.read_exact(&mut buf).await.is_err() {
                        break;
                    }
                    if &buf == SUBSCRIBE {
                        let _ = send.write_all(SUBSCRIBE_OK).await;
                        let _ = send.finish();
                    }
                }
            });
        }
    }

    /// Dials `forwarder_addr`, opens a stream, sends the subscription probe, and
    /// returns the connection plus the server's reply bytes.
    async fn subscribe(
        receiver: &Endpoint,
        forwarder_addr: EndpointAddr,
    ) -> Result<(Connection, [u8; 4]), BoxError> {
        let connection = receiver.connect(forwarder_addr).await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(SUBSCRIBE).await?;
        send.finish()?;
        let mut buf = [0u8; SUBSCRIBE_OK.len()];
        recv.read_exact(&mut buf).await?;
        Ok((connection, buf))
    }

    #[tokio::test]
    async fn denied_peer_cannot_subscribe() -> TestResult {
        let receiver = EndpointBuilder::test([40; 32]).bind().await?;
        let forwarder = EndpointBuilder::test([41; 32]).bind().await?;
        let forwarder_addr = forwarder.endpoint_addr().await;

        // Empty allow-list: the receiver is not authorized.
        let server = tokio::spawn(admission_server(forwarder.clone(), AllowList::default()));

        let result =
            tokio::time::timeout(Duration::from_secs(5), subscribe(&receiver, forwarder_addr))
                .await?;
        assert!(
            result.is_err(),
            "denied peer must not receive a SubscribeOk, got {result:?}"
        );

        server.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn try_register_connection_rejects_revoked_peer() -> TestResult {
        let receiver = EndpointBuilder::test([46; 32]).bind().await?;
        let forwarder = EndpointBuilder::test([47; 32]).bind().await?;
        let forwarder_addr = forwarder.endpoint_addr().await;

        let allow = AllowList::new([receiver.endpoint_id()]);

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.accept().await })
        };
        let _client_connection = receiver.connect(forwarder_addr).await?;
        let server_connection = tokio::time::timeout(Duration::from_secs(5), accept)
            .await???
            .ok_or("forwarder endpoint closed before accepting connection")?;
        let endpoint_id = server_connection.remote_id();

        allow.apply_update([])?;
        assert!(
            allow
                .try_register_connection(endpoint_id, server_connection.clone())
                .is_none(),
            "revoked peer must not be registered after allow-list removal"
        );

        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn revoke_force_closes_open_conn() -> TestResult {
        let receiver = EndpointBuilder::test([42; 32]).bind().await?;
        let forwarder = EndpointBuilder::test([43; 32]).bind().await?;
        let forwarder_addr = forwarder.endpoint_addr().await;

        let allow = AllowList::new([receiver.endpoint_id()]);
        let server = tokio::spawn(admission_server(forwarder.clone(), allow.clone()));

        // Establish and confirm an admitted, registered subscription.
        let (connection, reply) =
            tokio::time::timeout(Duration::from_secs(5), subscribe(&receiver, forwarder_addr))
                .await??;
        assert_eq!(&reply, SUBSCRIBE_OK);

        // Revoke the peer; its open connection must be force-closed.
        let revoked = allow.apply_update([])?;
        assert_eq!(revoked, vec![receiver.endpoint_id()]);

        let closed = tokio::time::timeout(Duration::from_secs(5), connection.closed()).await;
        assert!(
            closed.is_ok(),
            "revoked peer's open connection must be force-closed promptly"
        );

        server.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn offline_server_uses_cached_list() -> TestResult {
        let dir = tempfile::tempdir()?;
        let cache_path = dir.path().join("allowlist");

        let receiver = EndpointBuilder::test([44; 32]).bind().await?;

        // First run with the server online: persist the allowed set.
        let online = AllowList::load(&cache_path)?;
        online.apply_update([receiver.endpoint_id()])?;

        // Restart with the server offline: load only uses the cache, no
        // refresh. The cached peer must still be authorized.
        let cached = AllowList::load(&cache_path)?;
        assert!(
            cached.contains(&receiver.endpoint_id()),
            "cached allow-list must authorize the previously persisted peer"
        );

        // The cached list authenticates the peer end-to-end.
        let forwarder = EndpointBuilder::test([45; 32]).bind().await?;
        let forwarder_addr = forwarder.endpoint_addr().await;
        let server = tokio::spawn(admission_server(forwarder.clone(), cached));

        let (_connection, reply) =
            tokio::time::timeout(Duration::from_secs(5), subscribe(&receiver, forwarder_addr))
                .await??;
        assert_eq!(&reply, SUBSCRIBE_OK);

        server.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn apply_update_preserves_pinned_receivers() -> TestResult {
        let pinned = EndpointBuilder::test([70; 32]).bind().await?;
        let server_only = EndpointBuilder::test([71; 32]).bind().await?;

        let allow = AllowList::new(vec![]);
        allow.set_pinned([pinned.endpoint_id()]);

        // Server snapshot that does NOT contain the pinned receiver.
        let revoked = allow.apply_update([server_only.endpoint_id()])?;

        assert!(
            allow.contains(&pinned.endpoint_id()),
            "pinned receiver must survive server snapshot"
        );
        assert!(allow.contains(&server_only.endpoint_id()));
        assert!(
            !revoked.contains(&pinned.endpoint_id()),
            "pinned receiver must never be revoked"
        );

        pinned.close().await;
        server_only.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn apply_update_does_not_persist_pinned_to_cache() -> TestResult {
        let dir = tempfile::tempdir()?;
        let cache_path = dir.path().join("allowlist");

        let pinned = EndpointBuilder::test([72; 32]).bind().await?;
        let server_only = EndpointBuilder::test([73; 32]).bind().await?;

        let allow = AllowList::load(&cache_path)?;
        allow.set_pinned([pinned.endpoint_id()]);
        allow.apply_update([server_only.endpoint_id()])?;

        let cached = parse_endpoint_ids(&std::fs::read_to_string(&cache_path)?)?;
        assert_eq!(
            cached,
            HashSet::from([server_only.endpoint_id()]),
            "cache must hold the server snapshot only, never pinned receivers"
        );

        pinned.close().await;
        server_only.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn apply_update_does_not_force_close_pinned_connection() -> TestResult {
        let receiver = EndpointBuilder::test([74; 32]).bind().await?;
        let forwarder = EndpointBuilder::test([75; 32]).bind().await?;
        let forwarder_addr = forwarder.endpoint_addr().await;

        let allow = AllowList::new(vec![]);
        allow.set_pinned([receiver.endpoint_id()]);
        let server = tokio::spawn(admission_server(forwarder.clone(), allow.clone()));

        // Establish and confirm an admitted, registered subscription.
        let (connection, reply) =
            tokio::time::timeout(Duration::from_secs(5), subscribe(&receiver, forwarder_addr))
                .await??;
        assert_eq!(&reply, SUBSCRIBE_OK);

        // An empty server snapshot must not revoke or force-close the pinned peer.
        let revoked = allow.apply_update([])?;
        assert!(revoked.is_empty(), "pinned peer must not be revoked");

        // The connection stays functional: a fresh subscription probe on the
        // same connection still succeeds.
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(SUBSCRIBE).await?;
        send.finish()?;
        let mut buf = [0u8; SUBSCRIBE_OK.len()];
        tokio::time::timeout(Duration::from_secs(5), recv.read_exact(&mut buf)).await??;
        assert_eq!(&buf, SUBSCRIBE_OK);

        server.abort();
        receiver.close().await;
        forwarder.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn pinned_receivers_removed_from_config_are_not_resurrected_on_reboot() -> TestResult {
        let dir = tempfile::tempdir()?;
        let cache_path = dir.path().join("allowlist");

        let pinned = EndpointBuilder::test([76; 32]).bind().await?;
        let server_only = EndpointBuilder::test([77; 32]).bind().await?;

        // First boot: the receiver is pinned via static config and a server
        // snapshot is persisted to the cache.
        let first_boot = AllowList::load(&cache_path)?;
        first_boot.set_pinned([pinned.endpoint_id()]);
        first_boot.apply_update([server_only.endpoint_id()])?;

        // Reboot with the receiver removed from static config (no set_pinned):
        // the cache must not resurrect it.
        let second_boot = AllowList::load(&cache_path)?;
        assert!(
            !second_boot.contains(&pinned.endpoint_id()),
            "removing a receiver from static config must take effect on reboot"
        );
        assert!(second_boot.contains(&server_only.endpoint_id()));

        pinned.close().await;
        server_only.close().await;
        Ok(())
    }
}

//! Persistent receiver allow-list with revocation for the forwarder P2P endpoint.
//!
//! [`AllowList`] gates inbound receiver connections by their iroh node id (the
//! transport-layer `EndpointId`). It keeps three concerns together so the
//! accept loop can enforce them at the connection boundary:
//!
//! 1. **Authoritative in-memory set.** [`AllowList::try_register_connection`]
//!    admits and tracks a connection under one lock, before any control or data
//!    stream work happens.
//! 2. **Last-known persistence.** The allowed set is cached on disk so a
//!    refresh failure (e.g. an offline server) falls back to the previously
//!    persisted list rather than failing open or failing closed-empty. Updates
//!    persist *before* the in-memory swap, so a write failure leaves the
//!    last-known set in force ([`AllowList::apply_update`]).
//! 3. **Revocation / force-close.** Admitted connections are tracked by remote
//!    node id ([`AllowList::try_register_connection`]); when an update removes a
//!    peer, its open connections are force-closed immediately.
//!
//! Updates are sourced from the server: [`ServerAllowListClient`] fetches
//! the active receiver set over bearer-authenticated HTTP, and
//! [`run_allowlist_distribution`] keeps the list fresh from a startup fetch,
//! pushed snapshots, and periodic polling. Reader control/status event mapping
//! remains out of scope for this allow-list module.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rt_iroh::{Connection, NodeId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// QUIC application error code used when force-closing a revoked peer's
/// connection after it is removed from the allow-list.
const REVOKED_ERROR_CODE: u32 = 3;

/// Production poll cadence for refreshing receiver authorization from the server.
pub const DEFAULT_ALLOWLIST_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// How long each long-poll request asks the server to hold open while waiting
/// for an allow-list change. Bounds how long a forwarder waits between re-arming
/// the push subscription, and is kept under typical proxy idle timeouts.
pub const ALLOWLIST_PUSH_HOLD: Duration = Duration::from_secs(25);

/// Initial reconnect backoff for the long-poll push subscription after a server
/// error, doubled up to [`ALLOWLIST_PUSH_MAX_BACKOFF`].
const ALLOWLIST_PUSH_INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Cap on the long-poll push subscription reconnect backoff.
const ALLOWLIST_PUSH_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Conservative bound on a single server allow-list HTTP request. Without a
/// timeout a hung server would stall the distribution loop's initial refresh
/// and polling indefinitely; this caps how long any one fetch can block.
pub const DEFAULT_ALLOWLIST_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared, mutable allow-list state guarded by a single mutex.
#[derive(Debug, Default)]
struct State {
    /// Node ids currently permitted to connect.
    allowed: HashSet<NodeId>,
    /// Open admitted connections, keyed by remote node id then a per-connection
    /// registration id so multiple connections from one peer are tracked
    /// independently.
    connections: HashMap<NodeId, HashMap<u64, Connection>>,
    /// Monotonic id allocator for connection registrations.
    next_conn_id: u64,
}

/// Persistent, revocable allow-list of receiver node ids.
#[derive(Clone, Debug, Default)]
pub struct AllowList {
    state: Arc<Mutex<State>>,
    /// On-disk cache of the last-known allowed set, if persistence is enabled.
    cache_path: Option<Arc<PathBuf>>,
}

impl AllowList {
    /// Builds an in-memory allow-list (no persistence) from the given node ids.
    #[must_use]
    pub fn new(allowed: impl IntoIterator<Item = NodeId>) -> Self {
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
    /// a line that is not a valid node id.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let allowed = match fs::read_to_string(&path) {
            Ok(contents) => parse_node_ids(&contents)?,
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

    /// Returns whether `node_id` is currently permitted to connect.
    #[must_use]
    pub fn contains(&self, node_id: &NodeId) -> bool {
        self.lock().allowed.contains(node_id)
    }

    /// Unions `additional` node ids into the allowed set in memory only.
    ///
    /// Unlike [`AllowList::apply_update`], this neither persists to the cache
    /// nor revokes anything: it is additive. It exists so statically configured
    /// receivers can be allowed *on top of* the cached/last-known (or
    /// server-fetched) set at startup, rather than replacing it.
    pub fn add_allowed(&self, additional: impl IntoIterator<Item = NodeId>) {
        let mut state = self.lock();
        state.allowed.extend(additional);
    }

    /// Atomically replaces the allowed set with `allowed`, persisting it and
    /// force-closing every open connection whose node id was removed.
    ///
    /// Returns the revoked node ids. Persistence happens before the in-memory
    /// swap: if the cache cannot be written the in-memory state is left at its
    /// last-known value and the error is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence is enabled and the cache file cannot be
    /// written.
    pub fn apply_update(
        &self,
        allowed: impl IntoIterator<Item = NodeId>,
    ) -> io::Result<Vec<NodeId>> {
        let new_allowed: HashSet<NodeId> = allowed.into_iter().collect();
        let (revoked, connections_to_close) = {
            let mut state = self.lock();
            // Persist before swapping in memory while holding the state lock so
            // concurrent updates cannot interleave disk and memory changes.
            // If the cache write fails, the last-known set stays in force.
            if let Some(path) = &self.cache_path {
                persist(path, &new_allowed)?;
            }

            let revoked: Vec<NodeId> = state.allowed.difference(&new_allowed).copied().collect();
            state.allowed = new_allowed;
            let mut connections_to_close = Vec::new();
            for node_id in &revoked {
                if let Some(connections) = state.connections.remove(node_id) {
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

    /// Registers `connection` only if `node_id` is still allowed, returning a
    /// guard that deregisters the connection when dropped.
    #[must_use]
    pub(crate) fn try_register_connection(
        &self,
        node_id: NodeId,
        connection: Connection,
    ) -> Option<ConnectionGuard> {
        let mut state = self.lock();
        if !state.allowed.contains(&node_id) {
            return None;
        }
        let id = state.next_conn_id;
        state.next_conn_id += 1;
        state
            .connections
            .entry(node_id)
            .or_default()
            .insert(id, connection);
        Some(ConnectionGuard {
            allow_list: self.clone(),
            node_id,
            id,
        })
    }

    fn deregister(&self, node_id: NodeId, id: u64) {
        let mut state = self.lock();
        if let Some(connections) = state.connections.get_mut(&node_id) {
            connections.remove(&id);
            if connections.is_empty() {
                state.connections.remove(&node_id);
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

/// HTTP client for fetching receiver allow-list snapshots from the server.
#[derive(Clone)]
pub struct ServerAllowListClient {
    http: reqwest::Client,
    base_url: String,
    bearer_token: Arc<str>,
}

// Manual `Debug` so the bearer token can never leak through formatting (e.g.
// when a struct holding the client is logged or asserted on).
impl std::fmt::Debug for ServerAllowListClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerAllowListClient")
            .field("base_url", &self.base_url)
            .field("bearer_token", &"<redacted>")
            .finish()
    }
}

impl ServerAllowListClient {
    /// Builds a client with the [`DEFAULT_ALLOWLIST_REQUEST_TIMEOUT`] applied to
    /// every allow-list request.
    #[must_use]
    pub fn new(base_url: impl Into<String>, bearer_token: impl Into<String>) -> Self {
        Self::with_timeout(base_url, bearer_token, DEFAULT_ALLOWLIST_REQUEST_TIMEOUT)
    }

    /// Builds a client that bounds every allow-list request to `request_timeout`.
    #[must_use]
    pub fn with_timeout(
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
        request_timeout: Duration,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            // A builder failure here only means the platform TLS/transport
            // backend could not initialise. Never fall back to an untimed
            // client: that would silently drop the request timeout and let a
            // hung server stall the distribution loop indefinitely. Such a
            // failure is a fatal environment problem, so surface it loudly.
            .expect(
                "reqwest client builder with request timeout must initialise; \
                 a failure here indicates the platform TLS/transport backend is unavailable",
            );
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            bearer_token: Arc::from(bearer_token.into()),
        }
    }

    async fn fetch(&self) -> Result<ReceiverAllowListUpdate, AllowListRefreshError> {
        let url = format!("{}/allowlist/receivers", self.base_url);
        Ok(self
            .http
            .get(url)
            .bearer_auth(self.bearer_token.as_ref())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Long-polls the server allow-list: sends `since` (the last applied
    /// version) and a `wait` budget, so the server holds the request until the
    /// allow-list changes and returns the fresh snapshot — giving near-instant
    /// approval propagation. The per-request timeout is overridden to exceed
    /// `wait` so the held request is not aborted by the client's short default
    /// timeout. A `since` of `None` requests an immediate snapshot (initial
    /// fetch / resync).
    async fn fetch_waiting(
        &self,
        since: Option<u64>,
        wait: Duration,
    ) -> Result<ReceiverAllowListUpdate, AllowListRefreshError> {
        // Built by hand (rather than reqwest's `query` builder) so no extra
        // reqwest feature is required; both values are plain integers, so no
        // percent-encoding is needed.
        let url = match since {
            Some(since) => format!(
                "{}/allowlist/receivers?wait={}&since={since}",
                self.base_url,
                wait.as_secs()
            ),
            None => format!(
                "{}/allowlist/receivers?wait={}",
                self.base_url,
                wait.as_secs()
            ),
        };
        Ok(self
            .http
            .get(url)
            .bearer_auth(self.bearer_token.as_ref())
            // Allow the response to arrive any time within the server hold plus
            // a transport margin, regardless of the client's short default.
            .timeout(wait + Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

/// HTTP client for registering the forwarder and pushing its stream catalog.
#[derive(Clone)]
pub struct ServerCatalogClient {
    http: reqwest::Client,
    base_url: String,
    bearer_token: Arc<str>,
}

impl std::fmt::Debug for ServerCatalogClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerCatalogClient")
            .field("base_url", &self.base_url)
            .field("bearer_token", &"<redacted>")
            .finish()
    }
}

impl ServerCatalogClient {
    /// Builds a client with the default server request timeout.
    #[must_use]
    pub fn new(base_url: impl Into<String>, bearer_token: impl Into<String>) -> Self {
        Self::with_timeout(base_url, bearer_token, DEFAULT_ALLOWLIST_REQUEST_TIMEOUT)
    }

    /// Builds a client that bounds every registration/catalog request.
    #[must_use]
    pub fn with_timeout(
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
        request_timeout: Duration,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .expect(
                "reqwest client builder with request timeout must initialise; \
                 a failure here indicates the platform TLS/transport backend is unavailable",
            );
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            bearer_token: Arc::from(bearer_token.into()),
        }
    }

    /// Bootstrap this forwarder against the server `/register` endpoint using the
    /// configured bearer (an enrollment voucher on first boot, or the device's
    /// own token for same-endpoint recovery), returning the server-minted
    /// per-device token. The caller persists it and uses it for all later calls.
    pub async fn bootstrap(&self, endpoint_id: &str) -> Result<String, CatalogPushError> {
        let url = format!("{}/register", self.base_url);
        let response = self
            .http
            .post(url)
            .bearer_auth(self.bearer_token.as_ref())
            .json(&RegisterForwarderRequest {
                endpoint_id,
                device_kind: "forwarder",
            })
            .send()
            .await?
            .error_for_status()?
            .json::<RegisterResponse>()
            .await?;
        response
            .device_token
            .ok_or(CatalogPushError::MissingMintedToken)
    }

    /// Pushes this forwarder's latest identity and stream catalog.
    pub async fn push_catalog(&self, catalog: &ForwarderCatalog) -> Result<(), CatalogPushError> {
        let url = format!("{}/forwarder/catalog", self.base_url);
        self.http
            .post(url)
            .bearer_auth(self.bearer_token.as_ref())
            .json(catalog)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct RegisterForwarderRequest<'a> {
    endpoint_id: &'a str,
    device_kind: &'static str,
}

/// Subset of the server `/register` response the forwarder needs.
#[derive(Debug, Deserialize)]
struct RegisterResponse {
    #[serde(default)]
    device_token: Option<String>,
}

/// Wire-format forwarder catalog pushed to the server.
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderCatalog {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<String>,
    pub streams: Vec<ForwarderCatalogStream>,
}

/// Wire-format stream entry in a forwarder catalog push.
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderCatalogStream {
    pub stream_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogPushError {
    // `reqwest::Error`'s `Display` covers URL/status/transport but never
    // request headers, so the bearer token cannot leak here.
    #[error("server catalog request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server /register did not return a minted device token")]
    MissingMintedToken,
}

/// Wire-format receiver allow-list snapshot distributed by the server.
#[derive(Debug, Clone, Deserialize)]
pub struct ReceiverAllowListUpdate {
    pub receiver_endpoint_ids: Vec<String>,
    /// Monotonic allow-list version this snapshot reflects. Echoed back as
    /// `since` on the next long-poll. Defaults to 0 for an older server that
    /// does not emit a version, which simply degrades to immediate re-polling.
    #[serde(default)]
    pub version: u64,
}

impl ReceiverAllowListUpdate {
    #[must_use]
    pub fn replace(receiver_endpoint_ids: Vec<String>) -> Self {
        Self {
            receiver_endpoint_ids,
            version: 0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AllowListRefreshError {
    // `reqwest::Error`'s `Display` covers the failing URL and transport cause
    // but never request headers, so the bearer token cannot leak here.
    #[error("server allow-list request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("failed to apply receiver allow-list update: {0}")]
    Apply(#[from] io::Error),
}

/// Fetches the current server allow-list and applies it to `allow_list`.
pub async fn fetch_and_apply_once(
    client: &ServerAllowListClient,
    allow_list: &AllowList,
) -> Result<Vec<NodeId>, AllowListRefreshError> {
    let update = client.fetch().await?;
    apply_receiver_update(allow_list, update)
}

/// Applies a pushed or fetched wire-format receiver allow-list update.
pub fn apply_receiver_update(
    allow_list: &AllowList,
    update: ReceiverAllowListUpdate,
) -> Result<Vec<NodeId>, AllowListRefreshError> {
    let allowed = parse_update(update);
    Ok(allow_list.apply_update(allowed)?)
}

/// In-flight poll-fetch future held across `select!` iterations so pushed
/// updates stay responsive while a slow poll request is outstanding.
///
/// The poll *fetches only*; it does not apply. Applying is deferred to the
/// distribution loop so a snapshot captured before a newer pushed update can be
/// discarded instead of clobbering the newer state (see the generation guard in
/// [`run_allowlist_distribution`]).
type PollFetch = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<ReceiverAllowListUpdate, AllowListRefreshError>>
            + Send,
    >,
>;

/// Keeps `allow_list` fresh from startup fetches, pushed snapshots, and polling.
pub async fn run_allowlist_distribution(
    allow_list: AllowList,
    client: ServerAllowListClient,
    mut pushed_updates: mpsc::Receiver<ReceiverAllowListUpdate>,
    poll_interval: Duration,
) {
    if let Err(err) = fetch_and_apply_once(&client, &allow_list).await {
        tracing::warn!(error = %err, "initial receiver allow-list refresh failed");
    }

    let poll_interval = if poll_interval.is_zero() {
        DEFAULT_ALLOWLIST_POLL_INTERVAL
    } else {
        poll_interval
    };
    let mut ticker = tokio::time::interval(poll_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    let mut push_closed = false;
    // An in-flight poll fetch is held here rather than awaited inline so pushed
    // updates stay responsive even while a slow (or timing-out) poll request is
    // outstanding. At most one poll fetch runs at a time; a tick that lands
    // while one is still in flight is dropped (the next tick re-polls).
    let mut poll_fetch: Option<PollFetch> = None;
    // Monotonic count of applied updates. Bumped on every successful apply
    // (pushed or polled). A poll captures this when it begins fetching; if the
    // count has moved by the time the fetch completes, a newer update applied
    // while the poll was in flight, so the (now stale) poll snapshot is
    // discarded rather than overwriting the newer state. Without this guard a
    // slow poll holding a pre-revocation snapshot could re-authorize a receiver
    // that a pushed revocation had already removed.
    let mut generation: u64 = 0;
    let mut poll_generation: u64 = 0;

    loop {
        tokio::select! {
            update = pushed_updates.recv(), if !push_closed => {
                match update {
                    Some(update) => {
                        match apply_receiver_update(&allow_list, update) {
                            Ok(_) => generation = generation.wrapping_add(1),
                            Err(err) => tracing::warn!(
                                error = %err,
                                "pushed receiver allow-list update failed",
                            ),
                        }
                    }
                    None => push_closed = true,
                }
            }
            _ = ticker.tick() => {
                if poll_fetch.is_none() {
                    let client = client.clone();
                    poll_generation = generation;
                    poll_fetch = Some(Box::pin(async move { client.fetch().await }));
                }
            }
            result = async { poll_fetch.as_mut().expect("poll fetch present").await }, if poll_fetch.is_some() => {
                poll_fetch = None;
                match result {
                    Ok(update) => {
                        if generation == poll_generation {
                            match apply_receiver_update(&allow_list, update) {
                                Ok(_) => generation = generation.wrapping_add(1),
                                Err(err) => tracing::warn!(
                                    error = %err,
                                    "polled receiver allow-list refresh failed",
                                ),
                            }
                        } else {
                            tracing::debug!(
                                "discarding stale polled receiver allow-list snapshot \
                                 superseded by a newer update",
                            );
                        }
                    }
                    Err(err) => tracing::warn!(
                        error = %err,
                        "polled receiver allow-list refresh failed",
                    ),
                }
            }
        }
    }
}

/// Long-poll the server for receiver allow-list changes and forward each fresh
/// snapshot into `push_tx`, which [`run_allowlist_distribution`] applies.
///
/// This is the push transport that complements the periodic poll backstop: an
/// approval on the server releases the held request within milliseconds, so a
/// newly approved receiver is admitted almost immediately instead of waiting
/// up to a full poll interval. Loops until the receiving distribution loop is
/// gone (the channel send fails). Server errors back off and retry; the
/// distribution loop's polling keeps the allow-list correct meanwhile.
pub async fn run_allowlist_push_subscription(
    client: ServerAllowListClient,
    push_tx: mpsc::Sender<ReceiverAllowListUpdate>,
    hold: Duration,
) {
    // `None` requests an immediate snapshot; afterwards we echo the server's
    // version as `since` to long-poll for the *next* change.
    let mut since: Option<u64> = None;
    let mut backoff = ALLOWLIST_PUSH_INITIAL_BACKOFF;

    loop {
        let started = tokio::time::Instant::now();
        match client.fetch_waiting(since, hold).await {
            Ok(update) => {
                backoff = ALLOWLIST_PUSH_INITIAL_BACKOFF;
                let version = update.version;
                let changed = since != Some(version);
                since = Some(version);
                if changed {
                    // A real change (or the initial snapshot): hand it to the
                    // distribution loop. A closed channel means that loop is
                    // gone, so this subscription has no consumer left.
                    if push_tx.send(update).await.is_err() {
                        return;
                    }
                } else {
                    // No change within the hold. Pace the next request to the
                    // intended hold window so a non-holding or older server
                    // (which returns immediately and never sets a moving
                    // version) is not hammered; a compliant server already
                    // consumed ~`hold`, so this adds nothing.
                    tokio::time::sleep_until(started + hold).await;
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "receiver allow-list push subscription failed; retrying");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(ALLOWLIST_PUSH_MAX_BACKOFF);
            }
        }
    }
}

fn parse_update(update: ReceiverAllowListUpdate) -> Vec<NodeId> {
    update
        .receiver_endpoint_ids
        .into_iter()
        .filter_map(|endpoint_id| match NodeId::from_str(&endpoint_id) {
            Ok(node_id) => Some(node_id),
            Err(error) => {
                tracing::warn!(
                    endpoint_id = %endpoint_id,
                    error = %error,
                    "skipping malformed receiver endpoint id in allow-list update",
                );
                None
            }
        })
        .collect()
}

/// Deregisters a tracked connection when dropped, keeping the connection
/// registry bounded to currently-open admitted connections.
#[must_use = "dropping the guard immediately deregisters the tracked connection"]
pub(crate) struct ConnectionGuard {
    allow_list: AllowList,
    node_id: NodeId,
    id: u64,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.allow_list.deregister(self.node_id, self.id);
    }
}

/// Parses a newline-delimited list of node ids, ignoring blank lines.
fn parse_node_ids(contents: &str) -> io::Result<HashSet<NodeId>> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            NodeId::from_str(line)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
        })
        .collect()
}

/// Writes `allowed` to `path` via a write-then-rename so a crash never leaves a
/// partially written cache. Ids are sorted for a deterministic on-disk form.
fn persist(path: &Path, allowed: &HashSet<NodeId>) -> io::Result<()> {
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

    use std::sync::Arc;
    use std::time::Duration;

    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode, header::AUTHORIZATION},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use rt_iroh::{Endpoint, EndpointBuilder, NodeAddr};
    use tokio::sync::{Mutex as TokioMutex, mpsc, watch};

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
                let Ok(node_id) = connection.remote_node_id() else {
                    return;
                };
                let Some(_guard) = allow.try_register_connection(node_id, connection.clone())
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
        forwarder_addr: NodeAddr,
    ) -> Result<(Connection, [u8; 4]), BoxError> {
        receiver.add_node_addr(forwarder_addr.clone())?;
        let connection = receiver.connect(forwarder_addr).await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(SUBSCRIBE).await?;
        send.finish()?;
        let mut buf = [0u8; SUBSCRIBE_OK.len()];
        recv.read_exact(&mut buf).await?;
        Ok((connection, buf))
    }

    #[derive(Clone)]
    struct TestAllowListServerState {
        bearer_token: &'static str,
        receiver_endpoint_ids: Arc<TokioMutex<Vec<String>>>,
        // Retained count of completed authorized fetches. Unlike a `Notify`,
        // a `watch` holds its latest value, so a fetch that completes before a
        // waiter starts observing is not lost — letting tests deterministically
        // sequence "initial fetch happened" against later poll fetches.
        fetches: watch::Sender<u64>,
    }

    async fn test_allowlist_handler(
        State(state): State<TestAllowListServerState>,
        headers: HeaderMap,
    ) -> Response {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {}", state.bearer_token));
        if !authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }

        let receiver_endpoint_ids = state.receiver_endpoint_ids.lock().await.clone();
        // Count the fetch only after the current id set has been read, so an
        // observed count of N guarantees N fetches each saw the id set in force
        // at their read.
        state.fetches.send_modify(|count| *count += 1);
        Json(serde_json::json!({ "receiver_endpoint_ids": receiver_endpoint_ids })).into_response()
    }

    async fn spawn_test_allowlist_server(
        receiver_endpoint_ids: Arc<TokioMutex<Vec<String>>>,
    ) -> Result<(String, watch::Receiver<u64>, tokio::task::JoinHandle<()>), BoxError> {
        let (fetches, fetches_rx) = watch::channel(0u64);
        let app = Router::new()
            .route("/allowlist/receivers", get(test_allowlist_handler))
            .with_state(TestAllowListServerState {
                bearer_token: "thin-secret",
                receiver_endpoint_ids,
                fetches,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok((url, fetches_rx, server))
    }

    #[tokio::test]
    async fn denied_peer_cannot_subscribe() -> TestResult {
        let receiver = EndpointBuilder::test([40; 32]).bind().await?;
        let forwarder = EndpointBuilder::test([41; 32]).bind().await?;
        let forwarder_addr = forwarder.node_addr().await;

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
        let forwarder_addr = forwarder.node_addr().await;

        let allow = AllowList::new([receiver.node_id()]);
        receiver.add_node_addr(forwarder_addr.clone())?;

        let accept = {
            let forwarder = forwarder.clone();
            tokio::spawn(async move { forwarder.accept().await })
        };
        let _client_connection = receiver.connect(forwarder_addr).await?;
        let server_connection = tokio::time::timeout(Duration::from_secs(5), accept)
            .await???
            .ok_or("forwarder endpoint closed before accepting connection")?;
        let node_id = server_connection.remote_node_id()?;

        allow.apply_update([])?;
        assert!(
            allow
                .try_register_connection(node_id, server_connection.clone())
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
        let forwarder_addr = forwarder.node_addr().await;

        let allow = AllowList::new([receiver.node_id()]);
        let server = tokio::spawn(admission_server(forwarder.clone(), allow.clone()));

        // Establish and confirm an admitted, registered subscription.
        let (connection, reply) =
            tokio::time::timeout(Duration::from_secs(5), subscribe(&receiver, forwarder_addr))
                .await??;
        assert_eq!(&reply, SUBSCRIBE_OK);

        // Revoke the peer; its open connection must be force-closed.
        let revoked = allow.apply_update([])?;
        assert_eq!(revoked, vec![receiver.node_id()]);

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
        online.apply_update([receiver.node_id()])?;

        // Restart with the server offline: load only uses the cache, no
        // refresh. The cached peer must still be authorized.
        let cached = AllowList::load(&cache_path)?;
        assert!(
            cached.contains(&receiver.node_id()),
            "cached allow-list must authorize the previously persisted peer"
        );

        // The cached list authenticates the peer end-to-end.
        let forwarder = EndpointBuilder::test([45; 32]).bind().await?;
        let forwarder_addr = forwarder.node_addr().await;
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

    #[derive(Clone)]
    struct TestCatalogServerState {
        bearer_token: &'static str,
        received: Arc<TokioMutex<Option<(HeaderMap, serde_json::Value)>>>,
    }

    async fn test_catalog_handler(
        State(state): State<TestCatalogServerState>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {}", state.bearer_token));
        if !authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        *state.received.lock().await = Some((headers.clone(), body));
        StatusCode::OK.into_response()
    }

    /// `/register` mock: records the request and returns a minted device token.
    async fn test_register_handler(
        State(state): State<TestCatalogServerState>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {}", state.bearer_token));
        if !authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        *state.received.lock().await = Some((headers.clone(), body));
        Json(serde_json::json!({ "device_token": "rtk_minted_secret" })).into_response()
    }

    #[tokio::test]
    async fn catalog_client_posts_registration_and_catalog_with_bearer() -> TestResult {
        let received = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route("/register", post(test_register_handler))
            .route("/forwarder/catalog", post(test_catalog_handler))
            .with_state(TestCatalogServerState {
                bearer_token: "thin-secret",
                received: received.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = ServerCatalogClient::new(base_url, "thin-secret");
        let minted = client
            .bootstrap("fwd-node-1")
            .await
            .expect("bootstrap request succeeds");
        assert_eq!(minted, "rtk_minted_secret");
        let (headers, register_body) = received
            .lock()
            .await
            .take()
            .expect("server captured registration request");
        assert_eq!(
            headers.get(AUTHORIZATION).unwrap().to_str().unwrap(),
            "Bearer thin-secret"
        );
        assert_eq!(register_body["endpoint_id"], "fwd-node-1");
        assert_eq!(register_body["device_kind"], "forwarder");

        client
            .push_catalog(&ForwarderCatalog {
                endpoint_id: "fwd-node-1".to_owned(),
                display_name: Some("Start Line".to_owned()),
                direct_addrs: vec!["127.0.0.1:12345".to_owned()],
                streams: vec![ForwarderCatalogStream {
                    stream_id: "reader-a".to_owned(),
                    epoch: 3,
                    next_seq: 42,
                }],
            })
            .await
            .expect("catalog request succeeds");
        let (headers, catalog_body) = received
            .lock()
            .await
            .take()
            .expect("server captured catalog request");
        assert_eq!(
            headers.get(AUTHORIZATION).unwrap().to_str().unwrap(),
            "Bearer thin-secret"
        );
        assert_eq!(catalog_body["endpoint_id"], "fwd-node-1");
        assert_eq!(catalog_body["display_name"], "Start Line");
        assert_eq!(
            catalog_body["direct_addrs"],
            serde_json::json!(["127.0.0.1:12345"])
        );
        assert_eq!(catalog_body["streams"][0]["stream_id"], "reader-a");
        assert_eq!(catalog_body["streams"][0]["epoch"], 3);
        assert_eq!(catalog_body["streams"][0]["next_seq"], 42);

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn approved_receiver_appears_in_forwarder_list() -> TestResult {
        let receiver = EndpointBuilder::test([50; 32]).bind().await?;
        let receiver_endpoint_ids = Arc::new(TokioMutex::new(vec![receiver.node_id().to_string()]));
        let (base_url, _fetches, server) =
            spawn_test_allowlist_server(receiver_endpoint_ids).await?;
        let client = ServerAllowListClient::new(base_url, "thin-secret");
        let allow = AllowList::default();

        fetch_and_apply_once(&client, &allow).await?;

        assert!(allow.contains(&receiver.node_id()));
        server.abort();
        receiver.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn revoke_propagates() -> TestResult {
        let receiver = EndpointBuilder::test([51; 32]).bind().await?;
        let allow = AllowList::new([receiver.node_id()]);
        let receiver_endpoint_ids = Arc::new(TokioMutex::new(vec![receiver.node_id().to_string()]));
        let (base_url, _fetches, server) =
            spawn_test_allowlist_server(receiver_endpoint_ids).await?;
        let client = ServerAllowListClient::new(base_url, "thin-secret");
        let (tx, rx) = mpsc::channel(1);
        let sync = tokio::spawn(run_allowlist_distribution(
            allow.clone(),
            client,
            rx,
            Duration::from_secs(60),
        ));

        tx.send(ReceiverAllowListUpdate::replace(Vec::new()))
            .await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while allow.contains(&receiver.node_id()) {
                tokio::task::yield_now().await;
            }
        })
        .await?;

        assert!(!allow.contains(&receiver.node_id()));
        sync.abort();
        server.abort();
        receiver.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn poll_backstop_refreshes() -> TestResult {
        let receiver = EndpointBuilder::test([52; 32]).bind().await?;
        let receiver_endpoint_ids = Arc::new(TokioMutex::new(Vec::new()));
        let (base_url, mut fetches, server) =
            spawn_test_allowlist_server(receiver_endpoint_ids.clone()).await?;
        let client = ServerAllowListClient::new(base_url, "thin-secret");
        let allow = AllowList::default();
        let (_tx, rx) = mpsc::channel(1);
        // Short poll interval with real time: the backstop must re-fetch on its
        // own without any pushed update.
        let sync = tokio::spawn(run_allowlist_distribution(
            allow.clone(),
            client,
            rx,
            Duration::from_millis(50),
        ));

        // Wait (on retained state) until the initial fetch has read the empty
        // set, so the receiver can only be admitted by a later poll fetch.
        tokio::time::timeout(
            Duration::from_secs(5),
            fetches.wait_for(|&count| count >= 1),
        )
        .await??;
        assert!(!allow.contains(&receiver.node_id()));

        // Publish the receiver; only the polling backstop can pick this up.
        *receiver_endpoint_ids.lock().await = vec![receiver.node_id().to_string()];

        tokio::time::timeout(Duration::from_secs(5), async {
            while !allow.contains(&receiver.node_id()) {
                tokio::task::yield_now().await;
            }
        })
        .await?;

        assert!(allow.contains(&receiver.node_id()));
        sync.abort();
        server.abort();
        receiver.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn stale_poll_does_not_override_newer_revocation() -> TestResult {
        use std::sync::atomic::{AtomicU64, Ordering};

        let receiver = EndpointBuilder::test([55; 32]).bind().await?;
        // Start empty; the initial fetch admits the receiver from the old list.
        let allow = AllowList::default();

        #[derive(Clone)]
        struct StallState {
            // Old, pre-revocation list returned by every fetch: it still
            // authorizes the receiver.
            old_list: Vec<String>,
            // 1-based count of requests received by the server.
            requests: Arc<AtomicU64>,
            // Broadcasts the latest request count so the test can sequence on it.
            observed: watch::Sender<u64>,
            // Releases exactly one gated (poll) request when notified once.
            release: Arc<tokio::sync::Notify>,
        }

        async fn stall_handler(State(state): State<StallState>) -> Response {
            let n = state.requests.fetch_add(1, Ordering::SeqCst) + 1;
            state.observed.send_modify(|count| *count = (*count).max(n));
            // The initial fetch (request 1) returns immediately so the receiver
            // is admitted. Every later (poll) request blocks until explicitly
            // released, so the test controls exactly when a poll completes.
            if n >= 2 {
                state.release.notified().await;
            }
            Json(serde_json::json!({ "receiver_endpoint_ids": state.old_list })).into_response()
        }

        let (observed_tx, mut observed_rx) = watch::channel(0u64);
        let release = Arc::new(tokio::sync::Notify::new());
        let app = Router::new()
            .route("/allowlist/receivers", get(stall_handler))
            .with_state(StallState {
                old_list: vec![receiver.node_id().to_string()],
                requests: Arc::new(AtomicU64::new(0)),
                observed: observed_tx,
                release: release.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = ServerAllowListClient::new(base_url, "thin-secret");
        let (tx, rx) = mpsc::channel(1);
        // Short poll interval so a poll fetch starts promptly after the initial
        // fetch admits the receiver.
        let sync = tokio::spawn(run_allowlist_distribution(
            allow.clone(),
            client,
            rx,
            Duration::from_millis(50),
        ));

        // Initial fetch (request 1) admits the receiver from the old list.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !allow.contains(&receiver.node_id()) {
                tokio::task::yield_now().await;
            }
        })
        .await?;

        // A poll (request 2) is now in flight and blocked on the release gate.
        tokio::time::timeout(
            Duration::from_secs(5),
            observed_rx.wait_for(|&count| count >= 2),
        )
        .await??;

        // Apply a pushed revocation while the poll fetch is still in flight.
        tx.send(ReceiverAllowListUpdate::replace(Vec::new()))
            .await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while allow.contains(&receiver.node_id()) {
                tokio::task::yield_now().await;
            }
        })
        .await?;

        // Let the stale poll (request 2) complete with the old, pre-revocation
        // list. notify_one releases exactly one waiter, so request 3 stays
        // blocked and cannot itself re-admit the receiver.
        release.notify_one();

        // Request 3 starting proves the stale poll result has been fully
        // processed by the distribution loop (the poll slot freed, a new tick
        // fired). Request 3 then blocks on the gate without responding.
        tokio::time::timeout(
            Duration::from_secs(5),
            observed_rx.wait_for(|&count| count >= 3),
        )
        .await??;

        assert!(
            !allow.contains(&receiver.node_id()),
            "a stale in-flight poll must not re-authorize a receiver revoked while it was outstanding"
        );

        sync.abort();
        server.abort();
        receiver.close().await;
        Ok(())
    }

    #[derive(Clone)]
    struct PushServerState {
        bearer_token: &'static str,
        ids: Arc<TokioMutex<Vec<String>>>,
        version: Arc<std::sync::atomic::AtomicU64>,
        changed: Arc<tokio::sync::Notify>,
    }

    #[derive(serde::Deserialize)]
    struct PushQuery {
        since: Option<u64>,
        wait: Option<u64>,
    }

    /// Mock long-poll allow-list endpoint mirroring the real server: holds the
    /// request when the caller's `since` matches the current version, releasing
    /// on a version bump, and always echoes the current `version`.
    async fn push_allowlist_handler(
        State(state): State<PushServerState>,
        headers: HeaderMap,
        axum::extract::Query(query): axum::extract::Query<PushQuery>,
    ) -> Response {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {}", state.bearer_token));
        if !authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let current = state.version.load(std::sync::atomic::Ordering::SeqCst);
        let wait = query.wait.unwrap_or(0);
        if wait > 0 && query.since == Some(current) {
            let _ = tokio::time::timeout(Duration::from_secs(wait), state.changed.notified()).await;
        }
        let version = state.version.load(std::sync::atomic::Ordering::SeqCst);
        let ids = state.ids.lock().await.clone();
        Json(serde_json::json!({ "receiver_endpoint_ids": ids, "version": version }))
            .into_response()
    }

    #[tokio::test]
    async fn push_subscription_delivers_initial_snapshot_then_releases_on_bump() -> TestResult {
        let ids = Arc::new(TokioMutex::new(Vec::<String>::new()));
        let version = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let changed = Arc::new(tokio::sync::Notify::new());
        let app = Router::new()
            .route("/allowlist/receivers", get(push_allowlist_handler))
            .with_state(PushServerState {
                bearer_token: "thin-secret",
                ids: ids.clone(),
                version: version.clone(),
                changed: changed.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = ServerAllowListClient::new(base_url, "thin-secret");
        let (tx, mut rx) = mpsc::channel(8);
        let sub = tokio::spawn(run_allowlist_push_subscription(
            client,
            tx,
            Duration::from_secs(5),
        ));

        // The initial request omits `since`, so the server returns immediately
        // with the empty snapshot at version 0.
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await?
            .expect("initial snapshot delivered");
        assert_eq!(first.version, 0);
        assert!(first.receiver_endpoint_ids.is_empty());

        // The subscription is now held at version 0. An approval bumps the
        // version and must release it within milliseconds with the new set.
        *ids.lock().await = vec!["receiver-1".to_owned()];
        version.store(1, std::sync::atomic::Ordering::SeqCst);
        changed.notify_waiters();

        let second = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await?
            .expect("approval releases the held long-poll");
        assert_eq!(second.version, 1);
        assert_eq!(second.receiver_endpoint_ids, vec!["receiver-1".to_owned()]);

        sub.abort();
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn debug_redacts_bearer_token() {
        let client = ServerAllowListClient::new("http://thin.example", "super-secret-token");
        let rendered = format!("{client:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "bearer token must never appear in Debug output: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }

    #[tokio::test]
    async fn hung_server_request_times_out() -> TestResult {
        // A handler that never responds: only the client request timeout can
        // unblock the fetch.
        async fn hang() -> Response {
            std::future::pending::<()>().await;
            StatusCode::OK.into_response()
        }
        let app = Router::new().route("/allowlist/receivers", get(hang));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = ServerAllowListClient::with_timeout(
            base_url,
            "thin-secret",
            Duration::from_millis(200),
        );
        let allow = AllowList::default();

        // Outer guard far exceeds the request timeout: if the request timeout
        // works, the fetch returns an error well before this elapses.
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            fetch_and_apply_once(&client, &allow),
        )
        .await?;
        assert!(
            matches!(result, Err(AllowListRefreshError::Http(_))),
            "hung request must surface as a bounded HTTP error, got {result:?}"
        );

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn malformed_active_id_does_not_block_valid_revocation() -> TestResult {
        let retained = EndpointBuilder::test([53; 32]).bind().await?;
        let revoked = EndpointBuilder::test([54; 32]).bind().await?;
        let allow = AllowList::new([retained.node_id(), revoked.node_id()]);

        // The server can contain arbitrary endpoint_id text from receiver
        // registration. An invalid active id must be ignored, while the valid
        // omission of `revoked` still removes its authorization.
        let receiver_endpoint_ids = Arc::new(TokioMutex::new(vec![
            retained.node_id().to_string(),
            "not-a-valid-endpoint-id".to_owned(),
        ]));
        let (base_url, _fetches, server) =
            spawn_test_allowlist_server(receiver_endpoint_ids).await?;
        let client = ServerAllowListClient::new(base_url, "thin-secret");

        let result = fetch_and_apply_once(&client, &allow).await?;
        assert_eq!(result, vec![revoked.node_id()]);
        assert!(
            allow.contains(&retained.node_id()),
            "valid active receiver must remain authorized"
        );
        assert!(
            !allow.contains(&revoked.node_id()),
            "valid revocation must apply even when the response also contains a malformed id"
        );

        server.abort();
        retained.close().await;
        revoked.close().await;
        Ok(())
    }
}

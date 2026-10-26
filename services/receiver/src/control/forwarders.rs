//! Forwarder connections, connect intent, remote config, and reader-control
//! handlers.

use crate::control_api::{
    AppState, ConfigCommand, DiscoveredForwarders, FORWARDER_CONFIG_TIMEOUT,
    FORWARDER_WIFI_TIMEOUT, ForwarderConnState, ForwarderConnectionMode, ForwarderLiveStatus,
    ForwarderRuntimeStatus, ReaderCommand, ReaderLiveStatus, UpsStatusPayload, WifiCommand,
    derive_forwarder_network_info, derive_forwarder_state, optional_non_empty,
    sorted_reader_statuses, subscription_local_ports,
};
use crate::error::ReceiverError;
use rt_p2p_protocol::{ReaderControlResponse, ReaderInfo, WifiControlRequest, WifiControlResponse};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use tokio::sync::{mpsc, mpsc::error::TrySendError, oneshot};
use tracing::warn;

use super::status::{ServerDeviceStatus, server_device_status};

#[derive(Debug, Clone, Serialize)]
pub struct ForwarderConnectionStatus {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub state: ForwarderConnState,
    pub pending: bool,
    #[serde(default)]
    pub connection_mode: Option<ForwarderConnectionMode>,
    #[serde(default)]
    pub local_ip: Option<String>,
    pub subscribed_count: usize,
    pub available_count: usize,
    pub epoch_reads: Option<i64>,
    pub unique_chips: Option<i64>,
    pub readers: Vec<ReaderLiveStatus>,
    pub ups: Option<UpsStatusPayload>,
    /// Wire stream ids whose data subscription failed terminally on the live
    /// connection (protocol/data-integrity violation). These are not retried
    /// until the connection is re-established or the subscription config
    /// changes.
    pub failed_stream_ids: Vec<String>,
    pub restart_needed: Option<bool>,
    /// `true` only when this forwarder has a live control session that
    /// negotiated `CAP_REMOTE_CONFIG`; gates the UI's view/edit/restart
    /// affordances.
    pub remote_config_available: bool,
    pub reader_control_available: bool,
    pub wifi_control_available: bool,
    #[serde(default)]
    pub last_seen_secs: Option<u64>,
}

/// Result of [`get_forwarder_config`]: the forwarder's full config document and
/// whether applying the currently-persisted config requires a restart.
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderConfigResponse {
    pub config_json: String,
    pub restart_needed: bool,
}

/// Result of [`set_forwarder_config`].
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderConfigSetResult {
    pub ok: bool,
    pub restart_needed: bool,
    pub error: Option<String>,
}

/// Result of [`restart_forwarder`].
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderRestartResult {
    pub accepted: bool,
    pub error: Option<String>,
}

/// Result of a reader-control command proxied to a forwarder over P2P.
#[derive(Debug, Clone, Serialize)]
pub struct ReaderControlResult {
    pub success: bool,
    pub message: String,
    pub reader_info: Option<rt_domain::ReaderInfo>,
    pub current_epoch: Option<i64>,
    pub current_epoch_created_unix_ms: Option<i64>,
    pub current_epoch_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionsResponse {
    pub server: ServerDeviceStatus,
    pub forwarders: Vec<ForwarderConnectionStatus>,
}

pub async fn get_connections(state: &AppState) -> ConnectionsResponse {
    let server = server_device_status(state).await;
    let discovered = state.forwarders.discovered_forwarders.read().await.clone();
    let (subscriptions, intents) = {
        let db = state.storage.db.lock().await;
        let subscriptions = match db.load_stream_subscriptions() {
            Ok(subscriptions) => subscriptions,
            Err(error) => {
                warn!(error = %error, "failed to load subscriptions for connections response");
                Vec::new()
            }
        };
        // Single batched intent load per response: every per-forwarder state
        // below derives from this one map instead of a per-endpoint DB read
        // (N+1). The DB is authoritative in this async context; on a load
        // failure fall back to the sync-fallback cache of explicit disconnect
        // intents, matching `recompute_aggregate_connection_state`.
        let intents = match db.load_forwarder_intents() {
            Ok(intents) => intents,
            Err(error) => {
                warn!(error = %error, "failed to load forwarder intents for connections response");
                state
                    .forwarders
                    .disconnected_intents
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|endpoint_id| (endpoint_id.clone(), false))
                    .collect()
            }
        };
        (subscriptions, intents)
    };

    let runtime_statuses = state.forwarders.forwarder_runtime.lock().unwrap().clone();
    let live_statuses = state
        .forwarders
        .forwarder_live_status
        .lock()
        .unwrap()
        .clone();
    let config_endpoints = state.forwarder_config_endpoints();
    let reader_control_endpoints = state.forwarder_reader_control_endpoints();
    let wifi_control_endpoints = state.forwarder_wifi_control_endpoints();
    let mut endpoints: BTreeSet<String> = discovered.keys().cloned().collect();
    endpoints.extend(live_statuses.keys().cloned());
    endpoints.extend(config_endpoints.iter().cloned());
    endpoints.extend(reader_control_endpoints.iter().cloned());
    endpoints.extend(wifi_control_endpoints.iter().cloned());
    let mut subscribed_counts: HashMap<String, usize> = HashMap::new();
    let local_ports = subscription_local_ports(&subscriptions);
    for subscription in &subscriptions {
        endpoints.insert(subscription.forwarder_endpoint_id.clone());
        *subscribed_counts
            .entry(subscription.forwarder_endpoint_id.clone())
            .or_default() += 1;
    }

    let last_seen_map = state.forwarders.forwarder_last_seen.lock().unwrap().clone();
    let forwarders = assemble_forwarder_connection_statuses(
        endpoints,
        &discovered,
        &runtime_statuses,
        &intents,
        &live_statuses,
        &subscribed_counts,
        &local_ports,
        &config_endpoints,
        &reader_control_endpoints,
        &wifi_control_endpoints,
        &last_seen_map,
    );

    ConnectionsResponse { server, forwarders }
}

/// Assemble the per-forwarder entries of a [`ConnectionsResponse`] from
/// pre-loaded snapshots. Pure by construction: forwarder intent comes only
/// from the batch-loaded `intents` map (endpoints absent from the map default
/// to connect), so the response cannot re-read the DB per endpoint.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_forwarder_connection_statuses(
    endpoints: BTreeSet<String>,
    discovered: &DiscoveredForwarders,
    runtime_statuses: &HashMap<String, ForwarderRuntimeStatus>,
    intents: &HashMap<String, bool>,
    live_statuses: &HashMap<String, ForwarderLiveStatus>,
    subscribed_counts: &HashMap<String, usize>,
    local_ports: &HashMap<(String, String), Option<u16>>,
    config_endpoints: &[String],
    reader_control_endpoints: &[String],
    wifi_control_endpoints: &[String],
    last_seen_by_endpoint: &HashMap<String, std::time::Instant>,
) -> Vec<ForwarderConnectionStatus> {
    let mut forwarders = Vec::with_capacity(endpoints.len());
    for endpoint_id in endpoints {
        let discovered_forwarder = discovered.get(&endpoint_id);
        let runtime = runtime_statuses
            .get(&endpoint_id)
            .copied()
            .unwrap_or_default();
        let intent = *intents.get(&endpoint_id).unwrap_or(&true);
        let snapshot = derive_forwarder_state(runtime, intent);
        let live_status = live_statuses.get(&endpoint_id).cloned().unwrap_or_default();
        let readers = sorted_reader_statuses(&live_status, local_ports, &endpoint_id);
        let (epoch_reads, unique_chips) =
            readers
                .iter()
                .fold((None, None), |(acc_reads, acc_chips), r| {
                    let next_reads = match (acc_reads, r.reads_epoch) {
                        (Some(a), Some(b)) => Some(a + b),
                        (None, Some(b)) => Some(b),
                        (Some(a), None) => Some(a),
                        (None, None) => None,
                    };
                    let next_chips = match (acc_chips, r.unique_chips) {
                        (Some(a), Some(b)) => Some(a + b),
                        (None, Some(b)) => Some(b),
                        (Some(a), None) => Some(a),
                        (None, None) => None,
                    };
                    (next_reads, next_chips)
                });
        let last_seen_secs = if snapshot.state == ForwarderConnState::Subscribed
            || snapshot.state == ForwarderConnState::Connected
        {
            Some(0)
        } else {
            last_seen_by_endpoint
                .get(&endpoint_id)
                .map(|last_seen| last_seen.elapsed().as_secs())
        };
        let (connection_mode, local_ip) =
            derive_forwarder_network_info(snapshot.state, discovered_forwarder, &live_status);
        forwarders.push(ForwarderConnectionStatus {
            endpoint_id: endpoint_id.clone(),
            display_name: discovered_forwarder.and_then(|forwarder| forwarder.display_name.clone()),
            state: snapshot.state,
            pending: snapshot.pending,
            connection_mode,
            local_ip,
            subscribed_count: subscribed_counts.get(&endpoint_id).copied().unwrap_or(0),
            available_count: discovered_forwarder.map_or(0, |forwarder| forwarder.streams.len()),
            epoch_reads,
            unique_chips,
            readers,
            ups: live_status.ups,
            failed_stream_ids: live_status.failed_streams.into_keys().collect(),
            restart_needed: None,
            remote_config_available: config_endpoints.contains(&endpoint_id),
            reader_control_available: reader_control_endpoints.contains(&endpoint_id),
            wifi_control_available: wifi_control_endpoints.contains(&endpoint_id),
            last_seen_secs,
        });
    }
    forwarders
}

#[derive(Debug, Clone)]
pub(crate) struct ProbedForwarder {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<std::net::SocketAddr>,
    pub canonical_host: String,
}

#[derive(serde::Deserialize)]
struct ForwarderStatusProbe {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    p2p_endpoint_id: Option<String>,
    #[serde(default)]
    p2p_listen_port: Option<u16>,
    #[serde(default)]
    p2p_direct_addrs: Vec<String>,
    #[serde(default)]
    server: Option<ForwarderStatusServerProbe>,
}

#[derive(serde::Deserialize)]
struct ForwarderStatusServerProbe {
    #[serde(default)]
    endpoint_id: Option<String>,
}

fn parse_forwarder_host_input(raw: &str) -> Result<(String, Option<u16>), ReceiverError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ReceiverError::BadRequest(
            "forwarder IP or hostname is required".to_owned(),
        ));
    }
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let authority = without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .trim();
    if authority.is_empty() {
        return Err(ReceiverError::BadRequest(
            "forwarder IP or hostname is required".to_owned(),
        ));
    }

    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let Some((ipv6_host, after_bracket)) = rest.split_once(']') else {
            return Err(ReceiverError::BadRequest(
                "invalid IPv6 host syntax".to_owned(),
            ));
        };
        let port = if let Some(port_str) = after_bracket.strip_prefix(':') {
            let p = port_str
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| ReceiverError::BadRequest("invalid port number".to_owned()))?;
            Some(p)
        } else if after_bracket.is_empty() {
            None
        } else {
            return Err(ReceiverError::BadRequest(
                "invalid host:port syntax".to_owned(),
            ));
        };
        (format!("[{ipv6_host}]"), port)
    } else if let Some((h, port_str)) = authority.rsplit_once(':')
        && !h.contains(':')
    {
        let p = port_str
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| ReceiverError::BadRequest("invalid port number".to_owned()))?;
        (h.trim().to_owned(), Some(p))
    } else {
        (authority.to_owned(), None)
    };

    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':'))
    {
        return Err(ReceiverError::BadRequest(format!(
            "invalid forwarder host '{host}'"
        )));
    }
    Ok((host, port))
}

pub(crate) async fn probe_forwarder_host(
    client: &reqwest::Client,
    raw_host: &str,
) -> Result<ProbedForwarder, ReceiverError> {
    let (host, explicit_port) = parse_forwarder_host_input(raw_host)?;
    let candidate_ports: Vec<u16> = match explicit_port {
        Some(port) => vec![port],
        None => vec![80, 8080],
    };

    let lookup_host = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host.as_str());
    let resolved_ips: Vec<std::net::IpAddr> = match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::lookup_host((lookup_host, candidate_ports[0])),
    )
    .await
    {
        Ok(Ok(addrs)) => addrs.map(|a| a.ip()).collect(),
        _ => lookup_host
            .parse::<std::net::IpAddr>()
            .into_iter()
            .collect(),
    };

    let mut last_error = None;
    for http_port in &candidate_ports {
        let url = format!("http://{host}:{http_port}/api/v1/status");
        let resp = match client
            .get(&url)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                last_error = Some(format!("{url}: {err}"));
                continue;
            }
        };
        let resp = match resp.error_for_status() {
            Ok(resp) => resp,
            Err(err) => {
                last_error = Some(format!("{url}: {err}"));
                continue;
            }
        };
        let probe: ForwarderStatusProbe = match resp.json().await {
            Ok(probe) => probe,
            Err(err) => {
                last_error = Some(format!("{url}: invalid status JSON: {err}"));
                continue;
            }
        };

        let endpoint_id = probe
            .p2p_endpoint_id
            .or_else(|| probe.server.and_then(|s| s.endpoint_id))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ReceiverError::UpstreamError(format!(
                    "forwarder at {host}:{http_port} does not have a P2P endpoint enabled"
                ))
            })?;
        if endpoint_id.parse::<rt_iroh::EndpointId>().is_err() {
            return Err(ReceiverError::UpstreamError(format!(
                "forwarder at {host}:{http_port} reported an invalid P2P endpoint ID"
            )));
        }

        let advertised_addrs: Vec<std::net::SocketAddr> = probe
            .p2p_direct_addrs
            .iter()
            .filter_map(|s| s.parse::<std::net::SocketAddr>().ok())
            .collect();
        let p2p_port = probe
            .p2p_listen_port
            .or_else(|| advertised_addrs.first().map(std::net::SocketAddr::port))
            .filter(|p| *p > 0)
            .ok_or_else(|| {
                ReceiverError::UpstreamError(format!(
                    "forwarder at {host}:{http_port} did not report a P2P UDP port"
                ))
            })?;

        let host_is_loopback = resolved_ips.iter().any(|ip| ip.is_loopback());
        let mut direct_addrs = Vec::new();
        for &ip in &resolved_ips {
            if !ip.is_unspecified() {
                let candidate = std::net::SocketAddr::new(ip, p2p_port);
                if !direct_addrs.contains(&candidate) {
                    direct_addrs.push(candidate);
                }
            }
        }
        for addr in advertised_addrs {
            if addr.ip().is_unspecified() || addr.port() == 0 {
                continue;
            }
            if addr.ip().is_loopback() && !host_is_loopback {
                continue;
            }
            if !direct_addrs.contains(&addr) {
                direct_addrs.push(addr);
            }
        }
        if direct_addrs.is_empty() {
            return Err(ReceiverError::UpstreamError(format!(
                "could not resolve a reachable IP address for forwarder '{host}'"
            )));
        }

        let canonical_host = match explicit_port {
            Some(p) => format!("{host}:{p}"),
            None if *http_port != 80 => format!("{host}:{http_port}"),
            None => host,
        };
        return Ok(ProbedForwarder {
            endpoint_id,
            display_name: probe.display_name.and_then(optional_non_empty),
            direct_addrs,
            canonical_host,
        });
    }

    Err(ReceiverError::UpstreamError(format!(
        "could not reach forwarder at '{host}' ({})",
        last_error.unwrap_or_else(|| "connection failed".to_owned())
    )))
}

pub async fn add_forwarder_by_host(
    state: &AppState,
    host: String,
) -> Result<String, ReceiverError> {
    let probed = probe_forwarder_host(&state.http_client, &host).await?;
    let endpoint_id = probed.endpoint_id.clone();
    {
        let db = state.storage.db.lock().await;
        db.upsert_known_forwarder(&crate::db::KnownForwarderRecord {
            endpoint_id: endpoint_id.clone(),
            display_name: probed.display_name.clone(),
            direct_addrs: probed
                .direct_addrs
                .iter()
                .map(ToString::to_string)
                .collect(),
            manual_host: Some(probed.canonical_host),
        })
        .map_err(|e| ReceiverError::Internal(e.to_string()))?;
        db.set_forwarder_intent(&endpoint_id, true)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
        state.cache_forwarder_intent(&endpoint_id, true);
    }
    state
        .merge_discovered_forwarder(
            endpoint_id.clone(),
            probed.display_name,
            probed.direct_addrs,
        )
        .await;
    state.wake_reconcile();
    state.request_forwarder_reconnect(endpoint_id.clone()).await;
    state.emit_resync();
    Ok(endpoint_id)
}

pub async fn reconnect_server(state: &AppState) -> Result<(), ReceiverError> {
    state.request_connect().await;
    state.emit_resync();
    Ok(())
}

pub async fn connect_forwarder(state: &AppState, endpoint_id: String) -> Result<(), ReceiverError> {
    {
        let db = state.storage.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, true)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
        state.cache_forwarder_intent(&endpoint_id, true);
    }
    state.recompute_aggregate_connection_state().await;
    state.wake_reconcile();
    state.emit_resync();
    Ok(())
}

pub async fn disconnect_forwarder(
    state: &AppState,
    endpoint_id: String,
) -> Result<(), ReceiverError> {
    {
        let db = state.storage.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, false)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
        state.cache_forwarder_intent(&endpoint_id, false);
    }
    state.recompute_aggregate_connection_state().await;
    state.wake_reconcile();
    state.emit_resync();
    Ok(())
}

pub async fn reconnect_forwarder(
    state: &AppState,
    endpoint_id: String,
) -> Result<(), ReceiverError> {
    {
        let db = state.storage.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, true)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
        state.cache_forwarder_intent(&endpoint_id, true);
    }
    state.recompute_aggregate_connection_state().await;
    state.request_forwarder_reconnect(endpoint_id).await;
    state.emit_resync();
    Ok(())
}

/// Error returned when a remote-config command targets a forwarder that has no
/// live control session, or whose session did not negotiate `CAP_REMOTE_CONFIG`.
fn forwarder_remote_config_unavailable() -> ReceiverError {
    ReceiverError::NotConnected("forwarder not connected or remote config unavailable".to_owned())
}

fn enqueue_config_command(
    tx: &mpsc::Sender<ConfigCommand>,
    command: ConfigCommand,
) -> Result<(), ReceiverError> {
    match tx.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(ReceiverError::UpstreamError(
            "forwarder remote config channel busy".to_owned(),
        )),
        Err(TrySendError::Closed(_)) => Err(forwarder_remote_config_unavailable()),
    }
}

/// Await a remote-config `oneshot` response with a bounded timeout. A dropped
/// sender (control session torn down before replying) and an elapsed timeout
/// both surface as errors so the command never hangs.
async fn await_config_response<T>(rx: oneshot::Receiver<T>) -> Result<T, ReceiverError> {
    match tokio::time::timeout(FORWARDER_CONFIG_TIMEOUT, rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err(ReceiverError::NotConnected(
            "forwarder control session ended before responding".to_owned(),
        )),
        Err(_) => Err(ReceiverError::UpstreamError(
            "timed out waiting for forwarder config response".to_owned(),
        )),
    }
}

/// Fetch the forwarder's full config document over its live P2P control
/// session (requires a negotiated `CAP_REMOTE_CONFIG` session).
pub async fn get_forwarder_config(
    state: &AppState,
    endpoint_id: String,
) -> Result<ForwarderConfigResponse, ReceiverError> {
    let tx = state
        .forwarder_config_tx(&endpoint_id)
        .ok_or_else(forwarder_remote_config_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_config_command(&tx, ConfigCommand::Get { resp: resp_tx })?;
    let response = await_config_response(resp_rx).await?;
    Ok(ForwarderConfigResponse {
        config_json: response.config_json,
        restart_needed: response.restart_needed,
    })
}

/// Replace the forwarder's config with `config_json` (the full document, sent
/// verbatim — no merge/patch) over its live P2P control session.
pub async fn set_forwarder_config(
    state: &AppState,
    endpoint_id: String,
    config_json: String,
) -> Result<ForwarderConfigSetResult, ReceiverError> {
    let tx = state
        .forwarder_config_tx(&endpoint_id)
        .ok_or_else(forwarder_remote_config_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_config_command(
        &tx,
        ConfigCommand::Set {
            config_json,
            resp: resp_tx,
        },
    )?;
    let response = await_config_response(resp_rx).await?;
    Ok(ForwarderConfigSetResult {
        ok: response.ok,
        restart_needed: response.restart_needed,
        error: optional_non_empty(response.error),
    })
}

/// Ask the forwarder to restart over its live P2P control session.
pub async fn restart_forwarder(
    state: &AppState,
    endpoint_id: String,
) -> Result<ForwarderRestartResult, ReceiverError> {
    let tx = state
        .forwarder_config_tx(&endpoint_id)
        .ok_or_else(forwarder_remote_config_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_config_command(&tx, ConfigCommand::Restart { resp: resp_tx })?;
    let response = await_config_response(resp_rx).await?;
    Ok(ForwarderRestartResult {
        accepted: response.accepted,
        error: optional_non_empty(response.error),
    })
}

fn forwarder_wifi_control_unavailable() -> ReceiverError {
    ReceiverError::NotConnected("forwarder not connected or Wi-Fi control unavailable".to_owned())
}

fn enqueue_wifi_command(
    tx: &mpsc::Sender<WifiCommand>,
    command: WifiCommand,
) -> Result<(), ReceiverError> {
    match tx.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(ReceiverError::UpstreamError(
            "forwarder Wi-Fi control channel busy".to_owned(),
        )),
        Err(TrySendError::Closed(_)) => Err(forwarder_wifi_control_unavailable()),
    }
}

async fn await_wifi_response(
    rx: oneshot::Receiver<WifiControlResponse>,
) -> Result<WifiControlResponse, ReceiverError> {
    match tokio::time::timeout(FORWARDER_WIFI_TIMEOUT, rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err(ReceiverError::NotConnected(
            "forwarder control session ended before responding".to_owned(),
        )),
        Err(_) => Err(ReceiverError::UpstreamError(
            "timed out waiting for forwarder Wi-Fi control response".to_owned(),
        )),
    }
}

async fn send_wifi_request(
    state: &AppState,
    endpoint_id: &str,
    request: WifiControlRequest,
) -> Result<WifiControlResponse, ReceiverError> {
    let tx = state
        .forwarder_wifi_control_tx(endpoint_id)
        .ok_or_else(forwarder_wifi_control_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_wifi_command(
        &tx,
        WifiCommand::Request {
            request,
            resp: resp_tx,
        },
    )?;
    await_wifi_response(resp_rx).await
}

fn parse_wifi_payload<T: serde::de::DeserializeOwned>(
    payload_json: Option<&str>,
    context: &str,
) -> Result<T, ReceiverError> {
    let json = payload_json.filter(|s| !s.is_empty()).ok_or_else(|| {
        ReceiverError::UpstreamError(format!("forwarder returned empty {context} payload"))
    })?;
    serde_json::from_str(json).map_err(|error| {
        ReceiverError::UpstreamError(format!("forwarder returned invalid {context}: {error}"))
    })
}

pub async fn forwarder_wifi_status(
    state: &AppState,
    endpoint_id: String,
) -> Result<rt_domain::WifiStatus, ReceiverError> {
    let response = send_wifi_request(
        state,
        &endpoint_id,
        WifiControlRequest {
            request_id: String::new(),
            command: "status".to_owned(),
            ssid: None,
            password: None,
            hidden: None,
            rescan: None,
        },
    )
    .await?;
    if !response.ok {
        return Err(ReceiverError::UpstreamError(response.message));
    }
    parse_wifi_payload(response.payload_json.as_deref(), "wifi status")
}

pub async fn forwarder_wifi_scan(
    state: &AppState,
    endpoint_id: String,
    rescan: Option<bool>,
) -> Result<rt_domain::WifiScanResult, ReceiverError> {
    let response = send_wifi_request(
        state,
        &endpoint_id,
        WifiControlRequest {
            request_id: String::new(),
            command: "scan".to_owned(),
            ssid: None,
            password: None,
            hidden: None,
            rescan,
        },
    )
    .await?;
    if !response.ok {
        return Err(ReceiverError::UpstreamError(response.message));
    }
    parse_wifi_payload(response.payload_json.as_deref(), "wifi scan")
}

pub async fn forwarder_wifi_connect(
    state: &AppState,
    endpoint_id: String,
    ssid: String,
    password: Option<String>,
    hidden: Option<bool>,
) -> Result<rt_domain::WifiActionResult, ReceiverError> {
    let response = send_wifi_request(
        state,
        &endpoint_id,
        WifiControlRequest {
            request_id: String::new(),
            command: "connect".to_owned(),
            ssid: Some(ssid),
            password,
            hidden,
            rescan: None,
        },
    )
    .await?;
    if !response.ok {
        return Err(ReceiverError::UpstreamError(response.message));
    }
    let status = match response.payload_json.as_deref().filter(|s| !s.is_empty()) {
        Some(json) => Some(serde_json::from_str(json).map_err(|error| {
            ReceiverError::UpstreamError(format!(
                "forwarder returned invalid wifi status in connect response: {error}"
            ))
        })?),
        None => None,
    };
    Ok(rt_domain::WifiActionResult {
        ok: true,
        message: response.message,
        status,
    })
}

pub async fn forwarder_wifi_disconnect(
    state: &AppState,
    endpoint_id: String,
) -> Result<rt_domain::WifiActionResult, ReceiverError> {
    let response = send_wifi_request(
        state,
        &endpoint_id,
        WifiControlRequest {
            request_id: String::new(),
            command: "disconnect".to_owned(),
            ssid: None,
            password: None,
            hidden: None,
            rescan: None,
        },
    )
    .await?;
    if !response.ok {
        return Err(ReceiverError::UpstreamError(response.message));
    }
    let status = match response.payload_json.as_deref().filter(|s| !s.is_empty()) {
        Some(json) => Some(serde_json::from_str(json).map_err(|error| {
            ReceiverError::UpstreamError(format!(
                "forwarder returned invalid wifi status in disconnect response: {error}"
            ))
        })?),
        None => None,
    };
    Ok(rt_domain::WifiActionResult {
        ok: true,
        message: response.message,
        status,
    })
}

pub async fn forwarder_wifi_forget(
    state: &AppState,
    endpoint_id: String,
    ssid: String,
) -> Result<rt_domain::WifiActionResult, ReceiverError> {
    let response = send_wifi_request(
        state,
        &endpoint_id,
        WifiControlRequest {
            request_id: String::new(),
            command: "forget".to_owned(),
            ssid: Some(ssid),
            password: None,
            hidden: None,
            rescan: None,
        },
    )
    .await?;
    if !response.ok {
        return Err(ReceiverError::UpstreamError(response.message));
    }
    let status = match response.payload_json.as_deref().filter(|s| !s.is_empty()) {
        Some(json) => Some(serde_json::from_str(json).map_err(|error| {
            ReceiverError::UpstreamError(format!(
                "forwarder returned invalid wifi status in forget response: {error}"
            ))
        })?),
        None => None,
    };
    Ok(rt_domain::WifiActionResult {
        ok: true,
        message: response.message,
        status,
    })
}

fn forwarder_reader_control_unavailable() -> ReceiverError {
    ReceiverError::NotConnected("forwarder not connected or reader control unavailable".to_owned())
}

fn enqueue_reader_command(
    tx: &mpsc::Sender<ReaderCommand>,
    command: ReaderCommand,
) -> Result<(), ReceiverError> {
    match tx.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(ReceiverError::UpstreamError(
            "forwarder reader control channel busy".to_owned(),
        )),
        Err(TrySendError::Closed(_)) => Err(forwarder_reader_control_unavailable()),
    }
}

async fn await_reader_response(
    rx: oneshot::Receiver<ReaderControlResponse>,
) -> Result<ReaderControlResponse, ReceiverError> {
    match tokio::time::timeout(FORWARDER_CONFIG_TIMEOUT, rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err(ReceiverError::NotConnected(
            "forwarder control session ended before responding".to_owned(),
        )),
        Err(_) => Err(ReceiverError::UpstreamError(
            "timed out waiting for reader control response".to_owned(),
        )),
    }
}

async fn reader_control_command(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    action: rt_domain::ReaderControlAction,
) -> Result<ReaderControlResult, ReceiverError> {
    let tx = state
        .forwarder_reader_control_tx(&endpoint_id)
        .ok_or_else(forwarder_reader_control_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_reader_command(
        &tx,
        ReaderCommand::Request {
            stream_id: stream_id.clone(),
            action,
            resp: resp_tx,
        },
    )?;
    let response = await_reader_response(resp_rx).await?;
    let reader_info = match response
        .reader_info_json
        .as_deref()
        .filter(|json| !json.is_empty())
    {
        Some(json) => Some(serde_json::from_str(json).map_err(|error| {
            ReceiverError::UpstreamError(format!(
                "forwarder returned invalid reader_info_json: {error}"
            ))
        })?),
        None => None,
    };
    let has_reader_info = response.reader_info_json.is_some();
    if has_reader_info {
        state.store_forwarder_reader_info_sync(
            &endpoint_id,
            ReaderInfo {
                stream_id: if response.stream_id.is_empty() {
                    stream_id.clone().into_bytes()
                } else {
                    response.stream_id.clone()
                },
                hardware_reader_id: String::new(),
                firmware_version: String::new(),
                model: String::new(),
                reader_info_json: response.reader_info_json.clone(),
            },
        );
    }
    let has_epoch_metadata = response.current_epoch.is_some()
        || response.current_epoch_created_unix_ms.is_some()
        || response.current_epoch_name.is_some();
    if has_epoch_metadata {
        state.store_forwarder_reader_epoch_sync(
            &endpoint_id,
            &stream_id,
            response.current_epoch,
            response.current_epoch_created_unix_ms,
            response.current_epoch_name.clone(),
        );
    }
    if has_reader_info || has_epoch_metadata {
        state.recompute_aggregate_connection_state().await;
    }
    Ok(ReaderControlResult {
        success: response.success,
        message: response.message,
        reader_info,
        current_epoch: response.current_epoch,
        current_epoch_created_unix_ms: response.current_epoch_created_unix_ms,
        current_epoch_name: response.current_epoch_name,
    })
}

pub async fn reader_get_info(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::GetInfo,
    )
    .await
}

pub async fn reader_sync_clock(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SyncClock,
    )
    .await
}

pub async fn reader_set_epoch_name(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    name: Option<String>,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SetEpochName { name },
    )
    .await
}

pub async fn reader_advance_epoch(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    name: Option<String>,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::AdvanceEpoch { name },
    )
    .await
}

pub async fn reader_clear_reads(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::ClearReads,
    )
    .await
}

pub async fn reader_set_read_mode(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    mode: rt_domain::ReadMode,
    timeout: u8,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SetReadMode { mode, timeout },
    )
    .await
}

pub async fn reader_set_tto(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    enabled: bool,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SetTto { enabled },
    )
    .await
}

pub async fn reader_set_recording(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    enabled: bool,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SetRecording { enabled },
    )
    .await
}

pub async fn reader_clear_records(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::ClearRecords,
    )
    .await
}

pub async fn reader_start_download(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::StartDownload,
    )
    .await
}

pub async fn reader_stop_download(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::StopDownload,
    )
    .await
}

pub async fn reader_refresh(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::Refresh,
    )
    .await
}

pub async fn reader_reconnect(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::Reconnect,
    )
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use axum::{Json, Router, routing::get};

    #[test]
    fn parse_forwarder_host_input_accepts_ip_hostname_and_optional_port() {
        assert_eq!(
            parse_forwarder_host_input("192.168.1.50").unwrap(),
            ("192.168.1.50".to_owned(), None)
        );
        assert_eq!(
            parse_forwarder_host_input("rt-fwd-01.local").unwrap(),
            ("rt-fwd-01.local".to_owned(), None)
        );
        assert_eq!(
            parse_forwarder_host_input("http://192.168.1.50:8080/api").unwrap(),
            ("192.168.1.50".to_owned(), Some(8080))
        );
        assert!(parse_forwarder_host_input("   ").is_err());
        assert!(parse_forwarder_host_input("192.168.1.50:0").is_err());
    }

    #[tokio::test]
    async fn add_forwarder_by_host_probes_status_persists_and_merges_without_duplicates() {
        let endpoint_id = rt_iroh::SecretKey::from_bytes(&[99; 32])
            .public()
            .to_string();
        let eid_for_route = endpoint_id.clone();

        let app = Router::new().route(
            "/api/v1/status",
            get(move || {
                let eid = eid_for_route.clone();
                async move {
                    Json(serde_json::json!({
                        "forwarder_id": "fwd-test",
                        "display_name": "Finish Box",
                        "p2p_connected": true,
                        "p2p_endpoint_id": eid,
                        "p2p_listen_port": 7447,
                        "p2p_direct_addrs": ["192.168.1.50:7447"]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv-test".to_owned());
        let returned_id = add_forwarder_by_host(&state, format!("127.0.0.1:{}", addr.port()))
            .await
            .expect("probe should succeed");
        assert_eq!(returned_id, endpoint_id);

        // Merge a second time with a WAN address (simulating server discovery of the same peer)
        state
            .merge_discovered_forwarder(
                endpoint_id.clone(),
                None,
                vec!["203.0.113.10:7447".parse().unwrap()],
            )
            .await;

        let discovered = state.forwarders.discovered_forwarders.read().await;
        assert_eq!(discovered.len(), 1, "must not duplicate the forwarder");
        let entry = discovered.get(&endpoint_id).expect("forwarder entry");
        assert_eq!(entry.display_name.as_deref(), Some("Finish Box"));
        // LAN/loopback addresses must come before the public WAN address
        assert_eq!(entry.direct_addrs[0], "127.0.0.1:7447".parse().unwrap());
        assert!(
            entry
                .direct_addrs
                .contains(&"203.0.113.10:7447".parse().unwrap())
        );
        drop(discovered);

        let known = state
            .storage
            .db
            .lock()
            .await
            .load_known_forwarders()
            .unwrap();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].endpoint_id, endpoint_id);
        assert_eq!(
            known[0].manual_host.as_deref(),
            Some(format!("127.0.0.1:{}", addr.port()).as_str())
        );
    }
}

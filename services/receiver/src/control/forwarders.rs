//! Forwarder connections, connect intent, remote config, and reader-control
//! handlers.

use crate::control_api::{
    AppState, ConfigCommand, DiscoveredForwarders, FORWARDER_CONFIG_TIMEOUT, ForwarderConnState,
    ForwarderLiveStatus, ForwarderRuntimeStatus, ReaderCommand, ReaderLiveStatus, UpsStatusPayload,
    derive_forwarder_state, optional_non_empty, sorted_reader_statuses, subscription_local_ports,
};
use crate::error::ReceiverError;
use rt_p2p_protocol::{ReaderControlResponse, ReaderInfo};
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
    pub subscribed_count: usize,
    pub available_count: usize,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionsResponse {
    pub server: ServerDeviceStatus,
    pub forwarders: Vec<ForwarderConnectionStatus>,
}

pub async fn get_connections(state: &AppState) -> ConnectionsResponse {
    let server = server_device_status(state).await;
    let discovered = state.discovered_forwarders.read().await.clone();
    let (subscriptions, intents) = {
        let db = state.db.lock().await;
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

    let runtime_statuses = state.forwarder_runtime.lock().unwrap().clone();
    let live_statuses = state.forwarder_live_status.lock().unwrap().clone();
    let config_endpoints = state.forwarder_config_endpoints();
    let reader_control_endpoints = state.forwarder_reader_control_endpoints();
    let mut endpoints: BTreeSet<String> = discovered.keys().cloned().collect();
    endpoints.extend(live_statuses.keys().cloned());
    endpoints.extend(config_endpoints.iter().cloned());
    endpoints.extend(reader_control_endpoints.iter().cloned());
    let mut subscribed_counts: HashMap<String, usize> = HashMap::new();
    let local_ports = subscription_local_ports(&subscriptions);
    for subscription in &subscriptions {
        endpoints.insert(subscription.forwarder_endpoint_id.clone());
        *subscribed_counts
            .entry(subscription.forwarder_endpoint_id.clone())
            .or_default() += 1;
    }

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
        forwarders.push(ForwarderConnectionStatus {
            endpoint_id: endpoint_id.clone(),
            display_name: discovered_forwarder.and_then(|forwarder| forwarder.display_name.clone()),
            state: snapshot.state,
            pending: snapshot.pending,
            subscribed_count: subscribed_counts.get(&endpoint_id).copied().unwrap_or(0),
            available_count: discovered_forwarder.map_or(0, |forwarder| forwarder.streams.len()),
            readers: sorted_reader_statuses(&live_status, local_ports, &endpoint_id),
            ups: live_status.ups,
            failed_stream_ids: live_status.failed_streams.into_iter().collect(),
            restart_needed: None,
            remote_config_available: config_endpoints.contains(&endpoint_id),
            reader_control_available: reader_control_endpoints.contains(&endpoint_id),
        });
    }
    forwarders
}

pub async fn reconnect_server(state: &AppState) -> Result<(), ReceiverError> {
    state.request_connect().await;
    state.emit_resync();
    Ok(())
}

pub async fn connect_forwarder(state: &AppState, endpoint_id: String) -> Result<(), ReceiverError> {
    {
        let db = state.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, true)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    state.cache_forwarder_intent(&endpoint_id, true);
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
        let db = state.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, false)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    state.cache_forwarder_intent(&endpoint_id, false);
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
        let db = state.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, true)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    state.cache_forwarder_intent(&endpoint_id, true);
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
    if response.reader_info_json.is_some() {
        state.store_forwarder_reader_info_sync(
            &endpoint_id,
            ReaderInfo {
                stream_id: if response.stream_id.is_empty() {
                    stream_id.into_bytes()
                } else {
                    response.stream_id.clone()
                },
                hardware_reader_id: String::new(),
                firmware_version: String::new(),
                model: String::new(),
                reader_info_json: response.reader_info_json.clone(),
            },
        );
        state.recompute_aggregate_connection_state().await;
    }
    Ok(ReaderControlResult {
        success: response.success,
        message: response.message,
        reader_info,
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
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::AdvanceEpoch,
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

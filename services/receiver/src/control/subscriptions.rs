//! Stream and subscription CRUD, ports, and per-stream settings handlers.

use crate::control_api::{
    AppState, ConnectionState, StreamRef, StreamsResponse, validate_stream_identity,
};
use crate::db::StreamSubscription;
use crate::error::ReceiverError;
use crate::stream_key::LocalStreamKey;
use crate::ui_events::ReceiverUiEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionRequest {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub local_port_override: Option<u16>,
    pub event_type: Option<crate::db::EventType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_ip: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionsBody {
    pub subscriptions: Vec<SubscriptionRequest>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdatePortRequest {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub local_port_override: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EarliestEpochRequest {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub earliest_epoch: i64,
}

#[derive(Debug, Serialize)]
pub struct StreamEpochOption {
    pub stream_epoch: i64,
    /// Operator label for the epoch, from the forwarder catalog.
    pub name: Option<String>,
    /// Earliest reader timestamp of locally received rows in this epoch.
    pub first_seen_at: Option<String>,
    pub created_unix_ms: Option<i64>,
    /// Whether this epoch can be selected as an earliest-epoch override: the
    /// forwarder currently advertises it with a usable start_seq. Local-only
    /// epochs (durably received but no longer advertised) are listed for
    /// context but cannot resolve an override (fail-closed) and must not be
    /// selectable.
    pub selectable: bool,
}

#[derive(Debug, Serialize)]
pub struct StreamEpochsResponse {
    pub epochs: Vec<StreamEpochOption>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventTypeRequest {
    pub event_type: crate::db::EventType,
}

/// Opt a single stream in/out of announcer publishing (opt-in default off).
pub async fn set_stream_announcer_publish(
    state: &AppState,
    forwarder_endpoint_id: &str,
    stream_id: &str,
    publish: bool,
) -> Result<(), ReceiverError> {
    validate_stream_identity(forwarder_endpoint_id, stream_id)?;
    let local_stream_key = LocalStreamKey::new(forwarder_endpoint_id, stream_id);
    {
        let db = state.storage.db.lock().await;
        db.set_stream_announcer_publish(local_stream_key.as_str(), publish)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    // The per-stream publish flag rides on the streams snapshot, so broadcast
    // it for other clients (SSE-only; does not restart stream workers).
    state.emit_streams_snapshot().await;
    Ok(())
}

pub async fn put_earliest_epoch(
    state: &AppState,
    body: EarliestEpochRequest,
) -> Result<(), ReceiverError> {
    if body.earliest_epoch < 1 {
        return Err(ReceiverError::BadRequest(
            "earliest_epoch must be a positive integer (epochs start at 1)".to_owned(),
        ));
    }
    validate_stream_identity(&body.forwarder_endpoint_id, &body.stream_id)?;

    let db = state.storage.db.lock().await;
    match db.save_stream_earliest_epoch(
        &body.forwarder_endpoint_id,
        &body.stream_id,
        body.earliest_epoch,
    ) {
        Ok(()) => {
            drop(db);
            let _ = state.request_reconnect_if_connected().await;
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn get_streams(state: &AppState) -> StreamsResponse {
    state.build_streams_response().await
}

/// Epochs known for a stream, for the earliest-epoch picker: the
/// forwarder-advertised epochs (names, creation times, selectability) merged
/// with the epochs durably received locally (earliest reader timestamp).
pub async fn get_stream_epochs(
    state: &AppState,
    forwarder_endpoint_id: String,
    stream_id: String,
) -> Result<StreamEpochsResponse, ReceiverError> {
    validate_stream_identity(&forwarder_endpoint_id, &stream_id)?;
    let advertised = state
        .merged_epoch_options(&forwarder_endpoint_id, &stream_id)
        .await;
    let local_stream_key = LocalStreamKey::new(&forwarder_endpoint_id, &stream_id);
    let rows = {
        let db = state.storage.db.lock().await;
        db.load_replay_target_epochs(local_stream_key.as_str())
            .map_err(|e| ReceiverError::Internal(e.to_string()))?
    };

    let mut by_epoch = std::collections::BTreeMap::new();
    for option in advertised {
        by_epoch.insert(
            option.stream_epoch,
            StreamEpochOption {
                stream_epoch: option.stream_epoch,
                name: option.name,
                first_seen_at: None,
                created_unix_ms: option.created_unix_ms,
                selectable: option.start_seq.is_some(),
            },
        );
    }
    for (stream_epoch, first_seen_at) in rows {
        by_epoch
            .entry(stream_epoch)
            .and_modify(|option| option.first_seen_at = first_seen_at.clone())
            .or_insert(StreamEpochOption {
                stream_epoch,
                name: None,
                first_seen_at,
                created_unix_ms: None,
                selectable: false,
            });
    }
    Ok(StreamEpochsResponse {
        epochs: by_epoch.into_values().rev().collect(),
    })
}

pub async fn put_subscriptions(
    state: &AppState,
    body: SubscriptionsBody,
) -> Result<(), ReceiverError> {
    let mut seen = std::collections::HashSet::new();
    for s in &body.subscriptions {
        validate_stream_identity(&s.forwarder_endpoint_id, &s.stream_id)?;
        if let Some(0) = s.local_port_override {
            return Err(ReceiverError::BadRequest("port must be 1-65535".to_owned()));
        }
        if !seen.insert((s.forwarder_endpoint_id.clone(), s.stream_id.clone())) {
            return Err(ReceiverError::BadRequest(
                "duplicate subscriptions".to_owned(),
            ));
        }
    }

    let subs: Vec<StreamSubscription> = body
        .subscriptions
        .into_iter()
        .map(|s| StreamSubscription {
            forwarder_endpoint_id: s.forwarder_endpoint_id,
            stream_id: s.stream_id,
            local_port_override: s.local_port_override,
            event_type: s.event_type.unwrap_or(crate::db::EventType::Finish),
            forwarder_id: s.forwarder_id,
            reader_ip: s.reader_ip,
        })
        .collect();
    let mut db = state.storage.db.lock().await;
    match db.replace_stream_subscriptions(&subs) {
        Ok(()) => {
            drop(db);
            state.notify_subscriptions_changed();
            let conn_for_status = state.signals.connection_state.borrow().clone();
            let db = state.storage.db.lock().await;
            let streams_count = db.load_stream_subscriptions().map(|s| s.len()).unwrap_or(0);
            let receiver_id = state.receiver_id.read().await.clone();
            let _ = state.ui.ui_tx.send(ReceiverUiEvent::StatusChanged {
                connection_state: conn_for_status,
                streams_count,
                receiver_id,
            });
            drop(db);
            state.emit_streams_snapshot().await;
            let conn_for_reconnect = state.signals.connection_state.borrow().clone();
            if matches!(
                conn_for_reconnect,
                ConnectionState::Connected
                    | ConnectionState::Connecting
                    | ConnectionState::Disconnected
            ) {
                state.request_connect().await;
            }
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn get_subscriptions(state: &AppState) -> Result<SubscriptionsBody, ReceiverError> {
    let db = state.storage.db.lock().await;
    match db.load_stream_subscriptions() {
        Ok(subscriptions) => Ok(SubscriptionsBody {
            subscriptions: subscriptions
                .into_iter()
                .map(|s| SubscriptionRequest {
                    forwarder_endpoint_id: s.forwarder_endpoint_id,
                    stream_id: s.stream_id,
                    local_port_override: s.local_port_override,
                    event_type: Some(s.event_type),
                    forwarder_id: s.forwarder_id,
                    reader_ip: s.reader_ip,
                })
                .collect(),
        }),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn update_subscription_event_type(
    state: &AppState,
    forwarder_endpoint_id: &str,
    stream_id: &str,
    body: EventTypeRequest,
) -> Result<(), ReceiverError> {
    let db = state.storage.db.lock().await;
    match db.update_stream_subscription_event_type(
        forwarder_endpoint_id,
        stream_id,
        body.event_type,
    ) {
        Ok(Some(changed)) => {
            drop(db);
            // A same-value update is a no-op: signaling would needlessly reset
            // the DBF worker's pass state and force a cross-stream regenerate.
            if changed {
                state.notify_subscriptions_changed();
            }
            Ok(())
        }
        Ok(None) => Err(ReceiverError::BadRequest(
            "subscription not found".to_owned(),
        )),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_cursor(state: &AppState, body: StreamRef) -> Result<(), ReceiverError> {
    validate_stream_identity(&body.forwarder_endpoint_id, &body.stream_id)?;
    let local_stream_key = LocalStreamKey::new(&body.forwarder_endpoint_id, &body.stream_id);
    let db = state.storage.db.lock().await;
    match db.delete_stream_cursor(local_stream_key.as_str()) {
        Ok(()) => Ok(()),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_all_cursors(state: &AppState) -> Result<serde_json::Value, ReceiverError> {
    let db = state.storage.db.lock().await;
    match db.delete_all_cursors() {
        Ok(count) => Ok(serde_json::json!({ "deleted": count })),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

/// Reset one stream's locally received data (events, gap markers, cursor,
/// retention, announcer fence), preserving its subscription and earliest-epoch
/// override, then reconnect so the subscription re-fetches from scratch.
pub async fn admin_reset_stream_data(
    state: &AppState,
    body: StreamRef,
) -> Result<(), ReceiverError> {
    validate_stream_identity(&body.forwarder_endpoint_id, &body.stream_id)?;
    let local_stream_key = LocalStreamKey::new(&body.forwarder_endpoint_id, &body.stream_id);
    {
        let mut db = state.storage.db.lock().await;
        db.reset_stream_data(local_stream_key.as_str())
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    let _ = state.request_reconnect_if_connected().await;
    state.emit_streams_snapshot().await;
    Ok(())
}

pub async fn admin_reset_all_earliest_epochs(
    state: &AppState,
) -> Result<serde_json::Value, ReceiverError> {
    let db = state.storage.db.lock().await;
    match db.delete_all_earliest_epochs() {
        Ok(count) => Ok(serde_json::json!({ "deleted": count })),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_earliest_epoch(
    state: &AppState,
    body: StreamRef,
) -> Result<(), ReceiverError> {
    validate_stream_identity(&body.forwarder_endpoint_id, &body.stream_id)?;
    let local_stream_key = LocalStreamKey::new(&body.forwarder_endpoint_id, &body.stream_id);
    let db = state.storage.db.lock().await;
    match db.delete_stream_earliest_epoch(local_stream_key.as_str()) {
        Ok(()) => Ok(()),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_purge_subscriptions(
    state: &AppState,
) -> Result<serde_json::Value, ReceiverError> {
    let db = state.storage.db.lock().await;
    match db.delete_all_subscriptions() {
        Ok(count) => {
            drop(db);
            state.notify_subscriptions_changed();
            let conn_for_status = state.signals.connection_state.borrow().clone();
            let db = state.storage.db.lock().await;
            let streams_count = db.load_stream_subscriptions().map(|s| s.len()).unwrap_or(0);
            let receiver_id = state.receiver_id.read().await.clone();
            let _ = state.ui.ui_tx.send(ReceiverUiEvent::StatusChanged {
                connection_state: conn_for_status,
                streams_count,
                receiver_id,
            });
            drop(db);
            state.emit_streams_snapshot().await;
            let conn_for_reconnect = state.signals.connection_state.borrow().clone();
            if matches!(
                conn_for_reconnect,
                ConnectionState::Connected
                    | ConnectionState::Connecting
                    | ConnectionState::Disconnected
            ) {
                state.request_connect().await;
            }
            Ok(serde_json::json!({ "deleted": count }))
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_update_port(
    state: &AppState,
    body: UpdatePortRequest,
) -> Result<(), ReceiverError> {
    if let Some(0) = body.local_port_override {
        return Err(ReceiverError::BadRequest("port must be 1-65535".to_owned()));
    }
    let db = state.storage.db.lock().await;
    match db.update_stream_subscription_port(
        &body.forwarder_endpoint_id,
        &body.stream_id,
        body.local_port_override,
    ) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ReceiverError::NotFound("subscription not found".to_owned())),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

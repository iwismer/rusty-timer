use super::response::{bad_request, conflict, internal_error, not_found};
use crate::repo::announcer_config;
use crate::state::{AppState, ForwarderCommand};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use sqlx::Row;
use uuid::Uuid;

pub async fn list_streams(State(state): State<AppState>) -> impl IntoResponse {
    let rows = sqlx::query(
        r#"SELECT s.stream_id,
                  s.forwarder_id,
                  s.reader_ip,
                  s.display_alias,
                  s.forwarder_display_name,
                  s.stream_epoch,
                  s.online,
                  s.created_at,
                  em.name AS current_epoch_name
           FROM streams s
           LEFT JOIN stream_epoch_metadata em
             ON em.stream_id = s.stream_id AND em.stream_epoch = s.stream_epoch
           ORDER BY s.created_at ASC"#,
    )
    .fetch_all(&state.pool)
    .await;

    match rows {
        Ok(rows) => {
            let streams: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    let stream_id: uuid::Uuid = r.get("stream_id");
                    let forwarder_id: String = r.get("forwarder_id");
                    let reader_ip: String = r.get("reader_ip");
                    let display_alias: Option<String> = r.get("display_alias");
                    let forwarder_display_name: Option<String> = r.get("forwarder_display_name");
                    let stream_epoch: i64 = r.get("stream_epoch");
                    let online: bool = r.get("online");
                    let created_at: chrono::DateTime<chrono::Utc> = r.get("created_at");
                    let current_epoch_name: Option<String> = r.get("current_epoch_name");
                    serde_json::json!({
                        "stream_id": stream_id.to_string(),
                        "forwarder_id": forwarder_id,
                        "reader_ip": reader_ip,
                        "display_alias": display_alias,
                        "forwarder_display_name": forwarder_display_name,
                        "stream_epoch": stream_epoch,
                        "current_epoch_name": current_epoch_name,
                        "online": online,
                        "created_at": created_at.to_rfc3339(),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "streams": streams })),
            )
                .into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn patch_stream(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let display_alias = match body.get("display_alias").and_then(|v| v.as_str()) {
        Some(s) => s.to_owned(),
        None => return bad_request("display_alias is required"),
    };
    match sqlx::query!(
        "UPDATE streams SET display_alias = $1 WHERE stream_id = $2 RETURNING stream_id",
        display_alias,
        stream_id
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(_)) => {
            state
                .logger
                .log(format!("stream {stream_id} renamed to \"{display_alias}\""));
            let _ =
                state
                    .dashboard_tx
                    .send(crate::dashboard_events::DashboardEvent::StreamUpdated {
                        stream_id,
                        online: None,
                        stream_epoch: None,
                        display_alias: Some(display_alias.clone()),
                        forwarder_display_name: None,
                    });
            (
                StatusCode::OK,
                Json(serde_json::json!({ "display_alias": display_alias })),
            )
                .into_response()
        }
        Ok(None) => not_found("stream not found"),
        Err(e) => internal_error(e),
    }
}

pub async fn reset_epoch(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    let stream = sqlx::query!(
        "SELECT forwarder_id, stream_epoch, reader_ip FROM streams WHERE stream_id = $1",
        stream_id
    )
    .fetch_optional(&state.pool)
    .await;

    match stream {
        Ok(Some(s)) => {
            let senders = state.forwarder_command_senders.read().await;
            if let Some(tx) = senders.get(&s.forwarder_id) {
                let cmd = rt_protocol::EpochResetCommand {
                    session_id: String::new(),
                    forwarder_id: s.forwarder_id.clone(),
                    reader_ip: s.reader_ip.clone(),
                    new_stream_epoch: (s.stream_epoch + 1) as u64,
                };
                if tx.send(ForwarderCommand::EpochReset(cmd)).await.is_ok() {
                    if let Ok(config) = announcer_config::get_config(&state.pool).await
                        && config.selected_stream_ids.contains(&stream_id)
                    {
                        state.announcer_runtime.write().await.reset();
                    }
                    state
                        .logger
                        .log(format!("epoch reset for stream {stream_id}"));
                    return StatusCode::NO_CONTENT.into_response();
                }
            }
            conflict("forwarder not connected")
        }
        Ok(None) => not_found("stream not found"),
        Err(e) => internal_error(e),
    }
}

pub async fn list_epochs(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify the stream exists
    let stream = sqlx::query("SELECT stream_id FROM streams WHERE stream_id = $1")
        .bind(stream_id)
        .fetch_optional(&state.pool)
        .await;

    match stream {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("stream not found"),
        Err(e) => return internal_error(e),
    }

    // Query distinct epochs from both events and epoch-race mappings with metadata.
    let rows = sqlx::query(
        r#"SELECT epochs.epoch AS epoch,
                  em.name AS name,
                  COUNT(e.stream_id) AS event_count,
                  MIN(e.received_at) AS first_event_at,
                  MAX(e.received_at) AS last_event_at,
                  (epochs.epoch = s.stream_epoch) AS is_current
           FROM (
               SELECT DISTINCT stream_epoch AS epoch
               FROM events
               WHERE stream_id = $1
               UNION
               SELECT stream_epoch AS epoch
               FROM stream_epoch_races
               WHERE stream_id = $1
               UNION
               SELECT stream_epoch AS epoch
               FROM stream_epoch_metadata
               WHERE stream_id = $1 AND name IS NOT NULL
           ) AS epochs
           JOIN streams s ON s.stream_id = $1
           LEFT JOIN stream_epoch_metadata em
             ON em.stream_id = $1 AND em.stream_epoch = epochs.epoch
           LEFT JOIN events e
             ON e.stream_id = $1 AND e.stream_epoch = epochs.epoch
           GROUP BY epochs.epoch, em.name, s.stream_epoch
           ORDER BY epochs.epoch ASC"#,
    )
    .bind(stream_id)
    .fetch_all(&state.pool)
    .await;

    match rows {
        Ok(rows) => {
            let epochs: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    let epoch: i64 = r.get("epoch");
                    let name: Option<String> = r.get("name");
                    let event_count: i64 = r.get("event_count");
                    let first_event_at: Option<chrono::DateTime<chrono::Utc>> =
                        r.get("first_event_at");
                    let last_event_at: Option<chrono::DateTime<chrono::Utc>> =
                        r.get("last_event_at");
                    let is_current: bool = r.get("is_current");
                    serde_json::json!({
                        "epoch": epoch,
                        "name": name,
                        "event_count": event_count,
                        "first_event_at": first_event_at.map(|ts| ts.to_rfc3339()),
                        "last_event_at": last_event_at.map(|ts| ts.to_rfc3339()),
                        "is_current": is_current,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!(epochs))).into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn put_epoch_name(
    State(state): State<AppState>,
    Path((stream_id, epoch)): Path<(Uuid, i64)>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let normalized_name = match body.get("name") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(name)) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        }
        Some(_) => return bad_request("name must be a string or null"),
        None => return bad_request("name is required"),
    };

    let stream_exists = sqlx::query("SELECT 1 FROM streams WHERE stream_id = $1")
        .bind(stream_id)
        .fetch_optional(&state.pool)
        .await;
    match stream_exists {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("stream not found"),
        Err(e) => return internal_error(e),
    }

    let query_result = match normalized_name.clone() {
        Some(name) => sqlx::query(
            "INSERT INTO stream_epoch_metadata (stream_id, stream_epoch, name) VALUES ($1, $2, $3)
                 ON CONFLICT (stream_id, stream_epoch) DO UPDATE
                 SET name = EXCLUDED.name",
        )
        .bind(stream_id)
        .bind(epoch)
        .bind(name)
        .execute(&state.pool)
        .await,
        None => {
            sqlx::query(
                "DELETE FROM stream_epoch_metadata WHERE stream_id = $1 AND stream_epoch = $2",
            )
            .bind(stream_id)
            .bind(epoch)
            .execute(&state.pool)
            .await
        }
    };
    if let Err(e) = query_result {
        return internal_error(e);
    }

    match &normalized_name {
        Some(name) => state.logger.log(format!(
            "epoch {epoch} on stream {stream_id} named \"{name}\""
        )),
        None => state
            .logger
            .log(format!("epoch {epoch} on stream {stream_id} name cleared")),
    }
    let _ = state
        .dashboard_tx
        .send(crate::dashboard_events::DashboardEvent::StreamUpdated {
            stream_id,
            online: None,
            stream_epoch: None,
            display_alias: None,
            forwarder_display_name: None,
        });

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "stream_id": stream_id,
            "stream_epoch": epoch,
            "name": normalized_name,
        })),
    )
        .into_response()
}

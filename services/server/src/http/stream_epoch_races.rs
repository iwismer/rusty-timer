use super::response::{bad_request, conflict, gateway_timeout, internal_error, not_found};
use crate::state::{AppState, ForwarderCommand};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

const ACTIVATE_NEXT_SEND_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn put_stream_epoch_race(
    State(state): State<AppState>,
    Path((stream_id, epoch)): Path<(Uuid, i64)>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let race_id: Option<Uuid> = match body.get("race_id") {
        Some(serde_json::Value::Null) | None => None,
        Some(serde_json::Value::String(s)) => match s.parse::<Uuid>() {
            Ok(id) => Some(id),
            Err(_) => return bad_request("race_id must be a valid UUID or null"),
        },
        Some(_) => return bad_request("race_id must be a string UUID or null"),
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

    if let Some(race_id) = race_id {
        let race_exists = sqlx::query("SELECT 1 FROM races WHERE race_id = $1")
            .bind(race_id)
            .fetch_optional(&state.pool)
            .await;
        match race_exists {
            Ok(Some(_)) => {}
            Ok(None) => return not_found("race not found"),
            Err(e) => return internal_error(e),
        }

        if let Err(e) = sqlx::query(
            "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id) VALUES ($1, $2, $3)
             ON CONFLICT (stream_id, stream_epoch) DO UPDATE
             SET race_id = EXCLUDED.race_id",
        )
        .bind(stream_id)
        .bind(epoch)
        .bind(race_id)
        .execute(&state.pool)
        .await
        {
            return internal_error(e);
        }

        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "stream_id": stream_id,
                "stream_epoch": epoch,
                "race_id": race_id,
            })),
        )
            .into_response();
    }

    if let Err(e) =
        sqlx::query("DELETE FROM stream_epoch_races WHERE stream_id = $1 AND stream_epoch = $2")
            .bind(stream_id)
            .bind(epoch)
            .execute(&state.pool)
            .await
    {
        return internal_error(e);
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "stream_id": stream_id,
            "stream_epoch": epoch,
            "race_id": serde_json::Value::Null,
        })),
    )
        .into_response()
}

pub async fn list_stream_epochs_for_race(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
) -> impl IntoResponse {
    let race_exists = sqlx::query("SELECT 1 FROM races WHERE race_id = $1")
        .bind(race_id)
        .fetch_optional(&state.pool)
        .await;
    match race_exists {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("race not found"),
        Err(e) => return internal_error(e),
    }

    let rows = sqlx::query(
        r#"SELECT ser.stream_id, s.forwarder_id, s.reader_ip, ser.stream_epoch, ser.race_id
           FROM stream_epoch_races ser
           JOIN streams s ON s.stream_id = ser.stream_id
           WHERE ser.race_id = $1
           ORDER BY ser.stream_id ASC, ser.stream_epoch ASC"#,
    )
    .bind(race_id)
    .fetch_all(&state.pool)
    .await;

    match rows {
        Ok(rows) => {
            let mappings: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|row| {
                    serde_json::json!({
                        "stream_id": row.get::<Uuid, _>("stream_id"),
                        "forwarder_id": row.get::<String, _>("forwarder_id"),
                        "reader_ip": row.get::<String, _>("reader_ip"),
                        "stream_epoch": row.get::<i64, _>("stream_epoch"),
                        "race_id": row.get::<Uuid, _>("race_id"),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "mappings": mappings })),
            )
                .into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn activate_next_stream_epoch_for_race(
    State(state): State<AppState>,
    Path((race_id, stream_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    let mapped = sqlx::query(
        "SELECT 1 FROM stream_epoch_races WHERE stream_id = $1 AND race_id = $2 LIMIT 1",
    )
    .bind(stream_id)
    .bind(race_id)
    .fetch_optional(&state.pool)
    .await;
    match mapped {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("stream not mapped to race"),
        Err(e) => return internal_error(e),
    }

    let stream_row = sqlx::query(
        "SELECT forwarder_id, reader_ip, stream_epoch FROM streams WHERE stream_id = $1",
    )
    .bind(stream_id)
    .fetch_optional(&state.pool)
    .await;
    let Some(stream_row) = (match stream_row {
        Ok(row) => row,
        Err(e) => return internal_error(e),
    }) else {
        return not_found("stream not found");
    };

    let forwarder_id: String = stream_row.get("forwarder_id");
    let reader_ip: String = stream_row.get("reader_ip");
    let current_epoch: i64 = stream_row.get("stream_epoch");
    let next_epoch = current_epoch + 1;

    let sender = {
        let senders = state.forwarder_command_senders.read().await;
        senders.get(&forwarder_id).cloned()
    };
    let Some(sender) = sender else {
        return conflict("forwarder not connected");
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return internal_error(e),
    };

    if let Err(e) = sqlx::query(
        "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id) VALUES ($1, $2, $3)
         ON CONFLICT (stream_id, stream_epoch) DO UPDATE
         SET race_id = EXCLUDED.race_id",
    )
    .bind(stream_id)
    .bind(next_epoch)
    .bind(race_id)
    .execute(&mut *tx)
    .await
    {
        return internal_error(e);
    }

    if let Err(e) = tx.commit().await {
        return internal_error(e);
    }

    let cmd = rt_protocol::EpochResetCommand {
        session_id: String::new(),
        forwarder_id,
        reader_ip,
        new_stream_epoch: next_epoch as u64,
    };

    match tokio::time::timeout(
        ACTIVATE_NEXT_SEND_TIMEOUT,
        sender.send(ForwarderCommand::EpochReset(cmd)),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            return conflict(
                "race activation committed, but failed to deliver epoch reset command",
            );
        }
        Err(_) => {
            return gateway_timeout("forwarder command queue is saturated");
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

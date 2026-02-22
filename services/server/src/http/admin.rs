use super::response::{bad_request, conflict, internal_error, not_found, HttpResult};
use crate::dashboard_events::DashboardEvent;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt::Display;
use uuid::Uuid;

type AdminTx<'a> = sqlx::Transaction<'a, sqlx::Postgres>;

async fn begin_tx_or_500(pool: &sqlx::PgPool) -> HttpResult<AdminTx<'_>> {
    pool.begin().await.map_err(internal_error)
}

fn exec_or_500<T, E>(result: Result<T, E>) -> HttpResult<T>
where
    E: Display,
{
    result.map_err(internal_error)
}

async fn commit_or_500(tx: AdminTx<'_>) -> HttpResult {
    tx.commit().await.map_err(internal_error)
}

async fn delete_stream_graph(tx: &mut AdminTx<'_>, stream_id: Option<Uuid>) -> HttpResult<u64> {
    match stream_id {
        Some(stream_id) => {
            exec_or_500(
                sqlx::query!("DELETE FROM events WHERE stream_id = $1", stream_id)
                    .execute(&mut **tx)
                    .await,
            )?;
            exec_or_500(
                sqlx::query!("DELETE FROM stream_metrics WHERE stream_id = $1", stream_id)
                    .execute(&mut **tx)
                    .await,
            )?;
            exec_or_500(
                sqlx::query!(
                    "DELETE FROM receiver_cursors WHERE stream_id = $1",
                    stream_id
                )
                .execute(&mut **tx)
                .await,
            )?;
            let result = exec_or_500(
                sqlx::query!("DELETE FROM streams WHERE stream_id = $1", stream_id)
                    .execute(&mut **tx)
                    .await,
            )?;
            Ok(result.rows_affected())
        }
        None => {
            exec_or_500(sqlx::query!("DELETE FROM events").execute(&mut **tx).await)?;
            exec_or_500(
                sqlx::query!("DELETE FROM stream_metrics")
                    .execute(&mut **tx)
                    .await,
            )?;
            exec_or_500(
                sqlx::query!("DELETE FROM receiver_cursors")
                    .execute(&mut **tx)
                    .await,
            )?;
            let result = exec_or_500(sqlx::query!("DELETE FROM streams").execute(&mut **tx).await)?;
            Ok(result.rows_affected())
        }
    }
}

async fn delete_events_and_cursors(
    tx: &mut AdminTx<'_>,
    stream_id: Option<Uuid>,
    epoch: Option<i64>,
) -> HttpResult {
    match (stream_id, epoch) {
        (Some(stream_id), Some(epoch)) => {
            exec_or_500(
                sqlx::query!(
                    "DELETE FROM events WHERE stream_id = $1 AND stream_epoch = $2",
                    stream_id,
                    epoch
                )
                .execute(&mut **tx)
                .await,
            )?;
            exec_or_500(
                sqlx::query!(
                    "DELETE FROM receiver_cursors WHERE stream_id = $1",
                    stream_id
                )
                .execute(&mut **tx)
                .await,
            )?;
        }
        (Some(stream_id), None) => {
            exec_or_500(
                sqlx::query!("DELETE FROM events WHERE stream_id = $1", stream_id)
                    .execute(&mut **tx)
                    .await,
            )?;
            exec_or_500(
                sqlx::query!(
                    "DELETE FROM receiver_cursors WHERE stream_id = $1",
                    stream_id
                )
                .execute(&mut **tx)
                .await,
            )?;
        }
        (None, None) => {
            exec_or_500(sqlx::query!("DELETE FROM events").execute(&mut **tx).await)?;
            exec_or_500(
                sqlx::query!("DELETE FROM receiver_cursors")
                    .execute(&mut **tx)
                    .await,
            )?;
        }
        (None, Some(_)) => {}
    }

    Ok(())
}

pub async fn list_tokens(State(state): State<AppState>) -> impl IntoResponse {
    let rows = sqlx::query!(
        r#"SELECT token_id, device_type, device_id, created_at, (revoked_at IS NOT NULL) AS "revoked!" FROM device_tokens ORDER BY created_at ASC"#
    )
    .fetch_all(&state.pool)
    .await;

    match rows {
        Ok(rows) => {
            let tokens: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "token_id": r.token_id.to_string(),
                        "device_type": r.device_type,
                        "device_id": r.device_id,
                        "created_at": r.created_at.to_rfc3339(),
                        "revoked": r.revoked,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "tokens": tokens })),
            )
                .into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn revoke_token(
    State(state): State<AppState>,
    Path(token_id): Path<Uuid>,
) -> impl IntoResponse {
    match sqlx::query!(
        "UPDATE device_tokens SET revoked_at = now() WHERE token_id = $1 AND revoked_at IS NULL RETURNING token_id",
        token_id
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(_)) => {
            state.logger.log(format!("token {token_id} revoked"));
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => not_found("token not found or already revoked"),
        Err(e) => internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    pub device_id: String,
    pub device_type: String,
    pub token: Option<String>,
}

pub async fn create_token(
    State(state): State<AppState>,
    Json(body): Json<CreateTokenRequest>,
) -> impl IntoResponse {
    // Validate device_type
    if body.device_type != "forwarder" && body.device_type != "receiver" {
        return bad_request("device_type must be \"forwarder\" or \"receiver\"");
    }

    // Validate device_id
    let device_id = body.device_id.trim().to_owned();
    if device_id.is_empty() {
        return bad_request("device_id must not be empty");
    }

    // Generate or use provided token
    let raw_token = match &body.token {
        Some(t) => {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                return bad_request("token must not be empty or whitespace");
            }
            trimmed.to_owned()
        }
        None => {
            let mut bytes = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut bytes);
            URL_SAFE_NO_PAD.encode(bytes)
        }
    };

    // Hash the token
    let hash = Sha256::digest(raw_token.as_bytes());

    // Insert into DB
    match sqlx::query!(
        r#"INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3) RETURNING token_id"#,
        hash.as_slice(),
        body.device_type,
        &device_id,
    )
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => {
            state.logger.log(format!("token created for {} {}", body.device_type, device_id));
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "token_id": row.token_id.to_string(),
                    "device_id": device_id,
                    "device_type": body.device_type,
                    "token": raw_token,
                })),
            )
                .into_response()
        }
        Err(e) => {
            if let Some(db_err) = e.as_database_error() {
                if db_err.is_unique_violation() {
                    return conflict("a token with this value already exists");
                }
            }
            internal_error(e)
        }
    }
}

pub async fn delete_stream(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    let mut tx = match begin_tx_or_500(&state.pool).await {
        Ok(tx) => tx,
        Err(response) => return response,
    };

    let deleted_streams = match delete_stream_graph(&mut tx, Some(stream_id)).await {
        Ok(rows_affected) => rows_affected,
        Err(response) => return response,
    };

    if deleted_streams == 0 {
        return not_found("stream not found");
    }
    if let Err(response) = commit_or_500(tx).await {
        return response;
    }
    let _ = state.dashboard_tx.send(DashboardEvent::Resync);
    state.logger.log(format!("stream {stream_id} deleted"));
    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_all_streams(State(state): State<AppState>) -> impl IntoResponse {
    let mut tx = match begin_tx_or_500(&state.pool).await {
        Ok(tx) => tx,
        Err(response) => return response,
    };

    if let Err(response) = delete_stream_graph(&mut tx, None).await {
        return response;
    }

    if let Err(response) = commit_or_500(tx).await {
        return response;
    }

    let _ = state.dashboard_tx.send(DashboardEvent::Resync);
    state.logger.log("all streams deleted");
    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_all_events(State(state): State<AppState>) -> impl IntoResponse {
    let mut tx = match begin_tx_or_500(&state.pool).await {
        Ok(tx) => tx,
        Err(response) => return response,
    };

    if let Err(response) = delete_events_and_cursors(&mut tx, None, None).await {
        return response;
    }

    if let Err(response) = commit_or_500(tx).await {
        return response;
    }

    state.logger.log("all events deleted");
    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_stream_events(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    let mut tx = match begin_tx_or_500(&state.pool).await {
        Ok(tx) => tx,
        Err(response) => return response,
    };

    if let Err(response) = delete_events_and_cursors(&mut tx, Some(stream_id), None).await {
        return response;
    }

    if let Err(response) = commit_or_500(tx).await {
        return response;
    }

    state
        .logger
        .log(format!("events deleted for stream {stream_id}"));
    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_epoch_events(
    State(state): State<AppState>,
    Path((stream_id, epoch)): Path<(Uuid, i64)>,
) -> impl IntoResponse {
    if epoch < 1 {
        return bad_request("epoch must be >= 1");
    }

    let mut tx = match begin_tx_or_500(&state.pool).await {
        Ok(tx) => tx,
        Err(response) => return response,
    };

    if let Err(response) = delete_events_and_cursors(&mut tx, Some(stream_id), Some(epoch)).await {
        return response;
    }

    if let Err(response) = commit_or_500(tx).await {
        return response;
    }

    state.logger.log(format!(
        "events deleted for stream {stream_id} epoch {epoch}"
    ));
    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_all_cursors(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query!("DELETE FROM receiver_cursors")
        .execute(&state.pool)
        .await
    {
        Ok(_) => {
            state.logger.log("all receiver cursors cleared");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn list_cursors(State(state): State<AppState>) -> impl IntoResponse {
    let rows = sqlx::query!(
        r#"SELECT receiver_id, stream_id, stream_epoch, last_seq, updated_at
           FROM receiver_cursors ORDER BY receiver_id ASC, stream_id ASC"#
    )
    .fetch_all(&state.pool)
    .await;

    match rows {
        Ok(rows) => {
            let cursors: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "receiver_id": r.receiver_id,
                        "stream_id": r.stream_id.to_string(),
                        "stream_epoch": r.stream_epoch,
                        "last_seq": r.last_seq,
                        "updated_at": r.updated_at.to_rfc3339(),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "cursors": cursors })),
            )
                .into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn delete_receiver_cursors(
    State(state): State<AppState>,
    Path(receiver_id): Path<String>,
) -> impl IntoResponse {
    match sqlx::query!(
        "DELETE FROM receiver_cursors WHERE receiver_id = $1",
        receiver_id
    )
    .execute(&state.pool)
    .await
    {
        Ok(_) => {
            state
                .logger
                .log(format!("cursors cleared for receiver {receiver_id}"));
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn delete_receiver_stream_cursor(
    State(state): State<AppState>,
    Path((receiver_id, stream_id)): Path<(String, Uuid)>,
) -> impl IntoResponse {
    match sqlx::query!(
        "DELETE FROM receiver_cursors WHERE receiver_id = $1 AND stream_id = $2",
        receiver_id,
        stream_id
    )
    .execute(&state.pool)
    .await
    {
        Ok(_) => {
            state.logger.log(format!(
                "cursor cleared for receiver {receiver_id} stream {stream_id}"
            ));
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn delete_all_races(State(state): State<AppState>) -> impl IntoResponse {
    let mut tx = match begin_tx_or_500(&state.pool).await {
        Ok(tx) => tx,
        Err(response) => return response,
    };

    if let Err(response) = exec_or_500(
        sqlx::query!("DELETE FROM forwarder_races")
            .execute(&mut *tx)
            .await,
    ) {
        return response;
    }

    if let Err(response) = exec_or_500(sqlx::query!("DELETE FROM races").execute(&mut *tx).await) {
        return response;
    }

    if let Err(response) = commit_or_500(tx).await {
        return response;
    }

    let _ = state.dashboard_tx.send(DashboardEvent::Resync);
    StatusCode::NO_CONTENT.into_response()
}

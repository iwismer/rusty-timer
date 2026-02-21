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
use rt_protocol::HttpErrorEnvelope;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response(),
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
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(HttpErrorEnvelope {
                code: "NOT_FOUND".to_owned(),
                message: "token not found or already revoked".to_owned(),
                details: None,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response(),
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
        return (
            StatusCode::BAD_REQUEST,
            Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(),
                message: "device_type must be \"forwarder\" or \"receiver\"".to_owned(),
                details: None,
            }),
        )
            .into_response();
    }

    // Validate device_id
    let device_id = body.device_id.trim().to_owned();
    if device_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(),
                message: "device_id must not be empty".to_owned(),
                details: None,
            }),
        )
            .into_response();
    }

    // Generate or use provided token
    let raw_token = match &body.token {
        Some(t) => {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(HttpErrorEnvelope {
                        code: "BAD_REQUEST".to_owned(),
                        message: "token must not be empty or whitespace".to_owned(),
                        details: None,
                    }),
                )
                    .into_response();
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
        Ok(row) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "token_id": row.token_id.to_string(),
                "device_id": device_id,
                "device_type": body.device_type,
                "token": raw_token,
            })),
        )
            .into_response(),
        Err(e) => {
            if let Some(db_err) = e.as_database_error() {
                if db_err.is_unique_violation() {
                    return (
                        StatusCode::CONFLICT,
                        Json(HttpErrorEnvelope {
                            code: "CONFLICT".to_owned(),
                            message: "a token with this value already exists".to_owned(),
                            details: None,
                        }),
                    )
                        .into_response();
                }
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HttpErrorEnvelope {
                    code: "INTERNAL_ERROR".to_owned(),
                    message: e.to_string(),
                    details: None,
                }),
            )
                .into_response()
        }
    }
}

pub async fn delete_stream(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HttpErrorEnvelope {
                    code: "INTERNAL_ERROR".to_owned(),
                    message: e.to_string(),
                    details: None,
                }),
            )
                .into_response()
        }
    };

    if let Err(e) = sqlx::query!("DELETE FROM events WHERE stream_id = $1", stream_id)
        .execute(&mut *tx)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!("DELETE FROM stream_metrics WHERE stream_id = $1", stream_id)
        .execute(&mut *tx)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!(
        "DELETE FROM receiver_cursors WHERE stream_id = $1",
        stream_id
    )
    .execute(&mut *tx)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    match sqlx::query!("DELETE FROM streams WHERE stream_id = $1", stream_id)
        .execute(&mut *tx)
        .await
    {
        Ok(result) => {
            if result.rows_affected() == 0 {
                return (
                    StatusCode::NOT_FOUND,
                    Json(HttpErrorEnvelope {
                        code: "NOT_FOUND".to_owned(),
                        message: "stream not found".to_owned(),
                        details: None,
                    }),
                )
                    .into_response();
            }
            if let Err(e) = tx.commit().await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(HttpErrorEnvelope {
                        code: "INTERNAL_ERROR".to_owned(),
                        message: e.to_string(),
                        details: None,
                    }),
                )
                    .into_response();
            }
            let _ = state.dashboard_tx.send(DashboardEvent::Resync);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response(),
    }
}

pub async fn delete_all_streams(State(state): State<AppState>) -> impl IntoResponse {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HttpErrorEnvelope {
                    code: "INTERNAL_ERROR".to_owned(),
                    message: e.to_string(),
                    details: None,
                }),
            )
                .into_response()
        }
    };

    if let Err(e) = sqlx::query!("DELETE FROM events").execute(&mut *tx).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!("DELETE FROM stream_metrics")
        .execute(&mut *tx)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!("DELETE FROM receiver_cursors")
        .execute(&mut *tx)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!("DELETE FROM streams").execute(&mut *tx).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    let _ = state.dashboard_tx.send(DashboardEvent::Resync);
    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_all_events(State(state): State<AppState>) -> impl IntoResponse {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HttpErrorEnvelope {
                    code: "INTERNAL_ERROR".to_owned(),
                    message: e.to_string(),
                    details: None,
                }),
            )
                .into_response()
        }
    };

    if let Err(e) = sqlx::query!("DELETE FROM events").execute(&mut *tx).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!("DELETE FROM receiver_cursors")
        .execute(&mut *tx)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_stream_events(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HttpErrorEnvelope {
                    code: "INTERNAL_ERROR".to_owned(),
                    message: e.to_string(),
                    details: None,
                }),
            )
                .into_response()
        }
    };

    if let Err(e) = sqlx::query!("DELETE FROM events WHERE stream_id = $1", stream_id)
        .execute(&mut *tx)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!(
        "DELETE FROM receiver_cursors WHERE stream_id = $1",
        stream_id
    )
    .execute(&mut *tx)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_epoch_events(
    State(state): State<AppState>,
    Path((stream_id, epoch)): Path<(Uuid, i64)>,
) -> impl IntoResponse {
    if epoch < 1 {
        return (
            StatusCode::BAD_REQUEST,
            Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(),
                message: "epoch must be >= 1".to_owned(),
                details: None,
            }),
        )
            .into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HttpErrorEnvelope {
                    code: "INTERNAL_ERROR".to_owned(),
                    message: e.to_string(),
                    details: None,
                }),
            )
                .into_response()
        }
    };

    if let Err(e) = sqlx::query!(
        "DELETE FROM events WHERE stream_id = $1 AND stream_epoch = $2",
        stream_id,
        epoch
    )
    .execute(&mut *tx)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!(
        "DELETE FROM receiver_cursors WHERE stream_id = $1",
        stream_id
    )
    .execute(&mut *tx)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_all_cursors(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query!("DELETE FROM receiver_cursors")
        .execute(&state.pool)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response(),
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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response(),
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
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response(),
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
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response(),
    }
}

pub async fn delete_all_races(State(state): State<AppState>) -> impl IntoResponse {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HttpErrorEnvelope {
                    code: "INTERNAL_ERROR".to_owned(),
                    message: e.to_string(),
                    details: None,
                }),
            )
                .into_response()
        }
    };

    if let Err(e) = sqlx::query!("DELETE FROM forwarder_races")
        .execute(&mut *tx)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query!("DELETE FROM races").execute(&mut *tx).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(),
                message: e.to_string(),
                details: None,
            }),
        )
            .into_response();
    }

    let _ = state.dashboard_tx.send(DashboardEvent::Resync);
    StatusCode::NO_CONTENT.into_response()
}

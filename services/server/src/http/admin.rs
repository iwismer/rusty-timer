use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use rt_protocol::HttpErrorEnvelope;
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

    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_all_events(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query!("DELETE FROM events")
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

pub async fn delete_stream_events(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    match sqlx::query!("DELETE FROM events WHERE stream_id = $1", stream_id)
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

pub async fn delete_epoch_events(
    State(state): State<AppState>,
    Path((stream_id, epoch)): Path<(Uuid, i64)>,
) -> impl IntoResponse {
    match sqlx::query!(
        "DELETE FROM events WHERE stream_id = $1 AND stream_epoch = $2",
        stream_id,
        epoch
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

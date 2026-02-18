use crate::{
    repo::events::{count_unique_chips, fetch_stream_metrics},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use rt_protocol::HttpErrorEnvelope;
use uuid::Uuid;

pub async fn get_metrics(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    let metrics = match fetch_stream_metrics(&state.pool, stream_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "stream not found".to_owned(),
                    details: None,
                }),
            )
                .into_response()
        }
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

    let epoch =
        match sqlx::query_scalar::<_, i64>("SELECT stream_epoch FROM streams WHERE stream_id = $1")
            .bind(stream_id)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(Some(epoch)) => epoch,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(HttpErrorEnvelope {
                        code: "NOT_FOUND".to_owned(),
                        message: "stream not found".to_owned(),
                        details: None,
                    }),
                )
                    .into_response()
            }
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

    let unique_chips = match count_unique_chips(&state.pool, stream_id, epoch).await {
        Ok(count) => count,
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

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "raw_count": metrics.raw_count,
            "dedup_count": metrics.dedup_count,
            "retransmit_count": metrics.retransmit_count,
            "lag_ms": metrics.lag_ms,
            "backlog": 0u64,
            "epoch_raw_count": metrics.epoch_raw_count,
            "epoch_dedup_count": metrics.epoch_dedup_count,
            "epoch_retransmit_count": metrics.epoch_retransmit_count,
            "epoch_lag_ms": metrics.epoch_lag_ms,
            "epoch_last_received_at": metrics.epoch_last_received_at.map(|ts| ts.to_rfc3339()),
            "unique_chips": unique_chips,
        })),
    )
        .into_response()
}

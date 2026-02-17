use crate::state::AppState;
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
    let row = sqlx::query!(
        r#"SELECT raw_count, dedup_count, retransmit_count, last_canonical_event_received_at
           FROM stream_metrics WHERE stream_id = $1"#,
        stream_id
    )
    .fetch_optional(&state.pool)
    .await;

    match row {
        Ok(Some(r)) => {
            let lag_ms = r
                .last_canonical_event_received_at
                .map(|ts| (chrono::Utc::now() - ts).num_milliseconds().max(0) as u64);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "raw_count": r.raw_count,
                    "dedup_count": r.dedup_count,
                    "retransmit_count": r.retransmit_count,
                    "lag_ms": lag_ms,
                    "backlog": 0u64,
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(HttpErrorEnvelope {
                code: "NOT_FOUND".to_owned(),
                message: "stream not found".to_owned(),
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

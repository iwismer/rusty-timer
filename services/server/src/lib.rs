pub mod auth;
pub mod db;
pub mod repo;
pub mod state;
pub mod ws_forwarder;
pub mod ws_receiver;

pub use state::AppState;

use axum::{routing::{get, patch, post}, Router};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/ws/v1/forwarders", get(ws_forwarder::ws_forwarder_handler))
        .route("/ws/v1/receivers", get(ws_receiver::ws_receiver_handler))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1/streams", get(api::list_streams))
        .route("/api/v1/streams/:stream_id", patch(api::patch_stream))
        .route("/api/v1/streams/:stream_id/metrics", get(api::get_metrics))
        .route("/api/v1/streams/:stream_id/export.raw", get(api::export_raw))
        .route("/api/v1/streams/:stream_id/export.csv", get(api::export_csv))
        .route("/api/v1/streams/:stream_id/reset-epoch", post(api::reset_epoch))
        .with_state(state)
}

mod health {
    use axum::response::IntoResponse;
    pub async fn healthz() -> impl IntoResponse { "ok" }
    pub async fn readyz() -> impl IntoResponse { "ok" }
}

pub mod api {
    use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
    use rt_protocol::HttpErrorEnvelope;
    use uuid::Uuid;
    use crate::state::AppState;

    pub async fn list_streams(State(state): State<AppState>) -> impl IntoResponse {
        let rows = sqlx::query!(
            r#"SELECT stream_id, forwarder_id, reader_ip, display_alias, stream_epoch, online
               FROM streams ORDER BY created_at ASC"#
        )
        .fetch_all(&state.pool)
        .await;

        match rows {
            Ok(rows) => {
                let streams: Vec<serde_json::Value> = rows.into_iter().map(|r| serde_json::json!({
                    "stream_id": r.stream_id.to_string(),
                    "forwarder_id": r.forwarder_id,
                    "reader_ip": r.reader_ip,
                    "display_alias": r.display_alias,
                    "stream_epoch": r.stream_epoch,
                    "online": r.online,
                })).collect();
                (StatusCode::OK, Json(serde_json::json!({ "streams": streams }))).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
            })).into_response(),
        }
    }

    pub async fn patch_stream(
        State(state): State<AppState>,
        Path(stream_id): Path<Uuid>,
        Json(body): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        let display_alias = match body.get("display_alias").and_then(|v| v.as_str()) {
            Some(s) => s.to_owned(),
            None => return (StatusCode::BAD_REQUEST, Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(), message: "display_alias is required".to_owned(), details: None,
            })).into_response(),
        };
        match sqlx::query!(
            "UPDATE streams SET display_alias = $1 WHERE stream_id = $2 RETURNING stream_id",
            display_alias, stream_id
        ).fetch_optional(&state.pool).await {
            Ok(Some(_)) => (StatusCode::OK, Json(serde_json::json!({ "display_alias": display_alias }))).into_response(),
            Ok(None) => (StatusCode::NOT_FOUND, Json(HttpErrorEnvelope {
                code: "NOT_FOUND".to_owned(), message: "stream not found".to_owned(), details: None,
            })).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
            })).into_response(),
        }
    }

    pub async fn get_metrics(
        State(state): State<AppState>,
        Path(stream_id): Path<Uuid>,
    ) -> impl IntoResponse {
        let row = sqlx::query!(
            r#"SELECT raw_count, dedup_count, retransmit_count, last_canonical_event_received_at
               FROM stream_metrics WHERE stream_id = $1"#,
            stream_id
        ).fetch_optional(&state.pool).await;

        match row {
            Ok(Some(r)) => {
                let lag_ms = r.last_canonical_event_received_at.map(|ts| {
                    (chrono::Utc::now() - ts).num_milliseconds().max(0) as u64
                });
                (StatusCode::OK, Json(serde_json::json!({
                    "raw_count": r.raw_count,
                    "dedup_count": r.dedup_count,
                    "retransmit_count": r.retransmit_count,
                    "lag_ms": lag_ms,
                    "backlog": 0u64,
                }))).into_response()
            }
            Ok(None) => (StatusCode::NOT_FOUND, Json(HttpErrorEnvelope {
                code: "NOT_FOUND".to_owned(), message: "stream not found".to_owned(), details: None,
            })).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
            })).into_response(),
        }
    }

    pub async fn export_raw(_state: State<AppState>, _stream_id: Path<Uuid>) -> impl IntoResponse {
        StatusCode::NOT_IMPLEMENTED
    }

    pub async fn export_csv(_state: State<AppState>, _stream_id: Path<Uuid>) -> impl IntoResponse {
        StatusCode::NOT_IMPLEMENTED
    }

    pub async fn reset_epoch(
        State(state): State<AppState>,
        Path(stream_id): Path<Uuid>,
    ) -> impl IntoResponse {
        let stream = sqlx::query!(
            "SELECT forwarder_id, stream_epoch, reader_ip FROM streams WHERE stream_id = $1",
            stream_id
        ).fetch_optional(&state.pool).await;

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
                    if tx.send(cmd).await.is_ok() {
                        return StatusCode::NO_CONTENT.into_response();
                    }
                }
                (StatusCode::CONFLICT, Json(HttpErrorEnvelope {
                    code: "CONFLICT".to_owned(), message: "forwarder not connected".to_owned(), details: None,
                })).into_response()
            }
            Ok(None) => (StatusCode::NOT_FOUND, Json(HttpErrorEnvelope {
                code: "NOT_FOUND".to_owned(), message: "stream not found".to_owned(), details: None,
            })).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
                code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
            })).into_response(),
        }
    }
}

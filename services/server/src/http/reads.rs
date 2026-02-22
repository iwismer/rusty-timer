use super::response::{bad_request, internal_error, not_found};
use crate::repo::forwarder_races;
use crate::repo::reads::{self, DedupMode};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ReadsQuery {
    #[serde(default = "default_dedup")]
    pub dedup: String,
    #[serde(default = "default_window_secs")]
    pub window_secs: u64,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_order")]
    pub order: String,
}

fn default_order() -> String {
    "desc".to_owned()
}

fn default_dedup() -> String {
    "none".to_owned()
}
fn default_window_secs() -> u64 {
    5
}
fn default_limit() -> usize {
    100
}

/// GET /api/v1/streams/:stream_id/reads
pub async fn get_stream_reads(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
    Query(params): Query<ReadsQuery>,
) -> impl IntoResponse {
    let dedup_mode = match DedupMode::parse(&params.dedup) {
        Some(m) => m,
        None => return bad_request("invalid dedup mode; use none|first|last"),
    };

    // Look up forwarder_id for race assignment
    let forwarder_id = match sqlx::query("SELECT forwarder_id FROM streams WHERE stream_id = $1")
        .bind(stream_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(row)) => row.get::<String, _>("forwarder_id"),
        Ok(None) => return not_found("stream not found"),
        Err(e) => return internal_error(e),
    };

    let race_id = match forwarder_races::get_forwarder_race(&state.pool, &forwarder_id).await {
        Ok(race_id) => race_id,
        Err(e) => return internal_error(e),
    };

    let all_reads = match reads::fetch_stream_reads(&state.pool, stream_id, race_id).await {
        Ok(r) => r,
        Err(e) => return internal_error(e),
    };

    let mut deduped = reads::apply_dedup(all_reads, dedup_mode, params.window_secs);
    if params.order == "desc" {
        deduped.reverse();
    }
    let (page, total) = reads::paginate(deduped, params.limit, params.offset);

    build_reads_response(page, total, params.limit, params.offset)
}

/// GET /api/v1/forwarders/:forwarder_id/reads
pub async fn get_forwarder_reads(
    State(state): State<AppState>,
    Path(forwarder_id): Path<String>,
    Query(params): Query<ReadsQuery>,
) -> impl IntoResponse {
    let dedup_mode = match DedupMode::parse(&params.dedup) {
        Some(m) => m,
        None => return bad_request("invalid dedup mode; use none|first|last"),
    };

    let race_id = match forwarder_races::get_forwarder_race(&state.pool, &forwarder_id).await {
        Ok(race_id) => race_id,
        Err(e) => return internal_error(e),
    };

    let all_reads = match reads::fetch_forwarder_reads(&state.pool, &forwarder_id, race_id).await {
        Ok(r) => r,
        Err(e) => return internal_error(e),
    };

    let mut deduped = reads::apply_dedup(all_reads, dedup_mode, params.window_secs);
    if params.order == "desc" {
        deduped.reverse();
    }
    let (page, total) = reads::paginate(deduped, params.limit, params.offset);

    build_reads_response(page, total, params.limit, params.offset)
}

fn build_reads_response(
    page: Vec<reads::ReadRow>,
    total: usize,
    limit: usize,
    offset: usize,
) -> axum::response::Response {
    let reads_json: Vec<serde_json::Value> = page
        .iter()
        .map(|r| {
            serde_json::json!({
                "stream_id": r.stream_id.to_string(),
                "seq": r.seq,
                "reader_timestamp": r.reader_timestamp,
                "tag_id": r.tag_id,
                "received_at": r.received_at.to_rfc3339(),
                "bib": r.bib,
                "first_name": r.first_name,
                "last_name": r.last_name,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "reads": reads_json,
            "total": total,
            "limit": limit,
            "offset": offset,
        })),
    )
        .into_response()
}

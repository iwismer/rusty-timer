use super::response::{bad_request, internal_error};
use crate::{
    announcer::AnnouncerEvent,
    repo::announcer_config::{self, AnnouncerConfigRow, AnnouncerConfigUpdate},
    state::AppState,
};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};
use chrono::{Duration, Utc};
use futures_util::stream::Stream;
use serde::Deserialize;
use std::{collections::HashSet, convert::Infallible, time::Duration as StdDuration};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tracing::{error, warn};
use uuid::Uuid;

const MIN_LIST_SIZE: i32 = 1;
const MAX_LIST_SIZE: i32 = 500;
const ENABLED_TTL_HOURS: i64 = 24;

#[derive(Debug, Clone, serde::Serialize)]
struct PublicAnnouncerRow {
    announcement_id: u64,
    bib: Option<i32>,
    display_name: String,
    reader_timestamp: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PublicAnnouncerDelta {
    row: PublicAnnouncerRow,
    finisher_count: u64,
}

#[derive(Debug, Deserialize)]
pub struct PutAnnouncerConfigRequest {
    enabled: bool,
    #[serde(default)]
    selected_stream_ids: Vec<Uuid>,
    max_list_size: i32,
}

/// Typed error for announcer config operations, distinguishing validation
/// errors (client fault) from database errors (server fault).
#[derive(Debug)]
pub enum ConfigError {
    Validation(String),
    Database(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Validation(msg) => write!(f, "{msg}"),
            ConfigError::Database(msg) => write!(f, "{msg}"),
        }
    }
}

pub async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    match announcer_config::get_config(&state.pool).await {
        Ok(config) => Json(config_response(config)).into_response(),
        Err(err) => internal_error(err),
    }
}

pub async fn put_config(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    match put_config_value(&state, body).await {
        Ok(config) => Json(config).into_response(),
        Err(ConfigError::Database(e)) => internal_error(std::io::Error::other(e)),
        Err(ConfigError::Validation(e)) => bad_request(&e),
    }
}

pub async fn post_reset(State(state): State<AppState>) -> impl IntoResponse {
    state.reset_announcer_runtime().await;
    StatusCode::NO_CONTENT.into_response()
}

pub async fn get_state(State(state): State<AppState>) -> impl IntoResponse {
    let config = match announcer_config::get_config(&state.pool).await {
        Ok(config) => config,
        Err(err) => return internal_error(err),
    };
    let now = Utc::now();
    let public_enabled = is_public_enabled(&config, now);
    let runtime = state.announcer_runtime.read().await;
    let (finisher_count, rows) = if public_enabled {
        (
            runtime.finisher_count(),
            runtime.rows().iter().cloned().collect::<Vec<_>>(),
        )
    } else {
        (0, Vec::new())
    };

    Json(serde_json::json!({
        "enabled": config.enabled,
        "enabled_until": config.enabled_until.map(|ts| ts.to_rfc3339()),
        "selected_stream_ids": config.selected_stream_ids,
        "max_list_size": config.max_list_size,
        "updated_at": config.updated_at.to_rfc3339(),
        "public_enabled": public_enabled,
        "finisher_count": finisher_count,
        "rows": rows,
    }))
    .into_response()
}

pub async fn get_public_state(State(state): State<AppState>) -> impl IntoResponse {
    let config = match announcer_config::get_config(&state.pool).await {
        Ok(config) => config,
        Err(err) => return internal_error(err),
    };
    let now = Utc::now();
    let public_enabled = is_public_enabled(&config, now);
    let runtime = state.announcer_runtime.read().await;
    let finisher_count = if public_enabled {
        runtime.finisher_count()
    } else {
        0
    };
    let rows = if public_enabled {
        runtime
            .rows()
            .iter()
            .zip(0_u64..)
            .map(|(row, offset)| {
                public_row_from_runtime(row, finisher_count.saturating_sub(offset))
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    Json(serde_json::json!({
        "public_enabled": public_enabled,
        "finisher_count": finisher_count,
        "max_list_size": config.max_list_size,
        "rows": rows,
    }))
    .into_response()
}

pub async fn announcer_sse(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.announcer_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(AnnouncerEvent::Update(delta)) => match serde_json::to_string(&delta) {
            Ok(json) => Some(Ok(Event::default().event("announcer_update").data(json))),
            Err(e) => {
                error!("failed to serialize announcer SSE delta: {e}");
                Some(Ok(Event::default().event("resync").data("{}")))
            }
        },
        Ok(AnnouncerEvent::Resync) => Some(Ok(Event::default().event("resync").data("{}"))),
        Err(e) => {
            warn!("announcer SSE broadcast lag: {e}");
            Some(Ok(Event::default().event("resync").data("{}")))
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(StdDuration::from_secs(15))
            .text("keepalive"),
    )
}

pub async fn public_announcer_sse(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.announcer_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(AnnouncerEvent::Update(delta)) => {
            let payload = PublicAnnouncerDelta {
                row: public_row_from_runtime(&delta.row, delta.finisher_count),
                finisher_count: delta.finisher_count,
            };
            match serde_json::to_string(&payload) {
                Ok(json) => Some(Ok(Event::default().event("announcer_update").data(json))),
                Err(e) => {
                    error!("failed to serialize public announcer SSE delta: {e}");
                    Some(Ok(Event::default().event("resync").data("{}")))
                }
            }
        }
        Ok(AnnouncerEvent::Resync) => Some(Ok(Event::default().event("resync").data("{}"))),
        Err(e) => {
            warn!("public announcer SSE broadcast lag: {e}");
            Some(Ok(Event::default().event("resync").data("{}")))
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(StdDuration::from_secs(15))
            .text("keepalive"),
    )
}

// ---------------------------------------------------------------------------
// Standalone helpers callable from WS proxy handlers
// ---------------------------------------------------------------------------

/// Get the current announcer config as a JSON value.
/// Returns `Err(ConfigError)` on database or internal errors.
pub async fn get_config_value(state: &AppState) -> Result<serde_json::Value, ConfigError> {
    let config = announcer_config::get_config(&state.pool)
        .await
        .map_err(|e| ConfigError::Database(format!("database error: {e}")))?;
    Ok(config_response(config))
}

/// Update the announcer config from a JSON payload.
/// Returns the updated config as JSON, or `Err(ConfigError)` on validation/db errors.
///
/// Side effects: may reset the announcer runtime (if enabled state or selected
/// streams changed) or trigger a resync notification (if max_list_size changed).
pub async fn put_config_value(
    state: &AppState,
    payload: serde_json::Value,
) -> Result<serde_json::Value, ConfigError> {
    let body: PutAnnouncerConfigRequest = serde_json::from_value(payload)
        .map_err(|e| ConfigError::Validation(format!("invalid payload: {e}")))?;

    if body.enabled && body.selected_stream_ids.is_empty() {
        return Err(ConfigError::Validation(
            "enabled announcer requires at least one selected stream".to_owned(),
        ));
    }

    let selected_stream_ids = dedupe_stream_ids(body.selected_stream_ids);
    if body.enabled && !selected_stream_ids.is_empty() {
        let known_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM streams WHERE stream_id = ANY($1)")
                .bind(&selected_stream_ids)
                .fetch_one(&state.pool)
                .await
                .map_err(|e| ConfigError::Database(format!("database error: {e}")))?;
        if usize::try_from(known_count).unwrap_or(0) != selected_stream_ids.len() {
            return Err(ConfigError::Validation(
                "selected_stream_ids contains unknown stream id".to_owned(),
            ));
        }
    }

    let previous = announcer_config::get_config(&state.pool)
        .await
        .map_err(|e| ConfigError::Database(format!("database error: {e}")))?;

    let now = Utc::now();
    let enabled_until = if body.enabled {
        if !previous.enabled {
            Some(now + Duration::hours(ENABLED_TTL_HOURS))
        } else {
            previous.enabled_until
        }
    } else {
        None
    };

    let max_list_size = body.max_list_size.clamp(MIN_LIST_SIZE, MAX_LIST_SIZE);
    let update = AnnouncerConfigUpdate {
        enabled: body.enabled,
        enabled_until,
        selected_stream_ids: selected_stream_ids.clone(),
        max_list_size,
    };
    let updated = announcer_config::set_config(&state.pool, &update)
        .await
        .map_err(|e| ConfigError::Database(format!("database error: {e}")))?;

    if should_reset_runtime(&previous, &updated, &selected_stream_ids) {
        state.reset_announcer_runtime().await;
    } else if previous.max_list_size != updated.max_list_size {
        state.notify_announcer_resync();
    }

    Ok(config_response(updated))
}

fn dedupe_stream_ids(stream_ids: Vec<Uuid>) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    stream_ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect()
}

fn should_reset_runtime(
    previous: &AnnouncerConfigRow,
    updated: &AnnouncerConfigRow,
    updated_stream_ids: &[Uuid],
) -> bool {
    previous.enabled != updated.enabled || previous.selected_stream_ids != updated_stream_ids
}

fn is_public_enabled(config: &AnnouncerConfigRow, now: chrono::DateTime<Utc>) -> bool {
    if !config.enabled || config.selected_stream_ids.is_empty() {
        return false;
    }
    config.enabled_until.map(|ts| ts > now).unwrap_or(true)
}

fn config_response(config: AnnouncerConfigRow) -> serde_json::Value {
    serde_json::json!({
        "enabled": config.enabled,
        "enabled_until": config.enabled_until.map(|ts| ts.to_rfc3339()),
        "selected_stream_ids": config.selected_stream_ids,
        "max_list_size": config.max_list_size,
        "updated_at": config.updated_at.to_rfc3339(),
        "public_enabled": is_public_enabled(&config, Utc::now()),
    })
}

fn public_row_from_runtime(
    row: &crate::announcer::AnnouncerRow,
    announcement_id: u64,
) -> PublicAnnouncerRow {
    PublicAnnouncerRow {
        announcement_id,
        bib: row.bib,
        display_name: row.display_name.clone(),
        reader_timestamp: row.reader_timestamp.clone(),
    }
}

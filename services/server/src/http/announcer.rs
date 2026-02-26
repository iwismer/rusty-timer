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
use uuid::Uuid;

const MIN_LIST_SIZE: i32 = 1;
const MAX_LIST_SIZE: i32 = 500;
const ENABLED_TTL_HOURS: i64 = 24;

#[derive(Debug, Deserialize)]
pub struct PutAnnouncerConfigRequest {
    enabled: bool,
    #[serde(default)]
    selected_stream_ids: Vec<Uuid>,
    max_list_size: i32,
}

pub async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    match announcer_config::get_config(&state.pool).await {
        Ok(config) => Json(config_response(config)).into_response(),
        Err(err) => internal_error(err),
    }
}

pub async fn put_config(
    State(state): State<AppState>,
    Json(body): Json<PutAnnouncerConfigRequest>,
) -> impl IntoResponse {
    if body.enabled && body.selected_stream_ids.is_empty() {
        return bad_request("enabled announcer requires at least one selected stream");
    }

    let selected_stream_ids = dedupe_stream_ids(body.selected_stream_ids);
    if !selected_stream_ids.is_empty() {
        let known_count = match sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM streams WHERE stream_id = ANY($1)",
        )
        .bind(&selected_stream_ids)
        .fetch_one(&state.pool)
        .await
        {
            Ok(count) => count,
            Err(err) => return internal_error(err),
        };
        if usize::try_from(known_count).unwrap_or(0) != selected_stream_ids.len() {
            return bad_request("selected_stream_ids contains unknown stream id");
        }
    }

    let previous = match announcer_config::get_config(&state.pool).await {
        Ok(config) => config,
        Err(err) => return internal_error(err),
    };

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
    let updated = match announcer_config::set_config(&state.pool, &update).await {
        Ok(config) => config,
        Err(err) => return internal_error(err),
    };

    if should_reset_runtime(&previous, &updated, &selected_stream_ids) {
        state.reset_announcer_runtime().await;
    }

    Json(config_response(updated)).into_response()
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

    Json(serde_json::json!({
        "enabled": config.enabled,
        "enabled_until": config.enabled_until.map(|ts| ts.to_rfc3339()),
        "selected_stream_ids": config.selected_stream_ids,
        "max_list_size": config.max_list_size,
        "updated_at": config.updated_at.to_rfc3339(),
        "public_enabled": public_enabled,
        "finisher_count": runtime.finisher_count(),
        "rows": runtime.rows().iter().cloned().collect::<Vec<_>>(),
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
            Err(_) => None,
        },
        Ok(AnnouncerEvent::Resync) => Some(Ok(Event::default().event("resync").data("{}"))),
        Err(_) => Some(Ok(Event::default().event("resync").data("{}"))),
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(StdDuration::from_secs(15))
            .text("keepalive"),
    )
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

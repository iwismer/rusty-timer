use super::response::{bad_request, internal_error, not_found};
use crate::dashboard_events::DashboardEvent;
use crate::repo::forwarder_races as repo;
use crate::repo::races as race_repo;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

/// GET /api/v1/forwarder-races
pub async fn list_forwarder_races(State(state): State<AppState>) -> impl IntoResponse {
    match repo::list_forwarder_races(&state.pool).await {
        Ok(rows) => {
            let assignments: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "forwarder_id": r.forwarder_id,
                        "race_id": r.race_id.map(|id| id.to_string()),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "assignments": assignments })),
            )
                .into_response()
        }
        Err(e) => internal_error(e),
    }
}

/// GET /api/v1/forwarders/:forwarder_id/race
pub async fn get_forwarder_race(
    State(state): State<AppState>,
    Path(forwarder_id): Path<String>,
) -> impl IntoResponse {
    match repo::get_forwarder_race(&state.pool, &forwarder_id).await {
        Ok(race_id) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "forwarder_id": forwarder_id,
                "race_id": race_id.map(|id| id.to_string()),
            })),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

/// PUT /api/v1/forwarders/:forwarder_id/race
pub async fn set_forwarder_race(
    State(state): State<AppState>,
    Path(forwarder_id): Path<String>,
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

    // Validate race exists if assigning
    if let Some(rid) = race_id {
        match race_repo::race_exists(&state.pool, rid).await {
            Ok(false) => return not_found("race not found"),
            Err(e) => return internal_error(e),
            Ok(true) => {}
        }
    }

    if let Err(e) = repo::set_forwarder_race(&state.pool, &forwarder_id, race_id).await {
        return internal_error(e);
    }

    // Broadcast SSE event
    let _ = state
        .dashboard_tx
        .send(DashboardEvent::ForwarderRaceAssigned {
            forwarder_id: forwarder_id.clone(),
            race_id,
        });

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "forwarder_id": forwarder_id,
            "race_id": race_id.map(|id| id.to_string()),
        })),
    )
        .into_response()
}

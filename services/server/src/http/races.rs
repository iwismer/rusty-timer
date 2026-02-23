use super::response::{bad_request, conflict, internal_error, not_found};
use crate::repo::races as repo;
use crate::state::AppState;
use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use uuid::Uuid;

pub async fn list_races(State(state): State<AppState>) -> impl IntoResponse {
    match repo::list_races(&state.pool).await {
        Ok(rows) => {
            let races: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "race_id": r.race_id.to_string(),
                        "name": r.name,
                        "created_at": r.created_at.to_rfc3339(),
                        "participant_count": r.participant_count,
                        "chip_count": r.chip_count,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!({ "races": races }))).into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub async fn create_race(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return bad_request("name is required");
            }
            trimmed.to_owned()
        }
        None => return bad_request("name is required"),
    };

    match repo::create_race(&state.pool, &name).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "race_id": row.race_id.to_string(),
                "name": row.name,
                "created_at": row.created_at.to_rfc3339(),
                "participant_count": row.participant_count,
                "chip_count": row.chip_count,
            })),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

pub async fn delete_race(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
) -> impl IntoResponse {
    if state.has_active_receiver_session_for_race(race_id).await {
        return conflict("race is currently selected by an active receiver session");
    }

    match repo::delete_race(&state.pool, race_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("race not found"),
        Err(e) => internal_error(e),
    }
}

pub async fn list_participants(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
) -> impl IntoResponse {
    // Check race exists
    match repo::race_exists(&state.pool, race_id).await {
        Ok(false) => return not_found("race not found"),
        Err(e) => return internal_error(e),
        Ok(true) => {}
    }

    let participants = match repo::list_participants(&state.pool, race_id).await {
        Ok(rows) => rows,
        Err(e) => return internal_error(e),
    };

    let unmatched = match repo::list_unmatched_chips(&state.pool, race_id).await {
        Ok(rows) => rows,
        Err(e) => return internal_error(e),
    };

    let participants_json: Vec<serde_json::Value> = participants
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "bib": p.bib,
                "first_name": p.first_name,
                "last_name": p.last_name,
                "gender": p.gender,
                "affiliation": p.affiliation,
                "chip_ids": p.chip_ids,
            })
        })
        .collect();

    let unmatched_json: Vec<serde_json::Value> = unmatched
        .into_iter()
        .map(|u| {
            serde_json::json!({
                "chip_id": u.chip_id,
                "bib": u.bib,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "participants": participants_json,
            "chips_without_participant": unmatched_json,
        })),
    )
        .into_response()
}

pub async fn upload_participants(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Check race exists
    match repo::race_exists(&state.pool, race_id).await {
        Ok(false) => return not_found("race not found"),
        Err(e) => return internal_error(e),
        Ok(true) => {}
    }

    let bytes = match extract_file_bytes(&mut multipart).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };

    let parsed = match timer_core::util::io::parse_participant_bytes(&bytes) {
        Ok(parsed) => parsed,
        Err(e) => {
            return bad_request(format!("invalid participant file: {}", e));
        }
    };
    if parsed.is_empty() {
        return bad_request("participant file has no valid rows");
    }

    let tuples: Vec<(i32, String, String, String, Option<String>)> = parsed
        .into_iter()
        .map(|p| {
            (
                p.bib,
                p.first_name,
                p.last_name,
                p.gender.to_string(),
                p.affiliation,
            )
        })
        .collect();

    let refs: Vec<(i32, &str, &str, &str, Option<&str>)> = tuples
        .iter()
        .map(|(bib, first, last, gender, affil)| {
            (
                *bib,
                first.as_str(),
                last.as_str(),
                gender.as_str(),
                affil.as_deref(),
            )
        })
        .collect();

    match repo::replace_participants(&state.pool, race_id, &refs).await {
        Ok(count) => (
            StatusCode::OK,
            Json(serde_json::json!({ "imported": count })),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

pub async fn upload_chips(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Check race exists
    match repo::race_exists(&state.pool, race_id).await {
        Ok(false) => return not_found("race not found"),
        Err(e) => return internal_error(e),
        Ok(true) => {}
    }

    let bytes = match extract_file_bytes(&mut multipart).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };

    let parsed = match timer_core::util::io::parse_bibchip_bytes(&bytes) {
        Ok(parsed) => parsed,
        Err(e) => {
            return bad_request(format!("invalid bibchip file: {}", e));
        }
    };
    if parsed.is_empty() {
        return bad_request("bibchip file has no valid rows");
    }

    let tuples: Vec<(String, i32)> = parsed.into_iter().map(|c| (c.id, c.bib)).collect();

    let refs: Vec<(&str, i32)> = tuples.iter().map(|(id, bib)| (id.as_str(), *bib)).collect();

    match repo::replace_chips(&state.pool, race_id, &refs).await {
        Ok(count) => (
            StatusCode::OK,
            Json(serde_json::json!({ "imported": count })),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

async fn extract_file_bytes(multipart: &mut Multipart) -> Result<Vec<u8>, Response> {
    match multipart.next_field().await {
        Ok(Some(field)) => match field.bytes().await {
            Ok(bytes) => Ok(bytes.to_vec()),
            Err(e) => Err(bad_request(format!("failed to read file: {}", e))),
        },
        Ok(None) => Err(bad_request("no file uploaded")),
        Err(e) => Err(bad_request(format!("multipart error: {}", e))),
    }
}

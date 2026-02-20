use crate::repo::races as repo;
use crate::state::AppState;
use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rt_protocol::HttpErrorEnvelope;
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

pub async fn create_race(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(HttpErrorEnvelope {
                        code: "BAD_REQUEST".to_owned(),
                        message: "name is required".to_owned(),
                        details: None,
                    }),
                )
                    .into_response();
            }
            trimmed.to_owned()
        }
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(HttpErrorEnvelope {
                    code: "BAD_REQUEST".to_owned(),
                    message: "name is required".to_owned(),
                    details: None,
                }),
            )
                .into_response()
        }
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

pub async fn delete_race(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
) -> impl IntoResponse {
    match repo::delete_race(&state.pool, race_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(HttpErrorEnvelope {
                code: "NOT_FOUND".to_owned(),
                message: "race not found".to_owned(),
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

pub async fn list_participants(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
) -> impl IntoResponse {
    // Check race exists
    match repo::race_exists(&state.pool, race_id).await {
        Ok(false) => {
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "race not found".to_owned(),
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
        Ok(true) => {}
    }

    let participants = match repo::list_participants(&state.pool, race_id).await {
        Ok(rows) => rows,
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

    let unmatched = match repo::list_unmatched_chips(&state.pool, race_id).await {
        Ok(rows) => rows,
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
        Ok(false) => {
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "race not found".to_owned(),
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
        Ok(true) => {}
    }

    let bytes = match extract_file_bytes(&mut multipart).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };

    let parsed = match timer_core::util::io::parse_participant_bytes(&bytes) {
        Ok(parsed) => parsed,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(HttpErrorEnvelope {
                    code: "BAD_REQUEST".to_owned(),
                    message: format!("invalid participant file: {}", e),
                    details: None,
                }),
            )
                .into_response();
        }
    };
    if parsed.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(),
                message: "participant file has no valid rows".to_owned(),
                details: None,
            }),
        )
            .into_response();
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

pub async fn upload_chips(
    State(state): State<AppState>,
    Path(race_id): Path<Uuid>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Check race exists
    match repo::race_exists(&state.pool, race_id).await {
        Ok(false) => {
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "race not found".to_owned(),
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
        Ok(true) => {}
    }

    let bytes = match extract_file_bytes(&mut multipart).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };

    let parsed = match timer_core::util::io::parse_bibchip_bytes(&bytes) {
        Ok(parsed) => parsed,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(HttpErrorEnvelope {
                    code: "BAD_REQUEST".to_owned(),
                    message: format!("invalid bibchip file: {}", e),
                    details: None,
                }),
            )
                .into_response();
        }
    };
    if parsed.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(),
                message: "bibchip file has no valid rows".to_owned(),
                details: None,
            }),
        )
            .into_response();
    }

    let tuples: Vec<(String, i32)> = parsed.into_iter().map(|c| (c.id, c.bib)).collect();

    let refs: Vec<(&str, i32)> = tuples.iter().map(|(id, bib)| (id.as_str(), *bib)).collect();

    match repo::replace_chips(&state.pool, race_id, &refs).await {
        Ok(count) => (
            StatusCode::OK,
            Json(serde_json::json!({ "imported": count })),
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

async fn extract_file_bytes(multipart: &mut Multipart) -> Result<Vec<u8>, Response> {
    match multipart.next_field().await {
        Ok(Some(field)) => match field.bytes().await {
            Ok(bytes) => Ok(bytes.to_vec()),
            Err(e) => Err((
                StatusCode::BAD_REQUEST,
                Json(HttpErrorEnvelope {
                    code: "BAD_REQUEST".to_owned(),
                    message: format!("failed to read file: {}", e),
                    details: None,
                }),
            )
                .into_response()),
        },
        Ok(None) => Err((
            StatusCode::BAD_REQUEST,
            Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(),
                message: "no file uploaded".to_owned(),
                details: None,
            }),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(HttpErrorEnvelope {
                code: "BAD_REQUEST".to_owned(),
                message: format!("multipart error: {}", e),
                details: None,
            }),
        )
            .into_response()),
    }
}

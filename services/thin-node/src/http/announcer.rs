//! Announcer row push and fenced source generation endpoints.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::announcer::{AnnouncerInputEvent, AnnouncerRuntime};
use crate::http::{AppState, register};
use crate::registry::{self, AnnouncerRowRecord, AnnouncerStorageError};

const MAX_ANNOUNCER_ROWS: usize = 25;

#[derive(Debug, Deserialize)]
pub struct PushRowRequest {
    pub announcer_source_generation: u64,
    pub stream_id: String,
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_unix_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct PushRowResponse {
    pub announcer_source_generation: u64,
    pub finisher_count: u64,
}

#[derive(Debug, Serialize)]
pub struct TakeoverResponse {
    pub announcer_source_generation: u64,
}

pub async fn push_row(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PushRowRequest>,
) -> Response {
    if !register::authorized(&headers, &state.provisioning_token_hash) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    if Utc
        .timestamp_millis_opt(req.received_unix_ms)
        .single()
        .is_none()
    {
        return (StatusCode::BAD_REQUEST, "invalid received_unix_ms").into_response();
    }

    let record = AnnouncerRowRecord {
        announcer_source_generation: req.announcer_source_generation,
        stream_id: req.stream_id,
        seq: req.seq,
        chip_id: req.chip_id,
        bib: req.bib,
        display_name: req.display_name,
        reader_timestamp: req.reader_timestamp,
        received_unix_ms: req.received_unix_ms,
    };

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::upsert_announcer_row(&conn, &record).and_then(|()| {
            let rows = registry::list_announcer_rows_ordered(&conn)?;
            Ok(rebuild_runtime(&state.announcer_runtime, rows))
        })
    };

    match result {
        Ok(finisher_count) => (
            StatusCode::OK,
            Json(PushRowResponse {
                announcer_source_generation: record.announcer_source_generation,
                finisher_count,
            }),
        )
            .into_response(),
        Err(AnnouncerStorageError::StaleGeneration { current_generation }) => (
            StatusCode::CONFLICT,
            format!("stale announcer generation; current={current_generation}"),
        )
            .into_response(),
        Err(AnnouncerStorageError::ValueOutOfRange(field)) => {
            (StatusCode::BAD_REQUEST, format!("{field} out of range")).into_response()
        }
        Err(AnnouncerStorageError::Sqlite(err)) => {
            tracing::error!(error = %err, "announcer row push failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn takeover(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !register::authorized(&headers, &state.provisioning_token_hash) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::takeover_announcer_source(&conn)
    };

    match result {
        Ok(announcer_source_generation) => (
            StatusCode::OK,
            Json(TakeoverResponse {
                announcer_source_generation,
            }),
        )
            .into_response(),
        Err(AnnouncerStorageError::Sqlite(err)) => {
            tracing::error!(error = %err, "announcer takeover failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(
            AnnouncerStorageError::StaleGeneration { .. }
            | AnnouncerStorageError::ValueOutOfRange(_),
        ) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn rebuild_runtime(
    runtime: &std::sync::Mutex<AnnouncerRuntime>,
    rows: Vec<AnnouncerRowRecord>,
) -> u64 {
    let mut runtime = runtime.lock().expect("announcer runtime mutex poisoned");
    runtime.reset();
    for row in rows {
        let Some(received_at) = Utc.timestamp_millis_opt(row.received_unix_ms).single() else {
            continue;
        };
        let event = AnnouncerInputEvent {
            stream_id: row.stream_id,
            seq: row.seq,
            chip_id: row.chip_id,
            bib: row.bib,
            display_name: row.display_name,
            reader_timestamp: row.reader_timestamp,
            received_at,
        };
        runtime.ingest(event, MAX_ANNOUNCER_ROWS);
    }
    runtime.finisher_count()
}

#[cfg(test)]
mod tests {
    use crate::http::{AppState, router};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use rusqlite::Connection;
    use std::sync::TryLockError;
    use tower::ServiceExt;

    const PROV_TOKEN: &str = "prov-secret";

    fn test_state() -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        crate::registry::migrate(&conn).unwrap();
        AppState::new(conn, PROV_TOKEN)
    }

    fn json_request(uri: &str, body: &serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Authorization", format!("Bearer {PROV_TOKEN}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    async fn response_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn row_body(seq: u64, generation: u64, received_unix_ms: i64) -> serde_json::Value {
        serde_json::json!({
            "announcer_source_generation": generation,
            "stream_id": "finish-line",
            "seq": seq,
            "chip_id": format!("chip-{seq}"),
            "bib": 1000 + seq,
            "display_name": format!("Runner {seq}"),
            "reader_timestamp": "10:00:00",
            "received_unix_ms": received_unix_ms
        })
    }

    #[tokio::test]
    async fn upsert_idempotent() {
        let state = test_state();
        let app = router(state.clone());
        let body = row_body(7, 0, 1_000);

        let first = app
            .clone()
            .oneshot(json_request("/announcer/rows", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app
            .oneshot(json_request("/announcer/rows", &body))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);

        let conn = state.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM announcer_rows WHERE stream_id = 'finish-line' AND seq = 7",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn sequential_pushes_report_and_rebuild_full_finisher_count() {
        let state = test_state();
        let app = router(state.clone());

        let first = app
            .clone()
            .oneshot(json_request("/announcer/rows", &row_body(1, 0, 1_000)))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = response_json(first).await;
        assert_eq!(first_body["finisher_count"], 1);

        let second = app
            .oneshot(json_request("/announcer/rows", &row_body(2, 0, 2_000)))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = response_json(second).await;
        assert_eq!(second_body["finisher_count"], 2);

        let runtime = state.announcer_runtime.lock().unwrap();
        assert_eq!(runtime.finisher_count(), 2);
        assert_eq!(runtime.rows().len(), 2);
    }

    #[test]
    fn row_push_holds_registry_lock_until_runtime_rebuild_completes() {
        let state = test_state();
        let app = router(state.clone());
        let body = row_body(1, 0, 1_000);
        let runtime_guard = state.announcer_runtime.lock().unwrap();

        let push = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let response = app
                    .oneshot(json_request("/announcer/rows", &body))
                    .await
                    .unwrap();
                let status = response.status();
                let body = response_json(response).await;
                (status, body)
            })
        });

        let mut consecutive_locked_checks = 0;
        for _ in 0..10_000 {
            assert!(
                !push.is_finished(),
                "push completed while runtime rebuild was blocked"
            );

            match state.conn.try_lock() {
                Ok(conn) => {
                    consecutive_locked_checks = 0;
                    let count: i64 = conn
                        .query_row("SELECT COUNT(*) FROM announcer_rows", [], |row| row.get(0))
                        .unwrap();
                    assert_eq!(
                        count, 0,
                        "registry lock was released after persisting a row but before runtime rebuild completed"
                    );
                }
                Err(TryLockError::WouldBlock) => {
                    consecutive_locked_checks += 1;
                    if consecutive_locked_checks >= 1_000 {
                        break;
                    }
                }
                Err(TryLockError::Poisoned(err)) => panic!("registry mutex poisoned: {err}"),
            }

            std::thread::yield_now();
        }

        assert_eq!(
            consecutive_locked_checks, 1_000,
            "push did not hold registry lock while waiting to rebuild runtime"
        );

        drop(runtime_guard);
        let (status, body) = push.join().unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["finisher_count"], 1);
    }

    #[tokio::test]
    async fn older_generation_rejected() {
        let state = test_state();
        let app = router(state.clone());

        let takeover = app
            .clone()
            .oneshot(json_request("/announcer/takeover", &serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(takeover.status(), StatusCode::OK);

        let rejected = app
            .oneshot(json_request("/announcer/rows", &row_body(1, 0, 1_000)))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::CONFLICT);

        let conn = state.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn takeover_bumps_gen() {
        let state = test_state();
        let app = router(state);

        let first = app
            .clone()
            .oneshot(json_request("/announcer/takeover", &serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = response_json(first).await;
        assert_eq!(first_body["announcer_source_generation"], 1);

        let second = app
            .oneshot(json_request("/announcer/takeover", &serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = response_json(second).await;
        assert_eq!(second_body["announcer_source_generation"], 2);
    }
}

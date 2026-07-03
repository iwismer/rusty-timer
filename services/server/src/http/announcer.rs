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
use crate::http::validate::{MAX_ID_LEN, MAX_NAME_LEN, MAX_TIMESTAMP_LEN, check_len};
use crate::http::{AppState, authorize_active_device_kind};
use crate::registry::{self, AnnouncerRowRecord, AnnouncerStorageError, DeviceKind};

/// Fallback visible-row cap when a pushing receiver does not send its own
/// `max_list_size` (older receivers, or a malformed value). The announcer
/// source (receiver) owns this setting; this only bounds what the server
/// retains in the live runtime for the public feed.
pub(crate) const DEFAULT_MAX_ANNOUNCER_ROWS: usize = 25;

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
    /// Division display name resolved by the receiver, when known. Optional for
    /// backward compatibility with receivers that predate division support.
    #[serde(default)]
    pub division: Option<String>,
    /// Receiver-configured cap on visible announcer rows. Absent or zero falls
    /// back to [`DEFAULT_MAX_ANNOUNCER_ROWS`].
    #[serde(default)]
    pub max_list_size: Option<u32>,
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
    if let Err(status) = authorize_active_device_kind(&state, &headers, DeviceKind::Receiver) {
        return status.into_response();
    }
    if let Err(field) = validate_push_row(&req) {
        return (StatusCode::BAD_REQUEST, format!("{field} too long")).into_response();
    }

    if Utc
        .timestamp_millis_opt(req.received_unix_ms)
        .single()
        .is_none()
    {
        return (StatusCode::BAD_REQUEST, "invalid received_unix_ms").into_response();
    }

    let max_list_size = req
        .max_list_size
        .filter(|n| *n > 0)
        .map_or(DEFAULT_MAX_ANNOUNCER_ROWS, |n| n as usize);

    let record = AnnouncerRowRecord {
        announcer_source_generation: req.announcer_source_generation,
        stream_id: req.stream_id,
        seq: req.seq,
        chip_id: req.chip_id,
        bib: req.bib,
        display_name: req.display_name,
        reader_timestamp: req.reader_timestamp,
        received_unix_ms: req.received_unix_ms,
        division: req.division,
    };

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::upsert_announcer_row(&conn, &record).and_then(|()| {
            let rows = registry::list_announcer_rows_ordered(&conn)?;
            Ok(rebuild_runtime(
                &state.announcer_runtime,
                rows,
                max_list_size,
            ))
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

fn validate_push_row(req: &PushRowRequest) -> Result<(), &'static str> {
    check_len("stream_id", &req.stream_id, MAX_ID_LEN)?;
    check_len("chip_id", &req.chip_id, MAX_ID_LEN)?;
    check_len("display_name", &req.display_name, MAX_NAME_LEN)?;
    if let Some(timestamp) = req.reader_timestamp.as_deref() {
        check_len("reader_timestamp", timestamp, MAX_TIMESTAMP_LEN)?;
    }
    if let Some(division) = req.division.as_deref() {
        check_len("division", division, MAX_NAME_LEN)?;
    }
    Ok(())
}

pub async fn takeover(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(status) = authorize_active_device_kind(&state, &headers, DeviceKind::Receiver) {
        return status.into_response();
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

pub(crate) fn rebuild_runtime(
    runtime: &std::sync::Mutex<AnnouncerRuntime>,
    rows: Vec<AnnouncerRowRecord>,
    max_list_size: usize,
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
            division: row.division,
        };
        runtime.ingest(event, max_list_size);
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

    // Deterministic active-receiver bearer seeded by `test_state`.
    const RX_TOKEN: &str = "rtk_rxseed_rxsecret";

    fn test_state() -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        crate::registry::migrate(&conn).unwrap();
        crate::registry::seed_active_device(
            &conn,
            "rx-seed",
            crate::registry::DeviceKind::Receiver,
            "rxseed",
            "rxsecret",
        );
        AppState::new(conn, true)
    }

    fn json_request(uri: &str, body: &serde_json::Value) -> Request<Body> {
        json_request_with_token(uri, RX_TOKEN, body)
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
    async fn division_round_trips_through_push_into_runtime_row() {
        let state = test_state();
        let app = router(state.clone());
        let mut body = row_body(7, 0, 1_000);
        body["division"] = serde_json::json!("5k");

        let resp = app
            .oneshot(json_request("/announcer/rows", &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Persisted to the row table...
        {
            let conn = state.conn.lock().unwrap();
            let division: Option<String> = conn
                .query_row(
                    "SELECT division FROM announcer_rows WHERE stream_id = 'finish-line' AND seq = 7",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(division.as_deref(), Some("5k"));
        }
        // ...and surfaced on the rebuilt runtime row (the public feed source).
        let runtime = state.announcer_runtime.lock().unwrap();
        assert_eq!(runtime.rows()[0].division.as_deref(), Some("5k"));
    }

    #[tokio::test]
    async fn push_without_division_defaults_to_none() {
        let state = test_state();
        let app = router(state.clone());
        // row_body omits `division`; the request must still deserialize (serde
        // default) and store NULL.
        let resp = app
            .oneshot(json_request("/announcer/rows", &row_body(3, 0, 1_000)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let runtime = state.announcer_runtime.lock().unwrap();
        assert_eq!(runtime.rows()[0].division, None);
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

    #[tokio::test]
    async fn max_list_size_trims_visible_rows_but_counts_all_finishers() {
        let state = test_state();
        let app = router(state.clone());

        for seq in 1..=3u64 {
            let received = 1_000 * i64::try_from(seq).unwrap();
            let mut body = row_body(seq, 0, received);
            body["max_list_size"] = serde_json::json!(2);
            let resp = app
                .clone()
                .oneshot(json_request("/announcer/rows", &body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        let runtime = state.announcer_runtime.lock().unwrap();
        assert_eq!(runtime.finisher_count(), 3);
        assert_eq!(runtime.rows().len(), 2);
    }

    #[tokio::test]
    async fn missing_max_list_size_falls_back_to_default() {
        let state = test_state();
        let app = router(state.clone());

        let resp = app
            .oneshot(json_request("/announcer/rows", &row_body(1, 0, 1_000)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let runtime = state.announcer_runtime.lock().unwrap();
        assert_eq!(runtime.rows().len(), 1);
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
    async fn status_shows_persisted_rows_after_restart() {
        // A file-backed database so a second open (the "restart") sees the
        // rows persisted by the first connection.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("server.sqlite");

        {
            let conn = crate::db::open(&db_path).unwrap();
            crate::registry::upsert_announcer_row(
                &conn,
                &crate::registry::AnnouncerRowRecord {
                    announcer_source_generation: 0,
                    stream_id: "finish-line".to_string(),
                    seq: 1,
                    chip_id: "chip-1".to_string(),
                    bib: Some(1001),
                    display_name: "Runner 1".to_string(),
                    reader_timestamp: Some("10:00:00".to_string()),
                    received_unix_ms: 1_000,
                    division: None,
                },
            )
            .unwrap();
        }

        let conn = crate::db::open(&db_path).unwrap();
        let state = AppState::new(conn, true);
        let app = router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        assert_eq!(body["finisher_count"], 1);
        assert_eq!(body["announcer_rows"][0]["chip_id"], "chip-1");
    }

    #[tokio::test]
    async fn push_rejects_oversized_display_name() {
        let state = test_state();
        let app = router(state.clone());
        let mut body = row_body(1, 0, 1_000);
        body["display_name"] = serde_json::json!("x".repeat(10_000));

        let resp = app
            .oneshot(json_request("/announcer/rows", &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let conn = state.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn push_with_future_generation_is_rejected() {
        let state = test_state();
        let app = router(state.clone());

        // Current generation is 0; a push claiming generation 1 must be fenced
        // out (the source must call /announcer/takeover first).
        let rejected = app
            .oneshot(json_request("/announcer/rows", &row_body(1, 1, 1_000)))
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

    fn json_request_with_token(uri: &str, token: &str, body: &serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn announcer_accepts_active_receiver_token_and_denies_pending() {
        let state = test_state();
        let (active, pending) = {
            let conn = state.conn.lock().unwrap();
            let a = crate::registry::register_device_minted(
                &conn,
                "rx-active",
                crate::registry::DeviceKind::Receiver,
            )
            .unwrap();
            crate::registry::approve_device(&conn, "rx-active")
                .unwrap()
                .unwrap();
            let p = crate::registry::register_device_minted(
                &conn,
                "rx-pending",
                crate::registry::DeviceKind::Receiver,
            )
            .unwrap();
            (a.device_token, p.device_token)
        };

        let ok = router(state.clone())
            .oneshot(json_request_with_token(
                "/announcer/rows",
                &active,
                &row_body(1, 0, 1_000),
            ))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);

        let denied = router(state)
            .oneshot(json_request_with_token(
                "/announcer/rows",
                &pending,
                &row_body(2, 0, 2_000),
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }
}

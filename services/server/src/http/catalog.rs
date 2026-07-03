//! Forwarder catalog push endpoint.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::http::validate::{
    MAX_ADDR_LEN, MAX_CATALOG_STREAMS, MAX_DIRECT_ADDRS, MAX_ID_LEN, MAX_NAME_LEN, check_len,
};
use crate::http::{AppState, authorize_forwarder_catalog};
use crate::registry::{self, ForwarderCatalogStreamRecord};

#[derive(Debug, Deserialize)]
pub struct ForwarderCatalogRequest {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<String>,
    pub streams: Vec<ForwarderCatalogStreamRequest>,
}

#[derive(Debug, Deserialize)]
pub struct ForwarderCatalogStreamRequest {
    pub stream_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

#[derive(Debug, Serialize)]
pub struct ForwarderCatalogResponse {
    pub endpoint_id: String,
    pub stream_count: usize,
}

/// `POST /forwarder/catalog` — M2M forwarder identity and stream catalog push.
///
/// Authorized only by the forwarder's own minted device token, bound to the
/// `endpoint_id` in the request body (any approval state, so a pending forwarder
/// can publish its catalog for the admin to review). Receiver tokens must not
/// authorize this endpoint.
pub async fn push_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ForwarderCatalogRequest>,
) -> Response {
    if let Err(status) = authorize_forwarder_catalog(&state, &headers, &req.endpoint_id) {
        return status.into_response();
    }
    if let Err(message) = validate_catalog(&req) {
        return (StatusCode::BAD_REQUEST, message).into_response();
    }
    // Values are persisted as SQLite INTEGERs (i64); reject out-of-range
    // counters here rather than surfacing a 500 from the storage layer.
    for stream in &req.streams {
        if i64::try_from(stream.epoch).is_err() || i64::try_from(stream.next_seq).is_err() {
            return (StatusCode::BAD_REQUEST, "epoch/next_seq out of range").into_response();
        }
    }

    let streams = req
        .streams
        .into_iter()
        .map(|stream| ForwarderCatalogStreamRecord {
            stream_id: stream.stream_id,
            epoch: stream.epoch,
            next_seq: stream.next_seq,
        })
        .collect::<Vec<_>>();
    let stream_count = streams.len();

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::upsert_forwarder_catalog(
            &conn,
            &req.endpoint_id,
            req.display_name.as_deref(),
            &req.direct_addrs,
            &streams,
        )
    };

    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(ForwarderCatalogResponse {
                endpoint_id: req.endpoint_id,
                stream_count,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = %err, "forwarder catalog push failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn validate_catalog(req: &ForwarderCatalogRequest) -> Result<(), String> {
    let check = |field: &'static str, value: &str, max: usize| {
        check_len(field, value, max).map_err(|field| format!("{field} too long"))
    };
    check("endpoint_id", &req.endpoint_id, MAX_ID_LEN)?;
    if let Some(name) = req.display_name.as_deref() {
        check("display_name", name, MAX_NAME_LEN)?;
    }
    if req.direct_addrs.len() > MAX_DIRECT_ADDRS {
        return Err("too many direct_addrs".to_owned());
    }
    for addr in &req.direct_addrs {
        check("direct_addrs entry", addr, MAX_ADDR_LEN)?;
    }
    if req.streams.len() > MAX_CATALOG_STREAMS {
        return Err("too many streams".to_owned());
    }
    for stream in &req.streams {
        check("stream_id", &stream.stream_id, MAX_ID_LEN)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::http::{AppState, router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rusqlite::Connection;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        crate::registry::migrate(&conn).unwrap();
        AppState::new(conn, true)
    }

    fn catalog_request(token: &str, endpoint_id: &str) -> Request<Body> {
        catalog_request_with_body(
            token,
            &serde_json::json!({
                "endpoint_id": endpoint_id,
                "display_name": "Start Line",
                "direct_addrs": ["127.0.0.1:5000"],
                "streams": [{
                    "stream_id": "reader-a",
                    "epoch": 1,
                    "next_seq": 2
                }]
            }),
        )
    }

    fn catalog_request_with_body(token: &str, body: &serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/forwarder/catalog")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    /// Mint a forwarder device token for `endpoint_id` and return the bearer.
    fn forwarder_token(state: &AppState, endpoint_id: &str) -> String {
        let conn = state.conn.lock().unwrap();
        crate::registry::register_device_minted(
            &conn,
            endpoint_id,
            crate::registry::DeviceKind::Forwarder,
        )
        .unwrap()
        .device_token
    }

    #[tokio::test]
    async fn catalog_accepts_matching_forwarder_token() {
        let state = test_state();
        let token = forwarder_token(&state, "ep-fwd");
        let resp = router(state)
            .oneshot(catalog_request(&token, "ep-fwd"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn catalog_rejects_receiver_token() {
        let state = test_state();
        let token = {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device_minted(
                &conn,
                "ep-receiver",
                crate::registry::DeviceKind::Receiver,
            )
            .unwrap()
            .device_token
        };
        let resp = router(state)
            .oneshot(catalog_request(&token, "ep-receiver"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn catalog_rejects_too_many_direct_addrs() {
        let state = test_state();
        let token = forwarder_token(&state, "ep-fwd");
        let addrs: Vec<String> = (0..33).map(|i| format!("10.0.0.{i}:5000")).collect();
        let resp = router(state)
            .oneshot(catalog_request_with_body(
                &token,
                &serde_json::json!({
                    "endpoint_id": "ep-fwd",
                    "display_name": null,
                    "direct_addrs": addrs,
                    "streams": []
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn catalog_rejects_oversized_stream_id() {
        let state = test_state();
        let token = forwarder_token(&state, "ep-fwd");
        let resp = router(state)
            .oneshot(catalog_request_with_body(
                &token,
                &serde_json::json!({
                    "endpoint_id": "ep-fwd",
                    "display_name": null,
                    "direct_addrs": [],
                    "streams": [{
                        "stream_id": "s".repeat(10_000),
                        "epoch": 1,
                        "next_seq": 2
                    }]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn catalog_rejects_epoch_above_i64_max_with_400() {
        let state = test_state();
        let token = forwarder_token(&state, "ep-fwd");
        let resp = router(state)
            .oneshot(catalog_request_with_body(
                &token,
                &serde_json::json!({
                    "endpoint_id": "ep-fwd",
                    "display_name": null,
                    "direct_addrs": [],
                    "streams": [{
                        "stream_id": "reader-a",
                        "epoch": u64::MAX,
                        "next_seq": 2
                    }]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn catalog_rejects_forwarder_token_for_mismatched_endpoint() {
        let state = test_state();
        let token = forwarder_token(&state, "ep-fwd");
        // A valid forwarder token cannot push a catalog for a different endpoint.
        let resp = router(state)
            .oneshot(catalog_request(&token, "ep-other"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::http::{AppState, status};
use crate::registry::{self, DeviceKind};

/// Manually chosen voucher secrets must not be guessable; enforce a floor.
/// (Generated vouchers are 32+ hex chars and unaffected.)
const MIN_MANUAL_VOUCHER_LEN: usize = 16;

#[derive(Debug, Deserialize)]
pub struct CreateEnrollmentTokenRequest {
    pub device_kind: String,
    pub display_name: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateEnrollmentTokenResponse {
    pub token_id: String,
    pub device_kind: DeviceKind,
    pub display_name: Option<String>,
    pub token: String,
    pub created_unix_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct EnrollmentTokensResponse {
    pub tokens: Vec<registry::EnrollmentTokenRecord>,
}

pub async fn list_tokens(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !status::admin_authorized(&headers, state.admin_proxy_trusted) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::list_enrollment_tokens(&conn)
    };

    match result {
        Ok(tokens) => (StatusCode::OK, Json(EnrollmentTokensResponse { tokens })).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "failed to list enrollment tokens");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn create_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateEnrollmentTokenRequest>,
) -> Response {
    if !status::admin_authorized(&headers, state.admin_proxy_trusted) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let Some(device_kind) = DeviceKind::parse(&req.device_kind) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let token = match req.token {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if trimmed.len() < MIN_MANUAL_VOUCHER_LEN {
                return (StatusCode::BAD_REQUEST, "token too short (min 16 chars)").into_response();
            }
            trimmed.to_owned()
        }
        None => generate_token(),
    };
    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let token_id = generate_token_id();

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::create_enrollment_token(&conn, &token_id, device_kind, display_name, &token)
    };

    match result {
        Ok(record) => (
            StatusCode::OK,
            Json(CreateEnrollmentTokenResponse {
                token_id: record.token_id,
                device_kind: record.device_kind,
                display_name: record.display_name,
                token,
                created_unix_ms: record.created_unix_ms,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = %err, "failed to create enrollment token");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn revoke_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token_id): Path<String>,
) -> Response {
    if !status::admin_authorized(&headers, state.admin_proxy_trusted) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::revoke_enrollment_token(&conn, &token_id)
    };

    match result {
        Ok(Some(record)) => (StatusCode::OK, Json(record)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(error = %err, "failed to revoke enrollment token");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn generate_token() -> String {
    format!("rtfwd_{}", random_hex(32))
}

fn generate_token_id() -> String {
    format!("et_{}", random_hex(16))
}

fn random_hex(len: usize) -> String {
    let mut bytes = vec![0_u8; len];
    rand::rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(len * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::http::status::ADMIN_HEADER;
    use crate::http::{AppState, router};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use rusqlite::Connection;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        crate::registry::migrate(&conn).unwrap();
        AppState::new(conn, true)
    }

    async fn response_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn list_request(admin_header: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("GET")
            .uri("/admin/enrollment-tokens");
        if let Some(value) = admin_header {
            builder = builder.header(ADMIN_HEADER, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn create_request(admin_header: Option<&str>, body: &serde_json::Value) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/admin/enrollment-tokens")
            .header("Content-Type", "application/json");
        if let Some(value) = admin_header {
            builder = builder.header(ADMIN_HEADER, value);
        }
        builder
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    fn revoke_request(admin_header: Option<&str>, token_id: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("/admin/enrollment-tokens/{token_id}/revoke"));
        if let Some(value) = admin_header {
            builder = builder.header(ADMIN_HEADER, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn list_tokens_requires_admin_header() {
        let resp = router(test_state())
            .oneshot(list_request(None))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_generated_token_returns_secret_once() {
        let state = test_state();
        let resp = router(state.clone())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({
                    "device_kind": "forwarder",
                    "display_name": "Start Line"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        let token = body["token"].as_str().expect("token returned");
        assert!(token.starts_with("rtfwd_"));
        assert_eq!(token.len(), "rtfwd_".len() + 64);
        assert_eq!(body["device_kind"], "forwarder");
        assert_eq!(body["display_name"], "Start Line");

        let list_resp = router(state)
            .oneshot(list_request(Some("alice")))
            .await
            .unwrap();
        let listed = response_json(list_resp).await;
        assert_eq!(listed["tokens"][0]["token_id"], body["token_id"]);
        assert!(listed["tokens"][0].get("token").is_none());
    }

    #[tokio::test]
    async fn create_manual_forwarder_token_stores_record() {
        let state = test_state();
        let resp = router(state.clone())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({
                    "device_kind": "forwarder",
                    "display_name": "Manual Start",
                    "token": "manual-secret-0001"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        assert_eq!(body["token"], "manual-secret-0001");

        let list_resp = router(state)
            .oneshot(list_request(Some("alice")))
            .await
            .unwrap();
        let listed = response_json(list_resp).await;
        assert_eq!(listed["tokens"][0]["display_name"], "Manual Start");
        assert_eq!(listed["tokens"][0]["status"], "active");
    }

    #[tokio::test]
    async fn create_token_rejects_short_manual_secret() {
        // 15 chars: one below the 16-char floor.
        let resp = router(test_state())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({
                    "device_kind": "receiver",
                    "token": "fifteen-chars-x"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Exactly 16 chars passes.
        let resp = router(test_state())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({
                    "device_kind": "receiver",
                    "token": "sixteen-chars-ok"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_token_rejects_blank_manual_token() {
        let resp = router(test_state())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({
                    "device_kind": "forwarder",
                    "token": "   "
                }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_token_allows_receiver_device_kind() {
        let resp = router(test_state())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({
                    "device_kind": "receiver"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_token_rejects_invalid_device_kind() {
        let resp = router(test_state())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({
                    "device_kind": "banana"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn revoke_token_marks_token_revoked() {
        let state = test_state();
        let create_resp = router(state.clone())
            .oneshot(create_request(
                Some("alice"),
                &serde_json::json!({ "device_kind": "forwarder" }),
            ))
            .await
            .unwrap();
        let created = response_json(create_resp).await;
        let token_id = created["token_id"].as_str().unwrap();

        let revoke_resp = router(state)
            .oneshot(revoke_request(Some("alice"), token_id))
            .await
            .unwrap();

        assert_eq!(revoke_resp.status(), StatusCode::OK);
        let revoked = response_json(revoke_resp).await;
        assert_eq!(revoked["token_id"], token_id);
        assert_eq!(revoked["status"], "revoked");
        assert!(revoked["revoked_unix_ms"].is_number());
    }

    #[tokio::test]
    async fn revoke_unknown_token_404() {
        let resp = router(test_state())
            .oneshot(revoke_request(Some("alice"), "missing"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

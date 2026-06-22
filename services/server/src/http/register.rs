//! `POST /register` — TOFU device self-registration.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::http::AppState;
use crate::registry::{self, ApprovalState, DeviceKind, DeviceRecord};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    /// Stable endpoint identifier of the registering device.
    pub endpoint_id: String,
    /// `"forwarder"` or `"receiver"`.
    pub device_kind: String,
    /// Per-device bearer token; stored hashed, never in plaintext.
    pub device_token: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub endpoint_id: String,
    pub device_kind: DeviceKind,
    pub approval_state: ApprovalState,
}

/// Register a device under the TOFU model.
///
/// Requires a valid provisioning bearer token; otherwise responds `401`. A
/// brand-new endpoint is recorded as `pending`; an admin later approves it.
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Response {
    let Some(device_kind) = DeviceKind::parse(&req.device_kind) else {
        return (StatusCode::BAD_REQUEST, "invalid device_kind").into_response();
    };

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        register_authorized_device(
            &conn,
            &headers,
            &state.provisioning_token_hash,
            &req,
            device_kind,
        )
    };

    match result {
        Ok(Some(record)) => (
            StatusCode::OK,
            Json(RegisterResponse {
                endpoint_id: record.endpoint_id,
                device_kind: record.device_kind,
                approval_state: record.approval_state,
            }),
        )
            .into_response(),
        Ok(None) => StatusCode::UNAUTHORIZED.into_response(),
        Err(err) => {
            tracing::error!(error = %err, "device registration failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn register_authorized_device(
    conn: &rusqlite::Connection,
    headers: &HeaderMap,
    provisioning_token_hash: &[u8],
    req: &RegisterRequest,
    device_kind: DeviceKind,
) -> rusqlite::Result<Option<DeviceRecord>> {
    if authorized(headers, provisioning_token_hash) {
        return registry::register_device(conn, &req.endpoint_id, device_kind, &req.device_token)
            .map(Some);
    }

    let Some(raw_bearer) = bearer_token(headers) else {
        return Ok(None);
    };
    if raw_bearer != req.device_token {
        return Ok(None);
    }

    if registry::device_token_authorized(conn, &req.endpoint_id, device_kind, raw_bearer)? {
        return registry::register_device(conn, &req.endpoint_id, device_kind, &req.device_token)
            .map(Some);
    }

    registry::register_device_with_enrollment_token(conn, &req.endpoint_id, device_kind, raw_bearer)
}

/// Authorize a request against the provisioning bearer token.
pub(super) fn authorized(headers: &HeaderMap, expected_hash: &[u8]) -> bool {
    bearer_token(headers).is_some_and(|token| registry::verify_token(token, expected_hash))
}

pub(super) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.is_empty() {
        return None;
    }
    Some(token)
}

#[cfg(test)]
mod tests {
    use crate::http::{AppState, router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rusqlite::Connection;
    use tower::ServiceExt;

    const PROV_TOKEN: &str = "prov-secret";

    fn test_state() -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        crate::registry::migrate(&conn).unwrap();
        AppState::new(conn, PROV_TOKEN, true)
    }

    fn register_request(token: &str, body: &serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/register")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn register_creates_pending() {
        let state = test_state();
        let app = router(state.clone());

        let resp = app
            .oneshot(register_request(
                PROV_TOKEN,
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-1",
                    "device_kind": "forwarder",
                    "device_token": "device-bearer-1"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "ep-forwarder-1")
            .unwrap()
            .expect("device recorded");
        assert_eq!(
            device.approval_state,
            crate::registry::ApprovalState::Pending
        );
        assert_eq!(device.device_kind, crate::registry::DeviceKind::Forwarder);
    }

    #[tokio::test]
    async fn approve_marks_active() {
        let state = test_state();
        let app = router(state.clone());

        let resp = app
            .oneshot(register_request(
                PROV_TOKEN,
                &serde_json::json!({
                    "endpoint_id": "ep-receiver-1",
                    "device_kind": "receiver",
                    "device_token": "device-bearer-2"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let conn = state.conn.lock().unwrap();
        let approved = crate::registry::approve_device(&conn, "ep-receiver-1")
            .unwrap()
            .expect("device exists");
        assert_eq!(
            approved.approval_state,
            crate::registry::ApprovalState::Active
        );
    }

    #[tokio::test]
    async fn register_accepts_active_enrollment_token() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "enroll-secret",
            )
            .unwrap();
        }

        let resp = router(state.clone())
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-enrolled",
                    "device_kind": "forwarder",
                    "device_token": "enroll-secret"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "ep-forwarder-enrolled")
            .unwrap()
            .expect("device recorded");
        assert_eq!(device.device_kind, crate::registry::DeviceKind::Forwarder);
    }

    #[tokio::test]
    async fn register_consumes_enrollment_token() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "enroll-secret",
            )
            .unwrap();
        }

        let resp = router(state.clone())
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-enrolled",
                    "device_kind": "forwarder",
                    "device_token": "enroll-secret"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let conn = state.conn.lock().unwrap();
        let tokens = crate::registry::list_enrollment_tokens(&conn).unwrap();
        assert_eq!(
            tokens[0].status,
            crate::registry::EnrollmentTokenStatus::Used
        );
        assert_eq!(
            tokens[0].used_endpoint_id.as_deref(),
            Some("ep-forwarder-enrolled")
        );
    }

    #[tokio::test]
    async fn register_allows_reregistration_with_used_token_for_same_endpoint() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "enroll-secret",
            )
            .unwrap();
        }

        for _ in 0..2 {
            let resp = router(state.clone())
                .oneshot(register_request(
                    "enroll-secret",
                    &serde_json::json!({
                        "endpoint_id": "ep-forwarder-enrolled",
                        "device_kind": "forwarder",
                        "device_token": "enroll-secret"
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn register_rejects_consumed_enrollment_token_for_second_endpoint() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "enroll-secret",
            )
            .unwrap();
        }

        let first = router(state.clone())
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-one",
                    "device_kind": "forwarder",
                    "device_token": "enroll-secret"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = router(state)
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-two",
                    "device_kind": "forwarder",
                    "device_token": "enroll-secret"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn register_rejects_unused_enrollment_token_for_existing_endpoint() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device(
                &conn,
                "ep-forwarder-existing",
                crate::registry::DeviceKind::Forwarder,
                "existing-secret",
            )
            .unwrap();
            crate::registry::approve_device(&conn, "ep-forwarder-existing")
                .unwrap()
                .unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "enroll-secret",
            )
            .unwrap();
        }

        let resp = router(state.clone())
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-existing",
                    "device_kind": "forwarder",
                    "device_token": "enroll-secret"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "ep-forwarder-existing")
            .unwrap()
            .unwrap();
        assert_eq!(
            device.approval_state,
            crate::registry::ApprovalState::Active
        );
        assert!(
            crate::registry::device_token_authorized(
                &conn,
                "ep-forwarder-existing",
                crate::registry::DeviceKind::Forwarder,
                "existing-secret",
            )
            .unwrap()
        );
        assert_eq!(
            crate::registry::list_enrollment_tokens(&conn).unwrap()[0].status,
            crate::registry::EnrollmentTokenStatus::Active,
        );
    }

    #[tokio::test]
    async fn register_rejects_revoked_enrollment_token() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "enroll-secret",
            )
            .unwrap();
            crate::registry::revoke_enrollment_token(&conn, "tok-1")
                .unwrap()
                .unwrap();
        }

        let resp = router(state)
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-enrolled",
                    "device_kind": "forwarder",
                    "device_token": "enroll-secret"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn register_rejects_enrollment_token_device_token_mismatch() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "enroll-secret",
            )
            .unwrap();
        }

        let resp = router(state)
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-enrolled",
                    "device_kind": "forwarder",
                    "device_token": "different-secret"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bad_token_401() {
        let state = test_state();
        let app = router(state);

        let resp = app
            .oneshot(register_request(
                "wrong-token",
                &serde_json::json!({
                    "endpoint_id": "ep-forwarder-2",
                    "device_kind": "forwarder",
                    "device_token": "device-bearer-3"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

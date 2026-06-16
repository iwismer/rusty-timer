//! `POST /register` — TOFU device self-registration.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::http::AppState;
use crate::registry::{self, ApprovalState, DeviceKind};

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
    if !authorized(&headers, &state.provisioning_token_hash) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let Some(device_kind) = DeviceKind::parse(&req.device_kind) else {
        return (StatusCode::BAD_REQUEST, "invalid device_kind").into_response();
    };

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::register_device(&conn, &req.endpoint_id, device_kind, &req.device_token)
    };

    match result {
        Ok(record) => (
            StatusCode::OK,
            Json(RegisterResponse {
                endpoint_id: record.endpoint_id,
                device_kind: record.device_kind,
                approval_state: record.approval_state,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = %err, "device registration failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Authorize a request against the provisioning bearer token.
pub(super) fn authorized(headers: &HeaderMap, expected_hash: &[u8]) -> bool {
    let Some(value) = headers.get(AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    if token.is_empty() {
        return false;
    }
    registry::hash_token(token) == expected_hash
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
        AppState::new(conn, PROV_TOKEN)
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
        assert!(device.display_name.is_none());
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
        let approved = crate::registry::approve_device(&conn, "ep-receiver-1", "Finish Line")
            .unwrap()
            .expect("device exists");
        assert_eq!(
            approved.approval_state,
            crate::registry::ApprovalState::Active
        );
        assert_eq!(approved.display_name.as_deref(), Some("Finish Line"));
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

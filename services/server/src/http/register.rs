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
    /// Optional human-friendly name the device reports for itself (e.g. a
    /// receiver's configured receiver ID). Surfaced in the admin approval UI.
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub endpoint_id: String,
    pub device_kind: DeviceKind,
    pub approval_state: ApprovalState,
    /// The minted per-device bearer token, returned exactly once when a token
    /// is minted or rotated. `None` on an idempotent re-register by a device
    /// already presenting its own token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_token: Option<String>,
}

/// Bootstrap/recovery registration.
///
/// Authenticated by an enrollment voucher (new/recovering device) or the
/// device's own minted token (idempotent re-register); the provisioning token
/// is also accepted during the migration (Phases 1–3) and mints a token too.
/// Steady-state clients never call this once they hold a minted token.
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
        Ok(Some((record, device_token))) => (
            StatusCode::OK,
            Json(RegisterResponse {
                endpoint_id: record.endpoint_id,
                device_kind: record.device_kind,
                approval_state: record.approval_state,
                device_token,
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

/// Resolve the registration to `(record, minted_token)`, or `None` for `401`.
///
/// Auth order (never trusts a client-chosen token):
/// 1. the device's own minted token whose resolved `endpoint_id` matches the
///    request → idempotent, no new token;
/// 2. an unused/own enrollment voucher of the matching kind → consume + mint;
/// 3. [migration only] the provisioning token → mint;
/// 4. otherwise `None`.
fn register_authorized_device(
    conn: &rusqlite::Connection,
    headers: &HeaderMap,
    provisioning_token_hash: &[u8],
    req: &RegisterRequest,
    device_kind: DeviceKind,
) -> rusqlite::Result<Option<(DeviceRecord, Option<String>)>> {
    let Some((record, device_token)) =
        register_inner(conn, headers, provisioning_token_hash, req, device_kind)?
    else {
        return Ok(None);
    };

    // Persist the device's self-reported name (e.g. a receiver's receiver ID)
    // so the admin approval UI shows something human-friendly. A blank name is
    // ignored, and an admin-assigned enrollment-token name still takes
    // precedence at read time. Re-fetch so the returned record reflects the
    // resolved name.
    if let Some(name) = req.display_name.as_deref() {
        registry::set_device_display_name(conn, &record.endpoint_id, name)?;
        let refreshed = registry::get_device(conn, &record.endpoint_id)?;
        return Ok(refreshed.map(|record| (record, device_token)));
    }
    Ok(Some((record, device_token)))
}

fn register_inner(
    conn: &rusqlite::Connection,
    headers: &HeaderMap,
    provisioning_token_hash: &[u8],
    req: &RegisterRequest,
    device_kind: DeviceKind,
) -> rusqlite::Result<Option<(DeviceRecord, Option<String>)>> {
    if let Some(raw_bearer) = bearer_token(headers) {
        // 1. A valid device token must match the claimed endpoint (and kind).
        if let Some(record) = registry::authenticate_device(conn, raw_bearer)? {
            if record.endpoint_id == req.endpoint_id && record.device_kind == device_kind {
                return Ok(Some((record, None)));
            }
            return Ok(None);
        }
        // 2. Enrollment voucher (consume + mint/rebind).
        if let Some(minted) =
            registry::register_device_with_voucher(conn, &req.endpoint_id, device_kind, raw_bearer)?
        {
            return Ok(Some((minted.record, Some(minted.device_token))));
        }
    }

    // 3. Provisioning token (migration only): mint so the client can persist a
    //    real per-device credential.
    if authorized(headers, provisioning_token_hash) {
        let minted = registry::register_device_minted(conn, &req.endpoint_id, device_kind)?;
        return Ok(Some((minted.record, Some(minted.device_token))));
    }

    Ok(None)
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
    async fn register_persists_self_reported_display_name() {
        let state = test_state();
        let app = router(state.clone());

        let resp = app
            .oneshot(register_request(
                PROV_TOKEN,
                &serde_json::json!({
                    "endpoint_id": "ep-receiver-named",
                    "device_kind": "receiver",
                    "device_token": "device-bearer-named",
                    "display_name": "dev-receiver"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "ep-receiver-named")
            .unwrap()
            .expect("device recorded");
        assert_eq!(device.display_name.as_deref(), Some("dev-receiver"));
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
    async fn register_voucher_rebinds_existing_endpoint_and_resets_approval() {
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
                    "device_kind": "forwarder"
                }),
            ))
            .await
            .unwrap();
        // A fresh voucher rebinds an existing endpoint, mints a new token, and
        // resets approval to pending so an admin must re-approve (no silent
        // hijack of an already-active device).
        assert_eq!(resp.status(), StatusCode::OK);

        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "ep-forwarder-existing")
            .unwrap()
            .unwrap();
        assert_eq!(
            device.approval_state,
            crate::registry::ApprovalState::Pending
        );
        assert_eq!(
            crate::registry::list_enrollment_tokens(&conn).unwrap()[0].status,
            crate::registry::EnrollmentTokenStatus::Used,
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
    async fn register_mints_token_for_new_voucher() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Receiver,
                None,
                "enroll-secret",
            )
            .unwrap();
        }

        let resp = router(state.clone())
            .oneshot(register_request(
                "enroll-secret",
                &serde_json::json!({
                    "endpoint_id": "ep-receiver-1",
                    "device_kind": "receiver"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        let minted = body["device_token"]
            .as_str()
            .expect("a minted device token is returned")
            .to_owned();
        assert!(minted.starts_with("rtk_"));
        assert_eq!(body["approval_state"], "pending");

        // The minted token authenticates the device.
        let conn = state.conn.lock().unwrap();
        let record = crate::registry::authenticate_device(&conn, &minted)
            .unwrap()
            .expect("minted token resolves to its device");
        assert_eq!(record.endpoint_id, "ep-receiver-1");
        assert_eq!(record.device_kind, crate::registry::DeviceKind::Receiver);
    }

    #[tokio::test]
    async fn register_with_device_token_is_idempotent_without_remint() {
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
                    "endpoint_id": "ep-fwd-1",
                    "device_kind": "forwarder"
                }),
            ))
            .await
            .unwrap();
        let minted = response_json(first).await["device_token"]
            .as_str()
            .unwrap()
            .to_owned();

        // Re-registering with the minted token returns no new token.
        let second = router(state)
            .oneshot(register_request(
                &minted,
                &serde_json::json!({
                    "endpoint_id": "ep-fwd-1",
                    "device_kind": "forwarder"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let body = response_json(second).await;
        assert!(body.get("device_token").is_none() || body["device_token"].is_null());
    }

    #[tokio::test]
    async fn register_device_token_for_wrong_endpoint_401() {
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
                    "endpoint_id": "ep-fwd-1",
                    "device_kind": "forwarder"
                }),
            ))
            .await
            .unwrap();
        let minted = response_json(first).await["device_token"]
            .as_str()
            .unwrap()
            .to_owned();

        // The same token claiming a different endpoint must be rejected.
        let resp = router(state)
            .oneshot(register_request(
                &minted,
                &serde_json::json!({
                    "endpoint_id": "ep-fwd-OTHER",
                    "device_kind": "forwarder"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn provisioning_token_register_mints() {
        let state = test_state();
        let resp = router(state)
            .oneshot(register_request(
                PROV_TOKEN,
                &serde_json::json!({
                    "endpoint_id": "ep-prov-1",
                    "device_kind": "forwarder"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let minted = response_json(resp).await["device_token"]
            .as_str()
            .expect("provisioning registration mints a token")
            .to_owned();
        assert!(minted.starts_with("rtk_"));
    }

    async fn response_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
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

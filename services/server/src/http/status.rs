//! Status board and admin route wiring.
//!
//! # Endpoint auth matrix
//!
//! This module defines the server auth posture for the current stage. The
//! node assumes a Caddy + Authelia reverse proxy sits in front of it and is
//! responsible for terminating user authentication. The matrix is:
//!
//! - **Public read** (`GET /status`): unauthenticated. The status board is
//!   intended to be readable by anyone who can reach the node (e.g. on the
//!   local race network) and carries no secrets.
//! - **Admin routes** (`POST /admin/*`, e.g. device approval): require the
//!   upstream-injected admin identity header [`ADMIN_HEADER`]. Authelia injects
//!   this header *after* authenticating the user and strips any client-supplied
//!   copy, so the node trusts its presence as proof of an authenticated admin.
//!   **Caddy/Authelia MUST protect every `/admin/*` route** and MUST strip
//!   inbound copies of [`ADMIN_HEADER`] from untrusted clients. As a
//!   fail-closed guard, the node ignores [`ADMIN_HEADER`] entirely unless the
//!   operator sets `SERVER_TRUSTED_PROXY` at startup to assert that such a
//!   proxy is present; otherwise every `/admin/*` request is denied.
//! - **M2M / device routes**: `POST /register`, `POST /forwarder/catalog`, and
//!   `GET /allowlist/receivers` accept the shared provisioning bearer token for
//!   legacy deployments. Enrolled forwarders may also use their non-revoked
//!   forwarder token for idempotent registration, catalog pushes, and receiver
//!   allow-list fetches. `POST /announcer/rows` and `POST /announcer/takeover`
//!   still use the provisioning bearer token. These do not depend on the proxy.
//!
//! See `docs/network-architecture.md` for the deployment topology.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::announcer::AnnouncerRow;
use crate::http::AppState;
use crate::registry::{self, DeviceRecord, ForwarderRecord, ForwarderStreamRecord};

/// Upstream-injected header that proves an authenticated admin user.
///
/// Authelia sets this after authentication; Caddy must strip any inbound
/// client-supplied copy. The node treats a non-empty value as authorization
/// for `/admin/*` routes.
pub const ADMIN_HEADER: &str = "Remote-User";

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// Current announcer source generation (fencing token).
    pub announcer_source_generation: u64,
    /// Unique-chip finisher count from the live announcer runtime.
    pub finisher_count: u64,
    /// Most recent announcer rows held in the live runtime, newest first.
    pub announcer_rows: Vec<AnnouncerRow>,
    /// All registered devices and their approval state.
    pub devices: Vec<DeviceRecord>,
    /// Latest pushed forwarder identities, if any.
    pub forwarders: Vec<ForwarderRecord>,
    /// Backup forwarder stream catalog rows, if any.
    pub forwarder_streams: Vec<ForwarderStreamRecord>,
}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub endpoint_id: String,
}

/// `GET /status` — public, unauthenticated status board.
pub async fn status(State(state): State<AppState>) -> Response {
    let snapshot = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        let generation = match registry::current_announcer_source_generation(&conn) {
            Ok(generation) => generation,
            Err(err) => {
                tracing::error!(error = ?err, "failed to read announcer generation");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        let devices = match registry::list_devices(&conn) {
            Ok(devices) => devices,
            Err(err) => {
                tracing::error!(error = %err, "failed to list devices");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        let forwarders = match registry::list_forwarders(&conn) {
            Ok(forwarders) => forwarders,
            Err(err) => {
                tracing::error!(error = %err, "failed to list forwarders");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        let forwarder_streams = match registry::list_forwarder_streams(&conn) {
            Ok(streams) => streams,
            Err(err) => {
                tracing::error!(error = %err, "failed to list forwarder streams");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        (generation, devices, forwarders, forwarder_streams)
    };
    let (announcer_source_generation, devices, forwarders, forwarder_streams) = snapshot;

    let (finisher_count, announcer_rows) = {
        let runtime = state
            .announcer_runtime
            .lock()
            .expect("announcer runtime mutex poisoned");
        (
            runtime.finisher_count(),
            runtime.rows().iter().cloned().collect::<Vec<_>>(),
        )
    };

    (
        StatusCode::OK,
        Json(StatusResponse {
            announcer_source_generation,
            finisher_count,
            announcer_rows,
            devices,
            forwarders,
            forwarder_streams,
        }),
    )
        .into_response()
}

/// `POST /admin/devices/approve` — admin-only device approval.
///
/// Requires the upstream-injected [`ADMIN_HEADER`]; otherwise responds `401`.
pub async fn approve_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ApproveRequest>,
) -> Response {
    if !admin_authorized(&headers, state.admin_proxy_trusted) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let result = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        registry::approve_device(&conn, &req.endpoint_id)
    };

    match result {
        Ok(Some(record)) => {
            // Release forwarders long-polling the receiver allow-list so the
            // newly approved receiver is admitted within milliseconds instead
            // of waiting for the forwarder's periodic poll backstop.
            state.bump_allowlist_version();
            (StatusCode::OK, Json(record)).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(error = %err, "device approval failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Authorize an admin request by the presence of a non-empty upstream identity
/// header injected by Authelia.
///
/// Fail-closed: when `proxy_trusted` is `false` the node has not been told a
/// header-stripping reverse proxy sits in front of it, so the [`ADMIN_HEADER`]
/// cannot be trusted and admin access is always denied — otherwise any client
/// could forge the header and self-authorize. The operator opts in via
/// `SERVER_TRUSTED_PROXY` at startup.
pub(super) fn admin_authorized(headers: &HeaderMap, proxy_trusted: bool) -> bool {
    if !proxy_trusted {
        return false;
    }
    headers
        .get(ADMIN_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use crate::http::status::ADMIN_HEADER;
    use crate::http::{AppState, router};
    use axum::body::{Body, to_bytes};
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

    async fn response_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn response_text(resp: axum::response::Response) -> String {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn approve_request(admin_header: Option<&str>, body: &serde_json::Value) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/admin/devices/approve")
            .header("Content-Type", "application/json");
        if let Some(value) = admin_header {
            builder = builder.header(ADMIN_HEADER, value);
        }
        builder
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    fn catalog_request(token: &str, body: &serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/forwarder/catalog")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn forwarder_catalog_push_appears_in_status() {
        let state = test_state();
        let app = router(state.clone());

        let resp = app
            .clone()
            .oneshot(catalog_request(
                PROV_TOKEN,
                &serde_json::json!({
                    "endpoint_id": "fwd-node-1",
                    "display_name": "Start Line",
                    "direct_addrs": ["127.0.0.1:12345", "10.0.0.7:54321"],
                    "streams": [
                        { "stream_id": "reader-a", "epoch": 3, "next_seq": 42 },
                        { "stream_id": "reader-b", "epoch": 4, "next_seq": 7 }
                    ]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let status = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body = response_json(status).await;

        assert_eq!(body["forwarders"][0]["endpoint_id"], "fwd-node-1");
        assert_eq!(body["forwarders"][0]["display_name"], "Start Line");
        assert_eq!(
            body["forwarders"][0]["direct_addrs"],
            serde_json::json!(["127.0.0.1:12345", "10.0.0.7:54321"])
        );
        assert_eq!(body["forwarders"][0]["approval_state"], "pending");
        assert!(body["forwarders"][0]["last_seen_unix_ms"].as_i64().unwrap() > 0);
        assert_eq!(body["forwarder_streams"].as_array().unwrap().len(), 2);
        assert_eq!(body["forwarder_streams"][0]["stream_id"], "reader-a");
        assert_eq!(body["forwarder_streams"][0]["endpoint_id"], "fwd-node-1");
        assert_eq!(body["forwarder_streams"][0]["epoch"], 3);
        assert_eq!(body["forwarder_streams"][0]["next_seq"], 42);

        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "fwd-node-1")
            .unwrap()
            .expect("catalog push creates pending forwarder device");
        assert_eq!(device.device_kind, crate::registry::DeviceKind::Forwarder);
        assert_eq!(
            device.approval_state,
            crate::registry::ApprovalState::Pending
        );
    }

    #[tokio::test]
    async fn public_status_no_auth() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device(
                &conn,
                "ep-1",
                crate::registry::DeviceKind::Forwarder,
                "tok",
            )
            .unwrap();
        }
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
        assert_eq!(body["announcer_source_generation"], 0);
        assert_eq!(body["finisher_count"], 0);
        assert!(body["announcer_rows"].is_array());
        assert!(body["forwarder_streams"].is_array());

        let devices = body["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["endpoint_id"], "ep-1");
        assert_eq!(devices[0]["device_kind"], "forwarder");
        assert_eq!(devices[0]["approval_state"], "pending");
    }

    #[tokio::test]
    async fn ui_fallback_serves_non_embedded_placeholder() {
        let state = test_state();
        let app = router(state);

        for path in ["/", "/admin", "/announcer"] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "path {path}");

            let body = response_text(resp).await;
            assert!(body.contains("Server UI not embedded"));
        }
    }

    #[tokio::test]
    async fn admin_route_requires_header() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device(
                &conn,
                "ep-1",
                crate::registry::DeviceKind::Receiver,
                "tok",
            )
            .unwrap();
        }
        let app = router(state.clone());

        let body = serde_json::json!({
            "endpoint_id": "ep-1"
        });

        let denied = app
            .clone()
            .oneshot(approve_request(None, &body))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        // Device must remain unapproved after a header-less request.
        {
            let conn = state.conn.lock().unwrap();
            let device = crate::registry::get_device(&conn, "ep-1").unwrap().unwrap();
            assert_eq!(
                device.approval_state,
                crate::registry::ApprovalState::Pending
            );
        }

        let approved = app
            .oneshot(approve_request(Some("alice"), &body))
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);

        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "ep-1").unwrap().unwrap();
        assert_eq!(
            device.approval_state,
            crate::registry::ApprovalState::Active
        );
    }

    #[tokio::test]
    async fn admin_approve_missing_device_404() {
        let state = test_state();
        let app = router(state);

        let resp = app
            .oneshot(approve_request(
                Some("alice"),
                &serde_json::json!({
                    "endpoint_id": "missing"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn admin_authorized_fail_closed_and_trims() {
        use crate::http::status::admin_authorized;
        use axum::http::HeaderMap;

        let mut headers = HeaderMap::new();
        headers.insert(ADMIN_HEADER, "alice".parse().unwrap());
        // Header present but proxy not trusted -> denied (fail-closed).
        assert!(!admin_authorized(&headers, false));
        // Trusted proxy + real identity -> allowed.
        assert!(admin_authorized(&headers, true));

        // Whitespace-only identity is treated as absent even when trusted.
        let mut blank = HeaderMap::new();
        blank.insert(ADMIN_HEADER, "   ".parse().unwrap());
        assert!(!admin_authorized(&blank, true));

        // Missing header -> denied regardless of trust.
        let empty = HeaderMap::new();
        assert!(!admin_authorized(&empty, true));
    }

    #[tokio::test]
    async fn admin_denied_when_proxy_untrusted() {
        // Fail-closed: with admin_proxy_trusted = false, even a present
        // Remote-User header must not authorize an admin request.
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        crate::registry::migrate(&conn).unwrap();
        let state = AppState::new(conn, PROV_TOKEN, false);
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device(
                &conn,
                "ep-1",
                crate::registry::DeviceKind::Receiver,
                "tok",
            )
            .unwrap();
        }
        let app = router(state.clone());

        let resp = app
            .oneshot(approve_request(
                Some("alice"),
                &serde_json::json!({
                    "endpoint_id": "ep-1"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Device must remain unapproved when the proxy is untrusted.
        let conn = state.conn.lock().unwrap();
        let device = crate::registry::get_device(&conn, "ep-1").unwrap().unwrap();
        assert_eq!(
            device.approval_state,
            crate::registry::ApprovalState::Pending
        );
    }
}

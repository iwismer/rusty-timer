//! Receiver allow-list distribution endpoint for forwarders.

use std::time::Duration;

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::http::{AppState, register};
use crate::registry::{self, ApprovalState, DeviceKind};

/// Upper bound on how long a long-poll request is held server-side before
/// returning the current snapshot. Kept well under typical reverse-proxy idle
/// timeouts (60s+) so a held request is never severed mid-flight by Caddy.
const MAX_HOLD: Duration = Duration::from_secs(25);

#[derive(Debug, Serialize)]
pub struct ReceiverAllowListResponse {
    pub receiver_endpoint_ids: Vec<String>,
    /// Monotonic allow-list version this snapshot reflects. Forwarders echo it
    /// back as `since` on the next request to long-poll for the next change.
    pub version: u64,
}

/// Long-poll parameters for `GET /allowlist/receivers`.
///
/// Both are optional, preserving the original immediate-fetch behaviour for a
/// bare `GET`. A forwarder supplies `since=<last version>` and a `wait` budget
/// to be released the instant the allow-list changes.
#[derive(Debug, Default, Deserialize)]
pub struct AllowListQuery {
    pub since: Option<u64>,
    pub wait: Option<u64>,
}

/// `GET /allowlist/receivers` — M2M allow-list fetch for forwarders.
///
/// Returns the current active-receiver set plus the allow-list `version`. When
/// `since` matches the current version and a non-zero `wait` is given, the
/// request is held (up to [`MAX_HOLD`]) until the version changes, then returns
/// the fresh snapshot — giving forwarders near-instant approval propagation
/// while their periodic poll remains a backstop. A mismatched or absent `since`
/// returns immediately, so server/forwarder restarts self-heal.
pub async fn receiver_allowlist(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AllowListQuery>,
) -> Response {
    match authorize(&state, &headers) {
        Ok(true) => {}
        Ok(false) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(()) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }

    // Hold the request while the caller is already up to date, so an approval
    // releases it immediately. The mutex is never held across this await.
    let wait = query.wait.unwrap_or(0);
    if wait > 0 && query.since == Some(*state.allowlist_version.borrow()) {
        let mut version_rx = state.allowlist_version.subscribe();
        let hold = MAX_HOLD.min(Duration::from_secs(wait));
        let _ = tokio::time::timeout(hold, version_rx.changed()).await;
    }

    let version = *state.allowlist_version.borrow();
    let receiver_endpoint_ids = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        match registry::list_devices(&conn) {
            Ok(devices) => devices
                .into_iter()
                .filter(|device| {
                    device.device_kind == DeviceKind::Receiver
                        && device.approval_state == ApprovalState::Active
                })
                .map(|device| device.endpoint_id)
                .collect(),
            Err(err) => {
                tracing::error!(error = %err, "failed to list receiver allow-list");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    };

    (
        StatusCode::OK,
        Json(ReceiverAllowListResponse {
            receiver_endpoint_ids,
            version,
        }),
    )
        .into_response()
}

/// Authorize an allow-list request: the in-process provisioning token or any
/// non-revoked enrolled *forwarder* device token. `Err(())` signals an internal
/// lookup failure the caller maps to `500`.
fn authorize(state: &AppState, headers: &HeaderMap) -> Result<bool, ()> {
    if register::authorized(headers, &state.provisioning_token_hash) {
        return Ok(true);
    }
    let Some(raw_bearer) = register::bearer_token(headers) else {
        return Ok(false);
    };
    let conn = state.conn.lock().expect("registry mutex poisoned");
    registry::any_device_token_authorized(&conn, DeviceKind::Forwarder, raw_bearer).map_err(|err| {
        tracing::error!(error = %err, "allow-list token authorization failed");
    })
}

#[cfg(test)]
mod tests {
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

    fn allowlist_request(token: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri("/allowlist/receivers")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    async fn response_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn approved_receiver_appears_in_forwarder_list() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device(
                &conn,
                "receiver-active",
                crate::registry::DeviceKind::Receiver,
                "tok-receiver-active",
            )
            .unwrap();
            crate::registry::approve_device(&conn, "receiver-active")
                .unwrap()
                .unwrap();
            crate::registry::register_device(
                &conn,
                "receiver-pending",
                crate::registry::DeviceKind::Receiver,
                "tok-receiver-pending",
            )
            .unwrap();
            crate::registry::register_device(
                &conn,
                "forwarder-active",
                crate::registry::DeviceKind::Forwarder,
                "tok-forwarder-active",
            )
            .unwrap();
            crate::registry::approve_device(&conn, "forwarder-active")
                .unwrap()
                .unwrap();
        }

        let resp = router(state)
            .oneshot(allowlist_request(PROV_TOKEN))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        assert_eq!(
            body["receiver_endpoint_ids"],
            serde_json::json!(["receiver-active"])
        );
    }

    #[tokio::test]
    async fn allowlist_accepts_registered_forwarder_device_token() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "forwarder-secret",
            )
            .unwrap();
            crate::registry::register_device_with_enrollment_token(
                &conn,
                "ep-fwd",
                crate::registry::DeviceKind::Forwarder,
                "forwarder-secret",
            )
            .unwrap()
            .unwrap();
        }

        let resp = router(state)
            .oneshot(allowlist_request("forwarder-secret"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn allowlist_rejects_receiver_device_token() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device(
                &conn,
                "ep-receiver",
                crate::registry::DeviceKind::Receiver,
                "receiver-secret",
            )
            .unwrap();
        }

        let resp = router(state)
            .oneshot(allowlist_request("receiver-secret"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn allowlist_rejects_revoked_forwarder_device_token() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::create_enrollment_token(
                &conn,
                "tok-1",
                crate::registry::DeviceKind::Forwarder,
                None,
                "forwarder-secret",
            )
            .unwrap();
            crate::registry::register_device_with_enrollment_token(
                &conn,
                "ep-fwd",
                crate::registry::DeviceKind::Forwarder,
                "forwarder-secret",
            )
            .unwrap()
            .unwrap();
            crate::registry::revoke_enrollment_token(&conn, "tok-1")
                .unwrap()
                .unwrap();
        }

        let resp = router(state)
            .oneshot(allowlist_request("forwarder-secret"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn allowlist_requires_bearer_auth() {
        let resp = router(test_state())
            .oneshot(allowlist_request("wrong-token"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bare_request_returns_version_zero() {
        let resp = router(test_state())
            .oneshot(allowlist_request(PROV_TOKEN))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        assert_eq!(body["version"], serde_json::json!(0));
        assert_eq!(body["receiver_endpoint_ids"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn long_poll_mismatched_since_returns_immediately() {
        // since != current(0) must not hold even with a large wait budget.
        let state = test_state();
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::receiver_allowlist(
                axum::extract::State(state),
                bearer_headers(PROV_TOKEN),
                axum::extract::Query(super::AllowListQuery {
                    since: Some(99),
                    wait: Some(30),
                }),
            ),
        )
        .await
        .expect("mismatched since must return immediately, not hold");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn long_poll_releases_on_approval() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::register_device(
                &conn,
                "receiver-late",
                crate::registry::DeviceKind::Receiver,
                "tok-receiver-late",
            )
            .unwrap();
        }

        // A forwarder long-polls at the current version; it must block until an
        // approval bumps the version, then return the fresh snapshot.
        let held = {
            let state = state.clone();
            tokio::spawn(async move {
                super::receiver_allowlist(
                    axum::extract::State(state),
                    bearer_headers(PROV_TOKEN),
                    axum::extract::Query(super::AllowListQuery {
                        since: Some(0),
                        wait: Some(30),
                    }),
                )
                .await
            })
        };

        // Give the handler time to enter the hold, then approve + bump.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!held.is_finished(), "handler must hold while up to date");
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::approve_device(&conn, "receiver-late")
                .unwrap()
                .unwrap();
        }
        state.bump_allowlist_version();

        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), held)
            .await
            .expect("approval must release the held long-poll")
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        assert_eq!(body["version"], serde_json::json!(1));
        assert_eq!(
            body["receiver_endpoint_ids"],
            serde_json::json!(["receiver-late"])
        );
    }

    fn bearer_headers(token: &str) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }
}

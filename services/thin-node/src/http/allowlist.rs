//! Receiver allow-list distribution endpoint for forwarders.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::http::{AppState, register};
use crate::registry::{self, ApprovalState, DeviceKind};

#[derive(Debug, Serialize)]
pub struct ReceiverAllowListResponse {
    pub receiver_endpoint_ids: Vec<String>,
}

/// `GET /allowlist/receivers` — M2M allow-list fetch for forwarders.
pub async fn receiver_allowlist(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !register::authorized(&headers, &state.provisioning_token_hash) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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
        }),
    )
        .into_response()
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
        AppState::new(conn, PROV_TOKEN)
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
            crate::registry::approve_device(&conn, "receiver-active", "Finish")
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
            crate::registry::approve_device(&conn, "forwarder-active", "Start")
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
    async fn allowlist_requires_bearer_auth() {
        let resp = router(test_state())
            .oneshot(allowlist_request("wrong-token"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

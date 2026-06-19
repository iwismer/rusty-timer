//! Forwarder catalog push endpoint.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::http::{AppState, register};
use crate::registry::{self, DeviceKind, ForwarderCatalogStreamRecord};

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
/// Accepts the shared provisioning token for legacy deployments and enrolled
/// forwarder-scoped device tokens for per-device authorization. Receiver tokens
/// must not authorize this endpoint.
pub async fn push_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ForwarderCatalogRequest>,
) -> Response {
    let authorized = if register::authorized(&headers, &state.provisioning_token_hash) {
        true
    } else if let Some(raw_bearer) = register::bearer_token(&headers) {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        match registry::device_token_authorized(
            &conn,
            &req.endpoint_id,
            DeviceKind::Forwarder,
            raw_bearer,
        ) {
            Ok(authorized) => authorized,
            Err(err) => {
                tracing::error!(error = %err, "catalog token authorization failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else {
        false
    };
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
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
            &state.provisioning_token_hash,
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

    fn catalog_request(token: &str, endpoint_id: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/forwarder/catalog")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "endpoint_id": endpoint_id,
                    "display_name": "Start Line",
                    "direct_addrs": ["127.0.0.1:5000"],
                    "streams": [{
                        "stream_id": "reader-a",
                        "epoch": 1,
                        "next_seq": 2
                    }]
                }))
                .unwrap(),
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn catalog_accepts_registered_forwarder_device_token() {
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
            .oneshot(catalog_request("forwarder-secret", "ep-fwd"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn catalog_rejects_receiver_device_token() {
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
            .oneshot(catalog_request("receiver-secret", "ep-receiver"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn catalog_rejects_revoked_registered_device_token() {
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
            .oneshot(catalog_request("forwarder-secret", "ep-fwd"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

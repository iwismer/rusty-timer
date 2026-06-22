//! Approved-forwarder discovery endpoint for receivers.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::http::{AppState, authorize_active_device_kind};
use crate::registry::{self, ApprovedForwarderWithStreams, DeviceKind};

#[derive(Debug, Serialize)]
pub struct ForwardersResponse {
    pub forwarders: Vec<ApprovedForwarderWithStreams>,
}

/// `GET /forwarders` — M2M discovery of approved forwarders for receivers.
///
/// Authorized by the provisioning bearer token (same posture as
/// `GET /allowlist/receivers`). Returns only approved (`active`) forwarder
/// devices joined with their pushed stream catalog.
pub async fn list_forwarders(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(status) = authorize_active_device_kind(&state, &headers, DeviceKind::Receiver) {
        return status.into_response();
    }

    let forwarders = {
        let conn = state.conn.lock().expect("registry mutex poisoned");
        match registry::list_approved_forwarders_with_streams(&conn) {
            Ok(forwarders) => forwarders,
            Err(err) => {
                tracing::error!(error = %err, "failed to list approved forwarders");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    };

    (StatusCode::OK, Json(ForwardersResponse { forwarders })).into_response()
}

#[cfg(test)]
mod tests {
    use crate::http::{AppState, router};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use rusqlite::Connection;
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

    fn forwarders_request(token: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri("/forwarders")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    async fn response_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn approved_forwarder_with_streams_appears() {
        let state = test_state();
        {
            let conn = state.conn.lock().unwrap();
            crate::registry::upsert_forwarder_catalog(
                &conn,
                "fwd-approved",
                Some("Start Line"),
                &["127.0.0.1:5000".to_owned(), "10.0.0.7:5000".to_owned()],
                &[
                    crate::registry::ForwarderCatalogStreamRecord {
                        stream_id: "reader-a".to_owned(),
                        epoch: 3,
                        next_seq: 42,
                    },
                    crate::registry::ForwarderCatalogStreamRecord {
                        stream_id: "reader-b".to_owned(),
                        epoch: 4,
                        next_seq: 7,
                    },
                ],
            )
            .unwrap();
            crate::registry::approve_device(&conn, "fwd-approved")
                .unwrap()
                .unwrap();

            // An unapproved forwarder must be excluded.
            crate::registry::upsert_forwarder_catalog(
                &conn,
                "fwd-pending",
                Some("Pending"),
                &["127.0.0.1:6000".to_owned()],
                &[crate::registry::ForwarderCatalogStreamRecord {
                    stream_id: "reader-c".to_owned(),
                    epoch: 1,
                    next_seq: 1,
                }],
            )
            .unwrap();
        }

        let resp = router(state)
            .oneshot(forwarders_request(RX_TOKEN))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        let forwarders = body["forwarders"].as_array().unwrap();
        assert_eq!(forwarders.len(), 1, "only approved forwarders returned");
        assert_eq!(forwarders[0]["endpoint_id"], "fwd-approved");
        assert_eq!(forwarders[0]["display_name"], "Start Line");
        assert_eq!(
            forwarders[0]["direct_addrs"],
            serde_json::json!(["127.0.0.1:5000", "10.0.0.7:5000"])
        );
        let streams = forwarders[0]["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0]["stream_id"], "reader-a");
        assert_eq!(streams[0]["epoch"], 3);
        assert_eq!(streams[0]["next_seq"], 42);
        assert_eq!(streams[1]["stream_id"], "reader-b");
    }

    #[tokio::test]
    async fn forwarders_requires_bearer_auth() {
        let resp = router(test_state())
            .oneshot(forwarders_request("wrong-token"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn forwarders_accepts_active_receiver_device_token() {
        let state = test_state();
        let token = {
            let conn = state.conn.lock().unwrap();
            let minted = crate::registry::register_device_minted(
                &conn,
                "rx-1",
                crate::registry::DeviceKind::Receiver,
            )
            .unwrap();
            crate::registry::approve_device(&conn, "rx-1")
                .unwrap()
                .unwrap();
            minted.device_token
        };
        let resp = router(state)
            .oneshot(forwarders_request(&token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn forwarders_denies_pending_receiver_and_forwarder_tokens() {
        let state = test_state();
        let (pending_rx, active_fwd) = {
            let conn = state.conn.lock().unwrap();
            let rx = crate::registry::register_device_minted(
                &conn,
                "rx-1",
                crate::registry::DeviceKind::Receiver,
            )
            .unwrap();
            let fwd = crate::registry::register_device_minted(
                &conn,
                "fwd-1",
                crate::registry::DeviceKind::Forwarder,
            )
            .unwrap();
            crate::registry::approve_device(&conn, "fwd-1")
                .unwrap()
                .unwrap();
            (rx.device_token, fwd.device_token)
        };
        // Pending receiver is denied.
        let resp = router(state.clone())
            .oneshot(forwarders_request(&pending_rx))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // Active forwarder (wrong kind) is denied.
        let resp = router(state)
            .oneshot(forwarders_request(&active_fwd))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

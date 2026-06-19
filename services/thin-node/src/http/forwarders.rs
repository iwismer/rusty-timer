//! Approved-forwarder discovery endpoint for receivers.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::http::{AppState, register};
use crate::registry::{self, ApprovedForwarderWithStreams};

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
    if !register::authorized(&headers, &state.provisioning_token_hash) {
        return StatusCode::UNAUTHORIZED.into_response();
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

    const PROV_TOKEN: &str = "prov-secret";

    fn test_state() -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        crate::registry::migrate(&conn).unwrap();
        AppState::new(conn, PROV_TOKEN, true)
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
            let token_hash = crate::registry::hash_token(PROV_TOKEN);
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
                &token_hash,
            )
            .unwrap();
            crate::registry::approve_device(&conn, "fwd-approved", "Start Line")
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
                &token_hash,
            )
            .unwrap();
        }

        let resp = router(state)
            .oneshot(forwarders_request(PROV_TOKEN))
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
}

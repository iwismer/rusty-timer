//! Forwarder catalog push endpoint.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::http::{AppState, register};
use crate::registry::{self, ForwarderCatalogStreamRecord};

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
pub async fn push_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ForwarderCatalogRequest>,
) -> Response {
    if !register::authorized(&headers, &state.provisioning_token_hash) {
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

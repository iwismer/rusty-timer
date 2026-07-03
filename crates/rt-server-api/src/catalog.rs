use serde::{Deserialize, Serialize};

/// Wire-format forwarder catalog pushed to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderCatalogRequest {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<String>,
    pub streams: Vec<ForwarderCatalogStream>,
}

/// Wire-format stream entry in a forwarder catalog push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderCatalogStream {
    pub stream_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderCatalogResponse {
    pub endpoint_id: String,
    pub stream_count: usize,
}

/// Wire-format response for `GET /forwarder/catalog`: the caller's own stored
/// stream catalog (per-stream epoch/next_seq high-water), used by a forwarder
/// to restore stream identity after local journal loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderOwnCatalogResponse {
    pub streams: Vec<ForwarderCatalogStream>,
}

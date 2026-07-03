use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardersResponse {
    pub forwarders: Vec<ApprovedForwarder>,
}

/// One approved forwarder entry returned to receivers for discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedForwarder {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<String>,
    pub streams: Vec<ApprovedForwarderStream>,
}

/// One stream entry of an approved forwarder, for receiver discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedForwarderStream {
    pub stream_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

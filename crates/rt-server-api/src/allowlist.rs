use serde::{Deserialize, Serialize};

/// Wire-format receiver allow-list snapshot distributed by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverAllowListResponse {
    pub receiver_endpoint_ids: Vec<String>,
    /// Monotonic allow-list version this snapshot reflects. Forwarders echo it
    /// back as `since` on the next long-poll. Defaults to 0 for an older server
    /// that does not emit a version, degrading to immediate re-polling.
    #[serde(default)]
    pub version: u64,
}

impl ReceiverAllowListResponse {
    #[must_use]
    pub fn replace(receiver_endpoint_ids: Vec<String>) -> Self {
        Self {
            receiver_endpoint_ids,
            version: 0,
        }
    }
}

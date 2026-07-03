use serde::{Deserialize, Serialize};

use crate::device::{ApprovalState, DeviceKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    /// Stable endpoint identifier of the registering device.
    pub endpoint_id: String,
    /// `"forwarder"` or `"receiver"`.
    pub device_kind: String,
    /// Optional human-friendly name the device reports for itself.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub endpoint_id: String,
    pub device_kind: DeviceKind,
    pub approval_state: ApprovalState,
    /// The minted per-device bearer token, returned exactly once when a token
    /// is minted or rotated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_token: Option<String>,
}

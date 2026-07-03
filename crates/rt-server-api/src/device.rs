use serde::{Deserialize, Serialize};

/// Kind of device that can register with the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Forwarder,
    Receiver,
}

impl DeviceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forwarder => "forwarder",
            Self::Receiver => "receiver",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "forwarder" => Some(Self::Forwarder),
            "receiver" => Some(Self::Receiver),
            _ => None,
        }
    }
}

/// Approval state of a registered device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalState {
    Pending,
    Active,
}

impl ApprovalState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            _ => None,
        }
    }
}

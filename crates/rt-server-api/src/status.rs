use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::device::{ApprovalState, DeviceKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Current announcer source generation (fencing token).
    pub announcer_source_generation: u64,
    /// Unique-chip finisher count from the live announcer runtime.
    pub finisher_count: u64,
    /// Most recent announcer rows held in the live runtime, newest first.
    pub announcer_rows: Vec<AnnouncerRow>,
    /// All registered devices and their approval state.
    pub devices: Vec<DeviceRecord>,
    /// Latest pushed forwarder identities, if any.
    pub forwarders: Vec<ForwarderRecord>,
    /// Backup forwarder stream catalog rows, if any.
    pub forwarder_streams: Vec<ForwarderStreamRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnouncerRow {
    /// Iroh endpoint id of the forwarder the row originated from. Together
    /// with `stream_id` this forms the composite stream identity; the wire
    /// `stream_id` alone is ambiguous across forwarders.
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_at: DateTime<Utc>,
    pub division: Option<String>,
}

/// A registered device record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub endpoint_id: String,
    pub device_kind: DeviceKind,
    pub approval_state: ApprovalState,
    pub display_name: Option<String>,
}

/// A registered forwarder's latest pushed identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderRecord {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<String>,
    pub last_seen_unix_ms: i64,
    pub approval_state: ApprovalState,
}

/// A backup row from the forwarder stream catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderStreamRecord {
    pub stream_id: String,
    pub endpoint_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

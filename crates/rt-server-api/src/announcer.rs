use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRowRequest {
    pub announcer_source_generation: u64,
    pub stream_id: String,
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_unix_ms: i64,
    /// Division display name resolved by the receiver, when known. Optional for
    /// backward compatibility with receivers that predate division support.
    #[serde(default)]
    pub division: Option<String>,
    /// Receiver-configured cap on visible announcer rows. Absent or zero falls
    /// back to the server default.
    #[serde(default)]
    pub max_list_size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRowResponse {
    pub announcer_source_generation: u64,
    pub finisher_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeoverResponse {
    pub announcer_source_generation: u64,
}

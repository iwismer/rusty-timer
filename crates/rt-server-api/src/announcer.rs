use serde::{Deserialize, Serialize};

/// Maximum number of rows the server accepts in one `POST /announcer/rows`
/// batch. Receivers chunk their unpushed backlog to this size; the server
/// rejects larger (or empty) batches with `400`.
pub const MAX_PUSH_ROWS: usize = 500;

/// A batch of announcer rows pushed by a receiver.
///
/// One announcer source generation and one stream identity per request BY
/// CONSTRUCTION: the generation, the originating forwarder endpoint, and the
/// wire stream id are top-level fields, so mixed-generation or mixed-stream
/// batches are structurally impossible. The server keys storage and
/// idempotency on `(forwarder_endpoint_id, stream_id, seq)` — two forwarders
/// exposing the same wire `stream_id` never collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRowsRequest {
    pub announcer_source_generation: u64,
    /// Iroh endpoint id of the forwarder the rows originated from.
    pub forwarder_endpoint_id: String,
    /// Forwarder-scoped wire stream id (NOT the receiver's encoded local key).
    pub stream_id: String,
    pub rows: Vec<PushRow>,
    /// Receiver-configured cap on visible announcer rows. Absent or zero falls
    /// back to the server default.
    #[serde(default)]
    pub max_list_size: Option<u32>,
}

/// One announcer row within a [`PushRowsRequest`] batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRow {
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_unix_ms: i64,
    /// Division display name resolved by the receiver, when known.
    #[serde(default)]
    pub division: Option<String>,
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

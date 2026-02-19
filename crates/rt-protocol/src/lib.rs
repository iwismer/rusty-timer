// rt-protocol: Remote forwarding protocol types and serialization.
//
// All WebSocket messages use a top-level `kind` field for discriminated
// deserialization.  The enum variants map 1:1 to the frozen v1 message kinds.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared sub-types
// ---------------------------------------------------------------------------

/// A resume cursor for a single (stream, epoch) pair.
///
/// Used by both forwarder and receiver hello messages to communicate the
/// highest sequence number the device has already processed, enabling the
/// server to replay only the missing tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCursor {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub stream_epoch: u64,
    pub last_seq: u64,
}

/// A single read event carried in event batch messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadEvent {
    /// Redundant for self-describing messages.
    pub forwarder_id: String,
    pub reader_ip: String,
    pub stream_epoch: u64,
    pub seq: u64,
    /// Device-reported timestamp; accepted as-is, no server adjustment.
    pub reader_timestamp: String,
    /// UTF-8 text; ASCII IPICO payload expected.  Invalid UTF-8 is rejected.
    pub raw_read_line: String,
    /// E.g. "RAW" or "FSLS".
    pub read_type: String,
}

/// One entry in an ack message, covering the high-water mark for a
/// (stream, epoch) pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckEntry {
    /// Redundant for self-describing messages.
    pub forwarder_id: String,
    pub reader_ip: String,
    pub stream_epoch: u64,
    pub last_seq: u64,
}

// ---------------------------------------------------------------------------
// Forwarder -> Server messages
// ---------------------------------------------------------------------------

/// Forwarder hello / re-hello message.
///
/// Sent as the first message after connecting (and again after epoch reset).
/// Does NOT carry `session_id` -- the session_id is assigned by the server
/// and returned in the first `heartbeat`.
///
/// The `resume` list implicitly subscribes the server to begin (or resume)
/// delivering events for those (stream, epoch) pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderHello {
    /// Advisory identity; must match token claims if present.
    pub forwarder_id: String,
    /// IP addresses of locally attached readers.
    pub reader_ips: Vec<String>,
    /// Resume cursors so the server knows where to start acking from.
    /// An empty list means the forwarder starts fresh (no prior history).
    #[serde(default)]
    pub resume: Vec<ResumeCursor>,
    /// Human-friendly name for this forwarder (e.g. "Start Line").
    /// Configured in the forwarder's TOML config. Optional for backward compat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// A batch of read events from a forwarder.
///
/// `batch_id` is an opaque correlation ID for logging/debugging.  The server
/// does NOT use it for ack logic; acks reference (stream, epoch, last_seq).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderEventBatch {
    pub session_id: String,
    /// Opaque correlation ID -- no semantic ack meaning.
    pub batch_id: String,
    pub events: Vec<ReadEvent>,
}

// ---------------------------------------------------------------------------
// Server -> Forwarder messages
// ---------------------------------------------------------------------------

/// Server acknowledgement of a `ForwarderEventBatch`.
///
/// One ack is sent per persisted batch.  A single ack message may cover
/// multiple (stream, epoch) pairs when the batch spans epoch boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderAck {
    pub session_id: String,
    pub entries: Vec<AckEntry>,
}

// ---------------------------------------------------------------------------
// Receiver -> Server messages
// ---------------------------------------------------------------------------

/// Receiver hello / re-hello message.
///
/// Does NOT carry `session_id` -- the session_id is assigned by the server
/// and returned in the first `heartbeat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverHello {
    /// Advisory identity; must match token claims if present.
    pub receiver_id: String,
    /// Resume cursors for streams the receiver already has data for.
    #[serde(default)]
    pub resume: Vec<ResumeCursor>,
}

/// Subscribe to additional streams mid-session.
///
/// `receiver_hello` implicitly subscribes to streams in `resume`.
/// `receiver_subscribe` adds new streams after the session is established.
/// There is no unsubscribe in v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverSubscribe {
    pub session_id: String,
    pub streams: Vec<StreamRef>,
}

/// A reference to a stream by its immutable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamRef {
    pub forwarder_id: String,
    pub reader_ip: String,
}

// ---------------------------------------------------------------------------
// Server -> Receiver messages
// ---------------------------------------------------------------------------

/// A batch of read events delivered from the server to a receiver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverEventBatch {
    pub session_id: String,
    pub events: Vec<ReadEvent>,
}

/// Receiver acknowledgement of a `ReceiverEventBatch`.
///
/// A single ack may contain multiple entries spanning (stream, epoch) pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverAck {
    pub session_id: String,
    pub entries: Vec<AckEntry>,
}

// ---------------------------------------------------------------------------
// Bidirectional / server-initiated messages
// ---------------------------------------------------------------------------

/// Heartbeat message (server -> client).
///
/// Sent at 30-second intervals; three missed heartbeats (90 s timeout) cause
/// disconnect.  The *initial* server heartbeat also carries `session_id` and
/// `device_id` so the device can learn both values.  Clients must not send
/// their own heartbeats until they have received the first server heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub session_id: String,
    /// Resolved from token claims.  Devices use this to learn their own identity.
    pub device_id: String,
}

/// Frozen v1 error codes.
pub mod error_codes {
    pub const INVALID_TOKEN: &str = "INVALID_TOKEN";
    pub const SESSION_EXPIRED: &str = "SESSION_EXPIRED";
    pub const PROTOCOL_ERROR: &str = "PROTOCOL_ERROR";
    pub const IDENTITY_MISMATCH: &str = "IDENTITY_MISMATCH";
    pub const INTEGRITY_CONFLICT: &str = "INTEGRITY_CONFLICT";
    pub const INTERNAL_ERROR: &str = "INTERNAL_ERROR";
}

/// Protocol error message (server -> client).
///
/// | Code                | Retryable |
/// |---------------------|-----------|
/// | INVALID_TOKEN       | false     |
/// | SESSION_EXPIRED     | true      |
/// | PROTOCOL_ERROR      | false     |
/// | IDENTITY_MISMATCH   | false     |
/// | INTEGRITY_CONFLICT  | false     |
/// | INTERNAL_ERROR      | true      |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMessage {
    /// One of the frozen v1 error codes.
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// Server-to-forwarder command: reset the epoch for a stream.
///
/// Semantics:
/// - `stream_epoch` increments to `new_stream_epoch`.
/// - `seq` restarts at 1 for new events on the affected stream.
/// - Unacked events from older epochs remain eligible for replay/ack until drained.
/// - The forwarder confirms by sending a new `forwarder_hello` with the updated epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochResetCommand {
    pub session_id: String,
    pub forwarder_id: String,
    pub reader_ip: String,
    pub new_stream_epoch: u64,
}

// ---------------------------------------------------------------------------
// Config messages (server <-> forwarder)
// ---------------------------------------------------------------------------

/// Server-to-forwarder: request the current config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigGetRequest {
    pub request_id: String,
}

/// Forwarder-to-server: current config response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigGetResponse {
    pub request_id: String,
    pub ok: bool,
    pub error: Option<String>,
    pub config: serde_json::Value,
    pub restart_needed: bool,
}

/// Server-to-forwarder: update a config section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSetRequest {
    pub request_id: String,
    pub section: String,
    pub payload: serde_json::Value,
}

/// Forwarder-to-server: config update result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSetResponse {
    pub request_id: String,
    pub ok: bool,
    pub error: Option<String>,
    pub restart_needed: bool,
}

/// Server-to-forwarder: request a graceful restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartRequest {
    pub request_id: String,
}

/// Forwarder-to-server: restart acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartResponse {
    pub request_id: String,
    pub ok: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Top-level discriminated union
// ---------------------------------------------------------------------------

/// All WebSocket message kinds in the v1 protocol.
///
/// Serializes/deserializes using the `kind` field as a tag.
///
/// ```json
/// { "kind": "forwarder_hello", ... }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
pub enum WsMessage {
    ForwarderHello(ForwarderHello),
    ForwarderEventBatch(ForwarderEventBatch),
    ForwarderAck(ForwarderAck),
    ReceiverHello(ReceiverHello),
    ReceiverSubscribe(ReceiverSubscribe),
    ReceiverEventBatch(ReceiverEventBatch),
    ReceiverAck(ReceiverAck),
    Heartbeat(Heartbeat),
    Error(ErrorMessage),
    EpochResetCommand(EpochResetCommand),
    ConfigGetRequest(ConfigGetRequest),
    ConfigGetResponse(ConfigGetResponse),
    ConfigSetRequest(ConfigSetRequest),
    ConfigSetResponse(ConfigSetResponse),
    RestartRequest(RestartRequest),
    RestartResponse(RestartResponse),
}

// ---------------------------------------------------------------------------
// HTTP API response types (frozen schema definitions)
// ---------------------------------------------------------------------------

/// One entry in the `GET /api/v1/streams` response array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamInfo {
    pub stream_id: String,
    pub forwarder_id: String,
    pub reader_ip: String,
    /// Human-readable label e.g. "Start", "Finish", "CP-1".
    pub display_alias: Option<String>,
    pub stream_epoch: u64,
    /// True when the forwarder's WS session is currently connected.
    pub online: bool,
}

/// Request body for `PATCH /api/v1/streams/{stream_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPatchRequest {
    pub display_alias: String,
}

/// Response for `GET /api/v1/streams/{stream_id}/metrics`.
///
/// Invariant: `raw_count == dedup_count + retransmit_count`.
/// Lag is null when the stream has no canonical events.
/// Backlog is 0 when there are no active receivers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamMetrics {
    /// Total read lines received (includes retransmits).
    pub raw_count: u64,
    /// Unique canonical events stored.
    pub dedup_count: u64,
    /// Count of retransmissions of events already stored.
    pub retransmit_count: u64,
    /// Milliseconds since last canonical event (null if none).
    pub lag_ms: Option<u64>,
    /// Canonical events not yet acked by the slowest active receiver.
    pub backlog: u64,
}

/// Response body for `POST /api/v1/streams/{stream_id}/reset-epoch` on success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetEpochResponse {
    pub new_stream_epoch: u64,
}

/// Frozen HTTP error envelope used by all non-2xx responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpErrorEnvelope {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

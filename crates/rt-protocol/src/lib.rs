// rt-protocol: Remote forwarding protocol types and serialization.
//
// All WebSocket messages use a top-level `kind` field for discriminated
// deserialization. The enum covers the frozen v1/v1.2 message kinds plus
// reader-status tracking and reader-control extensions.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared sub-types
// ---------------------------------------------------------------------------

/// A resume cursor for a single (stream, epoch) pair.
///
/// Used by receiver hello messages to communicate the highest sequence number
/// already processed per stream, enabling the server to replay only the
/// missing tail.
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
    /// Exact bytes received from the reader frame.
    pub raw_frame: Vec<u8>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderHello {
    /// Advisory identity; must match token claims if present.
    pub forwarder_id: String,
    /// IP addresses of locally attached readers.
    pub reader_ips: Vec<String>,
    /// Human-friendly name for this forwarder (e.g. "Start Line").
    /// Configured in the forwarder's TOML config. Optional in payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Forwarder-to-server: report a reader connection/disconnection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderStatusUpdate {
    pub reader_ip: String,
    pub connected: bool,
}

/// Server-to-receiver: a reader's connection status has changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderStatusChanged {
    pub stream_id: Uuid,
    pub reader_ip: String,
    pub connected: bool,
}

/// Sentinel `read_type` prefix used to tunnel control messages through the
/// per-stream `ReadEvent` broadcast channel. The server's receiver WS handler
/// MUST filter events with this prefix, never forward them as chip reads,
/// and never advance stream cursors for them.
pub const SENTINEL_READ_TYPE_PREFIX: &str = "__";

/// Sentinel `read_type` for reader-status-changed control messages.
/// When a `ReadEvent` carries this type, its `raw_frame` contains the
/// JSON-serialized `WsMessage::ReaderStatusChanged` payload.
pub const READER_STATUS_CHANGED_READ_TYPE: &str = "__reader_status_changed";

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

/// A reference to a stream by its immutable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamRef {
    pub forwarder_id: String,
    pub reader_ip: String,
}

/// Explicit replay target for targeted replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayTarget {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub stream_epoch: i64,
    #[serde(default = "default_replay_from_seq")]
    pub from_seq: i64,
}

fn default_replay_from_seq() -> i64 {
    1
}

// ---------------------------------------------------------------------------
// Receiver v1.2 mode-based protocol types
// ---------------------------------------------------------------------------

/// Optional bound to start at the earliest epoch when replaying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarliestEpochOverride {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub earliest_epoch: i64,
}

/// Receiver operating mode for v1.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReceiverMode {
    /// Subscribe to live traffic with explicit streams and optional earliest epoch overrides.
    Live {
        streams: Vec<StreamRef>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        earliest_epochs: Vec<EarliestEpochOverride>,
    },
    /// Race-scoped mode.
    Race { race_id: String },
    /// Explicit targeted replay set.
    TargetedReplay { targets: Vec<ReplayTarget> },
}

/// Receiver hello / re-hello message for v1.2 mode-based protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverHelloV12 {
    pub receiver_id: String,
    pub mode: ReceiverMode,
    /// Resume cursors for streams the receiver already has data for.
    #[serde(default)]
    pub resume: Vec<ResumeCursor>,
}

/// Server acknowledgement of applied receiver mode in v1.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverModeApplied {
    pub mode_summary: String,
    pub resolved_stream_count: usize,
    #[serde(default)]
    pub warnings: Vec<String>,
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
/// - The forwarder applies the new epoch locally before sending subsequent events.
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
    /// Optional status hint from forwarder local handler (e.g. 400 vs 500).
    pub status_code: Option<u16>,
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
// Reader control types (server <-> forwarder)
// ---------------------------------------------------------------------------

/// IPICO reader read mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadMode {
    #[serde(rename = "raw")]
    Raw,
    #[serde(rename = "event")]
    Event,
    #[serde(rename = "fsls")]
    FirstLastSeen,
}

/// Reader connection state as reported by the forwarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderConnectionState {
    Connected,
    Connecting,
    Disconnected,
}

/// State of a reader download operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Downloading,
    Complete,
    Error,
    Idle,
}

/// Hardware identity information reported by the reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareInfo {
    pub fw_version: Option<String>,
    pub hw_code: Option<String>,
    pub reader_id: Option<String>,
}

/// Reader CONFIG3 settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config3Info {
    pub mode: ReadMode,
    pub timeout: u8,
}

/// Reader clock state and drift estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockInfo {
    pub reader_clock: String,
    pub drift_ms: i64,
}

/// Aggregated reader information, populated incrementally as details are discovered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<HardwareInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Config3Info>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tto_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_stored_reads: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<bool>,
    #[serde(default)]
    pub connect_failures: u8,
}

/// Actions that can be requested on a reader via the control channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReaderControlAction {
    GetInfo,
    SyncClock,
    SetReadMode { mode: ReadMode, timeout: u8 },
    SetTto { enabled: bool },
    SetRecording { enabled: bool },
    ClearRecords,
    StartDownload,
    StopDownload,
    Refresh,
    Reconnect,
}

/// Server-to-forwarder: request an action on a specific reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderControlRequest {
    pub request_id: String,
    pub reader_ip: String,
    pub action: ReaderControlAction,
}

/// Forwarder-to-server: result of a reader control action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderControlResponse {
    pub request_id: String,
    pub reader_ip: String,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

/// Forwarder-to-server: unsolicited reader state/info update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderInfoUpdate {
    pub reader_ip: String,
    pub state: ReaderConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

/// Forwarder-to-server: download progress notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderDownloadProgress {
    pub reader_ip: String,
    pub state: DownloadState,
    pub reads_received: u32,
    pub progress: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Top-level discriminated union
// ---------------------------------------------------------------------------

/// All WebSocket message kinds in the v1/v1.2 protocols, plus reader
/// status tracking extensions.
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
    ReceiverHelloV12(ReceiverHelloV12),
    ReceiverModeApplied(ReceiverModeApplied),
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
    ReaderStatusUpdate(ReaderStatusUpdate),
    ReaderStatusChanged(ReaderStatusChanged),
    ReaderControlRequest(ReaderControlRequest),
    ReaderControlResponse(ReaderControlResponse),
    ReaderInfoUpdate(ReaderInfoUpdate),
    ReaderDownloadProgress(ReaderDownloadProgress),
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
    /// True when the forwarder reports its TCP connection to the reader hardware
    /// is up. Invariant: `!online` implies `!reader_connected`.
    #[serde(default)]
    pub reader_connected: bool,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_status_update_round_trip() {
        let msg = WsMessage::ReaderStatusUpdate(ReaderStatusUpdate {
            reader_ip: "192.168.1.10".to_string(),
            connected: true,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"reader_status_update\""));
        assert!(json.contains("\"connected\":true"));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn reader_status_changed_round_trip() {
        let msg = WsMessage::ReaderStatusChanged(ReaderStatusChanged {
            stream_id: Uuid::nil(),
            reader_ip: "192.168.1.10".to_string(),
            connected: false,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"reader_status_changed\""));
        assert!(json.contains("\"connected\":false"));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn reader_control_request_round_trip() {
        let msg = WsMessage::ReaderControlRequest(ReaderControlRequest {
            request_id: "abc".into(),
            reader_ip: "192.168.0.1:10000".into(),
            action: ReaderControlAction::SetReadMode {
                mode: ReadMode::Event,
                timeout: 5,
            },
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn reader_control_response_round_trip() {
        let msg = WsMessage::ReaderControlResponse(ReaderControlResponse {
            request_id: "abc".into(),
            reader_ip: "192.168.0.1:10000".into(),
            success: true,
            error: None,
            reader_info: Some(ReaderInfo {
                banner: None,
                hardware: Some(HardwareInfo {
                    fw_version: Some("3.09".into()),
                    hw_code: Some("0x1234".into()),
                    reader_id: Some("READER01".into()),
                }),
                config: Some(Config3Info {
                    mode: ReadMode::Event,
                    timeout: 5,
                }),
                tto_enabled: Some(false),
                clock: Some(ClockInfo {
                    reader_clock: "2026-03-08T12:00:00".into(),
                    drift_ms: 42,
                }),
                estimated_stored_reads: Some(100),
                recording: Some(true),
                connect_failures: 0,
            }),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn reader_info_update_round_trip() {
        let msg = WsMessage::ReaderInfoUpdate(ReaderInfoUpdate {
            reader_ip: "192.168.0.1:10000".into(),
            state: ReaderConnectionState::Connected,
            reader_info: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn reader_download_progress_round_trip() {
        let msg = WsMessage::ReaderDownloadProgress(ReaderDownloadProgress {
            reader_ip: "192.168.0.1:10000".into(),
            state: DownloadState::Downloading,
            reads_received: 50,
            progress: 1024,
            total: 2048,
            error: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }
}

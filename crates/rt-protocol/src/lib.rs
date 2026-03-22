// rt-protocol: Remote forwarding protocol types and serialization.
//
// All WebSocket messages use a top-level `kind` field for discriminated
// deserialization. The enum covers the v1/v1.2 message kinds, reader-status
// tracking, reader-control extensions, receiver proxy commands, and race
// management.

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

fn deserialize_non_negative_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let v = i64::deserialize(d)?;
    if v < 0 {
        return Err(serde::de::Error::custom(format!(
            "expected non-negative value, got {v}"
        )));
    }
    Ok(v)
}

fn deserialize_non_negative_i32<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    let v = i32::deserialize(d)?;
    if v < 0 {
        return Err(serde::de::Error::custom(format!(
            "expected non-negative value, got {v}"
        )));
    }
    Ok(v)
}

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
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub stream_epoch: i64,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub last_seq: i64,
}

/// A single read event carried in event batch messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadEvent {
    /// Redundant for self-describing messages.
    pub forwarder_id: String,
    pub reader_ip: String,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub stream_epoch: i64,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub seq: i64,
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
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub stream_epoch: i64,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub last_seq: i64,
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

/// Sentinel `read_type` for reader-info-updated control messages.
/// When a `ReadEvent` carries this type, its `raw_frame` contains the
/// JSON-serialized `WsMessage::ReceiverReaderInfoUpdate` payload.
pub const READER_INFO_UPDATED_READ_TYPE: &str = "__reader_info_updated";

/// Sentinel `read_type` for reader-download-progress control messages.
/// When a `ReadEvent` carries this type, its `raw_frame` contains the
/// JSON-serialized `WsMessage::ReceiverReaderDownloadProgress` payload.
pub const READER_DOWNLOAD_PROGRESS_READ_TYPE: &str = "__reader_download_progress";

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
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub stream_epoch: i64,
    #[serde(
        default = "default_replay_from_seq",
        deserialize_with = "deserialize_non_negative_i64"
    )]
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
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
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

/// Per-stream metrics delivered over the authenticated WS connection.
///
/// Sent server→receiver after mode application and on live metric updates.
/// Contains both lifetime and current-epoch counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverStreamMetrics {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub raw_count: i64,
    pub dedup_count: i64,
    pub retransmit_count: i64,
    pub lag_ms: Option<u64>,
    pub epoch_raw_count: i64,
    pub epoch_dedup_count: i64,
    pub epoch_retransmit_count: i64,
    pub epoch_lag_ms: Option<u64>,
    pub epoch_last_received_at: Option<String>,
    pub unique_chips: i64,
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
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub new_stream_epoch: i64,
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
// Receiver proxy messages (receiver <-> server, for proxied forwarder config, control, and announcer)
// ---------------------------------------------------------------------------

/// Actions that can be requested on a forwarder device via the proxy channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceControlAction {
    RestartDevice,
    ShutdownDevice,
}

/// Receiver-to-server: request a forwarder's current config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyConfigGetRequest {
    pub request_id: String,
    pub forwarder_id: String,
}

/// Server-to-receiver: forwarder config response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyConfigGetResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub restart_needed: bool,
}

/// Receiver-to-server: update a forwarder config section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyConfigSetRequest {
    pub request_id: String,
    pub forwarder_id: String,
    pub section: String,
    pub payload: serde_json::Value,
}

/// Server-to-receiver: config update result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyConfigSetResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub restart_needed: bool,
}

/// Receiver-to-server: request a forwarder service restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyRestartRequest {
    pub request_id: String,
    pub forwarder_id: String,
}

/// Receiver-to-server: request a forwarder device control action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyDeviceControlRequest {
    pub request_id: String,
    pub forwarder_id: String,
    pub action: DeviceControlAction,
}

/// Server-to-receiver: result of a service restart or device control action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyControlResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Receiver proxy messages: announcer config and streams (receiver <-> server)
// ---------------------------------------------------------------------------

/// Receiver-to-server: get the server's stream list (for announcer stream selection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyStreamsListRequest {
    pub request_id: String,
}

/// Server-to-receiver: stream list response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyStreamsListResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub streams: Vec<StreamInfo>,
}

/// Receiver-to-server: get the current announcer configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyAnnouncerConfigGetRequest {
    pub request_id: String,
}

/// Receiver-to-server: update the announcer configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyAnnouncerConfigSetRequest {
    pub request_id: String,
    pub payload: serde_json::Value,
}

/// Receiver-to-server: reset the announcer runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyAnnouncerResetRequest {
    pub request_id: String,
}

/// Server-to-receiver: announcer config response, used for both get-config
/// and set-config requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyAnnouncerConfigResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Server-to-receiver: announcer reset result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyAnnouncerResetResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Receiver proxy messages for race management
// ---------------------------------------------------------------------------

/// Receiver-to-server: list all races.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyRacesListRequest {
    pub request_id: String,
}

/// Server-to-receiver: races list response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyRacesListResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub races: Vec<RaceInfo>,
}

/// A race entry returned from the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaceInfo {
    pub race_id: String,
    pub name: String,
    /// RFC 3339 timestamp.
    pub created_at: String,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub participant_count: i64,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub chip_count: i64,
}

/// Receiver-to-server: create a new race.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyRaceCreateRequest {
    pub request_id: String,
    pub name: String,
}

/// Server-to-receiver: race creation response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyRaceCreateResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race: Option<RaceInfo>,
}

/// Receiver-to-server: delete a race.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyRaceDeleteRequest {
    pub request_id: String,
    pub race_id: String,
}

/// Server-to-receiver: race deletion response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyRaceDeleteResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Receiver-to-server: get the race assigned to a forwarder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyForwarderRaceGetRequest {
    pub request_id: String,
    pub forwarder_id: String,
}

/// Server-to-receiver: forwarder race assignment response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyForwarderRaceGetResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub forwarder_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race_id: Option<String>,
}

/// Receiver-to-server: set/unset the race assigned to a forwarder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyForwarderRaceSetRequest {
    pub request_id: String,
    pub forwarder_id: String,
    /// `None` to unassign.
    pub race_id: Option<String>,
}

/// Server-to-receiver: forwarder race set response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyForwarderRaceSetResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub forwarder_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race_id: Option<String>,
}

/// Receiver-to-server: get participants for a race.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyParticipantsGetRequest {
    pub request_id: String,
    pub race_id: String,
}

/// Server-to-receiver: participants response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyParticipantsGetResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub participants: Vec<ParticipantInfo>,
    #[serde(default)]
    pub chips_without_participant: Vec<UnmatchedChipInfo>,
}

/// A participant with their associated chip IDs, returned as part of
/// `ReceiverProxyParticipantsGetResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    #[serde(deserialize_with = "deserialize_non_negative_i32")]
    pub bib: i32,
    pub first_name: String,
    pub last_name: String,
    /// Gender code from the PPL file (e.g. "M", "F", or "" if unspecified).
    pub gender: String,
    pub affiliation: Option<String>,
    pub chip_ids: Vec<String>,
}

/// A chip-to-bib mapping where the bib does not correspond to any uploaded
/// participant for the race.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmatchedChipInfo {
    pub chip_id: String,
    #[serde(deserialize_with = "deserialize_non_negative_i32")]
    pub bib: i32,
}

/// The kind of file being uploaded for a race.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadType {
    /// A .ppl participant file.
    Participants,
    /// A bib-chip mapping file.
    Chips,
}

/// Receiver-to-server: upload a file (ppl or bibchip) for a race.
///
/// The `file_data` field carries the entire file as a Base64-encoded string.
/// The client enforces a 10 MB limit; the server enforces ~15 MB encoded
/// (which corresponds to ~10 MB decoded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyFileUploadRequest {
    pub request_id: String,
    pub race_id: String,
    pub upload_type: UploadType,
    /// Base64-encoded file content.
    pub file_data: String,
    pub file_name: String,
}

/// Server-to-receiver: file upload response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyFileUploadResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_negative_i64")]
    pub imported: i64,
}

/// Receiver-to-server: request a reader control action proxied to a forwarder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyReaderControlRequest {
    pub request_id: String,
    pub forwarder_id: String,
    pub reader_ip: String,
    pub action: ReaderControlAction,
}

/// Server-to-receiver: result of a proxied reader control action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyReaderControlResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

/// Server-to-receiver: broadcast reader info update (tunneled via sentinel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverReaderInfoUpdate {
    pub stream_id: Uuid,
    pub reader_ip: String,
    pub state: ReaderConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

/// Server-to-receiver: broadcast download progress (tunneled via sentinel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverReaderDownloadProgress {
    pub stream_id: Uuid,
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
/// status tracking, reader control extensions, receiver-to-server
/// proxy messages for remote forwarder management, announcer configuration,
/// and race management.
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
    ReceiverStreamMetrics(ReceiverStreamMetrics),
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
    ReceiverProxyConfigGetRequest(ReceiverProxyConfigGetRequest),
    ReceiverProxyConfigGetResponse(ReceiverProxyConfigGetResponse),
    ReceiverProxyConfigSetRequest(ReceiverProxyConfigSetRequest),
    ReceiverProxyConfigSetResponse(ReceiverProxyConfigSetResponse),
    ReceiverProxyRestartRequest(ReceiverProxyRestartRequest),
    ReceiverProxyDeviceControlRequest(ReceiverProxyDeviceControlRequest),
    ReceiverProxyControlResponse(ReceiverProxyControlResponse),
    ReceiverProxyStreamsListRequest(ReceiverProxyStreamsListRequest),
    ReceiverProxyStreamsListResponse(ReceiverProxyStreamsListResponse),
    ReceiverProxyAnnouncerConfigGetRequest(ReceiverProxyAnnouncerConfigGetRequest),
    ReceiverProxyAnnouncerConfigSetRequest(ReceiverProxyAnnouncerConfigSetRequest),
    ReceiverProxyAnnouncerResetRequest(ReceiverProxyAnnouncerResetRequest),
    ReceiverProxyAnnouncerConfigResponse(ReceiverProxyAnnouncerConfigResponse),
    ReceiverProxyAnnouncerResetResponse(ReceiverProxyAnnouncerResetResponse),
    ReceiverProxyRacesListRequest(ReceiverProxyRacesListRequest),
    ReceiverProxyRacesListResponse(ReceiverProxyRacesListResponse),
    ReceiverProxyRaceCreateRequest(ReceiverProxyRaceCreateRequest),
    ReceiverProxyRaceCreateResponse(ReceiverProxyRaceCreateResponse),
    ReceiverProxyRaceDeleteRequest(ReceiverProxyRaceDeleteRequest),
    ReceiverProxyRaceDeleteResponse(ReceiverProxyRaceDeleteResponse),
    ReceiverProxyParticipantsGetRequest(ReceiverProxyParticipantsGetRequest),
    ReceiverProxyParticipantsGetResponse(ReceiverProxyParticipantsGetResponse),
    ReceiverProxyFileUploadRequest(ReceiverProxyFileUploadRequest),
    ReceiverProxyFileUploadResponse(ReceiverProxyFileUploadResponse),
    ReceiverProxyForwarderRaceGetRequest(ReceiverProxyForwarderRaceGetRequest),
    ReceiverProxyForwarderRaceGetResponse(ReceiverProxyForwarderRaceGetResponse),
    ReceiverProxyForwarderRaceSetRequest(ReceiverProxyForwarderRaceSetRequest),
    ReceiverProxyForwarderRaceSetResponse(ReceiverProxyForwarderRaceSetResponse),
    ReceiverProxyReaderControlRequest(ReceiverProxyReaderControlRequest),
    ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse),
    ReceiverReaderInfoUpdate(ReceiverReaderInfoUpdate),
    ReceiverReaderDownloadProgress(ReceiverReaderDownloadProgress),
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
    pub stream_epoch: i64,
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
    pub new_stream_epoch: i64,
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
    fn negative_stream_epoch_rejected() {
        let json = r#"{
            "kind": "forwarder_event_batch",
            "session_id": "s1",
            "batch_id": "b1",
            "events": [{
                "forwarder_id": "fwd",
                "reader_ip": "10.0.0.1:10000",
                "stream_epoch": -1,
                "seq": 1,
                "reader_timestamp": "2026-01-01T00:00:00Z",
                "raw_frame": [],
                "read_type": "RAW"
            }]
        }"#;
        let err = serde_json::from_str::<WsMessage>(json).unwrap_err();
        assert!(
            err.to_string().contains("non-negative"),
            "expected non-negative error, got: {err}"
        );
    }

    #[test]
    fn negative_seq_rejected() {
        let json = r#"{
            "kind": "forwarder_event_batch",
            "session_id": "s1",
            "batch_id": "b1",
            "events": [{
                "forwarder_id": "fwd",
                "reader_ip": "10.0.0.1:10000",
                "stream_epoch": 1,
                "seq": -5,
                "reader_timestamp": "2026-01-01T00:00:00Z",
                "raw_frame": [],
                "read_type": "RAW"
            }]
        }"#;
        let err = serde_json::from_str::<WsMessage>(json).unwrap_err();
        assert!(
            err.to_string().contains("non-negative"),
            "expected non-negative error, got: {err}"
        );
    }

    #[test]
    fn receiver_proxy_config_get_request_round_trip() {
        let msg = WsMessage::ReceiverProxyConfigGetRequest(ReceiverProxyConfigGetRequest {
            request_id: "req-1".into(),
            forwarder_id: "fwd-abc".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_config_get_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_config_get_response_round_trip() {
        let msg = WsMessage::ReceiverProxyConfigGetResponse(ReceiverProxyConfigGetResponse {
            request_id: "req-1".into(),
            ok: true,
            error: None,
            config: serde_json::json!({"general": {"display_name": "Start"}}),
            restart_needed: false,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_config_get_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_config_set_request_round_trip() {
        let msg = WsMessage::ReceiverProxyConfigSetRequest(ReceiverProxyConfigSetRequest {
            request_id: "req-2".into(),
            forwarder_id: "fwd-abc".into(),
            section: "general".into(),
            payload: serde_json::json!({"display_name": "Finish"}),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_config_set_response_round_trip() {
        let msg = WsMessage::ReceiverProxyConfigSetResponse(ReceiverProxyConfigSetResponse {
            request_id: "req-2".into(),
            ok: true,
            error: None,
            restart_needed: true,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_restart_request_round_trip() {
        let msg = WsMessage::ReceiverProxyRestartRequest(ReceiverProxyRestartRequest {
            request_id: "req-3".into(),
            forwarder_id: "fwd-abc".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_device_control_request_round_trip() {
        let msg = WsMessage::ReceiverProxyDeviceControlRequest(ReceiverProxyDeviceControlRequest {
            request_id: "req-4".into(),
            forwarder_id: "fwd-abc".into(),
            action: DeviceControlAction::RestartDevice,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_control_response_round_trip() {
        let msg = WsMessage::ReceiverProxyControlResponse(ReceiverProxyControlResponse {
            request_id: "req-3".into(),
            ok: true,
            error: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_control_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_streams_list_request_round_trip() {
        let msg = WsMessage::ReceiverProxyStreamsListRequest(ReceiverProxyStreamsListRequest {
            request_id: "req-s1".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_streams_list_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_streams_list_response_round_trip() {
        let msg = WsMessage::ReceiverProxyStreamsListResponse(ReceiverProxyStreamsListResponse {
            request_id: "req-s1".into(),
            ok: true,
            error: None,
            streams: vec![StreamInfo {
                stream_id: "abc".into(),
                forwarder_id: "fwd-1".into(),
                reader_ip: "10.0.0.1".into(),
                display_alias: None,
                stream_epoch: 1,
                online: true,
                reader_connected: true,
            }],
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_streams_list_response_error_round_trip() {
        let msg = WsMessage::ReceiverProxyStreamsListResponse(ReceiverProxyStreamsListResponse {
            request_id: "req-s2".into(),
            ok: false,
            error: Some("database error: connection refused".into()),
            streams: vec![],
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_announcer_config_get_request_round_trip() {
        let msg = WsMessage::ReceiverProxyAnnouncerConfigGetRequest(
            ReceiverProxyAnnouncerConfigGetRequest {
                request_id: "req-a1".into(),
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_announcer_config_get_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_announcer_config_set_request_round_trip() {
        let msg = WsMessage::ReceiverProxyAnnouncerConfigSetRequest(
            ReceiverProxyAnnouncerConfigSetRequest {
                request_id: "req-a2".into(),
                payload: serde_json::json!({"enabled": true, "selected_stream_ids": [], "max_list_size": 25}),
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_announcer_reset_request_round_trip() {
        let msg =
            WsMessage::ReceiverProxyAnnouncerResetRequest(ReceiverProxyAnnouncerResetRequest {
                request_id: "req-a3".into(),
            });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_announcer_config_response_round_trip() {
        let msg =
            WsMessage::ReceiverProxyAnnouncerConfigResponse(ReceiverProxyAnnouncerConfigResponse {
                request_id: "req-a1".into(),
                ok: true,
                error: None,
                config: serde_json::json!({"enabled": false, "max_list_size": 25}),
            });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_announcer_config_response_error_round_trip() {
        let msg =
            WsMessage::ReceiverProxyAnnouncerConfigResponse(ReceiverProxyAnnouncerConfigResponse {
                request_id: "req-a1".into(),
                ok: false,
                error: Some("database error: connection refused".into()),
                config: serde_json::Value::Null,
            });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_announcer_reset_response_round_trip() {
        let msg =
            WsMessage::ReceiverProxyAnnouncerResetResponse(ReceiverProxyAnnouncerResetResponse {
                request_id: "req-a3".into(),
                ok: true,
                error: None,
            });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_races_list_request_round_trip() {
        let msg = WsMessage::ReceiverProxyRacesListRequest(ReceiverProxyRacesListRequest {
            request_id: "req-10".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_races_list_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_races_list_response_round_trip() {
        let msg = WsMessage::ReceiverProxyRacesListResponse(ReceiverProxyRacesListResponse {
            request_id: "req-10".into(),
            ok: true,
            error: None,
            races: vec![RaceInfo {
                race_id: "race-1".into(),
                name: "5K Fun Run".into(),
                created_at: "2026-03-21T10:00:00Z".into(),
                participant_count: 42,
                chip_count: 50,
            }],
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_races_list_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_race_create_request_round_trip() {
        let msg = WsMessage::ReceiverProxyRaceCreateRequest(ReceiverProxyRaceCreateRequest {
            request_id: "req-11".into(),
            name: "10K Championship".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_race_create_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_race_create_response_round_trip() {
        let msg = WsMessage::ReceiverProxyRaceCreateResponse(ReceiverProxyRaceCreateResponse {
            request_id: "req-11".into(),
            ok: true,
            error: None,
            race: Some(RaceInfo {
                race_id: "race-2".into(),
                name: "10K Championship".into(),
                created_at: "2026-03-21T11:00:00Z".into(),
                participant_count: 0,
                chip_count: 0,
            }),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_race_create_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_race_delete_request_round_trip() {
        let msg = WsMessage::ReceiverProxyRaceDeleteRequest(ReceiverProxyRaceDeleteRequest {
            request_id: "req-12".into(),
            race_id: "race-1".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_race_delete_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_race_delete_response_round_trip() {
        let msg = WsMessage::ReceiverProxyRaceDeleteResponse(ReceiverProxyRaceDeleteResponse {
            request_id: "req-12".into(),
            ok: true,
            error: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_race_delete_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_participants_get_request_round_trip() {
        let msg =
            WsMessage::ReceiverProxyParticipantsGetRequest(ReceiverProxyParticipantsGetRequest {
                request_id: "req-13".into(),
                race_id: "race-1".into(),
            });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_participants_get_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_participants_get_response_round_trip() {
        let msg =
            WsMessage::ReceiverProxyParticipantsGetResponse(ReceiverProxyParticipantsGetResponse {
                request_id: "req-13".into(),
                ok: true,
                error: None,
                participants: vec![ParticipantInfo {
                    bib: 101,
                    first_name: "Alice".into(),
                    last_name: "Smith".into(),
                    gender: "F".into(),
                    affiliation: Some("Fast Runners Club".into()),
                    chip_ids: vec!["CHIP001".into(), "CHIP002".into()],
                }],
                chips_without_participant: vec![UnmatchedChipInfo {
                    chip_id: "CHIP999".into(),
                    bib: 999,
                }],
            });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_participants_get_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_file_upload_request_round_trip() {
        let msg = WsMessage::ReceiverProxyFileUploadRequest(ReceiverProxyFileUploadRequest {
            request_id: "req-14".into(),
            race_id: "race-1".into(),
            upload_type: UploadType::Participants,
            file_data: "SGVsbG8gV29ybGQ=".into(),
            file_name: "participants.csv".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_file_upload_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_file_upload_response_round_trip() {
        let msg = WsMessage::ReceiverProxyFileUploadResponse(ReceiverProxyFileUploadResponse {
            request_id: "req-14".into(),
            ok: true,
            error: None,
            imported: 25,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_file_upload_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_race_create_error_response_round_trip() {
        let msg = WsMessage::ReceiverProxyRaceCreateResponse(ReceiverProxyRaceCreateResponse {
            request_id: "req-err".into(),
            ok: false,
            error: Some("race name must not be empty".into()),
            race: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_races_list_error_response_round_trip() {
        let msg = WsMessage::ReceiverProxyRacesListResponse(ReceiverProxyRacesListResponse {
            request_id: "req-err2".into(),
            ok: false,
            error: Some("database connection failed".into()),
            races: vec![],
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn negative_race_participant_count_rejected() {
        let json = r#"{
            "kind": "receiver_proxy_races_list_response",
            "request_id": "req-neg",
            "ok": true,
            "races": [{
                "race_id": "race-1",
                "name": "5K",
                "created_at": "2026-03-21T10:00:00Z",
                "participant_count": -1,
                "chip_count": 0
            }]
        }"#;
        let err = serde_json::from_str::<WsMessage>(json).unwrap_err();
        assert!(
            err.to_string().contains("non-negative"),
            "expected non-negative error, got: {err}"
        );
    }

    #[test]
    fn negative_file_upload_imported_rejected() {
        let json = r#"{
            "kind": "receiver_proxy_file_upload_response",
            "request_id": "req-neg2",
            "ok": true,
            "imported": -5
        }"#;
        let err = serde_json::from_str::<WsMessage>(json).unwrap_err();
        assert!(
            err.to_string().contains("non-negative"),
            "expected non-negative error, got: {err}"
        );
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

    #[test]
    fn receiver_proxy_forwarder_race_get_request_round_trip() {
        let msg =
            WsMessage::ReceiverProxyForwarderRaceGetRequest(ReceiverProxyForwarderRaceGetRequest {
                request_id: "req-fg1".into(),
                forwarder_id: "fwd-abc".into(),
            });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_forwarder_race_get_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_forwarder_race_get_response_round_trip() {
        let msg = WsMessage::ReceiverProxyForwarderRaceGetResponse(
            ReceiverProxyForwarderRaceGetResponse {
                request_id: "req-fg1".into(),
                ok: true,
                error: None,
                forwarder_id: "fwd-abc".into(),
                race_id: Some("race-1".into()),
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_forwarder_race_get_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_forwarder_race_get_response_unassigned_round_trip() {
        let msg = WsMessage::ReceiverProxyForwarderRaceGetResponse(
            ReceiverProxyForwarderRaceGetResponse {
                request_id: "req-fg2".into(),
                ok: true,
                error: None,
                forwarder_id: "fwd-abc".into(),
                race_id: None,
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_forwarder_race_set_request_round_trip() {
        let msg =
            WsMessage::ReceiverProxyForwarderRaceSetRequest(ReceiverProxyForwarderRaceSetRequest {
                request_id: "req-fs1".into(),
                forwarder_id: "fwd-abc".into(),
                race_id: Some("race-1".into()),
            });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_forwarder_race_set_request\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_forwarder_race_set_request_unassign_round_trip() {
        let msg =
            WsMessage::ReceiverProxyForwarderRaceSetRequest(ReceiverProxyForwarderRaceSetRequest {
                request_id: "req-fs2".into(),
                forwarder_id: "fwd-abc".into(),
                race_id: None,
            });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_forwarder_race_set_response_round_trip() {
        let msg = WsMessage::ReceiverProxyForwarderRaceSetResponse(
            ReceiverProxyForwarderRaceSetResponse {
                request_id: "req-fs1".into(),
                ok: true,
                error: None,
                forwarder_id: "fwd-abc".into(),
                race_id: Some("race-1".into()),
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"receiver_proxy_forwarder_race_set_response\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn receiver_proxy_forwarder_race_set_response_error_round_trip() {
        let msg = WsMessage::ReceiverProxyForwarderRaceSetResponse(
            ReceiverProxyForwarderRaceSetResponse {
                request_id: "req-fs3".into(),
                ok: false,
                error: Some("race not found".into()),
                forwarder_id: "fwd-abc".into(),
                race_id: None,
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn upload_type_chips_round_trip() {
        let msg = WsMessage::ReceiverProxyFileUploadRequest(ReceiverProxyFileUploadRequest {
            request_id: "req-chips".into(),
            race_id: "race-1".into(),
            upload_type: UploadType::Chips,
            file_data: "YmlkLGNoaXAK".into(),
            file_name: "chips.bibchip".into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"upload_type\":\"chips\""));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn negative_participant_bib_rejected() {
        let json = r#"{
            "kind": "receiver_proxy_participants_get_response",
            "request_id": "req-neg-bib",
            "ok": true,
            "participants": [{
                "bib": -1,
                "first_name": "Alice",
                "last_name": "Smith",
                "gender": "F",
                "affiliation": null,
                "chip_ids": []
            }],
            "chips_without_participant": []
        }"#;
        let err = serde_json::from_str::<WsMessage>(json).unwrap_err();
        assert!(
            err.to_string().contains("non-negative"),
            "expected non-negative error, got: {err}"
        );
    }

    #[test]
    fn negative_unmatched_chip_bib_rejected() {
        let json = r#"{
            "kind": "receiver_proxy_participants_get_response",
            "request_id": "req-neg-chip-bib",
            "ok": true,
            "participants": [],
            "chips_without_participant": [{
                "chip_id": "CHIP001",
                "bib": -5
            }]
        }"#;
        let err = serde_json::from_str::<WsMessage>(json).unwrap_err();
        assert!(
            err.to_string().contains("non-negative"),
            "expected non-negative error, got: {err}"
        );
    }

    #[test]
    fn receiver_proxy_reader_control_request_round_trip() {
        let msg = WsMessage::ReceiverProxyReaderControlRequest(ReceiverProxyReaderControlRequest {
            request_id: "req-1".to_owned(),
            forwarder_id: "fwd-1".to_owned(),
            reader_ip: "192.168.1.100:10000".to_owned(),
            action: ReaderControlAction::SyncClock,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert!(json.contains("\"kind\":\"receiver_proxy_reader_control_request\""));
    }

    #[test]
    fn receiver_proxy_reader_control_response_round_trip() {
        let msg =
            WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: "req-1".to_owned(),
                ok: true,
                error: None,
                reader_info: None,
            });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert!(json.contains("\"kind\":\"receiver_proxy_reader_control_response\""));
    }

    #[test]
    fn receiver_reader_info_update_round_trip() {
        let msg = WsMessage::ReceiverReaderInfoUpdate(ReceiverReaderInfoUpdate {
            stream_id: Uuid::nil(),
            reader_ip: "192.168.1.100:10000".to_owned(),
            state: ReaderConnectionState::Connected,
            reader_info: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert!(json.contains("\"kind\":\"receiver_reader_info_update\""));
    }

    #[test]
    fn receiver_reader_download_progress_round_trip() {
        let msg = WsMessage::ReceiverReaderDownloadProgress(ReceiverReaderDownloadProgress {
            stream_id: Uuid::nil(),
            reader_ip: "192.168.1.100:10000".to_owned(),
            state: DownloadState::Downloading,
            reads_received: 42,
            progress: 100,
            total: 1000,
            error: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert!(json.contains("\"kind\":\"receiver_reader_download_progress\""));
    }
}

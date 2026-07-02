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

/// A durable read event used inside the forwarder/receiver data path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadEvent {
    pub forwarder_id: String,
    pub reader_ip: String,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub stream_epoch: i64,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub seq: i64,
    pub reader_timestamp: String,
    pub raw_frame: Vec<u8>,
    pub read_type: String,
}

/// A resume cursor for a single stream/epoch pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCursor {
    pub forwarder_id: String,
    pub reader_ip: String,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub stream_epoch: i64,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub last_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamRef {
    pub forwarder_id: String,
    pub reader_ip: String,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarliestEpochOverride {
    pub forwarder_id: String,
    pub reader_ip: String,
    #[serde(deserialize_with = "deserialize_non_negative_i64")]
    pub earliest_epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReceiverMode {
    Live {
        streams: Vec<StreamRef>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        earliest_epochs: Vec<EarliestEpochOverride>,
    },
    Race {
        race_id: String,
    },
    TargetedReplay {
        targets: Vec<ReplayTarget>,
    },
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadMode {
    #[serde(rename = "raw")]
    Raw,
    #[serde(rename = "event")]
    Event,
    #[serde(rename = "fsls")]
    FirstLastSeen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderConnectionState {
    Connected,
    Connecting,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Downloading,
    Complete,
    Error,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareInfo {
    pub fw_version: Option<String>,
    pub hw_code: Option<String>,
    pub reader_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config3Info {
    pub mode: ReadMode,
    pub timeout: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockInfo {
    pub reader_clock: String,
    pub drift_ms: i64,
}

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
    SetEpochName { name: Option<String> },
    AdvanceEpoch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderControlRequest {
    pub request_id: String,
    pub reader_ip: String,
    pub action: ReaderControlAction,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderStatusUpdate {
    pub reader_ip: String,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderInfoUpdate {
    pub reader_ip: String,
    pub state: ReaderConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadProgressUpdate {
    pub reader_ip: String,
    pub state: DownloadState,
    pub stored_reads: Option<u32>,
    pub downloaded_reads: u32,
    pub progress: u64,
    pub total: Option<u64>,
    pub last_read_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderUpsStatus {
    pub forwarder_id: String,
    pub available: bool,
    pub status: Option<UpsStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsStatus {
    pub battery_percent: u8,
    pub battery_voltage_mv: u16,
    pub charging: bool,
    pub power_plugged: bool,
    pub temperature_cdeg: i16,
    pub sampled_at: i64,
}

impl UpsStatus {
    pub fn same_readings(&self, other: &Self) -> bool {
        self.battery_percent == other.battery_percent
            && self.battery_voltage_mv == other.battery_voltage_mv
            && self.charging == other.charging
            && self.power_plugged == other.power_plugged
            && self.temperature_cdeg == other.temperature_cdeg
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderStatusChanged {
    pub stream_id: Uuid,
    pub reader_ip: String,
    pub connected: bool,
}

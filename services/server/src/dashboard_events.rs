use serde::{Serialize, Serializer};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum OptionalStringPatch {
    Set(String),
    Clear,
}

impl Serialize for OptionalStringPatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Set(value) => serializer.serialize_str(value),
            Self::Clear => serializer.serialize_none(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardEvent {
    Resync,
    StreamCreated {
        stream_id: Uuid,
        forwarder_id: String,
        reader_ip: String,
        display_alias: Option<String>,
        forwarder_display_name: Option<String>,
        online: bool,
        reader_connected: bool,
        stream_epoch: i64,
        created_at: String,
    },
    StreamUpdated {
        stream_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        online: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reader_connected: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stream_epoch: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        display_alias: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        forwarder_display_name: Option<OptionalStringPatch>,
    },
    MetricsUpdated {
        stream_id: Uuid,
        raw_count: i64,
        dedup_count: i64,
        retransmit_count: i64,
        lag_ms: Option<u64>,
        epoch_raw_count: i64,
        epoch_dedup_count: i64,
        epoch_retransmit_count: i64,
        epoch_lag_ms: Option<u64>,
        epoch_last_received_at: Option<String>,
        unique_chips: i64,
        last_tag_id: Option<String>,
        last_reader_timestamp: Option<String>,
    },
    ForwarderMetricsUpdated {
        forwarder_id: String,
        unique_chips: i64,
        total_reads: i64,
        last_read_at: Option<String>,
    },
    ForwarderRaceAssigned {
        forwarder_id: String,
        race_id: Option<Uuid>,
    },
    ReaderInfoUpdated {
        forwarder_id: String,
        reader_ip: String,
        state: rt_protocol::ReaderConnectionState,
        #[serde(skip_serializing_if = "Option::is_none")]
        reader_info: Option<rt_protocol::ReaderInfo>,
    },
    ReaderDownloadProgress {
        forwarder_id: String,
        #[serde(flatten)]
        progress: rt_protocol::ReaderDownloadProgress,
    },
    LogEntry {
        entry: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_download_progress_serializes_with_flattened_fields() {
        let event = DashboardEvent::ReaderDownloadProgress {
            forwarder_id: "fwd-1".to_owned(),
            progress: rt_protocol::ReaderDownloadProgress {
                reader_ip: "10.0.0.1:10000".to_owned(),
                state: rt_protocol::DownloadState::Downloading,
                reads_received: 42,
                progress: 50,
                total: 100,
                error: None,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "reader_download_progress");
        assert_eq!(json["forwarder_id"], "fwd-1");
        assert_eq!(json["reader_ip"], "10.0.0.1:10000");
        assert_eq!(json["reads_received"], 42);
        assert_eq!(json["progress"], 50);
        assert_eq!(json["total"], 100);
    }

    #[test]
    fn forwarder_metrics_updated_serializes_with_type_tag() {
        let event = DashboardEvent::ForwarderMetricsUpdated {
            forwarder_id: "fwd-1".to_owned(),
            unique_chips: 4,
            total_reads: 15,
            last_read_at: Some("2026-03-21T12:34:56.000Z".to_owned()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "forwarder_metrics_updated");
        assert_eq!(json["forwarder_id"], "fwd-1");
        assert_eq!(json["unique_chips"], 4);
        assert_eq!(json["total_reads"], 15);
        assert_eq!(json["last_read_at"], "2026-03-21T12:34:56.000Z");
    }
}

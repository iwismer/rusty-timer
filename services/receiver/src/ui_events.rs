use crate::control_api::{ConnectionState, StreamEntry};
use serde::Serialize;

/// Volatile per-reader counters from a forwarder `ReaderStatus` frame.
///
/// Delivered as a targeted UI event so read-count refreshes patch the
/// Connections tab in place instead of triggering a full connections reload
/// (`ConnectionsChanged` fires only on structural changes).
#[derive(Clone, Debug, Serialize)]
pub struct ForwarderReaderCounts {
    pub forwarder_id: String,
    pub stream_id: String,
    pub reads_session: u64,
    pub reads_total: i64,
    pub reads_epoch: Option<i64>,
    pub last_read_unix_ms: Option<i64>,
    pub last_seen_secs: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LastRead {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub chip_id: String,
    pub timestamp: String,
    pub bib: Option<String>,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub division: Option<String>,
}

/// Per-stream metrics for the UI. Identified by the composite
/// (`forwarder_endpoint_id`, wire `stream_id`); `forwarder_id`/`reader_ip`
/// are display metadata only and may collide across forwarders.
#[derive(Clone, Debug, Serialize)]
pub struct StreamMetricsPayload {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub forwarder_id: String,
    pub reader_ip: String,
    pub raw_count: i64,
    pub dedup_count: i64,
    pub retransmit_count: i64,
    pub lag_ms: Option<u64>,
    pub epoch_raw_count: i64,
    pub epoch_dedup_count: i64,
    pub epoch_retransmit_count: i64,
    pub unique_chips: i64,
    pub epoch_last_received_at: Option<String>,
    pub epoch_lag_ms: Option<u64>,
}

/// Format a unix-millisecond timestamp as RFC 3339, or `None` when out of
/// range.
pub fn unix_ms_to_rfc3339(unix_ms: i64) -> Option<String> {
    use chrono::TimeZone as _;
    chrono::Utc
        .timestamp_millis_opt(unix_ms)
        .single()
        .map(|dt| dt.to_rfc3339())
}

/// Extract chip ID from IPICO raw frame bytes.
/// The raw frame is ASCII text; characters 4..16 are the chip identifier
/// (e.g. "000000012345"), matching the server's `tag_id` format.
/// Only extracts from frames with a valid IPICO prefix ("aa" at bytes 0..2).
pub fn chip_id_from_raw_frame(raw_frame: &[u8]) -> String {
    if raw_frame.len() < 16 {
        return "unknown".to_owned();
    }
    // Validate IPICO frame type prefix: first two bytes should be "aa"
    if raw_frame.get(..2) != Some(b"aa") {
        return "unknown".to_owned();
    }
    std::str::from_utf8(&raw_frame[4..16])
        .unwrap_or("unknown")
        .to_owned()
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReceiverUiEvent {
    Resync,
    StatusChanged {
        connection_state: ConnectionState,
        streams_count: usize,
        receiver_id: String,
    },
    ConnectionsChanged,
    StreamsSnapshot {
        streams: Vec<StreamEntry>,
        degraded: bool,
        upstream_error: Option<String>,
    },
    LogEntry {
        entry: String,
    },
    ForwarderReaderCountsUpdated(ForwarderReaderCounts),
    ModeChanged {
        mode: rt_domain::ReceiverMode,
    },
    /// Coalesced per-stream UI updates: one event carries every stream that
    /// changed since the last emitter tick (4–10 Hz). Full snapshots are
    /// sent only on UI (re)connect/resync and control-plane changes.
    StreamDeltas {
        updates: Vec<StreamDelta>,
    },
    ForwarderUpsUpdated {
        forwarder_id: String,
        available: bool,
        status: Option<rt_domain::UpsStatus>,
    },
}

/// One stream's coalesced UI state for [`ReceiverUiEvent::StreamDeltas`].
#[derive(Clone, Debug, Serialize)]
pub struct StreamDelta {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub forwarder_id: String,
    pub reader_ip: String,
    pub reads_total: u64,
    pub reads_epoch: u64,
    pub metrics: StreamMetricsPayload,
    pub last_read: Option<LastRead>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_changed_serializes_with_type_tag() {
        let event = ReceiverUiEvent::StatusChanged {
            connection_state: ConnectionState::Connected,
            streams_count: 3,
            receiver_id: "recv-abc".to_owned(),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "status_changed");
        assert_eq!(json["connection_state"], "connected");
        assert_eq!(json["streams_count"], 3);
        assert_eq!(json["receiver_id"], "recv-abc");
    }

    #[test]
    fn resync_serializes_with_type_tag() {
        let event = ReceiverUiEvent::Resync;
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "resync");
    }

    #[test]
    fn connections_changed_serializes_with_type_tag() {
        let event = ReceiverUiEvent::ConnectionsChanged;
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "connections_changed");
    }

    #[test]
    fn log_entry_serializes_with_type_tag() {
        let event = ReceiverUiEvent::LogEntry {
            entry: "test log".to_owned(),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "log_entry");
        assert_eq!(json["entry"], "test log");
    }

    #[test]
    fn streams_snapshot_serializes_with_type_tag() {
        let event = ReceiverUiEvent::StreamsSnapshot {
            streams: vec![],
            degraded: false,
            upstream_error: None,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "streams_snapshot");
        assert_eq!(json["streams"].as_array().unwrap().len(), 0);
        assert_eq!(json["degraded"], false);
    }

    #[test]
    fn mode_changed_serializes_with_type_tag() {
        let event = ReceiverUiEvent::ModeChanged {
            mode: rt_domain::ReceiverMode::Race {
                race_id: "race-1".to_owned(),
            },
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "mode_changed");
        assert_eq!(json["mode"]["mode"], "race");
        assert_eq!(json["mode"]["race_id"], "race-1");
    }

    #[test]
    fn forwarder_reader_counts_updated_serializes_with_type_tag() {
        let event = ReceiverUiEvent::ForwarderReaderCountsUpdated(ForwarderReaderCounts {
            forwarder_id: "fwd-01".to_owned(),
            stream_id: "192.168.1.10:10000".to_owned(),
            reads_session: 12,
            reads_total: 345,
            reads_epoch: Some(40),
            last_read_unix_ms: Some(1_711_929_600_000),
            last_seen_secs: Some(3),
        });
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "forwarder_reader_counts_updated");
        assert_eq!(json["forwarder_id"], "fwd-01");
        assert_eq!(json["stream_id"], "192.168.1.10:10000");
        assert_eq!(json["reads_session"], 12);
        assert_eq!(json["reads_total"], 345);
        assert_eq!(json["last_read_unix_ms"], 1_711_929_600_000_i64);
        assert_eq!(json["last_seen_secs"], 3);
    }

    #[test]
    fn stream_delta_last_read_omits_division_when_absent() {
        let last_read = LastRead {
            forwarder_id: "fwd-01".to_owned(),
            reader_ip: "192.168.1.10".to_owned(),
            chip_id: "000000012345".to_owned(),
            timestamp: "14:23:05.123".to_owned(),
            bib: None,
            name: None,
            division: None,
        };
        let json: serde_json::Value = serde_json::to_value(&last_read).unwrap();
        assert_eq!(json["forwarder_id"], "fwd-01");
        assert_eq!(json["reader_ip"], "192.168.1.10");
        assert_eq!(json["timestamp"], "14:23:05.123");
        assert!(json["bib"].is_null());
        assert!(json["name"].is_null());
        // Division is omitted from the payload when absent.
        assert!(json.get("division").is_none());
    }

    #[test]
    fn stream_delta_last_read_carries_division_when_present() {
        let last_read = LastRead {
            forwarder_id: "fwd-01".to_owned(),
            reader_ip: "192.168.1.10".to_owned(),
            chip_id: "000000012345".to_owned(),
            timestamp: "14:23:05.123".to_owned(),
            bib: Some("42".to_owned()),
            name: Some("Ada Lovelace".to_owned()),
            division: Some("5k".to_owned()),
        };
        let json: serde_json::Value = serde_json::to_value(&last_read).unwrap();
        assert_eq!(json["bib"], "42");
        assert_eq!(json["name"], "Ada Lovelace");
        assert_eq!(json["division"], "5k");
    }

    #[test]
    fn stream_deltas_serialize_wire_composite_identity() {
        let metrics = StreamMetricsPayload {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "wire-stream".to_owned(),
            forwarder_id: "display-fwd".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            raw_count: 3,
            dedup_count: 3,
            retransmit_count: 0,
            lag_ms: None,
            epoch_raw_count: 3,
            epoch_dedup_count: 3,
            epoch_retransmit_count: 0,
            unique_chips: 1,
            epoch_last_received_at: None,
            epoch_lag_ms: None,
        };
        let event = ReceiverUiEvent::StreamDeltas {
            updates: vec![StreamDelta {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: "wire-stream".to_owned(),
                forwarder_id: "display-fwd".to_owned(),
                reader_ip: "10.0.0.1:10000".to_owned(),
                reads_total: 3,
                reads_epoch: 2,
                metrics,
                last_read: None,
            }],
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "stream_deltas");
        assert_eq!(json["updates"][0]["forwarder_endpoint_id"], "endpoint-1");
        assert_eq!(json["updates"][0]["stream_id"], "wire-stream");
        assert_eq!(json["updates"][0]["forwarder_id"], "display-fwd");
        assert_eq!(json["updates"][0]["reader_ip"], "10.0.0.1:10000");
        assert_eq!(
            json["updates"][0]["metrics"]["forwarder_endpoint_id"],
            "endpoint-1"
        );
        assert_eq!(json["updates"][0]["metrics"]["stream_id"], "wire-stream");
    }

    #[test]
    fn forwarder_ups_updated_serializes_with_type_tag() {
        let event = ReceiverUiEvent::ForwarderUpsUpdated {
            forwarder_id: "fwd-01".to_owned(),
            available: true,
            status: Some(rt_domain::UpsStatus {
                battery_percent: 73,
                battery_voltage_mv: 3870,
                charging: true,
                power_plugged: true,
                temperature_cdeg: 4250,
                sampled_at: 1711929600000,
            }),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "forwarder_ups_updated");
        assert_eq!(json["forwarder_id"], "fwd-01");
        assert_eq!(json["available"], true);
        assert_eq!(json["status"]["battery_percent"], 73);
    }

    #[test]
    fn chip_id_from_raw_frame_extracts_text_chars_4_through_15() {
        // Simulates an IPICO ASCII frame: "aa40000000012345..."
        let frame = b"aa40000000012345extra_stuff_here";
        assert_eq!(chip_id_from_raw_frame(frame), "000000012345");
    }

    #[test]
    fn chip_id_from_raw_frame_short_returns_unknown() {
        assert_eq!(chip_id_from_raw_frame(&[0u8; 10]), "unknown");
    }

    #[test]
    fn chip_id_from_raw_frame_non_ipico_prefix_returns_unknown() {
        // Frame is long enough but doesn't start with "aa"
        let frame = b"bb40000000012345extra_stuff_here";
        assert_eq!(chip_id_from_raw_frame(frame), "unknown");
    }

    #[test]
    fn chip_id_from_raw_frame_non_utf8_returns_unknown() {
        let mut frame = vec![0u8; 20];
        // Put non-UTF-8 bytes in positions 4..16
        frame[4] = 0xFF;
        frame[5] = 0xFE;
        assert_eq!(chip_id_from_raw_frame(&frame), "unknown");
    }
}

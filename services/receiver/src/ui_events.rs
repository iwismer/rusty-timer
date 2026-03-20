use crate::control_api::{ConnectionState, StreamEntry};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct StreamCountUpdate {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub reads_total: u64,
    pub reads_epoch: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LastRead {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub chip_id: String,
    pub timestamp: String,
    pub bib: Option<String>,
    pub name: Option<String>,
}

/// Extract chip ID from IPICO raw frame bytes.
/// Bytes 4..16 are the chip identifier, formatted as colon-separated hex pairs.
pub fn chip_id_from_raw_frame(raw_frame: &[u8]) -> String {
    if raw_frame.len() < 16 {
        return "unknown".to_owned();
    }
    raw_frame[4..16]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
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
    StreamsSnapshot {
        streams: Vec<StreamEntry>,
        degraded: bool,
        upstream_error: Option<String>,
    },
    LogEntry {
        entry: String,
    },
    StreamCountsUpdated {
        updates: Vec<StreamCountUpdate>,
    },
    ModeChanged {
        mode: rt_protocol::ReceiverMode,
    },
    LastRead(LastRead),
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
    fn stream_counts_updated_serializes_with_type_tag() {
        let event = ReceiverUiEvent::StreamCountsUpdated {
            updates: vec![StreamCountUpdate {
                forwarder_id: "f1".to_owned(),
                reader_ip: "10.0.0.1".to_owned(),
                reads_total: 42,
                reads_epoch: 7,
            }],
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "stream_counts_updated");
        assert_eq!(json["updates"][0]["forwarder_id"], "f1");
        assert_eq!(json["updates"][0]["reader_ip"], "10.0.0.1");
        assert_eq!(json["updates"][0]["reads_total"], 42);
        assert_eq!(json["updates"][0]["reads_epoch"], 7);
    }

    #[test]
    fn mode_changed_serializes_with_type_tag() {
        let event = ReceiverUiEvent::ModeChanged {
            mode: rt_protocol::ReceiverMode::Race {
                race_id: "race-1".to_owned(),
            },
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "mode_changed");
        assert_eq!(json["mode"]["mode"], "race");
        assert_eq!(json["mode"]["race_id"], "race-1");
    }

    #[test]
    fn last_read_serializes_with_type_tag() {
        let event = ReceiverUiEvent::LastRead(LastRead {
            forwarder_id: "fwd-01".to_owned(),
            reader_ip: "192.168.1.10".to_owned(),
            chip_id: "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55".to_owned(),
            timestamp: "14:23:05.123".to_owned(),
            bib: None,
            name: None,
        });
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "last_read");
        assert_eq!(json["forwarder_id"], "fwd-01");
        assert_eq!(json["reader_ip"], "192.168.1.10");
        assert_eq!(json["timestamp"], "14:23:05.123");
        assert!(json["bib"].is_null());
        assert!(json["name"].is_null());
    }

    #[test]
    fn chip_id_from_raw_frame_extracts_bytes_4_through_15() {
        let mut frame = vec![0u8; 20];
        frame[4] = 0xAA;
        frame[5] = 0xBB;
        frame[6] = 0xCC;
        frame[7] = 0xDD;
        frame[8] = 0x01;
        frame[9] = 0x02;
        frame[10] = 0x03;
        frame[11] = 0x04;
        frame[12] = 0x05;
        frame[13] = 0x06;
        frame[14] = 0x07;
        frame[15] = 0x08;
        assert_eq!(
            chip_id_from_raw_frame(&frame),
            "AA:BB:CC:DD:01:02:03:04:05:06:07:08"
        );
    }

    #[test]
    fn chip_id_from_raw_frame_short_returns_unknown() {
        assert_eq!(chip_id_from_raw_frame(&[0u8; 10]), "unknown");
    }
}

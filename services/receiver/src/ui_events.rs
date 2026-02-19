use crate::control_api::{ConnectionState, StreamEntry};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReceiverUiEvent {
    StatusChanged {
        connection_state: ConnectionState,
        streams_count: usize,
    },
    StreamsSnapshot {
        streams: Vec<StreamEntry>,
        degraded: bool,
        upstream_error: Option<String>,
    },
    LogEntry {
        entry: String,
    },
    UpdateAvailable {
        version: String,
        current_version: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_changed_serializes_with_type_tag() {
        let event = ReceiverUiEvent::StatusChanged {
            connection_state: ConnectionState::Connected,
            streams_count: 3,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "status_changed");
        assert_eq!(json["connection_state"], "connected");
        assert_eq!(json["streams_count"], 3);
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
}

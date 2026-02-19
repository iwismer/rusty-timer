use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForwarderUiEvent {
    StatusChanged {
        ready: bool,
        uplink_connected: bool,
        restart_needed: bool,
    },
    ReaderUpdated {
        ip: String,
        state: String,
        reads_session: u64,
        reads_total: i64,
        last_seen_secs: Option<u64>,
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
        let event = ForwarderUiEvent::StatusChanged {
            ready: true,
            uplink_connected: false,
            restart_needed: false,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "status_changed");
        assert_eq!(json["ready"], true);
    }

    #[test]
    fn reader_updated_serializes_with_type_tag() {
        let event = ForwarderUiEvent::ReaderUpdated {
            ip: "192.168.1.10".to_owned(),
            state: "connected".to_owned(),
            reads_session: 42,
            reads_total: 100,
            last_seen_secs: Some(3),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "reader_updated");
        assert_eq!(json["ip"], "192.168.1.10");
    }

    #[test]
    fn log_entry_serializes_with_type_tag() {
        let event = ForwarderUiEvent::LogEntry {
            entry: "test log".to_owned(),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "log_entry");
    }
}

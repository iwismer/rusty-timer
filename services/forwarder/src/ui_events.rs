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
        local_port: u16,
    },
    LogEntry {
        entry: String,
    },
    UpdateAvailable {
        version: String,
        current_version: String,
    },
}

/// Send a timestamped log entry to the UI broadcast channel.
///
/// Messages appear in the browser log viewer via the SSE `log_entry` event.
/// Silently drops the message if no subscribers are listening.
pub fn ui_log(tx: &tokio::sync::broadcast::Sender<ForwarderUiEvent>, msg: impl std::fmt::Display) {
    let ts = chrono::Utc::now().format("%H:%M:%S");
    let _ = tx.send(ForwarderUiEvent::LogEntry {
        entry: format!("{} {}", ts, msg),
    });
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
            local_port: 10010,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "reader_updated");
        assert_eq!(json["ip"], "192.168.1.10");
        assert_eq!(json["local_port"], 10010);
    }

    #[test]
    fn log_entry_serializes_with_type_tag() {
        let event = ForwarderUiEvent::LogEntry {
            entry: "test log".to_owned(),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "log_entry");
    }

    #[test]
    fn ui_log_sends_timestamped_entry() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        ui_log(&tx, "hello world");
        let event = rx.try_recv().unwrap();
        match event {
            ForwarderUiEvent::LogEntry { entry } => {
                // Should match "HH:MM:SS hello world"
                assert!(entry.ends_with(" hello world"), "unexpected entry: {entry}");
                // Timestamp part should be 8 chars "HH:MM:SS"
                assert_eq!(&entry[2..3], ":", "expected colon in timestamp: {entry}");
                assert_eq!(&entry[5..6], ":", "expected colon in timestamp: {entry}");
            }
            other => panic!("expected LogEntry, got: {:?}", other),
        }
    }
}

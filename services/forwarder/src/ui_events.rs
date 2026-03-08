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
        last_read_secs: Option<u64>,
        local_port: u16,
        current_epoch_name: Option<String>,
    },
    LogEntry {
        entry: String,
    },
    UpdateStatusChanged {
        status: rt_updater::UpdateStatus,
    },
    ReaderInfoUpdated {
        ip: String,
        #[serde(flatten)]
        info: crate::reader_control::ReaderInfo,
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
            last_read_secs: Some(3),
            local_port: 10010,
            current_epoch_name: Some("Race Day".to_owned()),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "reader_updated");
        assert_eq!(json["ip"], "192.168.1.10");
        assert_eq!(json["local_port"], 10010);
        assert_eq!(json["current_epoch_name"], "Race Day");
    }

    #[test]
    fn reader_info_updated_serializes_with_type_tag() {
        let event = ForwarderUiEvent::ReaderInfoUpdated {
            ip: "192.168.0.155".to_owned(),
            info: crate::reader_control::ReaderInfo {
                banner: Some("ARM9 Controller".to_owned()),
                hardware: Some(crate::reader_control::HardwareInfo {
                    fw_version: "15.8".to_owned(),
                    hw_code: 0,
                    reader_id: 0,
                    config3: 0,
                }),
                ..Default::default()
            },
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "reader_info_updated");
        assert_eq!(json["ip"], "192.168.0.155");
        assert_eq!(json["hardware"]["fw_version"], "15.8");
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

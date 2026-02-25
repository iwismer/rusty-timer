/// Contract golden tests: load each JSON example file, deserialize to Rust types,
/// serialize back to JSON, and verify round-trip fidelity.
///
/// These tests are intentionally written BEFORE the implementation so they fail
/// first and pass only after all types and example files exist.
use rt_protocol::WsMessage;

/// Helper: load a JSON example file and assert round-trip.
///
/// Returns the deserialized value so callers can inspect fields.
fn round_trip(relative_path: &str) -> WsMessage {
    // Example files live next to the workspace root, not the crate root.
    // Cargo sets CARGO_MANIFEST_DIR to the crate directory; we walk up two
    // levels to reach the workspace root.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = std::path::Path::new(manifest_dir)
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root");

    let file_path = workspace_root.join(relative_path);
    let json_text = std::fs::read_to_string(&file_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", file_path.display(), e));

    let value: WsMessage = serde_json::from_str(&json_text)
        .unwrap_or_else(|e| panic!("Failed to deserialize {}: {}", file_path.display(), e));

    // Serialize back to JSON and re-parse to confirm round-trip.
    let serialized = serde_json::to_string(&value)
        .unwrap_or_else(|e| panic!("Failed to serialize {}: {}", file_path.display(), e));

    let roundtripped: WsMessage = serde_json::from_str(&serialized).unwrap_or_else(|e| {
        panic!(
            "Failed to re-deserialize after serialize for {}: {}\nJSON: {}",
            file_path.display(),
            e,
            serialized
        )
    });

    // Basic structural equality check via re-serializing both.
    let original_json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    let roundtrip_json: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(
        original_json,
        roundtrip_json,
        "Round-trip mismatch for {}",
        file_path.display()
    );

    let _ = roundtripped; // suppress unused warning
    value
}

#[test]
fn forwarder_hello_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/forwarder_hello.json");
    match msg {
        WsMessage::ForwarderHello(inner) => {
            assert!(
                !inner.forwarder_id.is_empty(),
                "forwarder_id must be non-empty"
            );
            assert!(!inner.reader_ips.is_empty(), "reader_ips must be non-empty");
            // No session_id on hello.
        }
        other => panic!("Expected ForwarderHello, got {:?}", other),
    }
}

#[test]
fn forwarder_event_batch_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/forwarder_event_batch.json");
    match msg {
        WsMessage::ForwarderEventBatch(inner) => {
            assert!(!inner.session_id.is_empty());
            assert!(!inner.batch_id.is_empty());
            assert!(!inner.events.is_empty());
        }
        other => panic!("Expected ForwarderEventBatch, got {:?}", other),
    }
}

#[test]
fn read_event_raw_frame_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/forwarder_event_batch.json");
    match msg {
        WsMessage::ForwarderEventBatch(inner) => {
            assert!(!inner.events.is_empty());
            assert!(
                !inner.events[0].raw_frame.is_empty(),
                "raw_frame bytes should round-trip"
            );
        }
        other => panic!("Expected ForwarderEventBatch, got {:?}", other),
    }
}

#[test]
fn forwarder_ack_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/forwarder_ack.json");
    match msg {
        WsMessage::ForwarderAck(inner) => {
            assert!(!inner.session_id.is_empty());
            assert!(!inner.entries.is_empty());
        }
        other => panic!("Expected ForwarderAck, got {:?}", other),
    }
}

#[test]
fn receiver_hello_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/receiver_hello.json");
    match msg {
        WsMessage::ReceiverHello(inner) => {
            assert!(
                !inner.receiver_id.is_empty(),
                "receiver_id must be non-empty"
            );
            // No session_id on hello.
        }
        other => panic!("Expected ReceiverHello, got {:?}", other),
    }
}

#[test]
fn receiver_subscribe_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/receiver_subscribe.json");
    match msg {
        WsMessage::ReceiverSubscribe(inner) => {
            assert!(!inner.session_id.is_empty());
            assert!(!inner.streams.is_empty());
        }
        other => panic!("Expected ReceiverSubscribe, got {:?}", other),
    }
}

#[test]
fn receiver_event_batch_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/receiver_event_batch.json");
    match msg {
        WsMessage::ReceiverEventBatch(inner) => {
            assert!(!inner.session_id.is_empty());
            assert!(!inner.events.is_empty());
        }
        other => panic!("Expected ReceiverEventBatch, got {:?}", other),
    }
}

#[test]
fn receiver_ack_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/receiver_ack.json");
    match msg {
        WsMessage::ReceiverAck(inner) => {
            assert!(!inner.session_id.is_empty());
            assert!(!inner.entries.is_empty());
        }
        other => panic!("Expected ReceiverAck, got {:?}", other),
    }
}

#[test]
fn receiver_hello_v11_race_current_round_trip() {
    let msg = round_trip("contracts/ws/v1.1/examples/receiver_hello_v11_race_current.json");
    match msg {
        WsMessage::ReceiverHelloV11(inner) => {
            assert!(!inner.receiver_id.is_empty());
            assert!(matches!(
                inner.selection,
                rt_protocol::ReceiverSelection::Race { .. }
            ));
        }
        other => panic!("Expected ReceiverHelloV11, got {:?}", other),
    }
}

#[test]
fn receiver_set_selection_round_trip() {
    let msg = round_trip("contracts/ws/v1.1/examples/receiver_set_selection.json");
    match msg {
        WsMessage::ReceiverSetSelection(inner) => {
            assert!(matches!(
                inner.selection,
                rt_protocol::ReceiverSelection::Race { .. }
            ));
        }
        other => panic!("Expected ReceiverSetSelection, got {:?}", other),
    }
}

#[test]
fn receiver_selection_applied_round_trip() {
    let msg = round_trip("contracts/ws/v1.1/examples/receiver_selection_applied.json");
    match msg {
        WsMessage::ReceiverSelectionApplied(inner) => {
            assert!(inner.resolved_target_count > 0);
        }
        other => panic!("Expected ReceiverSelectionApplied, got {:?}", other),
    }
}

#[test]
fn heartbeat_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/heartbeat.json");
    match msg {
        WsMessage::Heartbeat(inner) => {
            assert!(!inner.session_id.is_empty());
            assert!(!inner.device_id.is_empty());
        }
        other => panic!("Expected Heartbeat, got {:?}", other),
    }
}

#[test]
fn error_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/error.json");
    match msg {
        WsMessage::Error(inner) => {
            assert!(!inner.code.is_empty());
            assert!(!inner.message.is_empty());
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn epoch_reset_command_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/epoch_reset_command.json");
    match msg {
        WsMessage::EpochResetCommand(inner) => {
            assert!(!inner.session_id.is_empty());
            assert!(!inner.forwarder_id.is_empty());
            assert!(!inner.reader_ip.is_empty());
            assert!(inner.new_stream_epoch > 0);
        }
        other => panic!("Expected EpochResetCommand, got {:?}", other),
    }
}

#[test]
fn config_get_request_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/config_get_request.json");
    match msg {
        WsMessage::ConfigGetRequest(inner) => {
            assert!(!inner.request_id.is_empty());
        }
        other => panic!("Expected ConfigGetRequest, got {:?}", other),
    }
}

#[test]
fn config_get_response_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/config_get_response.json");
    match msg {
        WsMessage::ConfigGetResponse(inner) => {
            assert!(!inner.request_id.is_empty());
            assert!(inner.ok);
            assert!(inner.error.is_none());
            assert!(!inner.config.is_null());
        }
        other => panic!("Expected ConfigGetResponse, got {:?}", other),
    }
}

#[test]
fn config_get_response_error_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/config_get_response_error.json");
    match msg {
        WsMessage::ConfigGetResponse(inner) => {
            assert!(!inner.request_id.is_empty());
            assert!(!inner.ok);
            assert!(inner.error.is_some());
            assert!(inner.config.is_null());
        }
        other => panic!("Expected ConfigGetResponse, got {:?}", other),
    }
}

#[test]
fn config_set_request_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/config_set_request.json");
    match msg {
        WsMessage::ConfigSetRequest(inner) => {
            assert!(!inner.request_id.is_empty());
            assert!(!inner.section.is_empty());
        }
        other => panic!("Expected ConfigSetRequest, got {:?}", other),
    }
}

#[test]
fn config_set_response_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/config_set_response.json");
    match msg {
        WsMessage::ConfigSetResponse(inner) => {
            assert!(!inner.request_id.is_empty());
            assert!(inner.ok);
            assert!(inner.status_code.is_none());
        }
        other => panic!("Expected ConfigSetResponse, got {:?}", other),
    }
}

#[test]
fn config_set_response_internal_error_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/config_set_response_error_internal.json");
    match msg {
        WsMessage::ConfigSetResponse(inner) => {
            assert!(!inner.request_id.is_empty());
            assert!(!inner.ok);
            assert_eq!(inner.status_code, Some(500));
        }
        other => panic!("Expected ConfigSetResponse, got {:?}", other),
    }
}

#[test]
fn restart_request_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/restart_request.json");
    match msg {
        WsMessage::RestartRequest(inner) => {
            assert!(!inner.request_id.is_empty());
        }
        other => panic!("Expected RestartRequest, got {:?}", other),
    }
}

#[test]
fn restart_response_round_trip() {
    let msg = round_trip("contracts/ws/v1/examples/restart_response.json");
    match msg {
        WsMessage::RestartResponse(inner) => {
            assert!(!inner.request_id.is_empty());
            assert!(inner.ok);
        }
        other => panic!("Expected RestartResponse, got {:?}", other),
    }
}

/// Verify that all frozen error codes are valid deserialization targets.
#[test]
fn error_codes_all_deserialize() {
    let codes = [
        ("INVALID_TOKEN", false),
        ("SESSION_EXPIRED", true),
        ("PROTOCOL_ERROR", false),
        ("IDENTITY_MISMATCH", false),
        ("INTEGRITY_CONFLICT", false),
        ("INTERNAL_ERROR", true),
    ];

    for (code, expected_retryable) in codes {
        let json = format!(
            r#"{{"kind":"error","code":"{code}","message":"test","retryable":{expected_retryable}}}"#
        );
        let msg: WsMessage = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("Failed to parse error with code {code}: {e}"));
        match msg {
            WsMessage::Error(inner) => {
                assert_eq!(inner.code, code, "code mismatch");
                assert_eq!(
                    inner.retryable, expected_retryable,
                    "retryable mismatch for {code}"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}

/// Verify that session_id is NOT present in forwarder_hello.
#[test]
fn forwarder_hello_has_no_session_id() {
    let json_text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("contracts/ws/v1/examples/forwarder_hello.json"),
    )
    .expect("read forwarder_hello.json");

    let raw: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    assert!(
        raw.get("session_id").is_none(),
        "forwarder_hello must NOT contain session_id"
    );
}

/// Verify that session_id is NOT present in receiver_hello.
#[test]
fn receiver_hello_has_no_session_id() {
    let json_text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("contracts/ws/v1/examples/receiver_hello.json"),
    )
    .expect("read receiver_hello.json");

    let raw: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    assert!(
        raw.get("session_id").is_none(),
        "receiver_hello must NOT contain session_id"
    );
}

/// Verify heartbeat carries both session_id AND device_id.
#[test]
fn heartbeat_carries_session_and_device_id() {
    let json_text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("contracts/ws/v1/examples/heartbeat.json"),
    )
    .expect("read heartbeat.json");

    let raw: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    assert!(
        raw.get("session_id").is_some(),
        "heartbeat must contain session_id"
    );
    assert!(
        raw.get("device_id").is_some(),
        "heartbeat must contain device_id"
    );
}

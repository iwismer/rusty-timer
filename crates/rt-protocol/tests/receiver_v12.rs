use rt_protocol::{ReplayTarget, StreamRef, WsMessage};

// Helper: round-trip any serde type via JSON and assert equality
fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + std::fmt::Debug + PartialEq,
{
    let json = serde_json::to_string(value).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(*value, back, "round-trip mismatch: {}", json);
    back
}

#[test]
fn receiver_mode_live_round_trip() {
    // Live with explicit streams and optional earliest_epoch overrides
    let streams = vec![
        StreamRef {
            forwarder_id: "fwd-1".to_string(),
            reader_ip: "10.0.0.1".to_string(),
        },
        StreamRef {
            forwarder_id: "fwd-2".to_string(),
            reader_ip: "10.0.0.2".to_string(),
        },
    ];

    let earliest_epochs = vec![rt_protocol::EarliestEpochOverride {
        forwarder_id: "fwd-1".to_string(),
        reader_ip: "10.0.0.1".to_string(),
        earliest_epoch: 5,
    }];

    let mode = rt_protocol::ReceiverMode::Live {
        streams,
        earliest_epochs,
    };
    let _ = round_trip(&mode);
}

#[test]
fn receiver_mode_race_round_trip() {
    // Race-scoped mode (no earliest epoch override in v1.2)
    let mode = rt_protocol::ReceiverMode::Race {
        race_id: "race-123".to_string(),
    };
    let _ = round_trip(&mode);
}

#[test]
fn receiver_mode_targeted_replay_round_trip() {
    // Targeted replay with a single target
    let mode = rt_protocol::ReceiverMode::TargetedReplay {
        targets: vec![ReplayTarget {
            forwarder_id: "fwd-1".to_string(),
            reader_ip: "10.0.0.1".to_string(),
            stream_epoch: 7,
            from_seq: 1,
        }],
    };
    let _ = round_trip(&mode);
}

#[test]
fn receiver_hello_v12_wrapped_in_wsmessage_round_trip() {
    let hello = rt_protocol::ReceiverHelloV12 {
        receiver_id: "rx-1".to_string(),
        mode: rt_protocol::ReceiverMode::Live {
            streams: vec![StreamRef {
                forwarder_id: "fwd-1".to_string(),
                reader_ip: "10.0.0.1".to_string(),
            }],
            earliest_epochs: vec![],
        },
        resume: vec![],
    };
    let msg = WsMessage::ReceiverHelloV12(hello);
    let _ = round_trip(&msg);
}

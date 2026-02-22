use forwarder::uplink::SendBatchResult;
use forwarder::uplink_replay::should_reconnect_after_replay_send;
use rt_protocol::{
    ConfigGetRequest, ConfigSetRequest, EpochResetCommand, ForwarderAck, RestartRequest,
};

#[test]
fn replay_ack_does_not_force_reconnect() {
    let result: Result<SendBatchResult, ()> = Ok(SendBatchResult::Ack(ForwarderAck {
        session_id: "session-1".to_string(),
        entries: vec![],
    }));

    assert!(!should_reconnect_after_replay_send(&result));
}

#[test]
fn replay_epoch_reset_forces_reconnect() {
    let result: Result<SendBatchResult, ()> = Ok(SendBatchResult::EpochReset(EpochResetCommand {
        session_id: "session-1".to_string(),
        forwarder_id: "fwd-1".to_string(),
        reader_ip: "10.0.0.1:10000".to_string(),
        new_stream_epoch: 3,
    }));

    assert!(should_reconnect_after_replay_send(&result));
}

#[test]
fn replay_send_error_forces_reconnect() {
    let result: Result<SendBatchResult, ()> = Err(());

    assert!(should_reconnect_after_replay_send(&result));
}

#[test]
fn replay_config_get_forces_reconnect() {
    let result: Result<SendBatchResult, ()> = Ok(SendBatchResult::ConfigGet(ConfigGetRequest {
        request_id: "cfg-get-1".to_string(),
    }));

    assert!(should_reconnect_after_replay_send(&result));
}

#[test]
fn replay_config_set_forces_reconnect() {
    let result: Result<SendBatchResult, ()> = Ok(SendBatchResult::ConfigSet(ConfigSetRequest {
        request_id: "cfg-set-1".to_string(),
        section: "uplink".to_string(),
        payload: serde_json::json!({"batch_max_events": 100}),
    }));

    assert!(should_reconnect_after_replay_send(&result));
}

#[test]
fn replay_restart_forces_reconnect() {
    let result: Result<SendBatchResult, ()> = Ok(SendBatchResult::Restart(RestartRequest {
        request_id: "restart-1".to_string(),
    }));

    assert!(should_reconnect_after_replay_send(&result));
}

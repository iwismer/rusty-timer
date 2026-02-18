use forwarder::uplink::SendBatchResult;
use forwarder::uplink_replay::should_reconnect_after_replay_send;
use rt_protocol::{EpochResetCommand, ForwarderAck};

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

use crate::uplink::SendBatchResult;

/// Replay-phase send outcomes that require reconnecting before entering the
/// main uplink loop.
pub fn should_reconnect_after_replay_send<E>(result: &Result<SendBatchResult, E>) -> bool {
    matches!(result, Ok(SendBatchResult::EpochReset(_)) | Err(_))
}

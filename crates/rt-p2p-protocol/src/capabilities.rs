//! Named capability tokens advertised in [`Hello.capabilities`](crate::Hello)
//! and returned (as the set-intersection) in [`HelloOk.capabilities`](crate::HelloOk).
//!
//! Capabilities let peers opt in to optional protocol features without bumping
//! the protocol minor. Negotiation keeps only the tokens both sides advertise
//! (see [`negotiate`](crate::negotiate)); use [`has_capability`] to test the
//! negotiated set.

/// Peer supports forwarder->receiver live status events on the control stream
/// (`ReaderStatus` / `ReaderInfo` / `UpsStatus` / `DownloadProgress` /
/// `SyncClock`).
pub const CAP_CONTROL_EVENTS: &str = "control-events";

/// Peer supports remote forwarder config get/set/restart verbs (used in
/// Phase 4 remote configuration).
pub const CAP_REMOTE_CONFIG: &str = "remote-config";

/// Peer supports reader-specific control verbs on the control stream.
pub const CAP_READER_CONTROL: &str = "reader_control";

/// Returns `true` when `cap` is present in a negotiated capability set.
///
/// `caps` is typically [`HelloOk.capabilities`](crate::HelloOk), the
/// set-intersection of both peers' advertised capabilities.
#[must_use]
pub fn has_capability(caps: &[String], cap: &str) -> bool {
    caps.iter().any(|c| c == cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_capability_detects_present_and_absent() {
        let caps = vec![CAP_CONTROL_EVENTS.to_string(), "some-other-cap".to_string()];

        assert!(has_capability(&caps, CAP_CONTROL_EVENTS));
        assert!(!has_capability(&caps, CAP_REMOTE_CONFIG));
        assert!(!has_capability(&caps, CAP_READER_CONTROL));
        assert!(!has_capability(&[], CAP_CONTROL_EVENTS));
    }
}

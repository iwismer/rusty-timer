use crate::state::ReaderDisplayState;

// ---------------------------------------------------------------------------
// Display geometry constants (240×320 ST7789, portrait)
// ---------------------------------------------------------------------------

pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;
/// Readers shown as full rows. A single-line count and a compact two-row
/// footer leave room for six legible rows; extra readers roll up into a
/// "+N more" indicator (see [`overflow_count`]).
pub const MAX_VISIBLE_READERS: usize = 6;
pub const HEADER_HEIGHT: u32 = 34;
/// Footer holds two rows: IP on top, CPU + battery side-by-side below.
pub const FOOTER_HEIGHT: u32 = 50;
pub const READER_ROW_HEIGHT: u32 = 34;

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Return up to [`MAX_VISIBLE_READERS`] readers sorted by state (Connected
/// first) then by IP address for stable ordering.
///
/// Uses the same ordering as the e-ink layout so both backends agree on which
/// readers are shown.
pub fn filter_readers(readers: &[ReaderDisplayState]) -> Vec<&ReaderDisplayState> {
    let mut sorted: Vec<&ReaderDisplayState> = readers.iter().collect();
    sorted.sort_by(|a, b| a.state.cmp(&b.state).then_with(|| a.ip.cmp(&b.ip)));
    sorted.truncate(MAX_VISIBLE_READERS);
    sorted
}

/// Number of readers hidden beyond [`MAX_VISIBLE_READERS`].
#[must_use]
pub fn overflow_count(total: usize) -> usize {
    total.saturating_sub(MAX_VISIBLE_READERS)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ReaderConnectionState, ReaderDisplayState};

    fn make_reader(ip: &str, state: ReaderConnectionState) -> ReaderDisplayState {
        ReaderDisplayState {
            ip: ip.to_string(),
            state,
            drift_ms: None,
            session_reads: 0,
        }
    }

    #[test]
    fn filter_empty_returns_empty() {
        assert!(filter_readers(&[]).is_empty());
    }

    #[test]
    fn filter_sorts_connected_first() {
        let readers = vec![
            make_reader("192.168.1.3", ReaderConnectionState::Disconnected),
            make_reader("192.168.1.1", ReaderConnectionState::Connecting),
            make_reader("192.168.1.2", ReaderConnectionState::Connected),
        ];
        let result = filter_readers(&readers);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].state, ReaderConnectionState::Connected);
        assert_eq!(result[0].ip, "192.168.1.2");
        assert_eq!(result[1].state, ReaderConnectionState::Connecting);
        assert_eq!(result[2].state, ReaderConnectionState::Disconnected);
    }

    #[test]
    fn filter_secondary_sort_by_ip() {
        let readers = vec![
            make_reader("192.168.1.3", ReaderConnectionState::Connected),
            make_reader("192.168.1.1", ReaderConnectionState::Connected),
            make_reader("192.168.1.2", ReaderConnectionState::Connected),
        ];
        let result = filter_readers(&readers);
        assert_eq!(result[0].ip, "192.168.1.1");
        assert_eq!(result[1].ip, "192.168.1.2");
        assert_eq!(result[2].ip, "192.168.1.3");
    }

    #[test]
    fn filter_truncates_to_max() {
        let readers: Vec<ReaderDisplayState> = (1..=10)
            .map(|i| make_reader(&format!("192.168.1.{i}"), ReaderConnectionState::Connected))
            .collect();
        let result = filter_readers(&readers);
        assert_eq!(result.len(), MAX_VISIBLE_READERS);
        assert_eq!(result.len(), 6);
    }

    #[test]
    fn filter_more_than_max_connected_takes_first_by_ip() {
        // IPs chosen so lexicographic order is deterministic and distinct.
        let readers: Vec<ReaderDisplayState> = (1..=9)
            .map(|i| make_reader(&format!("10.0.0.{i:02}"), ReaderConnectionState::Connected))
            .collect();
        let result = filter_readers(&readers);
        assert_eq!(result.len(), 6);
        assert_eq!(result[0].ip, "10.0.0.01");
        assert_eq!(result[5].ip, "10.0.0.06");
    }

    #[test]
    fn overflow_count_zero_when_within_cap() {
        assert_eq!(overflow_count(0), 0);
        assert_eq!(overflow_count(6), 0);
        assert_eq!(overflow_count(MAX_VISIBLE_READERS), 0);
    }

    #[test]
    fn overflow_count_counts_hidden_readers() {
        assert_eq!(overflow_count(7), 1);
        assert_eq!(overflow_count(8), 2);
        assert_eq!(overflow_count(100), 94);
    }
}

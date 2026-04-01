use crate::state::ReaderDisplayState;

// ---------------------------------------------------------------------------
// Display geometry constants
// ---------------------------------------------------------------------------

pub const DISPLAY_WIDTH: u32 = 250;
pub const DISPLAY_HEIGHT: u32 = 122;
pub const DIVIDER_X: u32 = 125;
pub const MAX_VISIBLE_READERS: usize = 4;

/// Two lines of FONT_7X13 at 13 px each.
const READER_BLOCK_HEIGHT: u32 = 26;

// ---------------------------------------------------------------------------
// Layout types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutMetrics {
    pub reader_y_positions: Vec<u32>,
    pub reader_gap: u32,
}

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Return up to [`MAX_VISIBLE_READERS`] readers sorted by state (Connected
/// first) then by IP address for stable ordering.
pub fn filter_readers(readers: &[ReaderDisplayState]) -> Vec<&ReaderDisplayState> {
    let mut sorted: Vec<&ReaderDisplayState> = readers.iter().collect();
    sorted.sort_by(|a, b| a.state.cmp(&b.state).then_with(|| a.ip.cmp(&b.ip)));
    sorted.truncate(MAX_VISIBLE_READERS);
    sorted
}

/// Compute the y positions and gap for `visible_reader_count` readers.
///
/// Remaining vertical space is distributed evenly as gaps above, between, and
/// below all reader blocks.
pub fn compute_layout(visible_reader_count: usize) -> LayoutMetrics {
    let count = visible_reader_count.min(MAX_VISIBLE_READERS);
    if count == 0 {
        return LayoutMetrics {
            reader_y_positions: vec![],
            reader_gap: 0,
        };
    }

    let used = count as u32 * READER_BLOCK_HEIGHT;
    let remaining = DISPLAY_HEIGHT.saturating_sub(used);
    let gaps_count = count as u32 + 1;
    let gap = remaining / gaps_count;

    let mut positions = Vec::with_capacity(count);
    let mut y = gap;
    for _ in 0..count {
        positions.push(y);
        y += READER_BLOCK_HEIGHT + gap;
    }

    LayoutMetrics {
        reader_y_positions: positions,
        reader_gap: gap,
    }
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

    // --- filter_readers tests ---

    #[test]
    fn filter_empty_returns_empty() {
        let result = filter_readers(&[]);
        assert!(result.is_empty());
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
        assert_eq!(result[0].ip, "192.168.1.2");
        assert_eq!(result[0].state, ReaderConnectionState::Connected);
        assert_eq!(result[1].ip, "192.168.1.1");
        assert_eq!(result[1].state, ReaderConnectionState::Connecting);
        assert_eq!(result[2].ip, "192.168.1.3");
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
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].ip, "192.168.1.1");
        assert_eq!(result[1].ip, "192.168.1.2");
        assert_eq!(result[2].ip, "192.168.1.3");
    }

    #[test]
    fn filter_truncates_to_max_visible() {
        let readers: Vec<ReaderDisplayState> = (1..=5)
            .map(|i| make_reader(&format!("192.168.1.{i}"), ReaderConnectionState::Connected))
            .collect();
        let result = filter_readers(&readers);
        assert_eq!(result.len(), MAX_VISIBLE_READERS);
    }

    #[test]
    fn filter_connected_fill_remaining_with_disconnected() {
        let readers = vec![
            make_reader("192.168.1.4", ReaderConnectionState::Disconnected),
            make_reader("192.168.1.5", ReaderConnectionState::Disconnected),
            make_reader("192.168.1.6", ReaderConnectionState::Disconnected),
            make_reader("192.168.1.1", ReaderConnectionState::Connected),
            make_reader("192.168.1.2", ReaderConnectionState::Connected),
        ];
        let result = filter_readers(&readers);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].state, ReaderConnectionState::Connected);
        assert_eq!(result[0].ip, "192.168.1.1");
        assert_eq!(result[1].state, ReaderConnectionState::Connected);
        assert_eq!(result[1].ip, "192.168.1.2");
        assert_eq!(result[2].state, ReaderConnectionState::Disconnected);
        assert_eq!(result[3].state, ReaderConnectionState::Disconnected);
    }

    #[test]
    fn filter_more_than_4_connected_takes_first_4_by_ip() {
        let readers: Vec<ReaderDisplayState> = (1..=5)
            .map(|i| make_reader(&format!("192.168.1.{i}"), ReaderConnectionState::Connected))
            .collect();
        let result = filter_readers(&readers);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].ip, "192.168.1.1");
        assert_eq!(result[1].ip, "192.168.1.2");
        assert_eq!(result[2].ip, "192.168.1.3");
        assert_eq!(result[3].ip, "192.168.1.4");
    }

    // --- compute_layout tests ---

    #[test]
    fn layout_zero_readers() {
        let metrics = compute_layout(0);
        assert!(metrics.reader_y_positions.is_empty());
        assert_eq!(metrics.reader_gap, 0);
    }

    #[test]
    fn layout_one_reader_centered() {
        // remaining = 122 - 26 = 96, gaps_count = 2, gap = 48
        let metrics = compute_layout(1);
        assert_eq!(metrics.reader_gap, 48);
        assert_eq!(metrics.reader_y_positions, vec![48]);
    }

    #[test]
    fn layout_four_readers_fits() {
        let metrics = compute_layout(4);
        assert_eq!(metrics.reader_y_positions.len(), 4);
        // No overlap: each block starts at y and ends at y + READER_BLOCK_HEIGHT
        for window in metrics.reader_y_positions.windows(2) {
            assert!(window[1] >= window[0] + READER_BLOCK_HEIGHT);
        }
        // Last reader fits within display height
        let last_y = *metrics.reader_y_positions.last().unwrap();
        assert!(last_y + READER_BLOCK_HEIGHT <= DISPLAY_HEIGHT);
    }

    #[test]
    fn layout_five_readers_clamped_to_four() {
        let four = compute_layout(4);
        let five = compute_layout(5);
        assert_eq!(four, five);
    }
}

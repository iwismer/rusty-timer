use chrono::{DateTime, Utc};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncerInputEvent {
    pub stream_id: String,
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_at: DateTime<Utc>,
    pub division: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnnouncerRow {
    pub stream_id: String,
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_at: DateTime<Utc>,
    pub division: Option<String>,
}

impl From<AnnouncerInputEvent> for AnnouncerRow {
    fn from(event: AnnouncerInputEvent) -> Self {
        Self {
            stream_id: event.stream_id,
            seq: event.seq,
            chip_id: event.chip_id,
            bib: event.bib,
            display_name: event.display_name,
            reader_timestamp: event.reader_timestamp,
            received_at: event.received_at,
            division: event.division,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnnouncerDelta {
    pub row: AnnouncerRow,
    pub finisher_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnouncerEvent {
    Update(AnnouncerDelta),
    Resync,
}

#[derive(Debug, Default)]
pub struct AnnouncerRuntime {
    seen_chips: HashSet<String>,
    rows: VecDeque<AnnouncerRow>,
    finisher_count: u64,
}

impl AnnouncerRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.seen_chips.clear();
        self.rows.clear();
        self.finisher_count = 0;
    }

    #[must_use]
    pub fn rows(&self) -> &VecDeque<AnnouncerRow> {
        &self.rows
    }

    #[must_use]
    pub fn finisher_count(&self) -> u64 {
        self.finisher_count
    }

    pub fn ingest(
        &mut self,
        event: AnnouncerInputEvent,
        max_list_size: usize,
    ) -> Option<AnnouncerDelta> {
        if !self.seen_chips.insert(event.chip_id.clone()) {
            return None;
        }

        self.finisher_count += 1;

        let row = AnnouncerRow::from(event);

        self.rows.push_front(row.clone());
        while self.rows.len() > max_list_size {
            self.rows.pop_back();
        }

        Some(AnnouncerDelta {
            row,
            finisher_count: self.finisher_count,
        })
    }
}

#[must_use]
pub fn compare_input_events(a: &AnnouncerInputEvent, b: &AnnouncerInputEvent) -> Ordering {
    a.received_at
        .cmp(&b.received_at)
        .then_with(|| a.stream_id.cmp(&b.stream_id))
        .then_with(|| a.seq.cmp(&b.seq))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn make_event(chip_id: &str, stream_id: &str, seq: u64, secs: i64) -> AnnouncerInputEvent {
        AnnouncerInputEvent {
            stream_id: stream_id.to_owned(),
            seq,
            chip_id: chip_id.to_owned(),
            bib: Some(101),
            display_name: "Runner".to_owned(),
            reader_timestamp: Some("10:00:00".to_owned()),
            received_at: Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap()
                + chrono::Duration::seconds(secs),
            division: None,
        }
    }

    #[test]
    fn accepts_first_chip_only_and_ignores_duplicate_chip() {
        let mut runtime = AnnouncerRuntime::new();

        let first = runtime.ingest(make_event("A", "stream-a", 1, 0), 25);
        assert!(first.is_some(), "first read for chip should be accepted");
        assert_eq!(runtime.finisher_count(), 1);

        let second = runtime.ingest(make_event("A", "stream-a", 2, 1), 25);
        assert!(second.is_none(), "duplicate chip should be ignored");
        assert_eq!(runtime.finisher_count(), 1);
        assert_eq!(runtime.rows().len(), 1);
    }

    #[test]
    fn keeps_newest_and_trims_oldest_while_counting_all_accepted_rows() {
        let mut runtime = AnnouncerRuntime::new();

        runtime.ingest(make_event("A", "stream-a", 1, 0), 2);
        runtime.ingest(make_event("B", "stream-a", 2, 1), 2);
        let latest = runtime
            .ingest(make_event("C", "stream-a", 3, 2), 2)
            .expect("third unique chip should be accepted");

        assert_eq!(latest.finisher_count, 3);
        assert_eq!(runtime.finisher_count(), 3);
        assert_eq!(runtime.rows().len(), 2);
        assert_eq!(runtime.rows()[0].chip_id, "C");
        assert_eq!(runtime.rows()[1].chip_id, "B");
    }

    #[test]
    fn reset_clears_state() {
        let mut runtime = AnnouncerRuntime::new();

        runtime.ingest(make_event("A", "stream-a", 1, 0), 25);
        runtime.ingest(make_event("B", "stream-a", 2, 1), 25);
        runtime.reset();

        assert_eq!(runtime.finisher_count(), 0);
        assert!(runtime.rows().is_empty());

        let after_reset = runtime.ingest(make_event("A", "stream-a", 3, 2), 25);
        assert!(after_reset.is_some(), "reset should clear seen chips");
        assert_eq!(runtime.finisher_count(), 1);
        assert_eq!(runtime.rows().len(), 1);
    }
}

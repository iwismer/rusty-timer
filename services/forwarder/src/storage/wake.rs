//! Per-stream commit wake-up signals for the forwarder journal.
//!
//! Subscribers (data-stream senders) need to learn when new events become
//! durably committed without busy-polling SQLite. Each stream exposes a
//! [`tokio::sync::watch`] channel carrying the highest seq that has been
//! committed for that stream. The watch value is advanced **only after** a
//! write transaction commits, so a subscriber can never be woken for a seq
//! that later rolled back.
//!
//! ## Subscriber pattern: arm-then-recheck plus poll fallback
//!
//! `watch` coalesces notifications: if several appends commit in a burst, a
//! lagging subscriber observes only the latest seq. This is harmless because
//! the seq is monotonic and the subscriber reads *all* events after its own
//! cursor, so a skipped intermediate tick never drops data.
//!
//! To avoid lost wake-ups, subscribers must **arm before they recheck**:
//!
//! ```ignore
//! let mut rx = registry.subscribe(stream_key);
//! loop {
//!     // 1. Arm: mark the current notification generation as seen *before*
//!     //    querying, so a commit racing the query still triggers `changed()`.
//!     rx.mark_unchanged();
//!     // 2. Recheck: read everything after the local cursor.
//!     let events = journal.read_events_after(stream_key, cursor, max)?;
//!     if !events.is_empty() { /* send + advance cursor */ continue; }
//!     // 3. Wait, with a periodic poll fallback as a safety net.
//!     tokio::select! {
//!         _ = rx.changed() => {}
//!         () = tokio::time::sleep(poll_interval) => {}
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::watch;

/// Registry of per-stream "latest committed seq" watch channels.
///
/// Cheap to clone behind an `Arc`; the journal holds one and hands clones to
/// subscribers via [`Journal::wake_registry`](crate::storage::journal::Journal::wake_registry).
#[derive(Debug, Default)]
pub struct WakeRegistry {
    streams: Mutex<HashMap<String, watch::Sender<u64>>>,
}

impl WakeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to commit notifications for a stream.
    ///
    /// The returned receiver's initial value is the latest committed seq known
    /// so far (`0` if nothing has committed yet). Subscribing creates the
    /// stream's channel lazily if it does not exist.
    #[must_use]
    pub fn subscribe(&self, stream_key: &str) -> watch::Receiver<u64> {
        let mut streams = self.streams.lock().expect("wake registry poisoned");
        streams
            .entry(stream_key.to_owned())
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    /// Latest committed seq currently published for a stream (`0` if none).
    #[must_use]
    pub fn latest(&self, stream_key: &str) -> u64 {
        let streams = self.streams.lock().expect("wake registry poisoned");
        streams.get(stream_key).map_or(0, |tx| *tx.borrow())
    }

    /// Publish a committed seq for a stream, waking subscribers.
    ///
    /// Must be called only **after** the write transaction that produced
    /// `committed_seq` has committed. The published value only ever moves
    /// forward, so an out-of-order or stale call cannot lower the high-water.
    /// Intermediate values may be coalesced for lagging receivers, which is
    /// harmless (see module docs).
    pub fn notify_committed(&self, stream_key: &str, committed_seq: u64) {
        let mut streams = self.streams.lock().expect("wake registry poisoned");
        let sender = streams
            .entry(stream_key.to_owned())
            .or_insert_with(|| watch::channel(0).0);
        sender.send_modify(|current| {
            if committed_seq > *current {
                *current = committed_seq;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::journal::Journal;

    fn open_temp_journal() -> (Journal, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("journal.db");
        let journal = Journal::open(&path).expect("open journal");
        (journal, dir)
    }

    /// A commit advances the stream's watch value to the committed seq, and
    /// only after the transaction has committed.
    #[tokio::test]
    async fn wake_fires_after_commit() {
        let (mut journal, _dir) = open_temp_journal();
        let stream_key = "10.0.0.20:10000";
        journal
            .ensure_stream_state(stream_key, 1)
            .expect("ensure stream state");

        let registry = journal.wake_registry();
        let mut rx = registry.subscribe(stream_key);
        assert_eq!(*rx.borrow_and_update(), 0, "no commits yet");

        let (_epoch, seq) = journal
            .append_read(stream_key, None, b"frame", "RAW")
            .expect("append_read");

        rx.changed().await.expect("watch should fire after commit");
        assert_eq!(*rx.borrow(), seq as u64, "watch value is the committed seq");
    }

    /// A subscriber that has already drained every committed seq still wakes
    /// when a brand-new read commits.
    #[tokio::test]
    async fn caught_up_subscriber_wakes_on_new_read() {
        let (mut journal, _dir) = open_temp_journal();
        let stream_key = "10.0.0.21:10000";
        journal
            .ensure_stream_state(stream_key, 1)
            .expect("ensure stream state");

        journal
            .append_read(stream_key, None, b"first", "RAW")
            .expect("append first");

        // Subscribe and become fully caught up: arm so the current generation
        // is marked seen.
        let mut rx = journal.wake_registry().subscribe(stream_key);
        rx.mark_unchanged();
        assert_eq!(*rx.borrow(), 1, "caught up to seq 1");

        let (_epoch, seq) = journal
            .append_read(stream_key, None, b"second", "RAW")
            .expect("append second");

        rx.changed()
            .await
            .expect("caught-up subscriber must wake on a new read");
        assert_eq!(*rx.borrow(), seq as u64);
        assert_eq!(seq, 2, "second read advanced the seq");
    }

    /// Bursts of commits coalesce: a subscriber that misses intermediate ticks
    /// still observes the latest seq and loses no data, since it reads all
    /// events after its cursor.
    #[tokio::test]
    async fn missed_intermediate_tick_is_harmless() {
        let (mut journal, _dir) = open_temp_journal();
        let stream_key = "10.0.0.22:10000";
        journal
            .ensure_stream_state(stream_key, 1)
            .expect("ensure stream state");

        let mut rx = journal.wake_registry().subscribe(stream_key);
        rx.mark_unchanged();

        // Three commits land before the subscriber rechecks.
        let mut last_seq = 0;
        for i in 0..3 {
            let frame = format!("burst-{i}");
            let (_epoch, seq) = journal
                .append_read(stream_key, None, frame.as_bytes(), "RAW")
                .expect("append_read");
            last_seq = seq;
        }

        // A single wake-up coalesces the burst into the latest seq.
        rx.changed().await.expect("burst must wake the subscriber");
        assert_eq!(
            *rx.borrow(),
            last_seq as u64,
            "watch coalesces to the latest committed seq"
        );

        // No further notification is pending: the intermediate ticks were
        // coalesced, not queued.
        assert!(
            !rx.has_changed().expect("sender alive"),
            "intermediate ticks must not queue extra wake-ups"
        );

        // Reading from the start still yields every event in the burst, so the
        // coalesced ticks dropped no data.
        let events = journal
            .read_events_after(stream_key, 0, usize::MAX)
            .expect("read events");
        assert_eq!(events.len(), 3, "all burst events remain readable");
        assert_eq!(events.last().unwrap().seq, last_seq);
    }
}

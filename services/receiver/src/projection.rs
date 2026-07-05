//! In-memory per-stream UI projection.
//!
//! Fed post-commit [`EventFact`]s from the durable hint channel; never reads
//! the DB on the hot path. Replaces the per-batch full-table
//! `project_stream_ui_state` reload that made UI projection O(N) per batch
//! (O(N²) total).
//!
//! Facts arrive already deduplicated: the persist path only emits facts for
//! rows actually inserted (idempotent on `(stream_id, seq)`), so `apply` can
//! count every fact without tracking seen seqs.

use std::collections::HashSet;

use crate::p2p_session::EventFact;
use crate::stream_key::LocalStreamKey;

/// Legacy display metadata carried on UI payloads. This is presentation-only:
/// two forwarders can expose the same `(forwarder_id, reader_ip)` pair, so it
/// must never key a cache — that is what [`LocalStreamKey`] is for.
#[derive(Clone, Debug)]
pub struct UiStreamDisplay {
    pub forwarder_id: String,
    pub reader_ip: String,
}

/// One per-epoch summary row from the startup rebuild query
/// (`Db::load_stream_projection_summary`).
#[derive(Clone, Debug)]
pub struct EpochSummary {
    pub epoch: i64,
    pub count: u64,
    pub max_received_unix_ms: i64,
    pub max_seq: i64,
}

/// In-memory per-stream UI projection. Fed post-commit facts; never reads the
/// DB.
#[derive(Clone, Debug, Default)]
pub struct StreamProjection {
    /// Lifetime total of inserted rows (across epochs).
    pub total: u64,
    /// The live (highest observed) epoch.
    pub epoch: i64,
    /// Inserted rows in the live epoch.
    pub epoch_count: u64,
    /// Distinct chip ids observed in the live epoch only.
    pub unique_chips: HashSet<String>,
    /// Max `received_unix_ms` over all observed facts.
    pub max_received_unix_ms: i64,
    /// Highest observed seq; `last_chip_id` tracks this row.
    pub last_seq: i64,
    /// Chip id of the max-seq fact, for the LastRead UI event.
    pub last_chip_id: Option<String>,
}

impl StreamProjection {
    /// Fold one post-commit fact into the projection. O(1).
    pub fn apply(&mut self, fact: &EventFact) {
        // Only a *newer* epoch resets epoch state. A stale (older) epoch fact —
        // possible via at-least-once redelivery — must not clobber the live
        // epoch, matching StreamCounts semantics (cache.rs).
        if fact.epoch > self.epoch {
            self.epoch = fact.epoch;
            self.epoch_count = 0;
            self.unique_chips.clear();
        }
        self.total += 1;
        if fact.epoch == self.epoch {
            self.epoch_count += 1;
            self.unique_chips.insert(fact.chip_id.clone());
        }
        self.max_received_unix_ms = self.max_received_unix_ms.max(fact.received_unix_ms);
        if fact.seq >= self.last_seq {
            self.last_seq = fact.seq;
            self.last_chip_id = Some(fact.chip_id.clone());
        }
    }

    /// Seed the projection from the one-time startup summary queries: per-epoch
    /// rows (ordered by epoch) plus the distinct chip set of the live epoch and
    /// the chip id of the latest stored row.
    pub fn seed_from_summary(
        rows: &[EpochSummary],
        live_epoch_chips: HashSet<String>,
        last_chip_id: Option<String>,
    ) -> Self {
        let mut proj = Self::default();
        for row in rows {
            proj.total += row.count;
            if row.epoch > proj.epoch {
                proj.epoch = row.epoch;
                proj.epoch_count = row.count;
            } else if row.epoch == proj.epoch {
                proj.epoch_count += row.count;
            }
            proj.max_received_unix_ms = proj.max_received_unix_ms.max(row.max_received_unix_ms);
            if row.max_seq >= proj.last_seq {
                proj.last_seq = row.max_seq;
            }
        }
        proj.unique_chips = live_epoch_chips;
        proj.last_chip_id = last_chip_id;
        proj
    }

    /// Build the stream-metrics UI payload from the projection. Lag is
    /// computed at emit time from `now_ms`.
    pub fn metrics(
        &self,
        local_key: &LocalStreamKey,
        display: &UiStreamDisplay,
        now_ms: i64,
    ) -> crate::ui_events::StreamMetricsPayload {
        let last_received = (self.max_received_unix_ms > 0).then_some(self.max_received_unix_ms);
        let lag_ms =
            last_received.map(|last| u64::try_from(now_ms.saturating_sub(last)).unwrap_or(0));
        crate::ui_events::StreamMetricsPayload {
            forwarder_endpoint_id: local_key.endpoint_id().to_owned(),
            stream_id: local_key.wire_stream_id().to_owned(),
            forwarder_id: display.forwarder_id.clone(),
            reader_ip: display.reader_ip.clone(),
            raw_count: i64::try_from(self.total).unwrap_or(i64::MAX),
            dedup_count: i64::try_from(self.total).unwrap_or(i64::MAX),
            retransmit_count: 0,
            lag_ms,
            epoch_raw_count: i64::try_from(self.epoch_count).unwrap_or(i64::MAX),
            epoch_dedup_count: i64::try_from(self.epoch_count).unwrap_or(i64::MAX),
            epoch_retransmit_count: 0,
            unique_chips: i64::try_from(self.unique_chips.len()).unwrap_or(i64::MAX),
            epoch_last_received_at: last_received.and_then(crate::ui_events::unix_ms_to_rfc3339),
            epoch_lag_ms: lag_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(seq: i64, epoch: i64, received_unix_ms: i64, chip_id: &str) -> EventFact {
        EventFact {
            seq,
            epoch,
            received_unix_ms,
            chip_id: chip_id.to_owned(),
        }
    }

    #[test]
    fn fold_counts_only_inserted_rows() {
        let mut proj = StreamProjection::default();
        proj.apply(&fact(1, 1, 100, "a"));
        proj.apply(&fact(2, 1, 200, "b"));
        proj.apply(&fact(3, 1, 300, "a"));
        assert_eq!(proj.total, 3);
        assert_eq!(proj.epoch_count, 3);
        assert_eq!(proj.unique_chips.len(), 2);
        assert_eq!(proj.epoch, 1);
    }

    #[test]
    fn epoch_rollover_clears_unique_chips() {
        let mut proj = StreamProjection::default();
        proj.apply(&fact(1, 1, 100, "a"));
        proj.apply(&fact(2, 1, 200, "b"));
        proj.apply(&fact(3, 2, 300, "c"));
        assert_eq!(proj.epoch, 2);
        assert_eq!(proj.epoch_count, 1, "epoch_raw_count resets on rollover");
        assert_eq!(
            proj.unique_chips,
            HashSet::from(["c".to_owned()]),
            "unique_chips counts only the live epoch"
        );
        assert_eq!(proj.total, 3, "total spans epochs");
    }

    #[test]
    fn stale_epoch_does_not_reset() {
        let mut proj = StreamProjection::default();
        proj.apply(&fact(1, 2, 100, "a"));
        proj.apply(&fact(2, 2, 200, "b"));
        // Redelivered epoch-1 fact: counts toward total, must not clobber the
        // live epoch state.
        proj.apply(&fact(3, 1, 300, "z"));
        assert_eq!(proj.epoch, 2);
        assert_eq!(proj.epoch_count, 2);
        assert_eq!(
            proj.unique_chips,
            HashSet::from(["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(proj.total, 3);
    }

    #[test]
    fn last_read_tracks_max_seq() {
        let mut proj = StreamProjection::default();
        proj.apply(&fact(5, 1, 500, "high"));
        proj.apply(&fact(3, 1, 300, "low"));
        assert_eq!(proj.last_seq, 5);
        assert_eq!(proj.last_chip_id.as_deref(), Some("high"));
    }

    fn display() -> UiStreamDisplay {
        UiStreamDisplay {
            forwarder_id: "fwd".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
        }
    }

    #[test]
    fn lag_computed_at_emit_time() {
        let mut proj = StreamProjection::default();
        proj.apply(&fact(1, 1, 1_000, "a"));
        proj.apply(&fact(2, 1, 4_000, "b"));
        let key = LocalStreamKey::new("endpoint-1", "10.0.0.1:10000");
        let metrics = proj.metrics(&key, &display(), 10_000);
        assert_eq!(metrics.lag_ms, Some(6_000));
        assert_eq!(metrics.epoch_lag_ms, Some(6_000));
        // A later emit with the same projection reflects the new now.
        let metrics = proj.metrics(&key, &display(), 14_000);
        assert_eq!(metrics.lag_ms, Some(10_000));
    }

    #[test]
    fn metrics_carry_composite_identity_and_display_metadata() {
        let proj = StreamProjection::default();
        let key = LocalStreamKey::new("endpoint-1", "wire-stream");
        let metrics = proj.metrics(&key, &display(), 10_000);
        assert_eq!(metrics.forwarder_endpoint_id, "endpoint-1");
        assert_eq!(metrics.stream_id, "wire-stream");
        assert_eq!(metrics.forwarder_id, "fwd");
        assert_eq!(metrics.reader_ip, "10.0.0.1:10000");
    }

    #[test]
    fn metrics_with_no_rows_has_no_lag() {
        let proj = StreamProjection::default();
        let key = LocalStreamKey::new("endpoint-1", "10.0.0.1:10000");
        let metrics = proj.metrics(&key, &display(), 10_000);
        assert_eq!(metrics.lag_ms, None);
        assert_eq!(metrics.epoch_last_received_at, None);
        assert_eq!(metrics.raw_count, 0);
    }

    #[test]
    fn rebuild_from_summary_rows_matches_fold() {
        let facts = [
            fact(1, 1, 100, "a"),
            fact(2, 1, 200, "b"),
            fact(3, 2, 300, "c"),
            fact(4, 2, 400, "c"),
            fact(5, 2, 500, "d"),
        ];
        let mut folded = StreamProjection::default();
        for f in &facts {
            folded.apply(f);
        }

        // The startup queries produce per-epoch summary rows plus the live
        // epoch's distinct chips and the latest row's chip id.
        let rows = vec![
            EpochSummary {
                epoch: 1,
                count: 2,
                max_received_unix_ms: 200,
                max_seq: 2,
            },
            EpochSummary {
                epoch: 2,
                count: 3,
                max_received_unix_ms: 500,
                max_seq: 5,
            },
        ];
        let seeded = StreamProjection::seed_from_summary(
            &rows,
            HashSet::from(["c".to_owned(), "d".to_owned()]),
            Some("d".to_owned()),
        );

        assert_eq!(seeded.total, folded.total);
        assert_eq!(seeded.epoch, folded.epoch);
        assert_eq!(seeded.epoch_count, folded.epoch_count);
        assert_eq!(seeded.unique_chips, folded.unique_chips);
        assert_eq!(seeded.max_received_unix_ms, folded.max_received_unix_ms);
        assert_eq!(seeded.last_seq, folded.last_seq);
        assert_eq!(seeded.last_chip_id, folded.last_chip_id);
    }
}

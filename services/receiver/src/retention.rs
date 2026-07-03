//! Bounded retention for `received_events`.
//!
//! Without pruning the receiver's table grows unboundedly over a long event
//! (millions of rows), inflating disk usage and every remaining O(N)
//! operation. Pruning deletes rows that every consumer has provably finished
//! with — strictly **behind the delivery low-water mark** — and always keeps a
//! generous safety window below it (reads are money; disk is cheaper than a
//! lost read).
//!
//! The low-water mark per stream is the minimum of:
//! - the durable **ack cursor** (rows above it may still be re-requested),
//! - the **DBF floor** when DBF output is enabled (never prune a row the DBF
//!   worker has not processed: a regenerate must be able to reproduce it),
//! - the **announcer floor** when the announcer is enabled (min unpushed seq),
//! - every **active durable-proxy consumer's** replay cursor.
//!
//! Deletes run through the writer actor (`WriteCommand::Prune`) as seq-range
//! deletes on the primary key, advancing the persisted `pruned_through_seq`
//! watermark in the same transaction. New proxy consumers start replay at
//! that watermark (see `local_proxy`), so pruning cannot strand them waiting
//! for a deleted seq 1.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::control_api::AppState;
use crate::stream_key::LocalStreamKey;

/// Retention tuning. Defaults keep 24 h *and* 100k rows per stream below the
/// low-water mark; environment knobs override for constrained deployments.
#[derive(Clone, Debug)]
pub struct RetentionConfig {
    /// Minimum rows retained per stream (env `RT_RECEIVER_RETAIN_MIN_ROWS`).
    pub retain_min_rows: i64,
    /// Minimum age retained per stream (env `RT_RECEIVER_RETAIN_MIN_HOURS`).
    pub retain_min_age_ms: i64,
    /// How often the retention worker scans (env
    /// `RT_RECEIVER_RETENTION_INTERVAL_SECS`).
    pub interval: Duration,
    /// Rows deleted per prune pass (bounds transaction size).
    pub max_rows_per_pass: i64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            retain_min_rows: 100_000,
            retain_min_age_ms: 24 * 60 * 60 * 1000,
            interval: Duration::from_secs(60),
            max_rows_per_pass: 20_000,
        }
    }
}

impl RetentionConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Some(rows) = std::env::var("RT_RECEIVER_RETAIN_MIN_ROWS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
        {
            config.retain_min_rows = rows.max(0);
        }
        if let Some(hours) = std::env::var("RT_RECEIVER_RETAIN_MIN_HOURS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
        {
            config.retain_min_age_ms = hours.max(0).saturating_mul(60 * 60 * 1000);
        }
        if let Some(secs) = std::env::var("RT_RECEIVER_RETENTION_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            config.interval = Duration::from_secs(secs.max(5));
        }
        config
    }
}

/// The per-stream floor inputs, gathered from the durable store and the live
/// consumer registry. `None` means "no constraint from this component".
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamFloors {
    /// Durable ack cursor (always a constraint).
    pub ack_cursor: i64,
    /// Max DBF-processed seq, bounded by pending rows; `None` when DBF output
    /// is disabled.
    pub dbf_floor: Option<i64>,
    /// Min announcer-unpushed seq minus one; `None` when the announcer is
    /// disabled (or not publishing this stream).
    pub announcer_floor: Option<i64>,
    /// Min active durable-proxy consumer cursor; `None` when no consumer is
    /// connected.
    pub proxy_floor: Option<i64>,
}

/// The delivery low-water mark: nothing at or below it is still needed by any
/// consumer.
pub fn low_water_mark(floors: &StreamFloors) -> i64 {
    let mut mark = floors.ack_cursor;
    for floor in [floors.dbf_floor, floors.announcer_floor, floors.proxy_floor]
        .into_iter()
        .flatten()
    {
        mark = mark.min(floor);
    }
    mark
}

/// Final prune target: the low-water mark bounded by the safety window
/// (`age_boundary_seq` = newest seq older than the retention age,
/// `rows_boundary_seq` = max_seq - retain_min_rows) and by the already-pruned
/// watermark. `None` when there is nothing (new) to prune.
pub fn prune_target(
    floors: &StreamFloors,
    age_boundary_seq: i64,
    rows_boundary_seq: i64,
    pruned_through_seq: i64,
) -> Option<i64> {
    let target = low_water_mark(floors)
        .min(age_boundary_seq)
        .min(rows_boundary_seq);
    (target > pruned_through_seq).then_some(target)
}

/// Registry of live durable-proxy consumer cursors, keyed by stream then by a
/// per-consumer id. Consumers register on connect, update after each drain,
/// and deregister on disconnect (RAII in `local_proxy`).
pub type ProxyConsumerCursors = Arc<std::sync::Mutex<HashMap<String, HashMap<u64, i64>>>>;

/// Min active consumer cursor for `stream_id`, or `None` when no consumer is
/// connected.
pub fn min_proxy_cursor(registry: &ProxyConsumerCursors, stream_id: &str) -> Option<i64> {
    registry
        .lock()
        .expect("proxy cursor registry poisoned")
        .get(stream_id)
        .and_then(|consumers| consumers.values().copied().min())
}

/// Periodic retention worker: computes each subscribed stream's prune target
/// and issues bounded `Prune` commands through the writer.
pub async fn run_retention_worker(
    state: Arc<AppState>,
    config: RetentionConfig,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(config.interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() { break; }
            }
            _ = tick.tick() => {
                if let Err(e) = run_retention_pass(&state, &config).await {
                    warn!(error = %e, "retention pass failed; will retry next tick");
                }
            }
        }
    }
}

async fn run_retention_pass(
    state: &Arc<AppState>,
    config: &RetentionConfig,
) -> Result<(), crate::db::DbError> {
    // Stream list + global toggles from the cold connection (tiny queries).
    // `dbf_active` mirrors the DBF worker's per-stream eligibility rule
    // (details resolvable, reader index <= 9): a stream the DBF worker never
    // delivers must not carry a DBF floor, or its floor pins near 0 and the
    // stream is silently never pruned.
    let (streams, announcer_enabled, announcer_streams) = {
        let db = state.db.lock().await;
        let subs = db.load_stream_subscriptions()?;
        let dbf_enabled = db.load_dbf_config().map(|c| c.enabled).unwrap_or(false);
        let announcer_enabled = db.load_announcer_enabled().unwrap_or(false);
        let announcer_streams = db.load_announcer_publish_streams().unwrap_or_default();
        let streams = subs
            .into_iter()
            .map(|sub| {
                let dbf_active = dbf_enabled
                    && matches!(
                        db.load_subscription_dbf_details(
                            &sub.forwarder_endpoint_id,
                            &sub.stream_id,
                        ),
                        Ok(Some((idx, _))) if idx <= 9
                    );
                (
                    LocalStreamKey::new(&sub.forwarder_endpoint_id, &sub.stream_id),
                    dbf_active,
                )
            })
            .collect::<Vec<_>>();
        (streams, announcer_enabled, announcer_streams)
    };

    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(i64::MAX);
    for (local_stream_key, dbf_active) in streams {
        let stream_id = local_stream_key.as_str().to_owned();
        let announcer_active =
            announcer_enabled && announcer_streams.iter().any(|s| s == &stream_id);
        let proxy_floor = min_proxy_cursor(&state.proxy_consumer_cursors, &stream_id);

        let sid = stream_id.clone();
        let age_cutoff_ms = now_ms - config.retain_min_age_ms;
        let retain_min_rows = config.retain_min_rows;
        let bounds = state
            .read_source
            .run(move |db| {
                let ack_cursor = db.load_stream_cursor(&sid)?;
                let pruned_through = db.load_pruned_through_seq(&sid)?;
                let Some(max_seq) = db.max_stream_seq(&sid)? else {
                    return Ok(None); // nothing stored, nothing to prune
                };
                let dbf_floor = if dbf_active {
                    let processed = db.max_dbf_delivered_seq(&sid)?.unwrap_or(0);
                    let pending_bound = db
                        .min_undelivered_dbf_seq(&sid)?
                        .map(|min_pending| min_pending - 1);
                    Some(pending_bound.map_or(processed, |bound| bound.min(processed)))
                } else {
                    None
                };
                let announcer_floor = if announcer_active {
                    // No unpushed rows => fully caught up => no constraint.
                    db.min_unpushed_announcer_seq(&sid)?.map(|min| min - 1)
                } else {
                    None
                };
                let age_boundary_seq =
                    age_boundary_seq(db, &sid, pruned_through, max_seq, age_cutoff_ms)?;
                let rows_boundary_seq = max_seq - retain_min_rows;
                Ok(Some((
                    StreamFloors {
                        ack_cursor,
                        dbf_floor,
                        announcer_floor,
                        proxy_floor: None, // filled in below (live registry)
                    },
                    age_boundary_seq,
                    rows_boundary_seq,
                    pruned_through,
                )))
            })
            .await?;
        let Some((mut floors, age_boundary_seq, rows_boundary_seq, pruned_through)) = bounds else {
            continue;
        };
        floors.proxy_floor = proxy_floor;

        let Some(target) =
            prune_target(&floors, age_boundary_seq, rows_boundary_seq, pruned_through)
        else {
            continue;
        };
        // Bound the transaction size: prune at most max_rows_per_pass seqs per
        // pass; later passes catch up.
        let bounded_target = target.min(pruned_through + config.max_rows_per_pass);
        match state.writer.prune(stream_id.clone(), bounded_target).await {
            Ok(deleted) => {
                info!(
                    stream_id = %local_stream_key,
                    through_seq = bounded_target,
                    deleted,
                    "pruned received_events behind the delivery low-water mark"
                );
                // Reclaim the freed WAL/db pages opportunistically.
                let _ = state.writer.checkpoint().await;
            }
            Err(e) => warn!(error = %e, stream_id = %local_stream_key, "prune command failed"),
        }
    }
    Ok(())
}

/// Newest seq whose row is older than the retention age, found by a bounded
/// binary search over the seq range using point lookups (received_unix_ms is
/// monotone with seq to within reordering noise; the safety window absorbs
/// the imprecision). Avoids an O(N) table scan per pass.
fn age_boundary_seq(
    db: &crate::db::Db,
    stream_id: &str,
    pruned_through: i64,
    max_seq: i64,
    age_cutoff_ms: i64,
) -> Result<i64, crate::db::DbError> {
    let mut lo = pruned_through; // everything at or below is gone already
    let mut hi = max_seq;
    // Invariant: rows at or below `lo` are older than the cutoff (or pruned);
    // find the highest seq whose received time is still older than cutoff.
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        match db.received_unix_ms_at_or_after(stream_id, mid)? {
            Some((_seq, received_unix_ms)) if received_unix_ms <= age_cutoff_ms => lo = mid,
            Some(_) => hi = mid - 1,
            None => hi = mid - 1,
        }
    }
    Ok(lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floors() -> StreamFloors {
        StreamFloors {
            ack_cursor: 1_000,
            dbf_floor: Some(900),
            announcer_floor: Some(950),
            proxy_floor: Some(800),
        }
    }

    #[test]
    fn each_component_can_hold_the_floor() {
        // Proxy consumer is the lowest.
        assert_eq!(low_water_mark(&floors()), 800);

        // DBF floor holds when it is the minimum.
        let f = StreamFloors {
            proxy_floor: Some(2_000),
            ..floors()
        };
        assert_eq!(low_water_mark(&f), 900);

        // Announcer floor holds.
        let f = StreamFloors {
            dbf_floor: None,
            proxy_floor: None,
            ..floors()
        };
        assert_eq!(low_water_mark(&f), 950);

        // Ack cursor holds when everything else is unconstrained.
        let f = StreamFloors {
            dbf_floor: None,
            announcer_floor: None,
            proxy_floor: None,
            ..floors()
        };
        assert_eq!(low_water_mark(&f), 1_000);
    }

    #[test]
    fn consumer_at_seq_zero_blocks_pruning() {
        let f = StreamFloors {
            proxy_floor: Some(0),
            ..floors()
        };
        assert_eq!(low_water_mark(&f), 0);
        assert_eq!(
            prune_target(&f, i64::MAX, i64::MAX, 0),
            None,
            "an existing consumer still at seq 0 must block pruning entirely"
        );
    }

    #[test]
    fn prune_respects_safety_window() {
        let f = StreamFloors {
            ack_cursor: 500_000,
            dbf_floor: None,
            announcer_floor: None,
            proxy_floor: None,
        };
        // Rows boundary keeps 100k rows even though 500k are acked.
        assert_eq!(prune_target(&f, i64::MAX, 400_000, 0), Some(400_000));
        // Age boundary can be stricter than the rows boundary.
        assert_eq!(prune_target(&f, 350_000, 400_000, 0), Some(350_000));
        // Nothing new below the watermark.
        assert_eq!(prune_target(&f, 350_000, 400_000, 350_000), None);
    }

    #[tokio::test]
    async fn retention_pass_prunes_canonical_keyed_rows_end_to_end() {
        // Full pipeline: floor gathering via the read pool + prune through
        // the writer, against a real temp-file DB. Rows are stored under the
        // receiver-local canonical key; the subscription boundary still carries
        // the wire stream id.
        let (state, _shutdown_rx, _dir) = crate::control_api::AppState::new_for_test();
        let endpoint_id = "fwd-ret";
        let wire_stream_id = "127.0.0.1:10900";
        let local_stream_key = crate::stream_key::LocalStreamKey::new(endpoint_id, wire_stream_id);
        {
            let mut db = state.db.lock().await;
            db.save_profile("http://server", "tok", "check-and-download", None)
                .unwrap();
            db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
                forwarder_endpoint_id: endpoint_id.to_owned(),
                stream_id: wire_stream_id.to_owned(),
                local_port_override: None,
                event_type: crate::db::EventType::Finish,
                forwarder_id: None,
                reader_ip: Some(wire_stream_id.to_owned()),
            }])
            .unwrap();
            db.save_dbf_config(&crate::db::DbfConfig {
                enabled: true,
                flush_interval_ms: crate::db::DEFAULT_DBF_FLUSH_INTERVAL_MS,
            })
            .unwrap();
            db.set_announcer_enabled(true).unwrap();
            db.set_stream_announcer_publish(local_stream_key.as_str(), true)
                .unwrap();
            for seq in 1..=10 {
                db.insert_received_event(&crate::db::ReceivedEventInsert {
                    stream_id: local_stream_key.as_str(),
                    seq,
                    epoch: 1,
                    raw_frame: b"frame",
                    read_kind: "chip",
                    reader_timestamp: None,
                    received_unix_ms: 1_700_000_000_000 + seq,
                    dbf_delivered_unix_ms: None,
                    chip_id: None,
                })
                .unwrap();
            }
            db.jump_stream_cursor(local_stream_key.as_str(), 10)
                .unwrap();
            db.mark_dbf_delivered_batch(
                local_stream_key.as_str(),
                &(1..=10).collect::<Vec<_>>(),
                1_700_000_010_000,
            )
            .unwrap();
            db.mark_announcer_pushed_batch(
                local_stream_key.as_str(),
                &(1..=9).collect::<Vec<_>>(),
                1_700_000_010_000,
            )
            .unwrap();
        }
        {
            let mut cursors = state.proxy_consumer_cursors.lock().unwrap();
            let _ = cursors
                .entry(local_stream_key.as_str().to_owned())
                .or_default()
                .insert(1, 10);
        }

        // All floors (ack, DBF, announcer, proxy) are past the target; the
        // safety window keeps 3 rows, and no age window applies.
        let config = RetentionConfig {
            retain_min_rows: 3,
            retain_min_age_ms: 0,
            interval: Duration::from_secs(60),
            max_rows_per_pass: 20_000,
        };
        run_retention_pass(&state, &config).await.unwrap();

        let db = state.db.lock().await;
        let remaining: Vec<i64> = db
            .load_received_events(local_stream_key.as_str())
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(remaining, vec![8, 9, 10], "safety window keeps 3 rows");
        assert_eq!(
            db.load_pruned_through_seq(local_stream_key.as_str())
                .unwrap(),
            7
        );
        assert_eq!(
            db.load_stream_cursor(local_stream_key.as_str()).unwrap(),
            10,
            "pruning never touches the ack cursor"
        );
        assert!(
            db.load_received_events(wire_stream_id).unwrap().is_empty(),
            "retention must not create or prune a bare wire-id stream"
        );
    }

    #[test]
    fn min_proxy_cursor_tracks_the_slowest_consumer() {
        let registry: ProxyConsumerCursors = Arc::default();
        assert_eq!(min_proxy_cursor(&registry, "s1"), None);
        {
            let mut map = registry.lock().unwrap();
            let consumers = map.entry("s1".to_owned()).or_default();
            let _ = consumers.insert(1, 500);
            let _ = consumers.insert(2, 120);
        }
        assert_eq!(min_proxy_cursor(&registry, "s1"), Some(120));
        assert_eq!(min_proxy_cursor(&registry, "other"), None);
    }
}

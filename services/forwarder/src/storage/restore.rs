//! Startup stream-identity restore from the server registry high-water.
//!
//! Receivers deduplicate durable events by `(stream_id, seq)`, and the stream
//! id is the reader network address (`ip:port`). If the local journal file is
//! lost/recreated, the same stream key would restart at seq 1 and receivers
//! would silently discard the new reads as duplicates. Before reader tasks (or
//! the P2P catalog seeding) touch stream state, startup checks each enabled
//! stream key and — for keys missing from the journal — restores
//! `epoch`/`next_seq` from the server's stored catalog
//! (`GET /forwarder/catalog`), with fixed slack added on top.
//!
//! Known design gap (accepted): with no coordination server configured there
//! is no durable high-water source, so journal loss on a server-less
//! deployment still risks seq reuse — the loud log is the mitigation. Even
//! with a server, the restored high-water can lag the lost journal by up to
//! one catalog-push interval; [`RESTORE_SEQ_SLACK`] is the mitigation for
//! that.

use std::time::Duration;

use rt_ui_log::{UiLogLevel, UiLogger};

use crate::storage::journal::{Journal, JournalError, RegistryStreamRestore};
use crate::ui_events::ForwarderUiEvent;

/// Fixed slack added on top of the server-restored `next_seq`.
///
/// The server snapshot can lag the lost journal by up to one catalog-push
/// interval (30s), so the true high-water may be above the restored value.
/// Sequence *gaps* are benign under at-least-once delivery with
/// `(stream_id, seq)` dedup (receivers already tolerate gaps from retention
/// pruning); sequence *reuse* silently drops reads. So we always over-shoot.
pub const RESTORE_SEQ_SLACK: i64 = 100_000;

/// One stream record from the server's stored catalog for this forwarder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryStreamRecord {
    pub stream_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

/// Outcome of trying to fetch this forwarder's registry catalog snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryFetch {
    /// The server responded with this forwarder's stored catalog (possibly
    /// empty). Streams absent from the snapshot are first boots.
    Snapshot(Vec<RegistryStreamRecord>),
    /// No server is configured — there is no durable high-water source at all
    /// (accepted design gap; see module docs).
    NotConfigured,
    /// The server was unreachable or errored after bounded retries. This
    /// includes 404 from an older server without `GET /forwarder/catalog` and
    /// a missing device token.
    Unavailable,
}

/// Per-stream decision made by [`restore_streams_at_startup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRestoreOutcome {
    /// The journal already has state for this stream; nothing to do.
    Existing,
    /// Missing locally but present in the registry snapshot: seeded with the
    /// server high-water plus [`RESTORE_SEQ_SLACK`].
    Restored { epoch: i64, next_seq: i64 },
    /// Missing locally and absent from a successfully fetched snapshot:
    /// expected first boot, seeded at seq 1.
    SeededFirstBoot,
    /// Missing locally with no usable registry data: seeded at seq 1 with a
    /// loud warning — if this host previously forwarded this stream, receiver
    /// dedup may silently discard the new reads.
    SeededWithoutRegistry,
}

/// Fetch the registry snapshot with bounded retries.
///
/// Calls `fetch` up to `attempts` times, sleeping `retry_delay` between
/// attempts (injectable so tests run instantly). Returns
/// [`RegistryFetch::Unavailable`] once every attempt has failed; never blocks
/// startup indefinitely.
pub async fn fetch_registry_snapshot_with_retries<F, Fut, E>(
    fetch: F,
    attempts: u32,
    retry_delay: Duration,
) -> RegistryFetch
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<Vec<RegistryStreamRecord>, E>>,
    E: std::fmt::Display,
{
    for attempt in 1..=attempts {
        match fetch().await {
            Ok(streams) => return RegistryFetch::Snapshot(streams),
            Err(error) => {
                tracing::warn!(
                    %error,
                    attempt,
                    attempts,
                    "registry catalog fetch for stream restore failed"
                );
            }
        }
        if attempt < attempts {
            tokio::time::sleep(retry_delay).await;
        }
    }
    RegistryFetch::Unavailable
}

/// Seed journal stream state for every `stream_keys` entry missing from the
/// journal, restoring epoch/next_seq from `fetch` where available.
///
/// Runs BEFORE reader tasks spawn and before `start_forwarder_p2p` (whose
/// idempotent `ensure_stream_state(stream, 1)` seeding would otherwise
/// pre-empt a restore with seq 1). Existing streams are never touched.
///
/// Case (c) (`Unavailable`) emits an `error!` plus a UI log entry via
/// `logger`; case (b) (snapshot without the stream) is an expected first boot
/// and logs at info.
pub fn restore_streams_at_startup(
    journal: &mut Journal,
    stream_keys: &[String],
    fetch: &RegistryFetch,
    logger: Option<&UiLogger<ForwarderUiEvent>>,
) -> Result<Vec<(String, StreamRestoreOutcome)>, JournalError> {
    let mut outcomes = Vec::with_capacity(stream_keys.len());
    for key in stream_keys {
        let outcome = restore_one_stream(journal, key, fetch, logger)?;
        outcomes.push((key.clone(), outcome));
    }
    Ok(outcomes)
}

fn restore_one_stream(
    journal: &mut Journal,
    key: &str,
    fetch: &RegistryFetch,
    logger: Option<&UiLogger<ForwarderUiEvent>>,
) -> Result<StreamRestoreOutcome, JournalError> {
    if journal.stream_exists(key)? {
        return Ok(StreamRestoreOutcome::Existing);
    }

    if let RegistryFetch::Snapshot(records) = fetch
        && let Some(record) = records.iter().find(|r| r.stream_id == key)
    {
        // The server validates epoch/next_seq fit in i64 on push; clamp
        // defensively (a floor of 1 satisfies the journal's seed validation,
        // saturation keeps a corrupt huge value from wrapping).
        let epoch = i64::try_from(record.epoch).unwrap_or(i64::MAX).max(1);
        let next_seq = i64::try_from(record.next_seq)
            .unwrap_or(i64::MAX)
            .max(1)
            .saturating_add(RESTORE_SEQ_SLACK);
        journal.ensure_stream_after_startup(
            key,
            Some(key),
            key,
            1,
            Some(RegistryStreamRestore {
                stream_id: key,
                epoch,
                next_seq,
            }),
        )?;
        tracing::info!(
            stream = %key,
            epoch,
            next_seq,
            registry_next_seq = record.next_seq,
            slack = RESTORE_SEQ_SLACK,
            "restored stream identity from server registry high-water"
        );
        if let Some(logger) = logger {
            logger.log(format!(
                "stream {key}: restored epoch {epoch} / next seq {next_seq} from server registry"
            ));
        }
        return Ok(StreamRestoreOutcome::Restored { epoch, next_seq });
    }

    // No registry record for this stream. `ensure_stream_after_startup` errors
    // when prior state is missing, no restore is provided, and prior_stream_id
    // equals new_stream_id — the stream key IS the reader address here, so the
    // fallback paths seed via plain `ensure_stream_state` instead.
    journal.ensure_stream_state(key, 1)?;

    match fetch {
        RegistryFetch::Snapshot(_) => {
            tracing::info!(
                stream = %key,
                "no server registry record for stream; seeding fresh at seq 1 (expected first boot)"
            );
            if let Some(logger) = logger {
                logger.log(format!("stream {key}: first boot, starting at seq 1"));
            }
            Ok(StreamRestoreOutcome::SeededFirstBoot)
        }
        RegistryFetch::NotConfigured => {
            tracing::warn!(
                stream = %key,
                "no coordination server configured: stream seeded at seq 1 with no durable \
                 high-water source; if this host previously forwarded this stream, receiver \
                 dedup may silently discard its reads"
            );
            if let Some(logger) = logger {
                logger.log_at(
                    UiLogLevel::Warn,
                    format!(
                        "stream {key}: seeded at seq 1 without a server high-water source; if \
                         this host previously forwarded this stream, receivers may discard its \
                         reads as duplicates"
                    ),
                );
            }
            Ok(StreamRestoreOutcome::SeededWithoutRegistry)
        }
        RegistryFetch::Unavailable => {
            tracing::error!(
                stream = %key,
                "server registry unavailable during stream restore: stream seeded at seq 1; if \
                 this host previously forwarded this stream, receiver dedup may silently \
                 discard its reads"
            );
            if let Some(logger) = logger {
                logger.log_at(
                    UiLogLevel::Error,
                    format!(
                        "stream {key}: server registry unavailable, seeded at seq 1 — if this \
                         host previously forwarded this stream, receivers may silently discard \
                         its reads as duplicates"
                    ),
                );
            }
            Ok(StreamRestoreOutcome::SeededWithoutRegistry)
        }
    }
}

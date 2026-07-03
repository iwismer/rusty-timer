//! Forwarder data-stream subscriber delivery.
//!
//! Each inbound data stream is served independently from the durable journal:
//! subscribers replay records via [`ReplayEngine::read_after`] and then wait on
//! the journal wake registry for live records. Acknowledgements update the
//! receiver cursor table that retention uses as its floor.

use std::sync::Arc;

use rt_iroh::{Connection, RecvStream, SendStream};
use rt_p2p_protocol::{
    Ack, DataC2F, DataF2C, EventBatch, ReadRecord, StreamEpochStarted, SubscribeMode, SubscribeOk,
    data_c2f, data_f2c,
};
use tokio::sync::{Mutex, mpsc};
use tokio::task::{JoinHandle, JoinSet};

use crate::replay::ReplayEngine;
use crate::storage::journal::{Journal, JournalEvent};

use super::control::{read_frame, write_frame};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Aborts the wrapped task when dropped.
///
/// The ack-reader task borrows the data stream's read half for the lifetime of
/// the subscription. Wrapping its handle in this guard guarantees it is aborted
/// on every `serve_data_stream` return path (including the many `?` early
/// returns), mirroring the explicit `reader.abort()` the control heartbeat does.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runtime knobs for data-stream delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataConfig {
    /// Maximum journal records sent in one [`EventBatch`].
    pub max_events_per_batch: usize,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            max_events_per_batch: 256,
        }
    }
}

/// Accepts inbound data streams on `connection` and serves each on its own task.
///
/// Per-stream task isolation is intentional: a receiver whose QUIC flow-control
/// window is exhausted can block only its own codec write, not other data
/// streams on the same connection.
///
/// All spawned per-stream tasks are tracked in a [`JoinSet`] that is aborted and
/// drained before this function returns. The accept loop ends when the QUIC
/// connection closes (the control plane closes it on any control-loop
/// termination), so closing the connection both stops admitting new data
/// streams and tears down every in-flight one before the allow-list connection
/// guard is dropped.
pub async fn serve_data_streams(
    connection: Connection,
    journal: Arc<Mutex<Journal>>,
    receiver_id: String,
    config: DataConfig,
) -> Result<(), BoxError> {
    let mut tasks: JoinSet<()> = JoinSet::new();
    let result = loop {
        let (send, recv) = match connection.accept_bi().await {
            Ok(streams) => streams,
            Err(error) => break Err(Box::new(error) as BoxError),
        };
        // Reap finished tasks so the set does not grow unbounded over a
        // long-lived connection.
        while tasks.try_join_next().is_some() {}
        let journal = Arc::clone(&journal);
        let receiver_id = receiver_id.clone();
        tasks.spawn(async move {
            if let Err(error) = serve_data_stream(send, recv, journal, receiver_id, config).await {
                tracing::warn!(%error, "p2p: data stream failed");
            }
        });
    };
    // Ensure no per-stream task outlives the accept loop (and thus the
    // allow-list/revocation guard held by the caller).
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    result
}

async fn serve_data_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    journal: Arc<Mutex<Journal>>,
    receiver_id: String,
    config: DataConfig,
) -> Result<(), BoxError> {
    let first = read_frame::<DataC2F>(&mut recv).await?;
    let subscribe = match first.msg {
        Some(data_c2f::Msg::DataSubscribe(subscribe)) => subscribe,
        other => {
            return Err(
                format!("expected DataSubscribe as first data frame, got {other:?}").into(),
            );
        }
    };

    let stream_key = wire_stream_key(&subscribe.stream_id)?;
    let stream_id = subscribe.stream_id.clone();
    let max = config.max_events_per_batch.max(1);
    let (earliest, latest_at_open) = {
        let journal = journal.lock().await;
        (
            journal.retention_state(&stream_key)?.earliest_available_seq,
            journal.latest_committed_seq(&stream_key)?,
        )
    };
    // Only `Unspecified` falls back to replay; an unknown enum value is a
    // protocol violation and fails the stream rather than silently replaying.
    let mode = SubscribeMode::try_from(subscribe.mode)
        .map_err(|_| -> BoxError { format!("unknown subscribe mode {}", subscribe.mode).into() })?;
    let after = i64_from_u64(subscribe.after_seq)?;
    let mut cursor = match mode {
        SubscribeMode::Live => latest_at_open,
        SubscribeMode::Replay | SubscribeMode::Unspecified => {
            // Reject a client-controlled cursor past the journal tail: a future
            // `after_seq` must never become a trusted high-water for retention
            // or next-seq allocation.
            if after > latest_at_open {
                return Err(format!(
                    "replay after_seq {after} exceeds latest_seq_at_open {latest_at_open}"
                )
                .into());
            }
            after
        }
    };

    write_frame(
        &mut send,
        &DataF2C {
            msg: Some(data_f2c::Msg::SubscribeOk(SubscribeOk {
                stream_id: stream_id.clone(),
                earliest_available_seq: u64_from_i64(earliest)?,
                latest_seq_at_open: u64_from_i64(latest_at_open)?,
            })),
        },
    )
    .await?;

    let (ack_tx, mut ack_rx) = mpsc::channel::<Ack>(32);
    let _ack_reader = AbortOnDrop(tokio::spawn(async move {
        while let Ok(frame) = read_frame::<DataC2F>(&mut recv).await {
            if let Some(data_c2f::Msg::Ack(ack)) = frame.msg
                && ack_tx.send(ack).await.is_err()
            {
                break;
            }
        }
    }));

    let wake_registry = { journal.lock().await.wake_registry() };
    let mut wake = wake_registry.subscribe(&stream_key);
    let replay = ReplayEngine::new();
    let mut last_epoch: Option<u64> = None;
    let mut caught_up_sent_at: Option<i64> = None;
    let mut ack_rx_open = true;

    loop {
        wake.mark_unchanged();
        // `cursor` is the high-water of everything delivered or explicitly
        // skipped (via a gap jump), so acks at or below it are legitimate.
        drain_acks(
            &journal,
            &receiver_id,
            &stream_id,
            cursor,
            &mut ack_rx,
            &mut ack_rx_open,
        )
        .await?;

        let batch = {
            let journal = journal.lock().await;
            replay.read_after(&journal, &stream_key, cursor, max)?
        };

        if let Some(gap) = batch.gap {
            write_frame(
                &mut send,
                &DataF2C {
                    msg: Some(data_f2c::Msg::GapNotice(rt_p2p_protocol::GapNotice {
                        stream_id: stream_id.clone(),
                        requested_after_seq: u64_from_i64(gap.requested_cursor)?,
                        earliest_available_seq: u64_from_i64(gap.earliest)?,
                        latest_available_seq: u64_from_i64(gap.latest)?,
                        reason: "retention_gap".to_owned(),
                    })),
                },
            )
            .await?;
            cursor = gap.earliest.saturating_sub(1);
            caught_up_sent_at = None;
            continue;
        }

        if batch.records.is_empty() {
            if caught_up_sent_at != Some(cursor) {
                write_frame(
                    &mut send,
                    &DataF2C {
                        msg: Some(data_f2c::Msg::CaughtUp(rt_p2p_protocol::CaughtUp {
                            stream_id: stream_id.clone(),
                            through_seq: u64_from_i64(cursor)?,
                        })),
                    },
                )
                .await?;
                caught_up_sent_at = Some(cursor);
            }

            if ack_rx_open {
                tokio::select! {
                    changed = wake.changed() => {
                        changed?;
                    }
                    maybe_ack = ack_rx.recv() => {
                        match maybe_ack {
                            Some(ack) => {
                                process_ack(&journal, &receiver_id, &stream_id, cursor, ack)
                                    .await?;
                            }
                            None => ack_rx_open = false,
                        }
                    }
                }
            } else {
                wake.changed().await?;
            }
            continue;
        }

        caught_up_sent_at = None;
        for segment in split_segments(&batch.records, latest_at_open) {
            let epoch = u64_from_i64(segment[0].stream_epoch)?;
            if last_epoch != Some(epoch) {
                write_frame(
                    &mut send,
                    &DataF2C {
                        msg: Some(data_f2c::Msg::StreamEpochStarted(StreamEpochStarted {
                            stream_id: stream_id.clone(),
                            epoch,
                            start_seq: u64_from_i64(segment[0].seq)?,
                            reason: "epoch_started".to_owned(),
                        })),
                    },
                )
                .await?;
                last_epoch = Some(epoch);
            }

            let records = segment
                .iter()
                .map(|event| to_read_record(event, &stream_id))
                .collect::<Result<Vec<_>, _>>()?;
            // Each segment is uniformly replay or live (split_segments breaks at
            // the latest_seq_at_open boundary), so the first seq decides the flag
            // and a batch never mixes replay and live records.
            let replay_segment = segment[0].seq <= latest_at_open;
            write_frame(
                &mut send,
                &DataF2C {
                    msg: Some(data_f2c::Msg::EventBatch(EventBatch {
                        records,
                        replay: replay_segment,
                    })),
                },
            )
            .await?;
            cursor = segment.last().expect("non-empty segment").seq;
        }
    }
}

async fn drain_acks(
    journal: &Mutex<Journal>,
    receiver_id: &str,
    subscribed_stream_id: &[u8],
    delivered_through: i64,
    ack_rx: &mut mpsc::Receiver<Ack>,
    ack_rx_open: &mut bool,
) -> Result<(), BoxError> {
    if !*ack_rx_open {
        return Ok(());
    }

    loop {
        match ack_rx.try_recv() {
            Ok(ack) => {
                process_ack(
                    journal,
                    receiver_id,
                    subscribed_stream_id,
                    delivered_through,
                    ack,
                )
                .await?;
            }
            Err(mpsc::error::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                *ack_rx_open = false;
                return Ok(());
            }
        }
    }
}

async fn process_ack(
    journal: &Mutex<Journal>,
    receiver_id: &str,
    subscribed_stream_id: &[u8],
    delivered_through: i64,
    ack: Ack,
) -> Result<(), BoxError> {
    // Ignore acks for any stream other than the one this task serves: a single
    // subscription must not be able to move another stream's cursor.
    if ack.stream_id.as_slice() != subscribed_stream_id {
        tracing::warn!("p2p: ignoring ack for unsubscribed stream");
        return Ok(());
    }

    let through_seq = i64_from_u64(ack.through_seq)?;
    // Ignore acks past what we have actually delivered or explicitly skipped, so
    // a client cannot persist a false high-water cursor.
    if through_seq > delivered_through {
        tracing::warn!(
            through_seq,
            delivered_through,
            "p2p: ignoring ack beyond delivered cursor"
        );
        return Ok(());
    }

    let stream_key = wire_stream_key(&ack.stream_id)?;
    journal
        .lock()
        .await
        .update_receiver_stream_cursor(receiver_id, &stream_key, through_seq)?;
    Ok(())
}

/// Split a seq-ordered slice into segments that are uniform in both epoch and
/// replay/live classification.
///
/// A segment break is inserted whenever the epoch changes or the
/// `latest_seq_at_open` replay/live boundary is crossed. Because records are
/// ordered by ascending seq, each resulting segment is entirely replay
/// (`seq <= latest_at_open`) or entirely live, so an `EventBatch` never mixes
/// the two under a single `replay` flag.
fn split_segments(events: &[JournalEvent], latest_at_open: i64) -> Vec<&[JournalEvent]> {
    let mut segments = Vec::new();
    if events.is_empty() {
        return segments;
    }

    let mut start = 0;
    for idx in 1..events.len() {
        let epoch_changed = events[idx].stream_epoch != events[start].stream_epoch;
        let replay_changed =
            (events[idx].seq <= latest_at_open) != (events[start].seq <= latest_at_open);
        if epoch_changed || replay_changed {
            segments.push(&events[start..idx]);
            start = idx;
        }
    }
    segments.push(&events[start..]);
    segments
}

fn to_read_record(event: &JournalEvent, stream_id: &[u8]) -> Result<ReadRecord, BoxError> {
    Ok(ReadRecord {
        stream_id: stream_id.to_vec(),
        seq: u64_from_i64(event.seq)?,
        epoch: u64_from_i64(event.stream_epoch)?,
        raw_frame: event.raw_frame.clone(),
        read_kind: event.read_type.clone(),
        reader_timestamp: event
            .reader_timestamp
            .as_deref()
            .and_then(|timestamp| timestamp.parse::<i64>().ok())
            .unwrap_or_default(),
        received_unix_ms: event.received_at.parse::<i64>().unwrap_or_default(),
    })
}

/// Resolves a wire `stream_id` to its durable journal stream key.
///
/// The forwarder journal keys reader streams by their network address (for
/// example `10.0.0.5:10000`) and the control catalog advertises those same
/// UTF-8 address bytes as the `stream_id`, so a `DataSubscribe.stream_id` is
/// always the UTF-8 journal key. This is decoded unconditionally: a
/// length-based UUID heuristic would misroute reader addresses that happen to
/// be exactly 16 bytes (e.g. `100.64.0.1:10000`).
fn wire_stream_key(stream_id: &[u8]) -> Result<String, BoxError> {
    Ok(std::str::from_utf8(stream_id)?.to_owned())
}

fn i64_from_u64(value: u64) -> Result<i64, BoxError> {
    i64::try_from(value).map_err(|_| format!("sequence {value} exceeds i64::MAX").into())
}

fn u64_from_i64(value: i64) -> Result<u64, BoxError> {
    u64::try_from(value).map_err(|_| format!("negative sequence {value}").into())
}

#[cfg(test)]
mod tests {
    use super::{DataConfig, serve_data_streams, split_segments};
    use crate::p2p::control::{read_frame, write_frame};
    use crate::storage::journal::{Journal, JournalEvent, RetentionContext, RetentionPolicy};
    use rt_iroh::{Endpoint, EndpointAddr, EndpointBuilder};
    use rt_p2p_protocol::{
        Ack, CaughtUp, DataC2F, DataF2C, DataSubscribe, EventBatch, SubscribeMode, SubscribeOk,
        data_c2f, data_f2c,
    };
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;

    type BoxError = Box<dyn std::error::Error + Send + Sync>;
    type TestResult<T = ()> = Result<T, BoxError>;

    /// Primary stream key: a reader network address, exactly as the journal and
    /// catalog represent stream ids in production.
    const STREAM_KEY: &str = "10.0.0.5:10000";
    /// Secondary stream key used by tests that need a second, distinct stream.
    const OTHER_STREAM_KEY: &str = "10.0.0.6:10000";
    const RECEIVER_ID: &str = "receiver-a";

    struct Harness {
        _dir: TempDir,
        journal: Arc<Mutex<Journal>>,
        _forwarder: Endpoint,
        receiver: Endpoint,
        forwarder_addr: EndpointAddr,
        _server: JoinHandle<()>,
    }

    fn stream_id() -> Vec<u8> {
        STREAM_KEY.as_bytes().to_vec()
    }

    fn stream_key() -> String {
        STREAM_KEY.to_owned()
    }

    async fn start_harness(config: DataConfig) -> TestResult<Harness> {
        let dir = tempfile::tempdir()?;
        let journal_path = dir.path().join("journal.db");
        let journal = Arc::new(Mutex::new(Journal::open(&journal_path)?));
        let forwarder = EndpointBuilder::test([91; 32]).bind().await?;
        let receiver = EndpointBuilder::test([92; 32]).bind().await?;
        let forwarder_addr = forwarder.endpoint_addr().await;
        let accept = forwarder.clone();
        let journal_for_task = Arc::clone(&journal);
        let server = tokio::spawn(async move {
            if let Ok(Some(connection)) = accept.accept().await {
                let _ = serve_data_streams(
                    connection,
                    journal_for_task,
                    RECEIVER_ID.to_owned(),
                    config,
                )
                .await;
            }
        });

        Ok(Harness {
            _dir: dir,
            journal,
            _forwarder: forwarder,
            receiver,
            forwarder_addr,
            _server: server,
        })
    }

    async fn open_subscription(
        harness: &Harness,
        after_seq: u64,
    ) -> TestResult<(rt_iroh::SendStream, rt_iroh::RecvStream, SubscribeOk)> {
        open_subscription_for_stream(harness, stream_id(), after_seq).await
    }

    async fn open_subscription_for_stream(
        harness: &Harness,
        stream_id: Vec<u8>,
        after_seq: u64,
    ) -> TestResult<(rt_iroh::SendStream, rt_iroh::RecvStream, SubscribeOk)> {
        let connection = harness
            .receiver
            .connect(harness.forwarder_addr.clone())
            .await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id,
                    after_seq,
                    mode: SubscribeMode::Replay as i32,
                })),
            },
        )
        .await?;

        let frame = read_frame::<DataF2C>(&mut recv).await?;
        let subscribe_ok = match frame.msg {
            Some(data_f2c::Msg::SubscribeOk(ok)) => ok,
            other => return Err(format!("expected SubscribeOk, got {other:?}").into()),
        };
        Ok((send, recv, subscribe_ok))
    }

    /// Send a `DataSubscribe` with arbitrary `after_seq`/`mode` and assert the
    /// forwarder fails the stream instead of replying with `SubscribeOk`.
    async fn subscribe_expecting_failure(
        harness: &Harness,
        after_seq: u64,
        mode: i32,
    ) -> TestResult<()> {
        let connection = harness
            .receiver
            .connect(harness.forwarder_addr.clone())
            .await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id: stream_id(),
                    after_seq,
                    mode,
                })),
            },
        )
        .await?;

        match tokio::time::timeout(Duration::from_secs(5), read_frame::<DataF2C>(&mut recv)).await?
        {
            Ok(frame) => Err(format!("expected stream failure, got {:?}", frame.msg).into()),
            Err(_) => Ok(()),
        }
    }

    /// Repeatedly prune until `stream`'s retention floor advances past 1, then
    /// return the floor it settled on.
    async fn prune_until_floor_moves(harness: &Harness, stream: &str) -> TestResult<i64> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut journal = harness.journal.lock().await;
                journal.prune_retention(
                    &RetentionPolicy {
                        min_retention_ms: 0,
                        max_retention_ms: i64::MAX,
                        emergency_free_disk_bytes: 0,
                        emergency_max_rows: i64::MAX,
                    },
                    RetentionContext {
                        now_unix_ms: i64::MAX,
                        free_disk_bytes: u64::MAX,
                    },
                )?;
                let floor = journal.retention_state(stream)?.earliest_available_seq;
                if floor > 1 {
                    break Ok::<i64, BoxError>(floor);
                }
                drop(journal);
                tokio::task::yield_now().await;
            }
        })
        .await?
    }

    async fn read_next(recv: &mut rt_iroh::RecvStream) -> TestResult<DataF2C> {
        tokio::time::timeout(Duration::from_secs(5), read_frame::<DataF2C>(recv)).await?
    }

    async fn read_batch(recv: &mut rt_iroh::RecvStream) -> TestResult<EventBatch> {
        loop {
            match read_next(recv).await?.msg {
                Some(data_f2c::Msg::EventBatch(batch)) => return Ok(batch),
                Some(data_f2c::Msg::StreamEpochStarted(_)) => {}
                other => return Err(format!("expected EventBatch, got {other:?}").into()),
            }
        }
    }

    async fn read_caught_up(recv: &mut rt_iroh::RecvStream) -> TestResult<CaughtUp> {
        loop {
            match read_next(recv).await?.msg {
                Some(data_f2c::Msg::CaughtUp(caught_up)) => return Ok(caught_up),
                Some(data_f2c::Msg::StreamEpochStarted(_)) => {}
                other => return Err(format!("expected CaughtUp, got {other:?}").into()),
            }
        }
    }

    #[tokio::test]
    async fn exactly_16_byte_address_routes_to_journal_stream() -> TestResult {
        // `100.64.0.1:10000` is exactly 16 bytes; a UUID length heuristic would
        // misroute it. The address must resolve to its journal stream key.
        let harness = start_harness(DataConfig::default()).await?;
        let address_key = "100.64.0.1:10000";
        assert_eq!(address_key.len(), 16);
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(address_key, 1)?;
            journal.append_read(address_key, Some("1000"), b"addr-record", "chip")?;
        }

        let (_send, mut recv, subscribe_ok) =
            open_subscription_for_stream(&harness, address_key.as_bytes().to_vec(), 0).await?;
        assert_eq!(subscribe_ok.earliest_available_seq, 1);
        assert_eq!(subscribe_ok.latest_seq_at_open, 1);
        let batch = read_batch(&mut recv).await?;
        assert_eq!(batch.records[0].raw_frame, b"addr-record");
        Ok(())
    }

    #[tokio::test]
    async fn subscribe_from_zero_replays_then_lives() -> TestResult {
        let harness = start_harness(DataConfig::default()).await?;
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            journal.append_read(&stream_key(), Some("1000"), b"replay-1", "chip")?;
            journal.append_read(&stream_key(), Some("1001"), b"replay-2", "chip")?;
        }

        let (_send, mut recv, subscribe_ok) = open_subscription(&harness, 0).await?;
        assert_eq!(subscribe_ok.earliest_available_seq, 1);
        assert_eq!(subscribe_ok.latest_seq_at_open, 2);

        let replay = read_batch(&mut recv).await?;
        assert!(replay.replay);
        assert_eq!(
            replay.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(replay.records[0].raw_frame, b"replay-1");

        let caught_up = read_caught_up(&mut recv).await?;
        assert_eq!(caught_up.through_seq, 2);

        {
            let mut journal = harness.journal.lock().await;
            journal.append_read(&stream_key(), Some("1002"), b"live-3", "chip")?;
        }
        let live = read_batch(&mut recv).await?;
        assert!(!live.replay);
        assert_eq!(live.records.len(), 1);
        assert_eq!(live.records[0].seq, 3);
        assert_eq!(live.records[0].raw_frame, b"live-3");
        Ok(())
    }

    #[tokio::test]
    async fn ack_advances_retention_floor() -> TestResult {
        let harness = start_harness(DataConfig::default()).await?;
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            for seq in 1..=3 {
                let frame = format!("frame-{seq}");
                journal.append_read(&stream_key(), None, frame.as_bytes(), "chip")?;
            }
        }

        let (mut send, mut recv, _ok) = open_subscription(&harness, 0).await?;
        let _batch = read_batch(&mut recv).await?;
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::Ack(Ack {
                    stream_id: stream_id(),
                    through_seq: 3,
                })),
            },
        )
        .await?;

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut journal = harness.journal.lock().await;
                let stats = journal.prune_retention(
                    &RetentionPolicy {
                        min_retention_ms: 0,
                        max_retention_ms: i64::MAX,
                        emergency_free_disk_bytes: 0,
                        emergency_max_rows: i64::MAX,
                    },
                    RetentionContext {
                        now_unix_ms: i64::MAX,
                        free_disk_bytes: u64::MAX,
                    },
                )?;
                if stats.acked_deleted == 3 {
                    let retention = journal.retention_state(&stream_key())?;
                    assert_eq!(retention.earliest_available_seq, 4);
                    break Ok::<(), BoxError>(());
                }
                drop(journal);
                tokio::task::yield_now().await;
            }
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn gap_notice_on_pruned_cursor() -> TestResult {
        let harness = start_harness(DataConfig::default()).await?;
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            for seq in 1..=3 {
                let frame = format!("frame-{seq}");
                journal.append_read(&stream_key(), None, frame.as_bytes(), "chip")?;
            }
            journal.update_receiver_stream_cursor("other", &stream_key(), 2)?;
            journal.prune_retention(
                &RetentionPolicy {
                    min_retention_ms: 0,
                    max_retention_ms: i64::MAX,
                    emergency_free_disk_bytes: 0,
                    emergency_max_rows: i64::MAX,
                },
                RetentionContext {
                    now_unix_ms: i64::MAX,
                    free_disk_bytes: u64::MAX,
                },
            )?;
        }

        let (_send, mut recv, _ok) = open_subscription(&harness, 0).await?;
        let gap = match read_next(&mut recv).await?.msg {
            Some(data_f2c::Msg::GapNotice(gap)) => gap,
            other => return Err(format!("expected GapNotice, got {other:?}").into()),
        };
        assert_eq!(gap.requested_after_seq, 0);
        assert_eq!(gap.earliest_available_seq, 3);
        assert_eq!(gap.latest_available_seq, 3);

        let batch = read_batch(&mut recv).await?;
        assert_eq!(
            batch.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![3]
        );
        Ok(())
    }

    #[tokio::test]
    async fn epoch_started_precedes_new_epoch_events() -> TestResult {
        let harness = start_harness(DataConfig {
            max_events_per_batch: 1,
        })
        .await?;
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            journal.append_read(&stream_key(), None, b"epoch-1", "chip")?;
            journal.bump_epoch(&stream_key(), 2)?;
            journal.append_read(&stream_key(), None, b"epoch-2", "chip")?;
        }

        let (_send, mut recv, _ok) = open_subscription(&harness, 0).await?;
        match read_next(&mut recv).await?.msg {
            Some(data_f2c::Msg::StreamEpochStarted(started)) => {
                assert_eq!(started.epoch, 1);
                assert_eq!(started.start_seq, 1);
            }
            other => {
                return Err(
                    format!("expected StreamEpochStarted for epoch 1, got {other:?}").into(),
                );
            }
        }
        let first = match read_next(&mut recv).await?.msg {
            Some(data_f2c::Msg::EventBatch(batch)) => batch,
            other => return Err(format!("expected first EventBatch, got {other:?}").into()),
        };
        assert_eq!(first.records[0].epoch, 1);

        match read_next(&mut recv).await?.msg {
            Some(data_f2c::Msg::StreamEpochStarted(started)) => {
                assert_eq!(started.epoch, 2);
                assert_eq!(started.start_seq, 2);
            }
            other => {
                return Err(
                    format!("expected StreamEpochStarted for epoch 2, got {other:?}").into(),
                );
            }
        }
        let second = match read_next(&mut recv).await?.msg {
            Some(data_f2c::Msg::EventBatch(batch)) => batch,
            other => return Err(format!("expected second EventBatch, got {other:?}").into()),
        };
        assert_eq!(second.records[0].epoch, 2);
        Ok(())
    }

    #[tokio::test]
    async fn slow_receiver_does_not_block_others() -> TestResult {
        let harness = start_harness(DataConfig {
            max_events_per_batch: 1,
        })
        .await?;
        let slow_stream = OTHER_STREAM_KEY.to_owned();
        let fast_stream = stream_key();
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&slow_stream, 1)?;
            journal.ensure_stream_state(&fast_stream, 1)?;
            for i in 0..64 {
                let frame = vec![u8::try_from(i).unwrap(); 128 * 1024];
                journal.append_read(&slow_stream, None, &frame, "chip")?;
            }
            journal.append_read(&fast_stream, None, b"fast", "chip")?;
        }

        let connection = harness
            .receiver
            .connect(harness.forwarder_addr.clone())
            .await?;
        let (mut slow_send, _slow_recv) = connection.open_bi().await?;
        write_frame(
            &mut slow_send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id: OTHER_STREAM_KEY.as_bytes().to_vec(),
                    after_seq: 0,
                    mode: SubscribeMode::Replay as i32,
                })),
            },
        )
        .await?;

        let (mut fast_send, mut fast_recv) = connection.open_bi().await?;
        write_frame(
            &mut fast_send,
            &DataC2F {
                msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                    stream_id: stream_id(),
                    after_seq: 0,
                    mode: SubscribeMode::Replay as i32,
                })),
            },
        )
        .await?;

        let fast_ok = read_next(&mut fast_recv).await?;
        assert!(matches!(fast_ok.msg, Some(data_f2c::Msg::SubscribeOk(_))));
        let fast_batch = read_batch(&mut fast_recv).await?;
        assert_eq!(fast_batch.records.len(), 1);
        assert_eq!(fast_batch.records[0].raw_frame, b"fast");
        Ok(())
    }

    #[tokio::test]
    async fn future_replay_subscribe_fails_stream() -> TestResult {
        let harness = start_harness(DataConfig::default()).await?;
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            for seq in 1..=3 {
                let frame = format!("frame-{seq}");
                journal.append_read(&stream_key(), None, frame.as_bytes(), "chip")?;
            }
        }

        // latest_seq_at_open is 3; an after_seq past the tail must be rejected.
        subscribe_expecting_failure(&harness, 100, SubscribeMode::Replay as i32).await
    }

    #[tokio::test]
    async fn unknown_subscribe_mode_fails_stream() -> TestResult {
        let harness = start_harness(DataConfig::default()).await?;
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            journal.append_read(&stream_key(), None, b"frame-1", "chip")?;
        }

        // 99 is not a defined SubscribeMode; it must fail rather than replay.
        subscribe_expecting_failure(&harness, 0, 99).await
    }

    #[tokio::test]
    async fn future_ack_does_not_advance_retention_floor() -> TestResult {
        let harness = start_harness(DataConfig::default()).await?;
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            for seq in 1..=3 {
                let frame = format!("frame-{seq}");
                journal.append_read(&stream_key(), None, frame.as_bytes(), "chip")?;
            }
        }

        let (mut send, mut recv, _ok) = open_subscription(&harness, 0).await?;
        let _batch = read_batch(&mut recv).await?;

        // A future ack (100) followed by a valid ack (2). FIFO delivery means the
        // future ack is processed first; the fix must drop it so only seq <= 2 is
        // prunable. Without the fix the future ack would set the floor to 4.
        for through_seq in [100, 2] {
            write_frame(
                &mut send,
                &DataC2F {
                    msg: Some(data_c2f::Msg::Ack(Ack {
                        stream_id: stream_id(),
                        through_seq,
                    })),
                },
            )
            .await?;
        }

        let floor = prune_until_floor_moves(&harness, &stream_key()).await?;
        assert_eq!(floor, 3, "future ack must not advance the retention floor");
        Ok(())
    }

    #[tokio::test]
    async fn mismatched_ack_stream_id_is_ignored() -> TestResult {
        let harness = start_harness(DataConfig::default()).await?;
        let other_stream = OTHER_STREAM_KEY.to_owned();
        {
            let mut journal = harness.journal.lock().await;
            journal.ensure_stream_state(&stream_key(), 1)?;
            journal.ensure_stream_state(&other_stream, 1)?;
            for seq in 1..=3 {
                let frame = format!("frame-{seq}");
                journal.append_read(&stream_key(), None, frame.as_bytes(), "chip")?;
                journal.append_read(&other_stream, None, frame.as_bytes(), "chip")?;
            }
        }

        let (mut send, mut recv, _ok) = open_subscription(&harness, 0).await?;
        let _batch = read_batch(&mut recv).await?;

        // Ack the unsubscribed stream first, then the subscribed one. The
        // mismatched ack must not move the other stream's cursor.
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::Ack(Ack {
                    stream_id: OTHER_STREAM_KEY.as_bytes().to_vec(),
                    through_seq: 3,
                })),
            },
        )
        .await?;
        write_frame(
            &mut send,
            &DataC2F {
                msg: Some(data_c2f::Msg::Ack(Ack {
                    stream_id: stream_id(),
                    through_seq: 2,
                })),
            },
        )
        .await?;

        let floor = prune_until_floor_moves(&harness, &stream_key()).await?;
        assert_eq!(
            floor, 3,
            "subscribed stream floor should advance to the ack"
        );
        let other_floor = {
            let journal = harness.journal.lock().await;
            journal
                .retention_state(&other_stream)?
                .earliest_available_seq
        };
        assert_eq!(
            other_floor, 1,
            "ack for an unsubscribed stream must not advance its floor"
        );
        Ok(())
    }

    fn test_event(seq: i64, epoch: i64) -> JournalEvent {
        JournalEvent {
            id: seq,
            stream_key: stream_key(),
            stream_epoch: epoch,
            seq,
            reader_timestamp: None,
            raw_frame: vec![1],
            read_type: "chip".to_owned(),
            received_at: "0".to_owned(),
        }
    }

    #[test]
    fn split_segments_breaks_on_replay_live_and_epoch_boundaries() {
        // latest_at_open = 2: seqs 1,2 are replay; 3,4 are live; epoch bumps at 4.
        let events = vec![
            test_event(1, 1),
            test_event(2, 1),
            test_event(3, 1),
            test_event(4, 2),
        ];
        let segments = split_segments(&events, 2);
        let seqs: Vec<Vec<i64>> = segments
            .iter()
            .map(|seg| seg.iter().map(|e| e.seq).collect())
            .collect();
        assert_eq!(seqs, vec![vec![1, 2], vec![3], vec![4]]);
        // No segment mixes replay and live records.
        for segment in &segments {
            let first_replay = segment[0].seq <= 2;
            assert!(segment.iter().all(|e| (e.seq <= 2) == first_replay));
        }
    }
}

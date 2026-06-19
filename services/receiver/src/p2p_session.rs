//! Receiver-side P2P data session over the iroh transport.
//!
//! The receiver dials a forwarder by its iroh `EndpointId`, performs the control-plane
//! `Hello` negotiation, then opens a per-stream data stream and subscribes from
//! its persisted cursor.
//!
//! The durability contract for each [`EventBatch`](rt_p2p_protocol::EventBatch)
//! is **insert-before-ack**:
//!
//! 1. Insert every record into `received_events` (idempotent on
//!    `(stream_id, seq)`).
//! 2. Advance the contiguous cursor over the durable rows.
//! 3. Send a cumulative [`Ack`](rt_p2p_protocol::Ack) whose `through_seq` is the
//!    durable contiguous cursor — never the latest *received* seq.
//!
//! On [`GapNotice`](rt_p2p_protocol::GapNotice) the session records a gap marker
//! and jumps the cursor to `earliest_available_seq - 1`. On disconnect it
//! reconnects with exponential backoff (1s → 30s) and resumes from the
//! persisted cursor.
//!
//! Production runtime wiring (dialing the configured forwarder for every
//! subscription, multiplexing many streams) is intentionally minimal here and
//! is owned by later tasks; this module provides the testable session core.

use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rt_iroh::{Connection, Endpoint, NodeAddr, RecvStream, SendStream};
use rt_p2p_protocol::{
    Ack, ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, EventBatch, GapNotice, Hello,
    HelloOk, MAX_FRAME_BYTES, StreamCatalog, SubscribeMode, control_c2f, control_f2c, data_c2f,
    data_f2c, encode_frame,
};
use tokio::sync::{Mutex, broadcast, watch};

use crate::control_api::AppState;
use crate::db::{Db, GapMarkerInsert, ReceivedEventInsert};

/// Aggregates live control/data connectivity per forwarder and reflects it
/// into the shared [`AppState`] connection state.
pub struct SessionStatusReporter {
    state: Arc<AppState>,
}

impl std::fmt::Debug for SessionStatusReporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStatusReporter")
            .finish_non_exhaustive()
    }
}

impl SessionStatusReporter {
    /// Build a reporter for per-forwarder connection state.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Record that a forwarder's control session is up.
    pub async fn on_control_connected(&self, endpoint_id: &str) -> ControlConnectedGuard {
        self.state
            .mark_forwarder_runtime(endpoint_id, |status| {
                status.control_up = true;
                status.pending_started_at = None;
            })
            .await;
        ControlConnectedGuard {
            state: Arc::clone(&self.state),
            endpoint_id: endpoint_id.to_owned(),
        }
    }

    /// Record one active data subscription stream for a forwarder.
    pub async fn on_data_session(&self, endpoint_id: &str) -> DataSessionGuard {
        self.state
            .mark_forwarder_runtime(endpoint_id, |status| {
                status.data_sessions = status.data_sessions.saturating_add(1);
            })
            .await;
        DataSessionGuard {
            state: Arc::clone(&self.state),
            endpoint_id: endpoint_id.to_owned(),
        }
    }

    /// Compatibility shim for the old per-stream worker path. It reports a
    /// control session for a synthetic endpoint until Task 1.4 removes that
    /// path from reconciliation.
    async fn on_connected(&self) -> ControlConnectedGuard {
        self.on_control_connected("__legacy_stream_session__").await
    }
}

/// RAII release guard for a forwarder control session. The release is
/// cancellation-safe.
pub struct ControlConnectedGuard {
    state: Arc<AppState>,
    endpoint_id: String,
}

impl Drop for ControlConnectedGuard {
    fn drop(&mut self) {
        self.state
            .update_forwarder_runtime_sync(&self.endpoint_id, |status| {
                status.control_up = false;
                status.pending_started_at = Some(std::time::Instant::now());
            });
        self.state
            .recompute_aggregate_connection_state_sync_default_trying();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.recompute_aggregate_connection_state().await;
            });
        }
    }
}

/// RAII release guard for a forwarder data subscription stream. The decrement
/// is cancellation-safe.
pub struct DataSessionGuard {
    state: Arc<AppState>,
    endpoint_id: String,
}

impl Drop for DataSessionGuard {
    fn drop(&mut self) {
        self.state
            .update_forwarder_runtime_sync(&self.endpoint_id, |status| {
                status.data_sessions = status.data_sessions.saturating_sub(1);
            });
        self.state
            .recompute_aggregate_connection_state_sync_default_trying();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.recompute_aggregate_connection_state().await;
            });
        }
    }
}

/// Errors raised by the receiver P2P data session.
#[derive(Debug, thiserror::Error)]
pub enum P2pSessionError {
    /// Endpoint-level error (dial, address registration).
    #[error("iroh endpoint error: {0}")]
    Iroh(#[from] rt_iroh::Error),
    /// Opening/accepting a QUIC stream failed.
    #[error("stream error: {0}")]
    Stream(String),
    /// Reading bytes from a stream failed (often a clean disconnect/EOF).
    #[error("stream read error: {0}")]
    Read(String),
    /// Writing bytes to a stream failed.
    #[error("stream write error: {0}")]
    Write(String),
    /// A frame payload failed to decode as the expected protobuf message.
    #[error("protocol decode error: {0}")]
    Decode(String),
    /// A frame's declared length exceeded the protocol maximum.
    #[error("frame too large: {0} bytes")]
    FrameTooLarge(usize),
    /// Durable store error.
    #[error("database error: {0}")]
    Db(#[from] crate::db::DbError),
    /// A control/data message arrived out of the expected sequence.
    #[error("unexpected message on {plane} plane")]
    UnexpectedMessage {
        /// Which plane the unexpected message arrived on.
        plane: &'static str,
    },
    /// A stream-scoped frame carried a `stream_id` that did not match the
    /// subscribed stream. Treated as a data-integrity violation: the frame is
    /// neither persisted nor acked.
    #[error("stream_id mismatch: subscribed to {expected:?}, frame carried {actual:02x?}")]
    StreamIdMismatch {
        /// The stream this session subscribed to.
        expected: String,
        /// The raw `stream_id` bytes carried by the offending frame.
        actual: Vec<u8>,
    },
    /// A duplicate `(stream_id, seq)` arrived whose immutable payload differs
    /// from the row already persisted. Treated as a data-integrity violation.
    #[error("conflicting duplicate for stream {stream_id} seq {seq}")]
    ConflictingDuplicate {
        /// The stream the conflicting record belongs to.
        stream_id: String,
        /// The sequence number with conflicting payloads.
        seq: i64,
    },
    /// A u64 wire value exceeded the receiver's i64 durable storage range.
    #[error("{field} value {value} exceeds i64::MAX")]
    NumericOutOfRange {
        /// The protobuf field carrying the malformed value.
        field: &'static str,
        /// The out-of-range wire value.
        value: u64,
    },
}

impl P2pSessionError {
    /// Whether the reconnect loop should retry after this error.
    ///
    /// Transient transport/read/write failures are retryable (the loop backs
    /// off and resumes from the persisted cursor). Durable failures — decode,
    /// frame-size, protocol-sequencing, and data-integrity errors, plus durable
    /// store errors — are surfaced to the caller instead of being retried
    /// forever.
    pub fn is_retryable(&self) -> bool {
        match self {
            P2pSessionError::Iroh(_)
            | P2pSessionError::Stream(_)
            | P2pSessionError::Read(_)
            | P2pSessionError::Write(_) => true,
            P2pSessionError::Decode(_)
            | P2pSessionError::FrameTooLarge(_)
            | P2pSessionError::Db(_)
            | P2pSessionError::UnexpectedMessage { .. }
            | P2pSessionError::StreamIdMismatch { .. }
            | P2pSessionError::ConflictingDuplicate { .. }
            | P2pSessionError::NumericOutOfRange { .. } => false,
        }
    }
}

/// Outcome of a single data-subscription attempt, used by the reconnect loop to
/// decide whether to reset the backoff window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The subscription opened (`SubscribeOk` received) and later disconnected.
    /// Backoff resets because forward progress was made.
    OpenedThenDisconnected,
    /// The connection closed/errored before `SubscribeOk` arrived. This is a
    /// retryable disconnect, but backoff does not reset because no subscription
    /// actually opened.
    DisconnectedBeforeOpen,
}

/// Exponential-backoff bounds for reconnection. Defaults to 1s → 30s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackoffConfig {
    /// Delay before the first reconnect attempt after a disconnect.
    pub initial: Duration,
    /// Maximum delay between reconnect attempts.
    pub max: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

/// Doubles `current`, capped at `max`.
fn next_backoff(current: Duration, max: Duration) -> Duration {
    current.saturating_mul(2).min(max)
}

/// Parameters for a reconnecting single-stream session.
#[derive(Clone, Debug)]
pub struct SessionParams {
    /// The stream to subscribe to. An arbitrary UTF-8 stream key (e.g. the
    /// forwarder journal key `ip:port`), stored verbatim in the durable store.
    pub stream_id: String,
    /// The client `Hello` to present during control-plane negotiation.
    pub client_hello: Hello,
    /// Subscription mode (typically [`SubscribeMode::Replay`] to resume + go live).
    pub mode: SubscribeMode,
    /// Reconnect backoff bounds.
    pub backoff: BackoffConfig,
    /// Optional post-commit durable hint sink. After every durable insert /
    /// cursor advance (and on gap-jump), the contiguous durable cursor is
    /// broadcast on this channel so downstream workers — the durable local
    /// proxy, the DBF feed, and the announcer push — can drain freshly
    /// persisted rows. The hint is sent *after* the durable write, preserving
    /// the insert-before-ack contract.
    pub durable_hint_tx: Option<broadcast::Sender<i64>>,
    /// Optional reporter that reflects this session's connect/disconnect
    /// lifecycle into the shared [`AppState`] connection state. `None` for
    /// session-core tests that do not exercise the aggregate connection state.
    pub reporter: Option<Arc<SessionStatusReporter>>,
}

/// An established control-plane session with a forwarder peer.
#[derive(Debug)]
pub struct ControlSession {
    /// The live connection; data streams are opened from it.
    pub connection: Connection,
    /// The negotiated handshake acknowledgement.
    pub hello_ok: HelloOk,
    /// The catalog delivered immediately after the handshake.
    pub catalog: StreamCatalog,
    // Held to keep the control stream alive for the connection's lifetime.
    _control_send: SendStream,
    _control_recv: RecvStream,
}

fn stream_id_bytes(stream_id: &str) -> Vec<u8> {
    stream_id.as_bytes().to_vec()
}

/// Validate that a stream-scoped frame's `stream_id` matches the subscribed
/// stream. The configured stream ID is a UTF-8 string, so wire bytes are
/// compared for exact equality against its UTF-8 bytes. Any divergence —
/// including non-UTF-8 wire bytes that cannot equal a UTF-8 key — is rejected as
/// [`P2pSessionError::StreamIdMismatch`] rather than lossily converted.
fn check_stream_id(expected: &str, actual: &[u8]) -> Result<(), P2pSessionError> {
    if actual == expected.as_bytes() {
        Ok(())
    } else {
        Err(P2pSessionError::StreamIdMismatch {
            expected: expected.to_owned(),
            actual: actual.to_vec(),
        })
    }
}

fn i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn u64_to_i64(value: u64, field: &'static str) -> Result<i64, P2pSessionError> {
    i64::try_from(value).map_err(|_| P2pSessionError::NumericOutOfRange { field, value })
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

async fn write_frame(send: &mut SendStream, message: &impl Message) -> Result<(), P2pSessionError> {
    send.write_all(&encode_frame(message))
        .await
        .map_err(|e| P2pSessionError::Write(e.to_string()))
}

async fn read_frame<M>(recv: &mut RecvStream) -> Result<M, P2pSessionError>
where
    M: Message + Default,
{
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(|e| P2pSessionError::Read(e.to_string()))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(P2pSessionError::FrameTooLarge(len));
    }
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload)
        .await
        .map_err(|e| P2pSessionError::Read(e.to_string()))?;
    M::decode(payload.as_slice()).map_err(|e| P2pSessionError::Decode(e.to_string()))
}

/// Dials `forwarder_addr`, opens the control stream, sends `client_hello`, and
/// reads back the negotiated `HelloOk` plus the `StreamCatalog`.
pub async fn connect_and_hello(
    endpoint: &Endpoint,
    forwarder_addr: NodeAddr,
    client_hello: Hello,
) -> Result<ControlSession, P2pSessionError> {
    endpoint.add_node_addr(forwarder_addr.clone())?;
    let connection = endpoint.connect(forwarder_addr).await?;

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| P2pSessionError::Stream(e.to_string()))?;

    write_frame(
        &mut send,
        &ControlC2F {
            msg: Some(control_c2f::Msg::Hello(client_hello)),
        },
    )
    .await?;

    let hello_ok = match read_frame::<ControlF2C>(&mut recv).await?.msg {
        Some(control_f2c::Msg::HelloOk(hello_ok)) => hello_ok,
        _ => return Err(P2pSessionError::UnexpectedMessage { plane: "control" }),
    };
    let catalog = match read_frame::<ControlF2C>(&mut recv).await?.msg {
        Some(control_f2c::Msg::StreamCatalog(catalog)) => catalog,
        _ => return Err(P2pSessionError::UnexpectedMessage { plane: "control" }),
    };

    Ok(ControlSession {
        connection,
        hello_ok,
        catalog,
        _control_send: send,
        _control_recv: recv,
    })
}

/// Insert every record of `batch` into the durable store (idempotent on
/// `(stream_id, seq)`), then advance and return the contiguous durable cursor.
///
/// Every record's `stream_id` is validated against the subscribed stream
/// *before* any insertion, so a batch carrying a foreign record persists
/// nothing. A duplicate `(stream_id, seq)` whose immutable payload differs from
/// the stored row is rejected as [`P2pSessionError::ConflictingDuplicate`].
fn persist_batch(db: &Db, stream_id: &str, batch: &EventBatch) -> Result<i64, P2pSessionError> {
    let mut records = Vec::with_capacity(batch.records.len());
    for record in &batch.records {
        check_stream_id(stream_id, &record.stream_id)?;
        let seq = u64_to_i64(record.seq, "record.seq")?;
        let epoch = u64_to_i64(record.epoch, "record.epoch")?;
        records.push((record, seq, epoch));
    }

    let received_default = now_unix_ms();
    for (record, seq, epoch) in records {
        let reader_timestamp = if record.reader_timestamp == 0 {
            None
        } else {
            Some(record.reader_timestamp.to_string())
        };
        let received_unix_ms = if record.received_unix_ms == 0 {
            received_default
        } else {
            record.received_unix_ms
        };
        let insert = ReceivedEventInsert {
            stream_id,
            seq,
            epoch,
            raw_frame: &record.raw_frame,
            read_kind: &record.read_kind,
            reader_timestamp: reader_timestamp.as_deref(),
            received_unix_ms,
            dbf_delivered_unix_ms: None,
        };
        if !db.insert_received_event(&insert)? {
            // Idempotent dedup: a row already exists for this (stream_id, seq).
            // The immutable payload must match — a divergent duplicate means the
            // forwarder re-sent a conflicting record under the same seq, which
            // is a data-integrity violation we must not silently ack past. A
            // non-zero received_unix_ms is part of that payload because it is
            // persisted and used as the announcer ordering key; zero means the
            // forwarder omitted it and the receiver supplied a local default.
            if let Some(existing) = db.load_received_event(stream_id, insert.seq)? {
                let received_unix_ms_conflicts = record.received_unix_ms != 0
                    && existing.received_unix_ms != insert.received_unix_ms;
                let conflicts = existing.epoch != insert.epoch
                    || existing.raw_frame != insert.raw_frame
                    || existing.read_kind != insert.read_kind
                    || existing.reader_timestamp.as_deref() != insert.reader_timestamp
                    || received_unix_ms_conflicts;
                if conflicts {
                    return Err(P2pSessionError::ConflictingDuplicate {
                        stream_id: stream_id.to_owned(),
                        seq: insert.seq,
                    });
                }
            }
        }
    }
    Ok(db.advance_cursor_contiguous_prefix(stream_id)?)
}

/// Record a gap marker and jump the cursor past the unavailable history,
/// returning the resulting durable cursor.
fn persist_gap(db: &Db, stream_id: &str, gap: &GapNotice) -> Result<i64, P2pSessionError> {
    check_stream_id(stream_id, &gap.stream_id)?;
    let requested_after_seq = u64_to_i64(gap.requested_after_seq, "gap.requested_after_seq")?;
    let earliest_available_seq =
        u64_to_i64(gap.earliest_available_seq, "gap.earliest_available_seq")?;
    let latest_available_seq = u64_to_i64(gap.latest_available_seq, "gap.latest_available_seq")?;
    let marker = GapMarkerInsert {
        stream_id,
        requested_after_seq,
        earliest_available_seq,
        latest_available_seq,
        reason: &gap.reason,
        created_unix_ms: now_unix_ms(),
    };
    db.save_gap_marker(&marker)?;
    let jump_to = earliest_available_seq.saturating_sub(1);
    db.jump_stream_cursor(stream_id, jump_to)?;
    Ok(db.load_stream_cursor(stream_id)?)
}

async fn send_ack(
    send: &mut SendStream,
    stream_id: &str,
    through_seq: i64,
) -> Result<(), P2pSessionError> {
    write_frame(
        send,
        &DataC2F {
            msg: Some(data_c2f::Msg::Ack(Ack {
                stream_id: stream_id_bytes(stream_id),
                through_seq: i64_to_u64(through_seq),
            })),
        },
    )
    .await
}

/// Run a single data-plane subscription over an established connection until the
/// data stream ends (clean disconnect/EOF).
///
/// Reads the persisted cursor, opens a data stream, sends `DataSubscribe { after_seq }`,
/// then pumps `EventBatch` (insert → advance cursor → cumulative `Ack`) and
/// `GapNotice` (record + jump cursor) until the stream closes.
///
/// `db` is a shared, `Send`-friendly handle. The lock is held only for short
/// synchronous persistence/cursor operations and is always released before any
/// network await, so the returned future stays `Send` and never blocks the
/// runtime on I/O while holding the connection.
///
/// Every stream-scoped frame's `stream_id` is validated against `stream_id`
/// before it is persisted or acked; a mismatch is rejected as
/// [`P2pSessionError::StreamIdMismatch`].
pub async fn run_data_subscription(
    connection: &Connection,
    db: &Arc<Mutex<Db>>,
    stream_id: &str,
    mode: SubscribeMode,
) -> Result<SessionOutcome, P2pSessionError> {
    run_data_subscription_with_hint(connection, db, stream_id, mode, None).await
}

/// Like [`run_data_subscription`], but broadcasts the durable contiguous cursor
/// on `durable_hint_tx` after each durable insert / cursor advance (and after a
/// gap-jump). The hint is sent strictly *after* the durable write so the
/// insert-before-ack contract is preserved; downstream workers (durable proxy,
/// DBF, announcer) use it to drain freshly persisted rows.
pub async fn run_data_subscription_with_hint(
    connection: &Connection,
    db: &Arc<Mutex<Db>>,
    stream_id: &str,
    mode: SubscribeMode,
    durable_hint_tx: Option<&broadcast::Sender<i64>>,
) -> Result<SessionOutcome, P2pSessionError> {
    let after_seq = { db.lock().await.load_stream_cursor(stream_id)? };

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| P2pSessionError::Stream(e.to_string()))?;

    write_frame(
        &mut send,
        &DataC2F {
            msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                stream_id: stream_id_bytes(stream_id),
                after_seq: i64_to_u64(after_seq),
                mode: mode as i32,
            })),
        },
    )
    .await?;

    match read_frame::<DataF2C>(&mut recv).await {
        Ok(frame) => match frame.msg {
            Some(data_f2c::Msg::SubscribeOk(ok)) => check_stream_id(stream_id, &ok.stream_id)?,
            _ => return Err(P2pSessionError::UnexpectedMessage { plane: "data" }),
        },
        // A read error before SubscribeOk is a disconnect, not a contract
        // violation; report it as a pre-open disconnect so the reconnect loop
        // retries with backoff without resetting the backoff window.
        Err(P2pSessionError::Read(_)) => return Ok(SessionOutcome::DisconnectedBeforeOpen),
        Err(other) => return Err(other),
    }

    loop {
        // A read error here is the expected end-of-stream / disconnect signal.
        let frame = match read_frame::<DataF2C>(&mut recv).await {
            Ok(frame) => frame,
            Err(P2pSessionError::Read(_)) => break,
            Err(other) => return Err(other),
        };

        match frame.msg {
            Some(data_f2c::Msg::EventBatch(batch)) => {
                // Durable-first: validate + insert rows, advance the contiguous
                // cursor, then ack only through that durable cursor. The lock is
                // released before the ack await.
                let through_seq = {
                    let db = db.lock().await;
                    persist_batch(&db, stream_id, &batch)
                }?;
                // Post-commit durable hint: rows are durable before the ack.
                if let Some(tx) = durable_hint_tx {
                    let _ = tx.send(through_seq);
                }
                send_ack(&mut send, stream_id, through_seq).await?;
            }
            Some(data_f2c::Msg::GapNotice(gap)) => {
                // Record the gap, jump the cursor past the unavailable history,
                // then ack the jumped cursor so the forwarder will not resend
                // the now-skipped seqs.
                let through_seq = {
                    let db = db.lock().await;
                    persist_gap(&db, stream_id, &gap)
                }?;
                if let Some(tx) = durable_hint_tx {
                    let _ = tx.send(through_seq);
                }
                send_ack(&mut send, stream_id, through_seq).await?;
            }
            // CaughtUp / StreamEpochStarted carry no durable state to persist
            // here, but they are stream-scoped: validate the stream_id and keep
            // listening for further live frames.
            Some(data_f2c::Msg::CaughtUp(caught_up)) => {
                check_stream_id(stream_id, &caught_up.stream_id)?;
            }
            Some(data_f2c::Msg::StreamEpochStarted(epoch_started)) => {
                check_stream_id(stream_id, &epoch_started.stream_id)?;
            }
            // A second SubscribeOk after open is out of sequence.
            Some(data_f2c::Msg::SubscribeOk(_)) => {
                return Err(P2pSessionError::UnexpectedMessage { plane: "data" });
            }
            None => {}
        }
    }

    Ok(SessionOutcome::OpenedThenDisconnected)
}

async fn run_once(
    endpoint: &Endpoint,
    forwarder_addr: NodeAddr,
    db: &Arc<Mutex<Db>>,
    params: &SessionParams,
) -> Result<SessionOutcome, P2pSessionError> {
    let session = connect_and_hello(endpoint, forwarder_addr, params.client_hello.clone()).await?;
    // The handshake succeeded: this session is live. Hold an RAII guard for the
    // duration of the data subscription so the shared live-session count is
    // released on drop even if this future is cancelled mid-session (worker
    // rebuild on subscription edit/removal, or runtime shutdown) rather than
    // only on a clean return.
    let _connected_guard = match &params.reporter {
        Some(reporter) => Some(reporter.on_connected().await),
        None => None,
    };
    run_data_subscription_with_hint(
        &session.connection,
        db,
        &params.stream_id,
        params.mode,
        params.durable_hint_tx.as_ref(),
    )
    .await
}

/// Run a reconnecting single-stream session: dial + hello + data subscription,
/// and on disconnect back off (1s → 30s) and resume from the persisted cursor.
///
/// Returns `Ok(())` when `shutdown` is set to `true`. Each reconnect resumes
/// from the cursor read inside [`run_data_subscription`], so no in-memory
/// progress is lost across reconnects.
///
/// Only transient transport/read failures (see [`P2pSessionError::is_retryable`])
/// are retried. Durable failures — decode, frame-size, protocol-sequencing, and
/// data-integrity errors, plus durable store errors — are returned to the
/// caller instead of being retried forever. Backoff resets only after a
/// subscription actually opened ([`SessionOutcome::OpenedThenDisconnected`]); a
/// pre-open disconnect or a retryable failure keeps growing the backoff window.
pub async fn run_session_with_reconnect(
    endpoint: &Endpoint,
    forwarder_addr: NodeAddr,
    db: &Arc<Mutex<Db>>,
    params: &SessionParams,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), P2pSessionError> {
    let mut backoff = params.backoff.initial;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }

        let outcome = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                continue;
            }
            outcome = run_once(endpoint, forwarder_addr.clone(), db, params) => outcome,
        };

        let reset_backoff = match outcome {
            Ok(SessionOutcome::OpenedThenDisconnected) => true,
            Ok(SessionOutcome::DisconnectedBeforeOpen) => false,
            Err(e) if e.is_retryable() => {
                tracing::warn!(error = %e, "p2p session transient failure; retrying");
                false
            }
            // Durable failures are not retryable: surface them to the caller.
            Err(e) => return Err(e),
        };

        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return Ok(()); }
            }
            () = tokio::time::sleep(backoff) => {}
        }

        backoff = if reset_backoff {
            params.backoff.initial
        } else {
            next_backoff(backoff, params.backoff.max)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_api::ConnectionState;
    use rt_iroh::EndpointBuilder;
    use rt_p2p_protocol::{ReadRecord, StreamCatalog, StreamEntry, SubscribeOk};
    use rt_test_utils::p2p::{ConnectivityFault, ForwarderScript, MockForwarderPeer};
    use rt_test_utils::poll_until;

    // A real forwarder P2P stream_id is an arbitrary UTF-8 journal key such as
    // `ip:port`, not a parseable UUID. The session must subscribe to it as a
    // plain string and validate wire bytes against its UTF-8 bytes.
    const STREAM_ID: &str = "127.0.0.1:10000";
    const OTHER_STREAM_ID: &str = "127.0.0.1:10001";
    const TEST_TIMEOUT: Duration = Duration::from_secs(20);

    fn stream_id() -> &'static str {
        STREAM_ID
    }

    fn sid_bytes() -> Vec<u8> {
        STREAM_ID.as_bytes().to_vec()
    }

    /// A `stream_id` for a *different* stream than the one the session
    /// subscribes to, used to exercise stream-id validation.
    fn other_sid_bytes() -> Vec<u8> {
        OTHER_STREAM_ID.as_bytes().to_vec()
    }

    fn test_db() -> Arc<Mutex<Db>> {
        Arc::new(Mutex::new(Db::open_in_memory().unwrap()))
    }

    fn test_hello(catalog_generation: u64) -> Hello {
        Hello {
            min_minor: 1,
            max_minor: 1,
            capabilities: vec!["data".to_owned()],
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
            catalog_generation,
        }
    }

    fn test_catalog() -> StreamCatalog {
        StreamCatalog {
            generation: 1,
            entries: vec![StreamEntry {
                stream_id: sid_bytes(),
                display_name: "Finish".to_owned(),
                network_addr: "10.0.0.1:10000".to_owned(),
                reader_connected: true,
                hardware_reader_id: "R1".to_owned(),
            }],
        }
    }

    fn record(seq: u64) -> ReadRecord {
        ReadRecord {
            stream_id: sid_bytes(),
            seq,
            epoch: 1,
            raw_frame: format!("frame-{seq}").into_bytes(),
            read_kind: "chip".to_owned(),
            reader_timestamp: 0,
            received_unix_ms: 0,
        }
    }

    fn batch(seqs: &[u64]) -> EventBatch {
        EventBatch {
            records: seqs.iter().copied().map(record).collect(),
            replay: false,
        }
    }

    fn base_script() -> ForwarderScript {
        ForwarderScript {
            server_hello: test_hello(1),
            catalog: test_catalog(),
            subscribe_ok: SubscribeOk {
                stream_id: sid_bytes(),
                earliest_available_seq: 1,
                latest_seq_at_open: 4,
            },
            gap_notice: None,
            batches: Vec::new(),
            caught_up_through: None,
            data_fault: ConnectivityFault::healthy(),
        }
    }

    async fn test_endpoint(seed: u8) -> Endpoint {
        EndpointBuilder::test([seed; 32]).bind().await.unwrap()
    }

    fn reporter_state() -> (Arc<AppState>, Arc<SessionStatusReporter>) {
        let (state, _shutdown_rx) =
            AppState::new(Db::open_in_memory().unwrap(), "recv-test".to_owned());
        let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));
        (state, reporter)
    }

    #[tokio::test]
    async fn reporter_tracks_per_forwarder_states() {
        let (state, _shutdown_rx) =
            AppState::new(Db::open_in_memory().unwrap(), "recv-test".to_owned());
        let reporter = SessionStatusReporter::new(Arc::clone(&state));

        let control_guard = reporter.on_control_connected("fwd-1").await;
        assert_eq!(
            state.forwarder_state("fwd-1").await.state,
            crate::control_api::ForwarderConnState::Connected
        );

        let data_guard = reporter.on_data_session("fwd-1").await;
        assert_eq!(
            state.forwarder_state("fwd-1").await.state,
            crate::control_api::ForwarderConnState::Subscribed
        );

        drop(data_guard);
        assert_eq!(
            state.forwarder_state("fwd-1").await.state,
            crate::control_api::ForwarderConnState::Connected
        );

        state
            .db
            .lock()
            .await
            .set_forwarder_intent("fwd-1", false)
            .unwrap();
        drop(control_guard);
        assert_eq!(
            state.forwarder_state("fwd-1").await.state,
            crate::control_api::ForwarderConnState::Disconnected
        );
    }

    #[tokio::test]
    async fn connected_session_guard_releases_on_cancellation() {
        // A connected session whose run_once future is cancelled (guard dropped
        // without a clean return) must still release its live-session count and
        // fall the aggregate state back to Connecting — otherwise the badge can
        // stay falsely "Connected" after the last session ends.
        let (state, reporter) = reporter_state();
        let guard = reporter.on_control_connected("fwd-1").await;
        assert_eq!(
            state.connection_state.borrow().clone(),
            ConnectionState::Connected
        );

        drop(guard); // simulates run_once cancellation mid-session

        assert_eq!(
            state.connection_state.borrow().clone(),
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn connected_guard_keeps_connected_while_another_session_live() {
        // Dropping one of two live sessions must NOT fall back to Connecting:
        // the aggregate stays Connected until the last session drops.
        let (state, reporter) = reporter_state();
        let g1 = reporter.on_control_connected("fwd-1").await;
        let g2 = reporter.on_control_connected("fwd-2").await;

        drop(g1);
        assert_eq!(
            state.connection_state.borrow().clone(),
            ConnectionState::Connected
        );

        drop(g2);
        assert_eq!(
            state.connection_state.borrow().clone(),
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn ack_after_durable_only() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // Seq 3 is missing, so the contiguous cursor must stop at 2 even
            // though seq 4 was received and stored.
            script.batches = vec![batch(&[1, 2, 4])];
            script.caught_up_through = Some(4);

            let forwarder = MockForwarderPeer::start([10; 32], script).await.unwrap();
            let endpoint = test_endpoint(11).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                .await
                .unwrap();

            // Every received record is durable.
            let guard = db.lock().await;
            let seqs: Vec<i64> = guard
                .load_received_events(stream_id)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect();
            assert_eq!(seqs, vec![1, 2, 4]);
            // Cursor tracks the durable *contiguous* prefix, not the latest seq.
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 2);
            drop(guard);

            poll_until(
                || async { !forwarder.acks().is_empty() },
                Duration::from_secs(5),
            )
            .await;
            let acks = forwarder.acks();
            assert_eq!(acks.len(), 1);
            assert_eq!(
                acks[0].through_seq, 2,
                "ack must reflect the durable contiguous cursor, not the latest received seq"
            );

            forwarder.shutdown().await;
        })
        .await
        .expect("ack_after_durable_only timed out");
    }

    #[tokio::test]
    async fn gap_notice_jumps_cursor() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.gap_notice = Some(GapNotice {
                stream_id: sid_bytes(),
                requested_after_seq: 0,
                earliest_available_seq: 15,
                latest_available_seq: 20,
                reason: "retention-window".to_owned(),
            });

            let forwarder = MockForwarderPeer::start([12; 32], script).await.unwrap();
            let endpoint = test_endpoint(13).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                .await
                .unwrap();

            let guard = db.lock().await;
            let markers = guard.load_gap_markers(stream_id).unwrap();
            assert_eq!(markers.len(), 1);
            assert_eq!(markers[0].requested_after_seq, 0);
            assert_eq!(markers[0].earliest_available_seq, 15);
            assert_eq!(markers[0].latest_available_seq, 20);
            // Cursor jumps to earliest_available_seq - 1.
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 14);
            drop(guard);

            forwarder.shutdown().await;
        })
        .await
        .expect("gap_notice_jumps_cursor timed out");
    }

    #[tokio::test]
    async fn reconnect_resumes_from_cursor() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.batches = vec![batch(&[1, 2])];
            script.caught_up_through = Some(2);

            let forwarder = MockForwarderPeer::start([14; 32], script).await.unwrap();
            let endpoint = test_endpoint(15).await;
            let db = test_db();

            // First connection: receive [1, 2], cursor advances to 2.
            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                .await
                .unwrap();
            assert_eq!(db.lock().await.load_stream_cursor(stream_id).unwrap(), 2);
            drop(session);

            // Reconnect: a fresh connection must resume from the persisted cursor.
            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                .await
                .unwrap();

            // The forwarder re-sent [1, 2]; dedup keeps a single row per seq.
            let guard = db.lock().await;
            assert_eq!(guard.load_received_events(stream_id).unwrap().len(), 2);
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 2);
            drop(guard);

            poll_until(
                || async { forwarder.subscribes().len() >= 2 },
                Duration::from_secs(5),
            )
            .await;
            let subscribes = forwarder.subscribes();
            assert_eq!(subscribes.len(), 2);
            assert_eq!(subscribes[0].after_seq, 0, "first subscribe starts at 0");
            assert_eq!(
                subscribes[1].after_seq, 2,
                "reconnect must resume from the persisted cursor"
            );

            forwarder.shutdown().await;
        })
        .await
        .expect("reconnect_resumes_from_cursor timed out");
    }

    #[tokio::test]
    async fn reconnect_storm_resumes_without_duplicates() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.batches = vec![batch(&[1, 2, 3])];
            script.caught_up_through = Some(3);

            let forwarder = MockForwarderPeer::start([42; 32], script).await.unwrap();
            let endpoint = test_endpoint(43).await;
            let db = test_db();
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let params = SessionParams {
                stream_id: stream_id.to_owned(),
                client_hello: test_hello(0),
                mode: SubscribeMode::Replay,
                backoff: BackoffConfig {
                    initial: Duration::from_millis(1),
                    max: Duration::from_millis(1),
                },
                durable_hint_tx: None,
                reporter: None,
            };

            let session_task = tokio::spawn({
                let endpoint = endpoint.clone();
                let db = Arc::clone(&db);
                let forwarder_addr = forwarder.node_addr();
                async move {
                    run_session_with_reconnect(&endpoint, forwarder_addr, &db, &params, shutdown_rx)
                        .await
                }
            });

            poll_until(
                || async { forwarder.subscribes().len() >= 4 },
                Duration::from_secs(5),
            )
            .await;
            shutdown_tx.send(true).unwrap();
            session_task.await.unwrap().unwrap();

            let guard = db.lock().await;
            let events = guard.load_received_events(stream_id).unwrap();
            assert_eq!(events.len(), 3, "reconnect storm must not duplicate rows");
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 3);
            drop(guard);

            let subscribes = forwarder.subscribes();
            assert!(subscribes.len() >= 4);
            assert_eq!(subscribes[0].after_seq, 0);
            assert!(
                subscribes
                    .iter()
                    .skip(1)
                    .all(|subscribe| subscribe.after_seq == 3),
                "all reconnects after the first must resume from the durable cursor: {subscribes:?}"
            );

            forwarder.shutdown().await;
        })
        .await
        .expect("reconnect_storm_resumes_without_duplicates timed out");
    }

    #[tokio::test]
    async fn duplicate_seq_deduped() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // Duplicate seqs within a single batch.
            script.batches = vec![batch(&[1, 1, 2, 2, 3])];
            script.caught_up_through = Some(3);

            let forwarder = MockForwarderPeer::start([16; 32], script).await.unwrap();
            let endpoint = test_endpoint(17).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                .await
                .unwrap();

            let guard = db.lock().await;
            let seqs: Vec<i64> = guard
                .load_received_events(stream_id)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect();
            assert_eq!(
                seqs,
                vec![1, 2, 3],
                "duplicate seqs collapse to one row each"
            );
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 3);
            drop(guard);

            poll_until(
                || async { !forwarder.acks().is_empty() },
                Duration::from_secs(5),
            )
            .await;
            let acks = forwarder.acks();
            assert_eq!(
                acks[0].through_seq, 3,
                "ack is monotonic through the contiguous cursor"
            );

            forwarder.shutdown().await;
        })
        .await
        .expect("duplicate_seq_deduped timed out");
    }

    #[tokio::test]
    async fn durable_hint_broadcast_after_persist() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.batches = vec![batch(&[1, 2])];
            script.caught_up_through = Some(2);

            let forwarder = MockForwarderPeer::start([60; 32], script).await.unwrap();
            let endpoint = test_endpoint(61).await;
            let db = test_db();
            let (hint_tx, mut hint_rx) = broadcast::channel(16);

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription_with_hint(
                &session.connection,
                &db,
                stream_id,
                SubscribeMode::Replay,
                Some(&hint_tx),
            )
            .await
            .unwrap();

            // The contiguous durable cursor (2) is broadcast as a post-commit hint.
            let hint = hint_rx.recv().await.unwrap();
            assert_eq!(hint, 2);

            forwarder.shutdown().await;
        })
        .await
        .expect("durable_hint_broadcast_after_persist timed out");
    }

    #[test]
    fn backoff_doubles_and_caps_at_max() {
        let config = BackoffConfig::default();
        assert_eq!(config.initial, Duration::from_secs(1));
        assert_eq!(config.max, Duration::from_secs(30));
        assert_eq!(
            next_backoff(Duration::from_secs(1), config.max),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(16), config.max),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30), config.max),
            Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn reconnect_loop_returns_when_shutdown_already_set() {
        let endpoint = test_endpoint(18).await;
        let db = test_db();
        let forwarder = MockForwarderPeer::start([19; 32], base_script())
            .await
            .unwrap();
        let (_tx, shutdown_rx) = watch::channel(true);
        let params = SessionParams {
            stream_id: stream_id().to_owned(),
            client_hello: test_hello(0),
            mode: SubscribeMode::Replay,
            backoff: BackoffConfig::default(),
            durable_hint_tx: None,
            reporter: None,
        };

        let result =
            run_session_with_reconnect(&endpoint, forwarder.node_addr(), &db, &params, shutdown_rx)
                .await;
        assert!(result.is_ok());

        forwarder.shutdown().await;
    }

    fn record_with(seq: u64, raw: &str) -> ReadRecord {
        ReadRecord {
            stream_id: sid_bytes(),
            seq,
            epoch: 1,
            raw_frame: raw.as_bytes().to_vec(),
            read_kind: "chip".to_owned(),
            reader_timestamp: 0,
            received_unix_ms: 0,
        }
    }

    fn record_with_received(seq: u64, raw: &str, received_unix_ms: i64) -> ReadRecord {
        ReadRecord {
            received_unix_ms,
            ..record_with(seq, raw)
        }
    }

    #[tokio::test]
    async fn mismatched_event_stream_id_rejected() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // A record claiming a different stream than the subscription.
            script.batches = vec![EventBatch {
                records: vec![ReadRecord {
                    stream_id: other_sid_bytes(),
                    ..record(1)
                }],
                replay: false,
            }];

            let forwarder = MockForwarderPeer::start([20; 32], script).await.unwrap();
            let endpoint = test_endpoint(21).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let result =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await;

            assert!(
                matches!(result, Err(P2pSessionError::StreamIdMismatch { .. })),
                "mismatched event stream_id must be rejected, got {result:?}"
            );
            // Nothing was persisted and nothing was acked.
            let guard = db.lock().await;
            assert!(guard.load_received_events(stream_id).unwrap().is_empty());
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 0);
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("mismatched_event_stream_id_rejected timed out");
    }

    #[tokio::test]
    async fn mismatched_gap_stream_id_rejected() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.gap_notice = Some(GapNotice {
                stream_id: other_sid_bytes(),
                requested_after_seq: 0,
                earliest_available_seq: 15,
                latest_available_seq: 20,
                reason: "retention-window".to_owned(),
            });

            let forwarder = MockForwarderPeer::start([22; 32], script).await.unwrap();
            let endpoint = test_endpoint(23).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let result =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await;

            assert!(
                matches!(result, Err(P2pSessionError::StreamIdMismatch { .. })),
                "mismatched gap stream_id must be rejected, got {result:?}"
            );
            let guard = db.lock().await;
            assert!(guard.load_gap_markers(stream_id).unwrap().is_empty());
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 0);
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("mismatched_gap_stream_id_rejected timed out");
    }

    #[tokio::test]
    async fn mismatched_subscribe_ok_stream_id_rejected() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.subscribe_ok.stream_id = other_sid_bytes();
            // Keep the forwarder blocked awaiting an ack so the SubscribeOk frame
            // is reliably delivered before the connection closes.
            script.batches = vec![batch(&[1])];

            let forwarder = MockForwarderPeer::start([24; 32], script).await.unwrap();
            let endpoint = test_endpoint(25).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let result =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await;

            assert!(
                matches!(result, Err(P2pSessionError::StreamIdMismatch { .. })),
                "mismatched SubscribeOk stream_id must be rejected, got {result:?}"
            );

            forwarder.shutdown().await;
        })
        .await
        .expect("mismatched_subscribe_ok_stream_id_rejected timed out");
    }

    #[tokio::test]
    async fn conflicting_duplicate_seq_rejected() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // Same seq, divergent immutable payload within one batch: the second
            // record is a conflicting duplicate of the first.
            script.batches = vec![EventBatch {
                records: vec![record_with(1, "original"), record_with(1, "tampered")],
                replay: false,
            }];

            let forwarder = MockForwarderPeer::start([26; 32], script).await.unwrap();
            let endpoint = test_endpoint(27).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let result =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await;

            assert!(
                matches!(
                    result,
                    Err(P2pSessionError::ConflictingDuplicate { seq: 1, .. })
                ),
                "a conflicting duplicate seq must be rejected, got {result:?}"
            );
            // The conflict aborts before any ack is sent.
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("conflicting_duplicate_seq_rejected timed out");
    }

    #[tokio::test]
    async fn benign_duplicate_seq_not_rejected() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // Identical payloads under the same seq are a benign retransmit and
            // must collapse to one row without error.
            script.batches = vec![EventBatch {
                records: vec![record_with(1, "same"), record_with(1, "same")],
                replay: false,
            }];
            script.caught_up_through = Some(1);

            let forwarder = MockForwarderPeer::start([28; 32], script).await.unwrap();
            let endpoint = test_endpoint(29).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                .await
                .unwrap();

            let guard = db.lock().await;
            assert_eq!(guard.load_received_events(stream_id).unwrap().len(), 1);
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 1);
            drop(guard);

            forwarder.shutdown().await;
        })
        .await
        .expect("benign_duplicate_seq_not_rejected timed out");
    }

    #[tokio::test]
    async fn duplicate_seq_with_different_received_unix_ms_rejected() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.batches = vec![EventBatch {
                records: vec![
                    record_with_received(1, "same", 1_700_000_000_100),
                    record_with_received(1, "same", 1_700_000_000_200),
                ],
                replay: false,
            }];

            let forwarder = MockForwarderPeer::start([34; 32], script).await.unwrap();
            let endpoint = test_endpoint(35).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let result =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await;

            assert!(
                matches!(
                    result,
                    Err(P2pSessionError::ConflictingDuplicate { seq: 1, .. })
                ),
                "same seq with different received_unix_ms must be rejected, got {result:?}"
            );
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("duplicate_seq_with_different_received_unix_ms_rejected timed out");
    }

    #[tokio::test]
    async fn over_i64_seq_rejected_without_persist_or_ack() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.batches = vec![EventBatch {
                records: vec![ReadRecord {
                    seq: i64::MAX as u64 + 1,
                    ..record(1)
                }],
                replay: false,
            }];

            let forwarder = MockForwarderPeer::start([36; 32], script).await.unwrap();
            let endpoint = test_endpoint(37).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let result =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await;

            assert!(
                matches!(
                    result,
                    Err(P2pSessionError::NumericOutOfRange {
                        field: "record.seq",
                        ..
                    })
                ),
                "over-i64 seq must be rejected, got {result:?}"
            );
            let guard = db.lock().await;
            assert!(guard.load_received_events(stream_id).unwrap().is_empty());
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 0);
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("over_i64_seq_rejected_without_persist_or_ack timed out");
    }

    #[tokio::test]
    async fn over_i64_gap_rejected_without_marker_or_ack() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.gap_notice = Some(GapNotice {
                stream_id: sid_bytes(),
                requested_after_seq: 0,
                earliest_available_seq: i64::MAX as u64 + 1,
                latest_available_seq: i64::MAX as u64 + 2,
                reason: "retention-window".to_owned(),
            });

            let forwarder = MockForwarderPeer::start([38; 32], script).await.unwrap();
            let endpoint = test_endpoint(39).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let result =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await;

            assert!(
                matches!(
                    result,
                    Err(P2pSessionError::NumericOutOfRange {
                        field: "gap.earliest_available_seq",
                        ..
                    })
                ),
                "over-i64 gap must be rejected, got {result:?}"
            );
            let guard = db.lock().await;
            assert!(guard.load_gap_markers(stream_id).unwrap().is_empty());
            assert_eq!(guard.load_stream_cursor(stream_id).unwrap(), 0);
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("over_i64_gap_rejected_without_marker_or_ack timed out");
    }

    #[tokio::test]
    async fn reconnect_loop_exits_when_shutdown_sender_dropped() {
        let endpoint = test_endpoint(40).await;
        let db = test_db();
        let forwarder = MockForwarderPeer::start([41; 32], base_script())
            .await
            .unwrap();
        let (tx, shutdown_rx) = watch::channel(false);
        drop(tx);
        let params = SessionParams {
            stream_id: stream_id().to_owned(),
            client_hello: test_hello(0),
            mode: SubscribeMode::Replay,
            backoff: BackoffConfig {
                initial: Duration::from_millis(10),
                max: Duration::from_millis(10),
            },
            durable_hint_tx: None,
            reporter: None,
        };

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            run_session_with_reconnect(&endpoint, forwarder.node_addr(), &db, &params, shutdown_rx),
        )
        .await;

        assert!(
            matches!(result, Ok(Ok(()))),
            "closed shutdown channel must exit instead of spinning, got {result:?}"
        );

        forwarder.shutdown().await;
    }

    #[tokio::test]
    async fn disconnect_before_subscribe_ok_reports_pre_open() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // Partition the data plane so the forwarder closes before SubscribeOk.
            script.data_fault = ConnectivityFault::partitioned();

            let forwarder = MockForwarderPeer::start([30; 32], script).await.unwrap();
            let endpoint = test_endpoint(31).await;
            let db = test_db();

            let session = connect_and_hello(&endpoint, forwarder.node_addr(), test_hello(0))
                .await
                .unwrap();
            let outcome =
                run_data_subscription(&session.connection, &db, stream_id, SubscribeMode::Replay)
                    .await
                    .unwrap();

            assert_eq!(
                outcome,
                SessionOutcome::DisconnectedBeforeOpen,
                "a pre-SubscribeOk disconnect must report a pre-open disconnect"
            );

            forwarder.shutdown().await;
        })
        .await
        .expect("disconnect_before_subscribe_ok_reports_pre_open timed out");
    }

    #[tokio::test]
    async fn reconnect_loop_surfaces_durable_error() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let mut script = base_script();
            // A durable data-integrity error: the forwarder hands back a foreign
            // stream_id on SubscribeOk. The reconnect loop must surface it, not
            // retry forever.
            script.subscribe_ok.stream_id = other_sid_bytes();
            // Keep the forwarder blocked awaiting an ack so the SubscribeOk frame
            // is reliably delivered before the connection closes.
            script.batches = vec![batch(&[1])];

            let forwarder = MockForwarderPeer::start([32; 32], script).await.unwrap();
            let endpoint = test_endpoint(33).await;
            let db = test_db();
            let (_tx, shutdown_rx) = watch::channel(false);
            let params = SessionParams {
                stream_id: stream_id().to_owned(),
                client_hello: test_hello(0),
                mode: SubscribeMode::Replay,
                backoff: BackoffConfig {
                    initial: Duration::from_millis(10),
                    max: Duration::from_millis(50),
                },
                durable_hint_tx: None,
                reporter: None,
            };

            let result = run_session_with_reconnect(
                &endpoint,
                forwarder.node_addr(),
                &db,
                &params,
                shutdown_rx,
            )
            .await;

            assert!(
                matches!(result, Err(P2pSessionError::StreamIdMismatch { .. })),
                "reconnect loop must surface durable errors, got {result:?}"
            );

            forwarder.shutdown().await;
        })
        .await
        .expect("reconnect_loop_surfaces_durable_error timed out");
    }

    #[test]
    fn error_classification_separates_transient_from_durable() {
        // Transient transport/read/write failures are retryable.
        assert!(P2pSessionError::Stream("x".to_owned()).is_retryable());
        assert!(P2pSessionError::Read("x".to_owned()).is_retryable());
        assert!(P2pSessionError::Write("x".to_owned()).is_retryable());
        // Durable failures are surfaced, never retried.
        assert!(!P2pSessionError::Decode("x".to_owned()).is_retryable());
        assert!(!P2pSessionError::FrameTooLarge(99).is_retryable());
        assert!(!P2pSessionError::UnexpectedMessage { plane: "data" }.is_retryable());
        assert!(
            !P2pSessionError::StreamIdMismatch {
                expected: stream_id().to_owned(),
                actual: other_sid_bytes(),
            }
            .is_retryable()
        );
        assert!(
            !P2pSessionError::ConflictingDuplicate {
                stream_id: stream_id().to_owned(),
                seq: 1,
            }
            .is_retryable()
        );
        assert!(
            !P2pSessionError::NumericOutOfRange {
                field: "record.seq",
                value: i64::MAX as u64 + 1,
            }
            .is_retryable()
        );
    }
}

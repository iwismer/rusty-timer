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
//! and jumps the cursor to `earliest_available_seq - 1`. On disconnect a data
//! subscription simply returns (a clean EOF is not an error); it does not loop.
//! Reconnection with exponential backoff (1s → 30s) and resume-from-cursor is
//! owned by the per-forwarder connection in [`crate::p2p_forwarder`], which
//! reopens the subscription on the next reconcile.
//!
//! This module provides the testable session core: the control handshake
//! ([`connect_and_hello`]) and a single data-plane subscription
//! ([`run_data_subscription_with_hint`]). Production runtime wiring — owning one
//! control session per forwarder and multiplexing many data streams over it,
//! with reconnection — lives in [`crate::p2p_forwarder`].

use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rt_iroh::{Connection, Endpoint, EndpointAddr, RecvStream, SendStream};
use rt_p2p_protocol::{
    Ack, ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, EventBatch, GapNotice, Hello,
    HelloOk, ProtocolError, StreamCatalog, SubscribeMode, control_c2f, control_f2c, data_c2f,
    data_f2c, decode_frame_len, decode_frame_payload, encode_frame,
};
use tokio::sync::broadcast;

use crate::control_api::AppState;
use crate::stream_key::LocalStreamKey;
use crate::writer::{PreparedGap, PreparedRecord, WriteError, WriterHandle};

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

    pub(crate) fn app_state(&self) -> &Arc<AppState> {
        &self.state
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
    /// A wire epoch was zero or negative; stream epochs start at 1.
    #[error("{field} value {value} must be >= 1")]
    NonPositiveEpoch {
        /// The protobuf field carrying the malformed value.
        field: &'static str,
        /// The non-positive wire value.
        value: i64,
    },
    /// The writer thread rejected or dropped a persist command (group commit
    /// failure or shutdown). Nothing was persisted or acked; resuming from the
    /// persisted cursor is safe, so this is retryable.
    #[error("writer error: {0}")]
    Writer(String),
}

impl P2pSessionError {
    /// Classifies an error as a transient transport failure versus a durable
    /// protocol/data-integrity failure.
    ///
    /// Transient transport/read/write failures (`true`) are the expected
    /// disconnect/EOF signals when a connection drops; resuming from the
    /// persisted cursor is safe. Durable-store errors are transient only when
    /// the underlying SQLite failure is contention (`SQLITE_BUSY` /
    /// `SQLITE_LOCKED`); nothing was acked, so a retry from the persisted
    /// cursor is safe. Durable failures (`false`) — decode, frame-size,
    /// protocol-sequencing, and data-integrity errors, plus non-contention
    /// store errors — indicate the forwarder sent something the session must
    /// not silently ack past (or the store itself is broken).
    ///
    /// [`crate::p2p_forwarder`] uses this as its respawn gate: a data task that
    /// ends with a retryable error (or a clean EOF) is recreated on the next
    /// reconcile pass from the persisted cursor, while a terminal error marks
    /// the stream failed and suppresses respawn until the connection is
    /// re-established or the subscription config changes.
    pub fn is_retryable(&self) -> bool {
        match self {
            P2pSessionError::Iroh(_)
            | P2pSessionError::Stream(_)
            | P2pSessionError::Read(_)
            | P2pSessionError::Write(_)
            | P2pSessionError::Writer(_) => true,
            P2pSessionError::Db(error) => error.is_transient(),
            P2pSessionError::Decode(_)
            | P2pSessionError::FrameTooLarge(_)
            | P2pSessionError::UnexpectedMessage { .. }
            | P2pSessionError::StreamIdMismatch { .. }
            | P2pSessionError::ConflictingDuplicate { .. }
            | P2pSessionError::NumericOutOfRange { .. }
            | P2pSessionError::NonPositiveEpoch { .. } => false,
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

/// An established control-plane session with a forwarder peer.
#[derive(Debug)]
pub struct ControlSession {
    /// The live connection; data streams are opened from it.
    pub connection: Connection,
    /// The negotiated handshake acknowledgement.
    pub hello_ok: HelloOk,
    /// The catalog delivered immediately after the handshake.
    pub catalog: StreamCatalog,
    /// Held to keep the control send stream alive for the connection's lifetime.
    pub control_send: SendStream,
    /// Control receive stream used by connection managers to detect disconnects.
    pub control_recv: RecvStream,
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

pub(crate) fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

pub(crate) async fn write_frame(
    send: &mut SendStream,
    message: &impl Message,
) -> Result<(), P2pSessionError> {
    send.write_all(&encode_frame(message))
        .await
        .map_err(|e| P2pSessionError::Write(e.to_string()))
}

pub(crate) async fn read_frame<M>(recv: &mut RecvStream) -> Result<M, P2pSessionError>
where
    M: Message + Default,
{
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(|e| P2pSessionError::Read(e.to_string()))?;
    let len = decode_frame_len(len_buf).map_err(map_frame_decode_error)?;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload)
        .await
        .map_err(|e| P2pSessionError::Read(e.to_string()))?;
    decode_frame_payload(len_buf, payload.as_slice()).map_err(map_frame_decode_error)
}

fn map_frame_decode_error(error: ProtocolError) -> P2pSessionError {
    match error {
        ProtocolError::FrameTooLarge { length, .. } => P2pSessionError::FrameTooLarge(length),
        ProtocolError::DecodeError { source, .. } => P2pSessionError::Decode(source.to_string()),
        other => P2pSessionError::Decode(other.to_string()),
    }
}

/// Dials `forwarder_addr`, opens the control stream, sends `client_hello`, and
/// reads back the negotiated `HelloOk` plus the `StreamCatalog`.
pub async fn connect_and_hello(
    endpoint: &Endpoint,
    forwarder_addr: EndpointAddr,
    client_hello: Hello,
) -> Result<ControlSession, P2pSessionError> {
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
        control_send: send,
        control_recv: recv,
    })
}

/// Read control-plane frames until the control stream closes.
pub async fn wait_control_stream_closed(recv: &mut RecvStream) -> Result<(), P2pSessionError> {
    loop {
        match read_frame::<ControlF2C>(recv).await {
            Ok(_frame) => {}
            Err(P2pSessionError::Read(_)) => return Ok(()),
            Err(other) => return Err(other),
        }
    }
}

/// Post-commit notification for one durably persisted `EventBatch` (or gap
/// jump). `inserted` contains only rows that were actually inserted this batch
/// (duplicates deduped by `(stream_id, seq)` are excluded), in seq order.
#[derive(Clone, Debug)]
pub struct DurableBatch {
    /// The durable contiguous cursor after this batch.
    pub through_seq: i64,
    /// Scalar facts for the rows actually inserted by this batch.
    pub inserted: Arc<Vec<EventFact>>,
}

/// Scalar projection of a persisted row; no `raw_frame` blob is retained.
#[derive(Clone, Debug)]
pub struct EventFact {
    pub seq: i64,
    pub epoch: i64,
    pub received_unix_ms: i64,
    /// Chip id parsed once from the raw frame at persist time.
    pub chip_id: String,
}

/// Validate an `EventBatch` and convert it into writer-ready records: stream
/// ids checked, u64→i64 converted, `received_unix_ms` defaulted, and the chip
/// id parsed once. A batch carrying a foreign record prepares nothing (and is
/// therefore never persisted or acked).
fn prepare_batch(
    stream_id: &str,
    batch: &EventBatch,
) -> Result<Vec<PreparedRecord>, P2pSessionError> {
    let received_default = now_unix_ms();
    let mut records = Vec::with_capacity(batch.records.len());
    for record in &batch.records {
        check_stream_id(stream_id, &record.stream_id)?;
        let seq = u64_to_i64(record.seq, "record.seq")?;
        let epoch = record.epoch;
        if epoch < 1 {
            return Err(P2pSessionError::NonPositiveEpoch {
                field: "record.epoch",
                value: epoch,
            });
        }
        // A non-zero received_unix_ms is part of the immutable payload for
        // duplicate-conflict checks (it is persisted and used as the announcer
        // ordering key); zero means the forwarder omitted it and the receiver
        // supplies a local default.
        let received_unix_ms_explicit = record.received_unix_ms != 0;
        records.push(PreparedRecord {
            seq,
            epoch,
            raw_frame: record.raw_frame.clone(),
            read_kind: record.read_kind.clone(),
            reader_timestamp: (record.reader_timestamp != 0)
                .then(|| record.reader_timestamp.to_string()),
            received_unix_ms: if received_unix_ms_explicit {
                record.received_unix_ms
            } else {
                received_default
            },
            received_unix_ms_explicit,
            chip_id: crate::ui_events::chip_id_from_raw_frame(&record.raw_frame),
        });
    }
    Ok(records)
}

/// Validate a `GapNotice` into a writer-ready gap.
fn prepare_gap(stream_id: &str, gap: &GapNotice) -> Result<PreparedGap, P2pSessionError> {
    check_stream_id(stream_id, &gap.stream_id)?;
    Ok(PreparedGap {
        requested_after_seq: u64_to_i64(gap.requested_after_seq, "gap.requested_after_seq")?,
        earliest_available_seq: u64_to_i64(
            gap.earliest_available_seq,
            "gap.earliest_available_seq",
        )?,
        latest_available_seq: u64_to_i64(gap.latest_available_seq, "gap.latest_available_seq")?,
        reason: gap.reason.clone(),
        created_unix_ms: now_unix_ms(),
    })
}

fn map_write_error(error: WriteError) -> P2pSessionError {
    match error {
        WriteError::ConflictingDuplicate { stream_id, seq } => {
            P2pSessionError::ConflictingDuplicate { stream_id, seq }
        }
        WriteError::Db(e) => P2pSessionError::Db(e),
        WriteError::Closed(message) => P2pSessionError::Writer(message),
    }
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
/// then pumps `EventBatch` (persist via the writer → cumulative `Ack`) and
/// `GapNotice` (record + jump cursor) until the stream closes.
///
/// All persistence flows through the group-commit `writer`; a reply from it is
/// proof of a durable commit, so the ack that follows preserves the
/// insert-before-ack contract.
///
/// Every stream-scoped frame's `stream_id` is validated against `stream_id`
/// before it is persisted or acked; a mismatch is rejected as
/// [`P2pSessionError::StreamIdMismatch`].
pub async fn run_data_subscription(
    connection: &Connection,
    writer: &WriterHandle,
    wire_stream_id: &str,
    local_stream_key: &LocalStreamKey,
    mode: SubscribeMode,
) -> Result<SessionOutcome, P2pSessionError> {
    // The caller supplies both the forwarder wire stream id and the local
    // durable key; they must describe the same stream so data is not requested
    // under one identity and persisted under another.
    debug_assert_eq!(
        local_stream_key.wire_stream_id(),
        wire_stream_id,
        "local stream key wire stream id must match subscription wire stream id"
    );
    run_data_subscription_with_hint(
        connection,
        writer,
        wire_stream_id,
        local_stream_key,
        mode,
        None,
        None,
    )
    .await
}

/// Like [`run_data_subscription`], but broadcasts the durable contiguous cursor
/// on `durable_hint_tx` after each durable insert / cursor advance (and after a
/// gap-jump). The hint is sent strictly *after* the durable write so the
/// insert-before-ack contract is preserved; downstream workers (durable proxy,
/// DBF, announcer) use it to drain freshly persisted rows.
///
/// `min_after_seq` is an optional subscribe-time floor (from an earliest-epoch
/// override resolved to the epoch's `start_seq - 1`). When it exceeds the
/// durable cursor, the skip is persisted as a cursor jump (gap marker reason
/// `"earliest_epoch_override"`) BEFORE `DataSubscribe` is sent, so the
/// contiguous-cursor/ack contract holds for everything delivered afterwards.
/// The jump is durable: clearing the override later does not re-fetch the
/// skipped seqs.
pub async fn run_data_subscription_with_hint(
    connection: &Connection,
    writer: &WriterHandle,
    wire_stream_id: &str,
    local_stream_key: &LocalStreamKey,
    mode: SubscribeMode,
    durable_hint_tx: Option<&broadcast::Sender<DurableBatch>>,
    min_after_seq: Option<i64>,
) -> Result<SessionOutcome, P2pSessionError> {
    // The caller supplies both the forwarder wire stream id and the local
    // durable key; they must describe the same stream so data is not requested
    // under one identity and persisted under another.
    debug_assert_eq!(
        local_stream_key.wire_stream_id(),
        wire_stream_id,
        "local stream key wire stream id must match subscription wire stream id"
    );
    let cursor = writer
        .load_cursor(local_stream_key.as_str().to_owned())
        .await
        .map_err(map_write_error)?;
    let after_seq = cursor.max(min_after_seq.unwrap_or(0));
    if after_seq > cursor {
        // Persist the deliberate skip before subscribing so the durable
        // cursor stays contiguous with the rows delivered above the floor.
        let through_seq = writer
            .persist_gap(
                local_stream_key.as_str().to_owned(),
                PreparedGap {
                    requested_after_seq: cursor,
                    earliest_available_seq: after_seq + 1,
                    latest_available_seq: 0,
                    reason: "earliest_epoch_override".to_owned(),
                    created_unix_ms: now_unix_ms(),
                },
            )
            .await
            .map_err(map_write_error)?;
        if let Some(tx) = durable_hint_tx {
            let _ = tx.send(DurableBatch {
                through_seq,
                inserted: Arc::new(Vec::new()),
            });
        }
    }

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| P2pSessionError::Stream(e.to_string()))?;

    write_frame(
        &mut send,
        &DataC2F {
            msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                stream_id: stream_id_bytes(wire_stream_id),
                after_seq: i64_to_u64(after_seq),
                mode: mode as i32,
            })),
        },
    )
    .await?;

    match read_frame::<DataF2C>(&mut recv).await {
        Ok(frame) => match frame.msg {
            Some(data_f2c::Msg::SubscribeOk(ok)) => check_stream_id(wire_stream_id, &ok.stream_id)?,
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
                // Durable-first: validate, persist through the writer (the
                // reply resolves only after a successful group commit), then
                // ack through the durable cursor.
                let records = prepare_batch(wire_stream_id, &batch)?;
                let durable = writer
                    .persist_batch(local_stream_key.as_str().to_owned(), records)
                    .await
                    .map_err(map_write_error)?;
                let through_seq = durable.through_seq;
                // Post-commit durable hint: rows are durable before the ack.
                if let Some(tx) = durable_hint_tx {
                    let _ = tx.send(durable);
                }
                send_ack(&mut send, wire_stream_id, through_seq).await?;
            }
            Some(data_f2c::Msg::GapNotice(gap)) => {
                // Record the gap, jump the cursor past the unavailable history,
                // then ack the jumped cursor so the forwarder will not resend
                // the now-skipped seqs.
                let gap = prepare_gap(wire_stream_id, &gap)?;
                let through_seq = writer
                    .persist_gap(local_stream_key.as_str().to_owned(), gap)
                    .await
                    .map_err(map_write_error)?;
                if let Some(tx) = durable_hint_tx {
                    let _ = tx.send(DurableBatch {
                        through_seq,
                        inserted: Arc::new(Vec::new()),
                    });
                }
                send_ack(&mut send, wire_stream_id, through_seq).await?;
            }
            // CaughtUp carries no durable state to persist here, but it is
            // stream-scoped: validate the stream_id and keep listening for
            // further live frames.
            Some(data_f2c::Msg::CaughtUp(caught_up)) => {
                check_stream_id(wire_stream_id, &caught_up.stream_id)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_api::ConnectionState;
    use rt_iroh::EndpointBuilder;
    use rt_p2p_protocol::{MAX_FRAME_BYTES, ReadRecord, StreamCatalog, StreamEntry, SubscribeOk};
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

    fn local_stream_key() -> LocalStreamKey {
        LocalStreamKey::new("test-forwarder", STREAM_ID)
    }

    fn sid_bytes() -> Vec<u8> {
        STREAM_ID.as_bytes().to_vec()
    }

    /// A `stream_id` for a *different* stream than the one the session
    /// subscribes to, used to exercise stream-id validation.
    fn other_sid_bytes() -> Vec<u8> {
        OTHER_STREAM_ID.as_bytes().to_vec()
    }

    /// Writer + Db pair sharing one temp-file DB (an in-memory DB cannot be
    /// shared with the writer's own connection). Keep the TempDir alive.
    struct TestStore {
        writer: WriterHandle,
        db: Arc<tokio::sync::Mutex<crate::db::Db>>,
        _dir: tempfile::TempDir,
    }

    fn test_store() -> TestStore {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-test.sqlite3");
        let db = crate::db::Db::open(&path).unwrap();
        let (writer, _thread) =
            crate::writer::spawn_writer(&path, crate::writer::WriterConfig::default()).unwrap();
        TestStore {
            writer,
            db: Arc::new(tokio::sync::Mutex::new(db)),
            _dir: dir,
        }
    }

    fn test_hello(catalog_generation: u64) -> Hello {
        Hello {
            min_minor: rt_p2p_protocol::PROTOCOL_MINOR,
            max_minor: rt_p2p_protocol::PROTOCOL_MINOR,
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
                epoch_summaries: Vec::new(),
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
            batch_gate: None,
            echo_subscribed_stream_id: false,
            close_connection_after_data: false,
            control_events: Vec::new(),
            control_pings: 0,
            control_ping_interval: Duration::from_millis(50),
            config_get_json: String::new(),
            config_restart_needed: false,
            respond_to_config_requests: true,
            reader_control_info_json: None,
            respond_to_reader_control_requests: true,
        }
    }

    async fn test_endpoint(seed: u8) -> Endpoint {
        EndpointBuilder::test([seed; 32]).bind().await.unwrap()
    }

    fn reporter_state() -> (Arc<AppState>, Arc<SessionStatusReporter>) {
        let (state, _shutdown_rx) = AppState::new(
            crate::db::Db::open_in_memory().unwrap(),
            "recv-test".to_owned(),
        );
        let reporter = Arc::new(SessionStatusReporter::new(Arc::clone(&state)));
        (state, reporter)
    }

    #[tokio::test]
    async fn reporter_tracks_per_forwarder_states() {
        let (state, _shutdown_rx) = AppState::new(
            crate::db::Db::open_in_memory().unwrap(),
            "recv-test".to_owned(),
        );
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
            .storage
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
    async fn guard_drop_fallback_respects_disconnect_intent() {
        // Dropping the last control guard for a forwarder whose persisted
        // intent is disconnect must fall the aggregate to Disconnected, not
        // Connecting: the sync fallback runs before the spawned async
        // recompute, and must not transiently report a deliberately
        // disconnected forwarder as trying.
        let (state, reporter) = reporter_state();
        let guard = reporter.on_control_connected("fwd-1").await;
        assert_eq!(
            state.signals.connection_state.borrow().clone(),
            ConnectionState::Connected
        );

        crate::control_api::disconnect_forwarder(&state, "fwd-1".to_owned())
            .await
            .unwrap();

        drop(guard);

        // No await between the drop and this assert: on the test's
        // single-threaded runtime the spawned async recompute has not run
        // yet, so this observes the sync fallback's result.
        assert_eq!(
            state.signals.connection_state.borrow().clone(),
            ConnectionState::Disconnected
        );
    }

    #[tokio::test]
    async fn connected_session_guard_releases_on_cancellation() {
        // A connected session whose control guard is dropped without a clean
        // return (e.g. the connection task is cancelled mid-session) must still
        // release its live-session count and fall the aggregate state back to
        // Connecting — otherwise the badge can stay falsely "Connected" after
        // the last session ends.
        let (state, reporter) = reporter_state();
        let guard = reporter.on_control_connected("fwd-1").await;
        assert_eq!(
            state.signals.connection_state.borrow().clone(),
            ConnectionState::Connected
        );

        drop(guard); // simulates connection-task cancellation mid-session

        assert_eq!(
            state.signals.connection_state.borrow().clone(),
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
            state.signals.connection_state.borrow().clone(),
            ConnectionState::Connected
        );

        drop(g2);
        assert_eq!(
            state.signals.connection_state.borrow().clone(),
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await
            .unwrap();

            // Every received record is durable.
            let guard = store.db.lock().await;
            let seqs: Vec<i64> = guard
                .load_received_events(local_stream_key().as_str())
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect();
            assert_eq!(seqs, vec![1, 2, 4]);
            // Cursor tracks the durable *contiguous* prefix, not the latest seq.
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                2
            );
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await
            .unwrap();

            let guard = store.db.lock().await;
            let markers = guard.load_gap_markers(local_stream_key().as_str()).unwrap();
            assert_eq!(markers.len(), 1);
            assert_eq!(markers[0].requested_after_seq, 0);
            assert_eq!(markers[0].earliest_available_seq, 15);
            assert_eq!(markers[0].latest_available_seq, 20);
            // Cursor jumps to earliest_available_seq - 1.
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                14
            );
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
            let store = test_store();

            // First connection: receive [1, 2], cursor advances to 2.
            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await
            .unwrap();
            assert_eq!(
                store
                    .db
                    .lock()
                    .await
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                2
            );
            drop(session);

            // Reconnect: a fresh connection must resume from the persisted cursor.
            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await
            .unwrap();

            // The forwarder re-sent [1, 2]; dedup keeps a single row per seq.
            let guard = store.db.lock().await;
            assert_eq!(
                guard
                    .load_received_events(local_stream_key().as_str())
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                2
            );
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
    async fn override_floor_raises_after_seq_and_jumps_cursor() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // Epoch 2 starts at seq 11; the receiver should only fetch 11+.
            script.subscribe_ok = SubscribeOk {
                stream_id: sid_bytes(),
                earliest_available_seq: 1,
                latest_seq_at_open: 12,
            };
            script.batches = vec![batch(&[11, 12])];
            script.caught_up_through = Some(12);

            let forwarder = MockForwarderPeer::start([61; 32], script).await.unwrap();
            let endpoint = test_endpoint(62).await;
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription_with_hint(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
                None,
                Some(10),
            )
            .await
            .unwrap();

            // The subscribe carried the floor, not the empty cursor.
            let subscribes = forwarder.subscribes();
            assert_eq!(subscribes.len(), 1);
            assert_eq!(
                subscribes[0].after_seq, 10,
                "override floor must raise the requested after_seq"
            );

            let guard = store.db.lock().await;
            // The skip was recorded durably before any batch was acked.
            let markers = guard.load_gap_markers(local_stream_key().as_str()).unwrap();
            assert_eq!(markers.len(), 1);
            assert_eq!(markers[0].reason, "earliest_epoch_override");
            assert_eq!(markers[0].requested_after_seq, 0, "jump from empty cursor");
            assert_eq!(markers[0].earliest_available_seq, 11);
            assert!(markers[0].created_unix_ms > 0, "marker carries a timestamp");
            // Rows 11..12 landed; nothing below the floor exists locally.
            let events = guard
                .load_received_events(local_stream_key().as_str())
                .unwrap();
            assert_eq!(
                events.iter().map(|e| e.seq).collect::<Vec<_>>(),
                vec![11, 12]
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                12
            );
            drop(guard);

            // A floor at or below the durable cursor is a no-op: no second
            // gap marker, resume from the cursor as usual.
            let session2 = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription_with_hint(
                &session2.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
                None,
                Some(10),
            )
            .await
            .unwrap();
            let subscribes = forwarder.subscribes();
            assert_eq!(subscribes.len(), 2);
            assert_eq!(
                subscribes[1].after_seq, 12,
                "cursor above the floor wins on resume"
            );
            let guard = store.db.lock().await;
            assert_eq!(
                guard
                    .load_gap_markers(local_stream_key().as_str())
                    .unwrap()
                    .len(),
                1,
                "no duplicate override gap marker on resume"
            );
            drop(guard);

            forwarder.shutdown().await;
        })
        .await
        .expect("override_floor_raises_after_seq_and_jumps_cursor timed out");
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await
            .unwrap();

            let guard = store.db.lock().await;
            let seqs: Vec<i64> = guard
                .load_received_events(local_stream_key().as_str())
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect();
            assert_eq!(
                seqs,
                vec![1, 2, 3],
                "duplicate seqs collapse to one row each"
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                3
            );
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
            let store = test_store();
            let (hint_tx, mut hint_rx) = broadcast::channel(16);

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription_with_hint(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
                Some(&hint_tx),
                None,
            )
            .await
            .unwrap();

            // The contiguous durable cursor (2) is broadcast as a post-commit hint,
            // carrying the facts for the rows inserted by this batch.
            let hint = hint_rx.recv().await.unwrap();
            assert_eq!(hint.through_seq, 2);
            assert_eq!(hint.inserted.len(), 2);
            assert_eq!(hint.inserted[0].seq, 1);
            assert_eq!(hint.inserted[1].seq, 2);

            forwarder.shutdown().await;
        })
        .await
        .expect("durable_hint_broadcast_after_persist timed out");
    }

    #[test]
    fn backoff_default_bounds() {
        let config = BackoffConfig::default();
        assert_eq!(config.initial, Duration::from_secs(1));
        assert_eq!(config.max, Duration::from_secs(30));
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await;

            assert!(
                matches!(result, Err(P2pSessionError::StreamIdMismatch { .. })),
                "mismatched event stream_id must be rejected, got {result:?}"
            );
            // Nothing was persisted and nothing was acked.
            let guard = store.db.lock().await;
            assert!(
                guard
                    .load_received_events(local_stream_key().as_str())
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                0
            );
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await;

            assert!(
                matches!(result, Err(P2pSessionError::StreamIdMismatch { .. })),
                "mismatched gap stream_id must be rejected, got {result:?}"
            );
            let guard = store.db.lock().await;
            assert!(
                guard
                    .load_gap_markers(local_stream_key().as_str())
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                0
            );
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
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
    async fn batch_persists_atomically() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            // Record 3 of 5 is a conflicting duplicate of record 1 (same seq,
            // divergent payload). The whole batch must roll back: zero rows
            // persisted (previously rows 1-2 committed before the conflict was
            // detected) and no ack sent. At-least-once redelivery covers the
            // rolled-back rows.
            script.batches = vec![EventBatch {
                records: vec![
                    record_with(1, "one"),
                    record_with(2, "two"),
                    record_with(1, "tampered"),
                    record_with(4, "four"),
                    record_with(5, "five"),
                ],
                replay: false,
            }];

            let forwarder = MockForwarderPeer::start([62; 32], script).await.unwrap();
            let endpoint = test_endpoint(63).await;
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await;

            assert!(
                matches!(
                    result,
                    Err(P2pSessionError::ConflictingDuplicate { seq: 1, .. })
                ),
                "the conflicting duplicate must abort the batch, got {result:?}"
            );
            let guard = store.db.lock().await;
            assert!(
                guard
                    .load_received_events(local_stream_key().as_str())
                    .unwrap()
                    .is_empty(),
                "a batch with a conflicting duplicate must persist nothing"
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                0
            );
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("batch_persists_atomically timed out");
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await
            .unwrap();

            let guard = store.db.lock().await;
            assert_eq!(
                guard
                    .load_received_events(local_stream_key().as_str())
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                1
            );
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
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
            let guard = store.db.lock().await;
            assert!(
                guard
                    .load_received_events(local_stream_key().as_str())
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                0
            );
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("over_i64_seq_rejected_without_persist_or_ack timed out");
    }

    #[tokio::test]
    async fn negative_epoch_rejected_without_persist_or_ack() {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let stream_id = stream_id();
            let mut script = base_script();
            script.batches = vec![EventBatch {
                records: vec![ReadRecord {
                    epoch: -1,
                    ..record(1)
                }],
                replay: false,
            }];

            let forwarder = MockForwarderPeer::start([40; 32], script).await.unwrap();
            let endpoint = test_endpoint(41).await;
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
            .await;

            assert!(
                matches!(
                    result,
                    Err(P2pSessionError::NonPositiveEpoch {
                        field: "record.epoch",
                        value: -1,
                    })
                ),
                "negative epoch must be rejected, got {result:?}"
            );
            let guard = store.db.lock().await;
            assert!(
                guard
                    .load_received_events(local_stream_key().as_str())
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                0
            );
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("negative_epoch_rejected_without_persist_or_ack timed out");
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let result = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
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
            let guard = store.db.lock().await;
            assert!(
                guard
                    .load_gap_markers(local_stream_key().as_str())
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                guard
                    .load_stream_cursor(local_stream_key().as_str())
                    .unwrap(),
                0
            );
            drop(guard);
            assert!(forwarder.acks().is_empty());

            forwarder.shutdown().await;
        })
        .await
        .expect("over_i64_gap_rejected_without_marker_or_ack timed out");
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
            let store = test_store();

            let session = connect_and_hello(&endpoint, forwarder.endpoint_addr(), test_hello(0))
                .await
                .unwrap();
            let outcome = run_data_subscription(
                &session.connection,
                &store.writer,
                stream_id,
                &local_stream_key(),
                SubscribeMode::Replay,
            )
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

    #[test]
    fn error_classification_separates_transient_from_durable() {
        // Transient transport/read/write failures are retryable.
        assert!(P2pSessionError::Stream("x".to_owned()).is_retryable());
        assert!(P2pSessionError::Read("x".to_owned()).is_retryable());
        assert!(P2pSessionError::Write("x".to_owned()).is_retryable());
        assert!(P2pSessionError::Writer("x".to_owned()).is_retryable());
        // Db errors are transient only for SQLite contention (busy/locked);
        // any other durable-store failure is terminal.
        let sqlite_db_error = |code: std::os::raw::c_int| {
            crate::db::DbError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                None,
            ))
        };
        assert!(P2pSessionError::Db(sqlite_db_error(rusqlite::ffi::SQLITE_BUSY)).is_retryable());
        assert!(P2pSessionError::Db(sqlite_db_error(rusqlite::ffi::SQLITE_LOCKED)).is_retryable());
        assert!(
            !P2pSessionError::Db(sqlite_db_error(rusqlite::ffi::SQLITE_CORRUPT)).is_retryable()
        );
        assert!(!P2pSessionError::Db(crate::db::DbError::ProfileMissing).is_retryable());
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
        assert!(
            !P2pSessionError::NonPositiveEpoch {
                field: "record.epoch",
                value: 0,
            }
            .is_retryable()
        );
    }
}

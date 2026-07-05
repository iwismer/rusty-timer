//! Receiver-side announcer push plumbing.
//!
//! Reads durable `received_events` from the local store and pushes "announcer
//! rows" to a downstream announcer sink (in production, a server
//! `/announcer/rows` endpoint). The transport is abstracted behind
//! [`AnnouncerPushClient`] and participant identity behind
//! [`ParticipantResolver`] so this module never depends on server code being
//! present in this worktree, and so tests can prove behavior with mocks.
//!
//! Contract:
//! * **Idempotency key `(forwarder_endpoint_id, stream_id, seq)`.** Rows are
//!   pushed in batches that carry the composite stream identity (decoded from
//!   the receiver-local [`LocalStreamKey`] at this boundary — the encoded key
//!   never crosses HTTP). Once an event is pushed it is marked in the durable
//!   store, so a repush never re-sends an already-delivered row.
//! * **Ordering key `received_unix_ms`.** Rows are emitted in receipt order.
//! * **Fenced `announcer_source_generation`.** Every push carries the current
//!   source generation. A push whose generation is older than the highest
//!   generation already accepted for the stream is rejected without sending, so
//!   stale generations never reach the announcer. Pushes are also serialized per
//!   stream in-process, so overlapping calls cannot let an older generation
//!   proceed after a newer one has been accepted by another local push.
//! * **Resolved participant name when available.** Names/bibs are resolved
//!   locally from race/participant data via the injected resolver.

use rt_server_api::announcer::{
    MAX_ANNOUNCER_DISPLAY_NAME_LEN, MAX_ANNOUNCER_DIVISION_LEN, MAX_ANNOUNCER_ID_LEN,
    MAX_PUSH_ROWS, PushRow, PushRowsRequest, TakeoverResponse,
};
use rt_server_api::register::{RegisterRequest, RegisterResponse};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex as StdMutex, MutexGuard as StdMutexGuard, PoisonError};
use std::time::Duration;
use thiserror::Error;

/// Connect/request timeout for all blocking server HTTP calls. Bounds each
/// call so a hung server cannot wedge the runtime (including shutdown).
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

use crate::db::{AnnouncerGenerationAcceptance, Db, DbError};
use crate::stream_key::LocalStreamKey;
use crate::ui_events::chip_id_from_raw_frame;

/// A participant identity resolved from local race/participant data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedParticipant {
    pub bib: String,
    pub name: Option<String>,
    /// Division display name, when the participant has a known division.
    pub division: Option<String>,
}

/// Narrow resolver from a chip ID to a participant identity.
///
/// The runtime injects a real lookup (backed by the existing chip→participant
/// map); tests inject a closure or fixture map. Returning `None` means the chip
/// is not yet known, in which case the row is still pushed without a name.
pub trait ParticipantResolver {
    fn resolve(&self, chip_id: &str) -> Option<ResolvedParticipant>;
}

impl<F> ParticipantResolver for F
where
    F: Fn(&str) -> Option<ResolvedParticipant>,
{
    fn resolve(&self, chip_id: &str) -> Option<ResolvedParticipant> {
        (self)(chip_id)
    }
}

/// A single announcer row pushed downstream as part of an [`AnnouncerBatch`].
///
/// `seq` (with the batch's composite stream identity) forms the idempotency
/// key and `received_unix_ms` is the ordering key. `bib`/`name` are populated
/// when the chip resolves to a known participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncerRow {
    pub seq: i64,
    pub received_unix_ms: i64,
    pub chip_id: String,
    pub bib: Option<String>,
    pub name: Option<String>,
    pub division: Option<String>,
}

/// A batch of announcer rows sharing one composite stream identity and one
/// source generation — by construction, since the identity and generation are
/// batch-level fields.
///
/// `forwarder_endpoint_id`/`stream_id` are the DECODED halves of the
/// receiver-local [`LocalStreamKey`]; the encoded form (with its U+001F
/// separator) never crosses HTTP. `max_list_size` is the receiver-configured
/// cap on the number of rows the server keeps visible in the public announcer
/// feed; it rides every push so the server can re-trim its runtime when the
/// operator changes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncerBatch<'a> {
    pub forwarder_endpoint_id: &'a str,
    pub stream_id: &'a str,
    pub announcer_source_generation: i64,
    pub max_list_size: u32,
    pub rows: &'a [AnnouncerRow],
}

/// Transport abstraction for delivering announcer row batches downstream.
///
/// One `push` call is one downstream request; callers bound `batch.rows` to
/// [`MAX_PUSH_ROWS`].
pub trait AnnouncerPushClient {
    fn push(&self, batch: &AnnouncerBatch<'_>) -> Result<(), AnnouncerPushError>;
}

/// Shared blocking HTTP client for announcer row pushes.
///
/// One client for the life of the process (≥ the life of every push worker) so
/// backlog drains reuse a single keep-alive connection pool instead of paying
/// connection setup per request. It lives in a `static` because
/// [`push_announcer_rows`] runs on blocking threads while the owning worker
/// structs live in async tasks: a `reqwest::blocking::Client` must be
/// constructed and dropped outside async contexts, and a `static` is
/// initialized on first use (a blocking thread) and never dropped.
static PUSH_HTTP_CLIENT: LazyLock<Result<reqwest::blocking::Client, String>> =
    LazyLock::new(|| {
        reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())
    });

/// Real HTTP transport for the server `/announcer/rows` endpoint.
///
/// [`push`] posts the whole batch as ONE `PushRowsRequest` (bearer auth) over
/// the shared [`PUSH_HTTP_CLIENT`]. The bearer token is held privately and
/// never logged.
///
/// [`push`]: AnnouncerPushClient::push
pub struct ServerAnnouncerClient {
    rows_url: String,
    token: String,
}

impl ServerAnnouncerClient {
    /// Build a client targeting `base_url` (e.g. `http://127.0.0.1:8080`).
    pub fn new(base_url: &str, token: impl Into<String>) -> Self {
        Self {
            rows_url: format!("{}/announcer/rows", base_url.trim_end_matches('/')),
            token: token.into(),
        }
    }
}

fn clamp_wire_string(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_owned();
    }
    let mut boundary = max_len;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

fn checked_identity_field(
    field: &str,
    value: &str,
    max_len: usize,
) -> Result<String, AnnouncerPushError> {
    if value.len() <= max_len {
        Ok(value.to_owned())
    } else {
        Err(AnnouncerPushError::BadRequest(format!(
            "{field} length {} exceeds max {max_len}",
            value.len()
        )))
    }
}

/// Build the `PushRowsRequest` body posted to the server `/announcer/rows`
/// endpoint. Extracted so the wire shape (composite stream identity, division,
/// bib fallback labels) is unit testable without a live server. Server `bib`
/// is an optional integer; a non-numeric bib is sent as null rather than
/// failing the whole push.
fn push_rows_request_body(
    batch: &AnnouncerBatch<'_>,
) -> Result<PushRowsRequest, AnnouncerPushError> {
    let announcer_source_generation =
        u64::try_from(batch.announcer_source_generation).map_err(|_| {
            AnnouncerPushError::Transport(
                "announcer_source_generation must be non-negative".to_owned(),
            )
        })?;
    let rows = batch
        .rows
        .iter()
        .map(|row| {
            let seq = u64::try_from(row.seq).map_err(|_| {
                AnnouncerPushError::Transport("seq must be non-negative".to_owned())
            })?;
            let display_name = row.name.clone().unwrap_or_else(|| {
                row.bib
                    .as_deref()
                    .map_or_else(String::new, |bib| format!("Unknown Participant {bib}"))
            });
            // Clamp receiver-sourced strings at the HTTP boundary. Truncating a
            // too-long imported participant field is preferable to letting one
            // poison row wedge this stream's push backlog forever.
            Ok(PushRow {
                seq,
                chip_id: clamp_wire_string(&row.chip_id, MAX_ANNOUNCER_ID_LEN),
                bib: row.bib.as_deref().and_then(|b| b.parse::<i32>().ok()),
                display_name: clamp_wire_string(&display_name, MAX_ANNOUNCER_DISPLAY_NAME_LEN),
                reader_timestamp: None,
                received_unix_ms: row.received_unix_ms,
                division: row
                    .division
                    .as_deref()
                    .map(|division| clamp_wire_string(division, MAX_ANNOUNCER_DIVISION_LEN)),
            })
        })
        .collect::<Result<Vec<_>, AnnouncerPushError>>()?;
    Ok(PushRowsRequest {
        announcer_source_generation,
        forwarder_endpoint_id: checked_identity_field(
            "forwarder_endpoint_id",
            batch.forwarder_endpoint_id,
            MAX_ANNOUNCER_ID_LEN,
        )?,
        stream_id: checked_identity_field("stream_id", batch.stream_id, MAX_ANNOUNCER_ID_LEN)?,
        rows,
        max_list_size: Some(batch.max_list_size),
    })
}

impl AnnouncerPushClient for ServerAnnouncerClient {
    fn push(&self, batch: &AnnouncerBatch<'_>) -> Result<(), AnnouncerPushError> {
        if batch.rows.is_empty() {
            return Ok(());
        }
        let client = PUSH_HTTP_CLIENT
            .as_ref()
            .map_err(|e| AnnouncerPushError::Transport(e.clone()))?;
        let body = push_rows_request_body(batch)?;
        let response = client
            .post(&self.rows_url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(classify_push_failure(status, body));
        }
        Ok(())
    }
}

/// Register this receiver endpoint with the server `/register` endpoint
/// (`device_kind = "receiver"`), using `token` as the bearer (an enrollment
/// voucher on first boot, the provisioning token during migration, or the
/// device's own minted token for an idempotent re-register).
///
/// The configured `receiver_id` is sent as the device's self-reported
/// `display_name` so the server's admin approval UI can show a human-friendly
/// name instead of the opaque endpoint ID. A blank receiver ID is omitted.
///
/// Returns the server-minted per-device token when one is issued (first mint or
/// rotation), or `None` for an idempotent re-register that mints nothing. The
/// bearer token is never logged.
pub fn register_receiver_with_server(
    base_url: &str,
    token: &str,
    endpoint_id: &str,
    receiver_id: &str,
) -> Result<Option<String>, AnnouncerPushError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    let trimmed_id = receiver_id.trim();
    let body = serde_json::to_value(RegisterRequest {
        endpoint_id: endpoint_id.to_owned(),
        device_kind: "receiver".to_owned(),
        display_name: (!trimmed_id.is_empty()).then(|| trimmed_id.to_owned()),
    })
    .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    let response = client
        .post(format!("{}/register", base_url.trim_end_matches('/')))
        .bearer_auth(token)
        .json(&body)
        .send()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    if !response.status().is_success() {
        return Err(AnnouncerPushError::Transport(format!(
            "server /register returned {}",
            response.status()
        )));
    }
    let value: RegisterResponse = response
        .json()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    Ok(value.device_token)
}

/// Take over the announcer source generation via server `/announcer/takeover`
/// and return the freshly-fenced generation.
pub fn takeover_announcer_generation(
    base_url: &str,
    token: &str,
) -> Result<i64, AnnouncerPushError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    let response = client
        .post(format!(
            "{}/announcer/takeover",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({}))
        .send()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    if !response.status().is_success() {
        return Err(AnnouncerPushError::Transport(format!(
            "server /announcer/takeover returned {}",
            response.status()
        )));
    }
    let value: TakeoverResponse = response
        .json()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    i64::try_from(value.announcer_source_generation).map_err(|_| {
        AnnouncerPushError::Transport(
            "server /announcer/takeover response generation out of range".to_owned(),
        )
    })
}

/// One stream entry from the server `GET /forwarders` discovery feed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForwarderDiscoveryStream {
    pub stream_id: String,
    pub epoch: i64,
    pub next_seq: i64,
}

/// One forwarder entry from the server `GET /forwarders` discovery feed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForwarderDiscoveryEntry {
    pub endpoint_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub direct_addrs: Vec<String>,
    #[serde(default)]
    pub streams: Vec<ForwarderDiscoveryStream>,
}

#[derive(Debug, Deserialize)]
struct ForwardersResponse {
    #[serde(default)]
    forwarders: Vec<ForwarderDiscoveryEntry>,
}

/// Fetch the approved-forwarder discovery feed from the server
/// `GET /forwarders` endpoint (bearer auth). Blocking; intended to run inside a
/// blocking task. The bearer token is never logged.
pub fn fetch_approved_forwarders(
    base_url: &str,
    token: &str,
) -> Result<Vec<ForwarderDiscoveryEntry>, AnnouncerPushError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    let response = client
        .get(format!("{}/forwarders", base_url.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    if !response.status().is_success() {
        return Err(AnnouncerPushError::Transport(format!(
            "server /forwarders returned {}",
            response.status()
        )));
    }
    let body: ForwardersResponse = response
        .json()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    Ok(body.forwarders)
}

type StreamPushLock = Arc<StdMutex<()>>;
type StreamPushLocks = HashMap<String, StreamPushLock>;

static ANNOUNCER_PUSH_LOCKS: LazyLock<StdMutex<StreamPushLocks>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn lock_unpoisoned<T>(mutex: &StdMutex<T>) -> StdMutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn announcer_push_lock(stream_id: &str) -> StreamPushLock {
    let mut locks = lock_unpoisoned(&ANNOUNCER_PUSH_LOCKS);
    locks
        .entry(stream_id.to_owned())
        .or_insert_with(|| Arc::new(StdMutex::new(())))
        .clone()
}

#[derive(Debug, Error)]
pub enum AnnouncerPushError {
    #[error("db: {0}")]
    Db(#[from] DbError),
    #[error("announcer push transport: {0}")]
    Transport(String),
    /// The server rejected the push with 400, or the receiver built an invalid
    /// identity-bearing request. The batch is left unmarked; the push worker
    /// parks this stream until a generation/config rebuild creates a fresh
    /// worker rather than retrying the same poisoned batch forever.
    #[error("announcer push bad request: {0}")]
    BadRequest(String),
    /// The server rejected the push with 409: our announcer source generation
    /// diverged from the server's (server DB reset, or another receiver took
    /// over). The caller must re-run register + takeover to re-fence before
    /// pushing again. Carries the server's response text for logging.
    #[error("announcer generation stale on server: {0}")]
    StaleGeneration(String),
}

/// Classify a non-success `/announcer/rows` response. A 409 means the server's
/// generation fence rejected our generation ([`AnnouncerPushError::StaleGeneration`],
/// recoverable only via re-takeover). A 400 means receiver-side request
/// construction is invalid; leave rows pending rather than marking a poisoned
/// batch as pushed. The worker logs the response body once and parks the stream.
/// Other statuses are transient transport failures worth a plain retry.
pub(crate) fn classify_push_failure(
    status: reqwest::StatusCode,
    body: String,
) -> AnnouncerPushError {
    if status == reqwest::StatusCode::CONFLICT {
        AnnouncerPushError::StaleGeneration(body)
    } else if status == reqwest::StatusCode::BAD_REQUEST {
        AnnouncerPushError::BadRequest(body)
    } else {
        AnnouncerPushError::Transport(format!("server /announcer/rows returned {status}: {body}"))
    }
}

/// Result of a [`push_announcer_rows`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// The push was accepted; `rows` newly-pushed rows were sent and marked.
    Pushed { rows: usize },
    /// The push carried a generation older than the fenced generation and was
    /// rejected without sending anything.
    StaleGeneration { fenced: i64, attempted: i64 },
}

/// Push not-yet-pushed durable events for `stream_key` to the announcer.
///
/// Local durable state (events, fence, push markers) stays keyed by the
/// ENCODED [`LocalStreamKey`]; the key is decoded into its composite
/// `(forwarder_endpoint_id, wire stream_id)` halves here, at the push
/// boundary, so the encoded form never reaches the transport.
///
/// Returns [`PushOutcome::StaleGeneration`] without sending if `generation` is
/// older than the highest generation already fenced for the stream. Otherwise
/// the fence is raised, pending events are resolved into rows, pushed via
/// `client` in batches of at most [`MAX_PUSH_ROWS`] (one transport request and
/// one durable mark transaction per batch), and only marked pushed after a
/// successful push — so a failed transport leaves rows unmarked for a later
/// retry, and a successful push followed by a repush sends nothing
/// (idempotent).
pub fn push_announcer_rows(
    db: &mut Db,
    client: &dyn AnnouncerPushClient,
    resolver: &dyn ParticipantResolver,
    stream_key: &LocalStreamKey,
    generation: i64,
    pushed_unix_ms: i64,
) -> Result<PushOutcome, AnnouncerPushError> {
    let stream_id = stream_key.as_str();
    let stream_lock = announcer_push_lock(stream_id);
    let _stream_guard = lock_unpoisoned(&stream_lock);

    match db.accept_announcer_generation(stream_id, generation)? {
        AnnouncerGenerationAcceptance::Current { .. } => {}
        AnnouncerGenerationAcceptance::Stale { current, attempted } => {
            return Ok(PushOutcome::StaleGeneration {
                fenced: current,
                attempted,
            });
        }
    }

    // Drain unpushed rows in server-batch-sized chunks: bounds memory when
    // enabling the announcer against a large backlog (rows carry raw frames)
    // and each chunk is exactly one transport request downstream.
    let max_list_size = db.load_announcer_max_list_size()?;
    let mut total_marked = 0usize;
    loop {
        let pending = db.load_unpushed_announcer_events_limited(stream_id, MAX_PUSH_ROWS)?;
        if pending.is_empty() {
            break;
        }
        let fetched = pending.len();

        let rows: Vec<AnnouncerRow> = pending
            .iter()
            .map(|event| {
                let chip_id = chip_id_from_raw_frame(&event.raw_frame);
                let resolved = resolver.resolve(&chip_id);
                AnnouncerRow {
                    seq: event.seq,
                    received_unix_ms: event.received_unix_ms,
                    chip_id,
                    bib: resolved.as_ref().map(|p| p.bib.clone()),
                    name: resolved.as_ref().and_then(|p| p.name.clone()),
                    division: resolved.and_then(|p| p.division),
                }
            })
            .collect();

        // Re-check the fence per chunk (a newer generation can take over
        // while this pass runs); rows already pushed under the then-current
        // generation stay marked — same at-least-once semantics as across
        // passes.
        if let Some(fenced) = db.load_announcer_fence(stream_id)?
            && generation < fenced
        {
            return Ok(PushOutcome::StaleGeneration {
                fenced,
                attempted: generation,
            });
        }

        client.push(&AnnouncerBatch {
            forwarder_endpoint_id: stream_key.endpoint_id(),
            stream_id: stream_key.wire_stream_id(),
            announcer_source_generation: generation,
            max_list_size,
            rows: &rows,
        })?;

        // Only mark after a successful push, so a failed transport leaves
        // rows pending for a later retry (at-least-once + idempotency key
        // downstream). One transaction per chunk — per-row autocommits blew
        // the fsync budget on slow disks whenever the announcer was enabled.
        let seqs: Vec<i64> = pending.iter().map(|event| event.seq).collect();
        total_marked += db.mark_announcer_pushed_batch(stream_id, &seqs, pushed_unix_ms)?;

        if fetched < MAX_PUSH_ROWS {
            break;
        }
    }
    Ok(PushOutcome::Pushed { rows: total_marked })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AnnouncerGenerationAcceptance, ReceivedEventInsert};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn conflict_status_maps_to_stale_generation_and_bad_request_is_terminal() {
        assert!(matches!(
            classify_push_failure(reqwest::StatusCode::CONFLICT, "gen".into()),
            AnnouncerPushError::StaleGeneration(_)
        ));
        assert!(matches!(
            classify_push_failure(
                reqwest::StatusCode::BAD_REQUEST,
                "display_name too long".into()
            ),
            AnnouncerPushError::BadRequest(_)
        ));
        assert!(matches!(
            classify_push_failure(reqwest::StatusCode::BAD_GATEWAY, "x".into()),
            AnnouncerPushError::Transport(_)
        ));
    }

    fn sample_raw_frame() -> Vec<u8> {
        // chip_id_from_raw_frame extracts "000000012345" from this frame.
        b"aa400000000123450a2a01123018455927a7".to_vec()
    }

    fn insert_event(db: &Db, stream_id: &str, seq: i64, raw: &[u8], received_unix_ms: i64) {
        db.insert_received_event(&ReceivedEventInsert {
            stream_id,
            seq,
            epoch: 1,
            raw_frame: raw,
            read_kind: "chip",
            reader_timestamp: None,
            received_unix_ms,
            dbf_delivered_unix_ms: None,
            chip_id: None,
        })
        .unwrap();
    }

    fn key(endpoint_id: &str, wire_stream_id: &str) -> LocalStreamKey {
        LocalStreamKey::new(endpoint_id, wire_stream_id)
    }

    /// An owned snapshot of one pushed [`AnnouncerBatch`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedBatch {
        forwarder_endpoint_id: String,
        stream_id: String,
        announcer_source_generation: i64,
        max_list_size: u32,
        rows: Vec<AnnouncerRow>,
    }

    /// Records every batch pushed so tests can assert what was sent (and how
    /// many transport requests it took).
    #[derive(Default)]
    struct RecordingClient {
        batches: Mutex<Vec<RecordedBatch>>,
    }

    /// Records the real HTTP request-body type built from a pushed batch.
    #[derive(Default)]
    struct RecordingRequestClient {
        requests: Mutex<Vec<PushRowsRequest>>,
    }

    impl RecordingRequestClient {
        fn recorded(&self) -> Vec<PushRowsRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl AnnouncerPushClient for RecordingRequestClient {
        fn push(&self, batch: &AnnouncerBatch<'_>) -> Result<(), AnnouncerPushError> {
            self.requests
                .lock()
                .unwrap()
                .push(push_rows_request_body(batch)?);
            Ok(())
        }
    }

    impl RecordingClient {
        fn recorded(&self) -> Vec<RecordedBatch> {
            self.batches.lock().unwrap().clone()
        }

        fn all_rows(&self) -> Vec<AnnouncerRow> {
            self.batches
                .lock()
                .unwrap()
                .iter()
                .flat_map(|batch| batch.rows.iter().cloned())
                .collect()
        }

        fn batch_count(&self) -> usize {
            self.batches.lock().unwrap().len()
        }
    }

    impl AnnouncerPushClient for RecordingClient {
        fn push(&self, batch: &AnnouncerBatch<'_>) -> Result<(), AnnouncerPushError> {
            self.batches.lock().unwrap().push(RecordedBatch {
                forwarder_endpoint_id: batch.forwarder_endpoint_id.to_owned(),
                stream_id: batch.stream_id.to_owned(),
                announcer_source_generation: batch.announcer_source_generation,
                max_list_size: batch.max_list_size,
                rows: batch.rows.to_vec(),
            });
            Ok(())
        }
    }

    fn map_resolver(entries: &[(&str, &str, &str)]) -> impl ParticipantResolver {
        let map: HashMap<String, ResolvedParticipant> = entries
            .iter()
            .map(|(chip, bib, name)| {
                (
                    (*chip).to_owned(),
                    ResolvedParticipant {
                        bib: (*bib).to_owned(),
                        name: Some((*name).to_owned()),
                        division: None,
                    },
                )
            })
            .collect();
        move |chip_id: &str| map.get(chip_id).cloned()
    }

    #[test]
    fn backlog_drains_in_max_500_row_batches() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "127.0.0.1:10400");
        let raw = sample_raw_frame();
        for seq in 1..=1_100 {
            insert_event(&db, stream_key.as_str(), seq, &raw, 1_700_000_000_000 + seq);
        }
        let client = RecordingClient::default();
        let resolver = map_resolver(&[]);

        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            1,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 1_100 });
        assert!(
            db.load_unpushed_announcer_events(stream_key.as_str())
                .unwrap()
                .is_empty(),
            "one pass marks every pushed row (one mark transaction per batch)"
        );

        // ceil(1100 / 500) = 3 transport requests, sized 500/500/100.
        let sizes: Vec<usize> = client.recorded().iter().map(|b| b.rows.len()).collect();
        assert_eq!(sizes, vec![500, 500, 100]);
    }

    #[test]
    fn late_arriving_low_seq_row_is_pushed_on_next_pass() {
        // The unpushed scan is marker-based (announcer_pushed_unix_ms IS
        // NULL), never a seq cursor: announcer ordering is by
        // received_unix_ms, so a late-received low-seq row must still be
        // picked up by a later pass.
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "127.0.0.1:10500");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 2, &raw, 1_700_000_000_200);
        insert_event(&db, stream_key.as_str(), 3, &raw, 1_700_000_000_300);
        let client = RecordingClient::default();
        let resolver = map_resolver(&[]);
        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            1,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 2 });

        // Seq 1 arrives late (redelivery after a gap), with a *newer*
        // received time.
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_400);
        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            1,
            1_700_000_020_000,
        )
        .unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 1 });
        let last_batch = client.recorded().last().cloned().unwrap();
        assert_eq!(last_batch.rows.len(), 1);
        assert_eq!(last_batch.rows[0].seq, 1, "the late low-seq row is pushed");
    }

    #[test]
    fn pushes_resolved_rows() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "127.0.0.1:10000");
        let raw = sample_raw_frame();
        // Insert out of received order to prove ordering by received_unix_ms.
        insert_event(&db, stream_key.as_str(), 2, &raw, 1_700_000_000_200);
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = map_resolver(&[("000000012345", "42", "Ada Lovelace")]);

        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            7,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 2 });

        let batches = client.recorded();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.forwarder_endpoint_id, "fwd-endpoint");
        assert_eq!(batch.stream_id, "127.0.0.1:10000");
        assert_eq!(batch.announcer_source_generation, 7);
        let rows = &batch.rows;
        assert_eq!(rows.len(), 2);
        // Ordered by received_unix_ms.
        assert_eq!(rows[0].seq, 1);
        assert_eq!(rows[1].seq, 2);
        for row in rows {
            assert_eq!(row.chip_id, "000000012345");
            assert_eq!(row.bib.as_deref(), Some("42"));
            assert_eq!(row.name.as_deref(), Some("Ada Lovelace"));
        }
        assert_eq!(rows[0].received_unix_ms, 1_700_000_000_100);
        assert_eq!(rows[1].received_unix_ms, 1_700_000_000_200);
    }

    #[test]
    fn push_decodes_local_stream_key_into_composite_fields() {
        // The encoded LocalStreamKey (with its U+001F separator) must never
        // cross the push boundary: batches carry the decoded composite
        // identity instead.
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("endpointaaaa", "127.0.0.1:10000");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = map_resolver(&[]);
        push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            1,
            1_700_000_010_000,
        )
        .unwrap();

        let batches = client.recorded();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].forwarder_endpoint_id, "endpointaaaa");
        assert_eq!(batches[0].stream_id, "127.0.0.1:10000");
        assert!(
            !batches[0].forwarder_endpoint_id.contains('\u{1f}')
                && !batches[0].stream_id.contains('\u{1f}'),
            "encoded LocalStreamKey separator must not cross the push boundary"
        );
    }

    fn sample_row() -> AnnouncerRow {
        AnnouncerRow {
            seq: 7,
            received_unix_ms: 1_700_000_000_100,
            chip_id: "000000012345".to_owned(),
            bib: Some("42".to_owned()),
            name: Some("Ada Lovelace".to_owned()),
            division: Some("5k".to_owned()),
        }
    }

    fn sample_batch(rows: &[AnnouncerRow]) -> AnnouncerBatch<'_> {
        AnnouncerBatch {
            forwarder_endpoint_id: "fwd-endpoint",
            stream_id: "finish-line",
            announcer_source_generation: 3,
            max_list_size: 25,
            rows,
        }
    }

    #[test]
    fn push_rows_request_body_carries_composite_identity_and_division() {
        let rows = vec![sample_row()];
        let body =
            serde_json::to_value(push_rows_request_body(&sample_batch(&rows)).unwrap()).unwrap();
        assert_eq!(body["forwarder_endpoint_id"], "fwd-endpoint");
        assert_eq!(body["stream_id"], "finish-line");
        assert_eq!(body["announcer_source_generation"], 3);
        assert_eq!(body["max_list_size"], 25);
        assert_eq!(body["rows"][0]["seq"], 7);
        assert_eq!(body["rows"][0]["division"], "5k");
        assert_eq!(body["rows"][0]["bib"], 42);
        assert_eq!(body["rows"][0]["display_name"], "Ada Lovelace");
        // An absent division serializes as null rather than being dropped.
        let rows = vec![AnnouncerRow {
            division: None,
            ..sample_row()
        }];
        let body =
            serde_json::to_value(push_rows_request_body(&sample_batch(&rows)).unwrap()).unwrap();
        assert!(body["rows"][0]["division"].is_null());
    }

    #[test]
    fn push_rows_request_body_labels_bib_without_participant() {
        let rows = vec![AnnouncerRow {
            bib: Some("1488".to_owned()),
            name: None,
            division: None,
            ..sample_row()
        }];
        let body =
            serde_json::to_value(push_rows_request_body(&sample_batch(&rows)).unwrap()).unwrap();
        assert_eq!(body["rows"][0]["bib"], 1488);
        assert_eq!(body["rows"][0]["display_name"], "Unknown Participant 1488");
    }

    #[test]
    fn push_rows_request_body_rejects_negative_generation_without_panicking() {
        let rows = vec![sample_row()];
        let mut batch = sample_batch(&rows);
        batch.announcer_source_generation = -1;

        let result = std::panic::catch_unwind(|| push_rows_request_body(&batch));
        assert!(
            result.is_ok(),
            "negative generation must return an error, not panic"
        );
        assert!(matches!(
            result.unwrap(),
            Err(AnnouncerPushError::Transport(message))
                if message.contains("announcer_source_generation")
        ));
    }

    #[test]
    fn push_rows_request_body_rejects_negative_seq_without_panicking() {
        let rows = vec![AnnouncerRow {
            seq: -1,
            ..sample_row()
        }];
        let batch = sample_batch(&rows);

        let result = std::panic::catch_unwind(|| push_rows_request_body(&batch));
        assert!(
            result.is_ok(),
            "negative seq must return an error, not panic"
        );
        assert!(matches!(
            result.unwrap(),
            Err(AnnouncerPushError::Transport(message)) if message.contains("seq")
        ));
    }

    #[test]
    fn pushes_division_into_announcer_row_payload() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "127.0.0.1:10000");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingRequestClient::default();
        // A resolver that carries a division display name through resolve.
        let resolver = |chip_id: &str| {
            (chip_id == "000000012345").then(|| ResolvedParticipant {
                bib: "42".to_owned(),
                name: Some("Ada Lovelace".to_owned()),
                division: Some("5k".to_owned()),
            })
        };

        push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            7,
            1_700_000_010_000,
        )
        .unwrap();
        let requests = client.recorded();
        assert_eq!(requests.len(), 1);
        let row = &requests[0].rows[0];
        assert_eq!(row.division.as_deref(), Some("5k"));
        assert_eq!(row.bib, Some(42));
        assert_eq!(row.display_name, "Ada Lovelace");
    }

    #[test]
    fn over_limit_stream_identity_is_rejected_in_push_request_body() {
        let rows = vec![sample_row()];
        let mut batch = sample_batch(&rows);
        let over_limit_stream_id = "s".repeat(MAX_ANNOUNCER_ID_LEN + 1);
        batch.stream_id = &over_limit_stream_id;

        assert!(matches!(
            push_rows_request_body(&batch),
            Err(AnnouncerPushError::BadRequest(message)) if message.contains("stream_id")
        ));
    }

    #[test]
    fn over_limit_display_name_is_clamped_in_push_request_body() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "127.0.0.1:10000");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingRequestClient::default();
        let over_limit_name = format!("{}étail", "a".repeat(MAX_ANNOUNCER_DISPLAY_NAME_LEN - 1));
        let over_limit_division =
            format!("{}étail", "b".repeat(MAX_ANNOUNCER_DISPLAY_NAME_LEN - 1));
        let resolver = |chip_id: &str| {
            (chip_id == "000000012345").then(|| ResolvedParticipant {
                bib: "42".to_owned(),
                name: Some(over_limit_name.clone()),
                division: Some(over_limit_division.clone()),
            })
        };

        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            7,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 1 });

        let requests = client.recorded();
        assert_eq!(requests.len(), 1);
        let row = &requests[0].rows[0];
        assert_eq!(row.display_name.len(), MAX_ANNOUNCER_DISPLAY_NAME_LEN - 1);
        assert_eq!(
            row.display_name,
            "a".repeat(MAX_ANNOUNCER_DISPLAY_NAME_LEN - 1)
        );
        let division = row.division.as_deref().unwrap();
        assert_eq!(division.len(), MAX_ANNOUNCER_DISPLAY_NAME_LEN - 1);
        assert_eq!(division, "b".repeat(MAX_ANNOUNCER_DISPLAY_NAME_LEN - 1));
    }

    #[test]
    fn pushes_rows_with_bib_and_without_name_when_participant_is_unknown() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = |chip_id: &str| {
            (chip_id == "000000012345").then(|| ResolvedParticipant {
                bib: "1488".to_owned(),
                name: None,
                division: None,
            })
        };

        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            1,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 1 });

        let rows = client.all_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bib.as_deref(), Some("1488"));
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[0].chip_id, "000000012345");
    }

    #[test]
    fn pushes_rows_without_bib_or_name_when_unresolved() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        // Resolver knows nothing about this chip.
        let resolver = map_resolver(&[]);

        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            1,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 1 });

        let rows = client.all_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bib, None);
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[0].chip_id, "000000012345");
    }

    #[test]
    fn idempotent_repush() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "cccccccc-cccc-cccc-cccc-cccccccccccc");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);
        insert_event(&db, stream_key.as_str(), 2, &raw, 1_700_000_000_200);

        let client = RecordingClient::default();
        let resolver = map_resolver(&[("000000012345", "42", "Ada Lovelace")]);

        let first = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            3,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(first, PushOutcome::Pushed { rows: 2 });

        // Repush with the same generation must send nothing new.
        let second = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            3,
            1_700_000_020_000,
        )
        .unwrap();
        assert_eq!(second, PushOutcome::Pushed { rows: 0 });

        assert_eq!(client.all_rows().len(), 2, "repush must not duplicate rows");
        assert_eq!(
            client.batch_count(),
            1,
            "repush with nothing pending must not call the transport"
        );
    }

    #[test]
    fn stale_generation_not_sent() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_key = key("fwd-endpoint", "dddddddd-dddd-dddd-dddd-dddddddddddd");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = map_resolver(&[("000000012345", "42", "Ada Lovelace")]);

        // Accept a newer generation first, raising the fence to 5.
        let fresh = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            5,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(fresh, PushOutcome::Pushed { rows: 1 });

        // A later event arrives, but a stale (older) generation tries to push it.
        insert_event(&db, stream_key.as_str(), 2, &raw, 1_700_000_000_300);
        let stale = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            3,
            1_700_000_020_000,
        )
        .unwrap();
        assert_eq!(
            stale,
            PushOutcome::StaleGeneration {
                fenced: 5,
                attempted: 3
            }
        );

        // Only the first (fresh) row was ever sent; the stale generation sent nothing.
        assert_eq!(client.all_rows().len(), 1);
        assert_eq!(client.batch_count(), 1);
        // The stale event remains pending for a future in-generation push.
        let pending = db
            .load_unpushed_announcer_events(stream_key.as_str())
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, 2);
    }

    struct NewerGenerationWinsDuringResolve<'a> {
        db: &'a Db,
        stream_id: &'a str,
        triggered: AtomicBool,
    }

    impl ParticipantResolver for NewerGenerationWinsDuringResolve<'_> {
        fn resolve(&self, _chip_id: &str) -> Option<ResolvedParticipant> {
            if !self.triggered.swap(true, Ordering::SeqCst) {
                let accepted = self
                    .db
                    .accept_announcer_generation(self.stream_id, 5)
                    .unwrap();
                assert_eq!(
                    accepted,
                    AnnouncerGenerationAcceptance::Current { generation: 5 }
                );
            }
            None
        }
    }

    #[test]
    fn interleaved_newer_generation_prevents_stale_send() {
        // Two connections on one temp-file DB: the resolver raises the fence
        // on its own connection while the push holds `&mut db`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ann.sqlite3");
        let mut db = Db::open(&path).unwrap();
        let resolver_db = Db::open(&path).unwrap();
        let stream_key = key("fwd-endpoint", "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee");
        let raw = sample_raw_frame();
        insert_event(&db, stream_key.as_str(), 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = NewerGenerationWinsDuringResolve {
            db: &resolver_db,
            stream_id: stream_key.as_str(),
            triggered: AtomicBool::new(false),
        };

        let outcome = push_announcer_rows(
            &mut db,
            &client,
            &resolver,
            &stream_key,
            3,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(
            outcome,
            PushOutcome::StaleGeneration {
                fenced: 5,
                attempted: 3
            }
        );
        assert!(resolver.triggered.load(Ordering::SeqCst));
        assert_eq!(client.all_rows(), Vec::<AnnouncerRow>::new());
        assert_eq!(client.batch_count(), 0);

        let pending = db
            .load_unpushed_announcer_events(stream_key.as_str())
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, 1);
    }
}

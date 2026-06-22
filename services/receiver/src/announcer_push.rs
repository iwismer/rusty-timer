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
//! * **Idempotency key `(stream_id, seq)`.** Each row carries its durable
//!   `(stream_id, seq)`. Once an event is pushed it is marked in the durable
//!   store, so a repush never re-sends an already-delivered row.
//! * **Ordering key `received_unix_ms`.** Rows are emitted in receipt order.
//! * **Fenced `announcer_source_generation`.** Every push carries the current
//!   source generation. A push whose generation is older than the highest
//!   generation already accepted for the stream is rejected without sending, so
//!   stale generations never reach the announcer. Pushes are also serialized per
//!   `stream_id` in-process, so overlapping calls cannot let an older generation
//!   proceed after a newer one has been accepted by another local push.
//! * **Resolved participant name when available.** Names/bibs are resolved
//!   locally from race/participant data via the injected resolver.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex as StdMutex, MutexGuard as StdMutexGuard, PoisonError};
use std::time::Duration;
use thiserror::Error;

/// Connect/request timeout for all blocking server HTTP calls. Bounds each
/// call so a hung server cannot wedge the runtime (including shutdown).
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

use crate::db::{AnnouncerGenerationAcceptance, Db, DbError};
use crate::ui_events::chip_id_from_raw_frame;

/// A participant identity resolved from local race/participant data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedParticipant {
    pub bib: String,
    pub name: String,
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

/// A single announcer row pushed downstream.
///
/// `stream_id`/`seq` form the idempotency key, `received_unix_ms` is the
/// ordering key, and `generation` fences the announcer source. `bib`/`name` are
/// populated when the chip resolves to a known participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnnouncerRow {
    pub stream_id: String,
    pub seq: i64,
    pub received_unix_ms: i64,
    pub announcer_source_generation: i64,
    pub chip_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bib: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Transport abstraction for delivering announcer rows downstream.
///
/// `max_list_size` is the receiver-configured cap on the number of rows the
/// server keeps visible in the public announcer feed; it rides every push so
/// the server can re-trim its runtime when the operator changes it.
pub trait AnnouncerPushClient {
    fn push(&self, rows: &[AnnouncerRow], max_list_size: u32) -> Result<(), AnnouncerPushError>;
}

/// Real HTTP transport for the server `/announcer/rows` endpoint.
///
/// Server accepts **one row per POST** with bearer auth, so [`push`] posts
/// each row individually. A blocking reqwest client is built lazily inside
/// [`push`] (never held across calls) because [`push_announcer_rows`] is
/// synchronous and is driven from a blocking task in the headless P2P runtime;
/// constructing or dropping a blocking client inside an async context panics, so
/// the client must live entirely on the blocking thread. The bearer token is
/// held privately and never logged.
///
/// [`push`]: AnnouncerPushClient::push
pub struct ServerAnnouncerClient {
    rows_url: String,
    token: String,
}

impl ServerAnnouncerClient {
    /// Build a client targeting `base_url` (e.g. `http://127.0.0.1:8080`).
    pub fn new(base_url: &str, token: impl Into<String>) -> Result<Self, AnnouncerPushError> {
        Ok(Self {
            rows_url: format!("{}/announcer/rows", base_url.trim_end_matches('/')),
            token: token.into(),
        })
    }
}

impl AnnouncerPushClient for ServerAnnouncerClient {
    fn push(&self, rows: &[AnnouncerRow], max_list_size: u32) -> Result<(), AnnouncerPushError> {
        if rows.is_empty() {
            return Ok(());
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
        for row in rows {
            // Server `bib` is an optional integer; a non-numeric bib is sent
            // as null rather than failing the whole push.
            let bib = row.bib.as_deref().and_then(|b| b.parse::<i32>().ok());
            let body = serde_json::json!({
                "announcer_source_generation": row.announcer_source_generation,
                "stream_id": row.stream_id,
                "seq": row.seq,
                "chip_id": row.chip_id,
                "bib": bib,
                "display_name": row.name.clone().unwrap_or_default(),
                "reader_timestamp": serde_json::Value::Null,
                "received_unix_ms": row.received_unix_ms,
                "max_list_size": max_list_size,
            });
            let response = client
                .post(&self.rows_url)
                .bearer_auth(&self.token)
                .json(&body)
                .send()
                .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
            if !response.status().is_success() {
                return Err(AnnouncerPushError::Transport(format!(
                    "server /announcer/rows returned {}",
                    response.status()
                )));
            }
        }
        Ok(())
    }
}

/// Register this receiver endpoint with server under the TOFU `/register`
/// model (`device_kind = "receiver"`). Already-registered / active endpoints are
/// tolerated: any `2xx` response is success. The bearer token is never logged.
///
/// The configured `receiver_id` is sent as the device's self-reported
/// `display_name` so the server's admin approval UI can show a human-friendly
/// name instead of the opaque endpoint ID. A blank receiver ID is omitted.
pub fn register_receiver_with_server(
    base_url: &str,
    token: &str,
    endpoint_id: &str,
    receiver_id: &str,
) -> Result<(), AnnouncerPushError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    let mut body = serde_json::json!({
        "endpoint_id": endpoint_id,
        "device_kind": "receiver",
        "device_token": token,
    });
    let trimmed_id = receiver_id.trim();
    if !trimmed_id.is_empty() {
        body["display_name"] = serde_json::Value::String(trimmed_id.to_owned());
    }
    let response = client
        .post(format!("{}/register", base_url.trim_end_matches('/')))
        .bearer_auth(token)
        .json(&body)
        .send()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(AnnouncerPushError::Transport(format!(
            "server /register returned {}",
            response.status()
        )))
    }
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
    let value: serde_json::Value = response
        .json()
        .map_err(|e| AnnouncerPushError::Transport(e.to_string()))?;
    let generation = value
        .get("announcer_source_generation")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            AnnouncerPushError::Transport(
                "server /announcer/takeover response missing announcer_source_generation"
                    .to_owned(),
            )
        })?;
    Ok(generation)
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

/// Push not-yet-pushed durable events for `stream_id` to the announcer.
///
/// Returns [`PushOutcome::StaleGeneration`] without sending if `generation` is
/// older than the highest generation already fenced for the stream. Otherwise
/// the fence is raised, pending events are resolved into rows, pushed via
/// `client`, and only then marked pushed in the durable store — so a failed
/// transport leaves rows unmarked for a later retry, and a successful push
/// followed by a repush sends nothing (idempotent).
pub fn push_announcer_rows(
    db: &Db,
    client: &dyn AnnouncerPushClient,
    resolver: &dyn ParticipantResolver,
    stream_id: &str,
    generation: i64,
    pushed_unix_ms: i64,
) -> Result<PushOutcome, AnnouncerPushError> {
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

    let pending = db.load_unpushed_announcer_events(stream_id)?;
    if pending.is_empty() {
        return Ok(PushOutcome::Pushed { rows: 0 });
    }

    let rows: Vec<AnnouncerRow> = pending
        .iter()
        .map(|event| {
            let chip_id = chip_id_from_raw_frame(&event.raw_frame);
            let resolved = resolver.resolve(&chip_id);
            AnnouncerRow {
                stream_id: event.stream_id.clone(),
                seq: event.seq,
                received_unix_ms: event.received_unix_ms,
                announcer_source_generation: generation,
                chip_id,
                bib: resolved.as_ref().map(|p| p.bib.clone()),
                name: resolved.map(|p| p.name),
            }
        })
        .collect();

    if let Some(fenced) = db.load_announcer_fence(stream_id)?
        && generation < fenced
    {
        return Ok(PushOutcome::StaleGeneration {
            fenced,
            attempted: generation,
        });
    }

    let max_list_size = db.load_announcer_max_list_size()?;
    client.push(&rows, max_list_size)?;

    // Only mark after a successful push, so a failed transport leaves rows
    // pending for a later retry (at-least-once + idempotency key downstream).
    let mut marked = 0usize;
    for event in &pending {
        if db.mark_announcer_pushed(stream_id, event.seq, pushed_unix_ms)? {
            marked += 1;
        }
    }
    Ok(PushOutcome::Pushed { rows: marked })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AnnouncerGenerationAcceptance, ReceivedEventInsert};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

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
        })
        .unwrap();
    }

    /// Records every batch of rows pushed so tests can assert what was sent.
    #[derive(Default)]
    struct RecordingClient {
        batches: Mutex<Vec<Vec<AnnouncerRow>>>,
    }

    impl RecordingClient {
        fn all_rows(&self) -> Vec<AnnouncerRow> {
            self.batches
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .cloned()
                .collect()
        }

        fn batch_count(&self) -> usize {
            self.batches.lock().unwrap().len()
        }
    }

    impl AnnouncerPushClient for RecordingClient {
        fn push(
            &self,
            rows: &[AnnouncerRow],
            _max_list_size: u32,
        ) -> Result<(), AnnouncerPushError> {
            self.batches.lock().unwrap().push(rows.to_vec());
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
                        name: (*name).to_owned(),
                    },
                )
            })
            .collect();
        move |chip_id: &str| map.get(chip_id).cloned()
    }

    #[test]
    fn pushes_resolved_rows() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "127.0.0.1:10000";
        let raw = sample_raw_frame();
        // Insert out of received order to prove ordering by received_unix_ms.
        insert_event(&db, stream_id, 2, &raw, 1_700_000_000_200);
        insert_event(&db, stream_id, 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = map_resolver(&[("000000012345", "42", "Ada Lovelace")]);

        let outcome =
            push_announcer_rows(&db, &client, &resolver, stream_id, 7, 1_700_000_010_000).unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 2 });

        let rows = client.all_rows();
        assert_eq!(rows.len(), 2);
        // Ordered by received_unix_ms.
        assert_eq!(rows[0].seq, 1);
        assert_eq!(rows[1].seq, 2);
        for row in &rows {
            assert_eq!(row.stream_id, stream_id);
            assert_eq!(row.announcer_source_generation, 7);
            assert_eq!(row.chip_id, "000000012345");
            assert_eq!(row.bib.as_deref(), Some("42"));
            assert_eq!(row.name.as_deref(), Some("Ada Lovelace"));
        }
        assert_eq!(rows[0].received_unix_ms, 1_700_000_000_100);
        assert_eq!(rows[1].received_unix_ms, 1_700_000_000_200);
    }

    #[test]
    fn pushes_rows_without_name_when_unresolved() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let raw = sample_raw_frame();
        insert_event(&db, stream_id, 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        // Resolver knows nothing about this chip.
        let resolver = map_resolver(&[]);

        let outcome =
            push_announcer_rows(&db, &client, &resolver, stream_id, 1, 1_700_000_010_000).unwrap();
        assert_eq!(outcome, PushOutcome::Pushed { rows: 1 });

        let rows = client.all_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bib, None);
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[0].chip_id, "000000012345");
    }

    #[test]
    fn idempotent_repush() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        let raw = sample_raw_frame();
        insert_event(&db, stream_id, 1, &raw, 1_700_000_000_100);
        insert_event(&db, stream_id, 2, &raw, 1_700_000_000_200);

        let client = RecordingClient::default();
        let resolver = map_resolver(&[("000000012345", "42", "Ada Lovelace")]);

        let first =
            push_announcer_rows(&db, &client, &resolver, stream_id, 3, 1_700_000_010_000).unwrap();
        assert_eq!(first, PushOutcome::Pushed { rows: 2 });

        // Repush with the same generation must send nothing new.
        let second =
            push_announcer_rows(&db, &client, &resolver, stream_id, 3, 1_700_000_020_000).unwrap();
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
        let db = Db::open_in_memory().unwrap();
        let stream_id = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        let raw = sample_raw_frame();
        insert_event(&db, stream_id, 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = map_resolver(&[("000000012345", "42", "Ada Lovelace")]);

        // Accept a newer generation first, raising the fence to 5.
        let fresh =
            push_announcer_rows(&db, &client, &resolver, stream_id, 5, 1_700_000_010_000).unwrap();
        assert_eq!(fresh, PushOutcome::Pushed { rows: 1 });

        // A later event arrives, but a stale (older) generation tries to push it.
        insert_event(&db, stream_id, 2, &raw, 1_700_000_000_300);
        let stale =
            push_announcer_rows(&db, &client, &resolver, stream_id, 3, 1_700_000_020_000).unwrap();
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
        let pending = db.load_unpushed_announcer_events(stream_id).unwrap();
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
        let db = Db::open_in_memory().unwrap();
        let stream_id = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        let raw = sample_raw_frame();
        insert_event(&db, stream_id, 1, &raw, 1_700_000_000_100);

        let client = RecordingClient::default();
        let resolver = NewerGenerationWinsDuringResolve {
            db: &db,
            stream_id,
            triggered: AtomicBool::new(false),
        };

        let outcome =
            push_announcer_rows(&db, &client, &resolver, stream_id, 3, 1_700_000_010_000).unwrap();
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

        let pending = db.load_unpushed_announcer_events(stream_id).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, 1);
    }
}

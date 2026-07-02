use rt_domain::{ReceiverMode, ResumeCursor};
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
const SCHEMA_SQL: &str = include_str!("storage/schema.sql");
pub const DEFAULT_UPDATE_MODE: &str = "check-and-download";

/// Counts describing the imported participant/chip data and how they overlap.
/// Surfaced to the UI so it can show participant/chip totals and how many
/// participants are still missing a chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStats {
    /// Total participant rows.
    pub participants: usize,
    /// Total bib->chip assignment rows.
    pub chips: usize,
    /// Participants that have at least one chip assignment.
    pub matched_participants: usize,
    /// Participants with no chip assignment (`participants - matched`).
    pub participants_without_chips: usize,
    /// Chip assignments whose bib resolves to a participant.
    pub resolvable_chips: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Start,
    Finish,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::Start => "start",
            EventType::Finish => "finish",
        }
    }
}

impl std::str::FromStr for EventType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(EventType::Start),
            "finish" => Ok(EventType::Finish),
            other => Err(format!(
                "invalid event type: '{other}', must be 'start' or 'finish'"
            )),
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Integrity: {0}")]
    IntegrityCheckFailed(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Profile missing")]
    ProfileMissing,
}
pub type DbResult<T> = Result<T, DbError>;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub server_url: String,
    pub token: String,
    pub update_mode: String,
    pub receiver_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub local_port_override: Option<u16>,
    pub event_type: EventType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSubscription {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub local_port_override: Option<u16>,
    pub event_type: EventType,
    pub forwarder_id: Option<String>,
    pub reader_ip: Option<String>,
}

/// Canonical earliest-epoch override keyed by `stream_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEarliestEpoch {
    pub stream_id: String,
    pub forwarder_endpoint_id: String,
    pub earliest_epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamCursorRecord {
    pub stream_id: String,
    pub stream_epoch: Option<i64>,
    pub last_seq: i64,
    pub forwarder_id: Option<String>,
    pub reader_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbfConfig {
    pub enabled: bool,
}

/// Default Race Director working directory containing participant/chip DBF files.
pub const DEFAULT_RD_IMPORT_DIR: &str = r"C:\Winrace\Files";

/// Default poll cadence for the Race Director background import (seconds).
pub const DEFAULT_RD_IMPORT_INTERVAL_SECS: u32 = 15;

/// Configuration for the Race Director DBF participant/chip import. Parallels
/// [`DbfConfig`] (which drives the `IPICO.DBF` *writer*); this drives the
/// *reader* / import poller. Persisted in the `profile` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdImportConfig {
    /// Enables the background poll. The manual import action ignores this.
    pub enabled: bool,
    /// RD working directory containing `checkchip.dbf` / `RACE.DBF` /
    /// `DIVISION.DBF`. Uses `String` (not `PathBuf`) for serde/cross-platform
    /// parity with [`DbfConfig`].
    pub dir: String,
    /// Poll cadence in seconds.
    pub interval_secs: u32,
}

impl RdImportConfig {
    /// Validate the config. A non-empty directory is required when enabled and
    /// the interval must be at least one second.
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled && self.dir.trim().is_empty() {
            return Err("Race Director import directory must not be empty when enabled".to_owned());
        }
        if self.interval_secs == 0 {
            return Err("Race Director import interval must be at least 1 second".to_owned());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRecord {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub stream_epoch: i64,
    pub last_seq: i64,
}

/// New P2P event payload to persist. `stream_id` is an arbitrary UTF-8 stream
/// key (e.g. the forwarder journal key `ip:port`) stored verbatim as TEXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedEventInsert<'a> {
    pub stream_id: &'a str,
    pub seq: i64,
    pub epoch: i64,
    pub raw_frame: &'a [u8],
    pub read_kind: &'a str,
    pub reader_timestamp: Option<&'a str>,
    pub received_unix_ms: i64,
    pub dbf_delivered_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedEvent {
    pub stream_id: String,
    pub seq: i64,
    pub epoch: i64,
    pub raw_frame: Vec<u8>,
    pub read_kind: String,
    pub reader_timestamp: Option<String>,
    pub received_unix_ms: i64,
    pub dbf_delivered_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapMarkerInsert<'a> {
    pub stream_id: &'a str,
    pub requested_after_seq: i64,
    pub earliest_available_seq: i64,
    pub latest_available_seq: i64,
    pub reason: &'a str,
    pub created_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapMarker {
    pub stream_id: String,
    pub requested_after_seq: i64,
    pub earliest_available_seq: i64,
    pub latest_available_seq: i64,
    pub reason: String,
    pub created_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnouncerGenerationAcceptance {
    Current { generation: i64 },
    Stale { current: i64, attempted: i64 },
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        let c = Connection::open(path)?;
        let d = Self { conn: c };
        d.apply_pragmas()?;
        d.apply_schema()?;
        Ok(d)
    }
    pub fn open_in_memory() -> DbResult<Self> {
        let c = Connection::open_in_memory()?;
        let d = Self { conn: c };
        d.apply_pragmas()?;
        d.apply_schema()?;
        Ok(d)
    }
    pub fn integrity_check(&self) -> DbResult<()> {
        let r: String = self
            .conn
            .pragma_query_value(None, "integrity_check", |row| row.get(0))?;
        if r != "ok" {
            return Err(DbError::IntegrityCheckFailed(r));
        }
        Ok(())
    }
    pub fn load_profile(&self) -> DbResult<Option<Profile>> {
        let mut s = self
            .conn
            .prepare("SELECT server_url, token, update_mode, receiver_id FROM profile LIMIT 1")?;
        let mut rows = s.query_map([], |r| {
            Ok(Profile {
                server_url: r.get(0)?,
                token: r.get(1)?,
                update_mode: r.get(2)?,
                receiver_id: r.get(3)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }
    pub fn save_profile(
        &mut self,
        url: &str,
        tok: &str,
        update_mode: &str,
        receiver_id: Option<&str>,
    ) -> DbResult<()> {
        let receiver_mode_json = self.load_receiver_mode_json_raw()?;
        let dbf_config = self.load_dbf_config()?;
        // Preserve config flags that live on the profile row across the
        // delete+insert (mirrors dbf handling).
        let announcer_enabled = self.load_announcer_enabled()?;
        let announcer_max_list_size = self.load_announcer_max_list_size()?;
        let rd_import = self.load_rd_import_config()?;
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM profile")?;
        tx.execute(
            "INSERT INTO profile (server_url, token, update_mode, receiver_mode_json, receiver_id, dbf_enabled, announcer_enabled, announcer_max_list_size, rd_import_enabled, rd_import_dir, rd_import_interval_secs) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![url, tok, update_mode, receiver_mode_json, receiver_id, dbf_config.enabled as i64, i64::from(announcer_enabled), i64::from(announcer_max_list_size), i64::from(rd_import.enabled), &rd_import.dir, i64::from(rd_import.interval_secs)],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn load_receiver_mode(&self) -> DbResult<Option<ReceiverMode>> {
        let Some(raw_json) = self.load_receiver_mode_json_raw()? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str::<ReceiverMode>(&raw_json)?))
    }

    pub fn save_receiver_mode(&self, mode: &ReceiverMode) -> DbResult<()> {
        let json = serde_json::to_string(mode)?;
        let updated = self.conn.execute(
            "UPDATE profile SET receiver_mode_json = ?1",
            rusqlite::params![json],
        )?;
        if updated == 0 {
            return Err(DbError::ProfileMissing);
        }
        Ok(())
    }

    /// Persists the receiver ID. If no profile row exists yet, a minimal
    /// placeholder row is created (empty server_url/token). Code that checks
    /// for a configured profile must use `profile_has_connect_credentials`
    /// rather than just testing for `Some(profile)`.
    pub fn save_receiver_id(&self, receiver_id: &str) -> DbResult<()> {
        let updated = self.conn.execute(
            "UPDATE profile SET receiver_id = ?1",
            rusqlite::params![receiver_id],
        )?;
        if updated == 0 {
            self.conn.execute(
                "INSERT INTO profile (server_url, token, update_mode, receiver_id)
                 SELECT '', '', ?1, ?2
                 WHERE NOT EXISTS (SELECT 1 FROM profile)",
                rusqlite::params![DEFAULT_UPDATE_MODE, receiver_id],
            )?;
        }
        Ok(())
    }

    /// Canonical loader for earliest-epoch overrides keyed by `stream_id`.
    pub fn load_stream_earliest_epochs(&self) -> DbResult<Vec<StreamEarliestEpoch>> {
        let mut stmt = self.conn.prepare(
            "SELECT stream_id, forwarder_endpoint_id, earliest_epoch
             FROM earliest_epochs
             ORDER BY stream_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(StreamEarliestEpoch {
                stream_id: r.get(0)?,
                forwarder_endpoint_id: r.get(1)?,
                earliest_epoch: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Canonical writer for earliest-epoch overrides. Compatibility
    /// `forwarder_id`/`reader_ip` columns are left NULL so no caller reads a
    /// canonical row as if `stream_id` were a `reader_ip`.
    pub fn save_stream_earliest_epoch(
        &self,
        forwarder_endpoint_id: &str,
        stream_id: &str,
        epoch: i64,
    ) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO earliest_epochs
             (stream_id, forwarder_endpoint_id, earliest_epoch, forwarder_id, reader_ip)
             VALUES (?1, ?2, ?3, NULL, NULL)",
            rusqlite::params![stream_id, forwarder_endpoint_id, epoch],
        )?;
        Ok(())
    }

    /// Canonical delete keyed by `stream_id`.
    pub fn delete_stream_earliest_epoch(&self, stream_id: &str) -> DbResult<()> {
        self.conn.execute(
            "DELETE FROM earliest_epochs WHERE stream_id = ?1",
            rusqlite::params![stream_id],
        )?;
        Ok(())
    }

    /// Compatibility loader returning `(forwarder_id, reader_ip, earliest_epoch)`.
    /// Only rows that carry real display metadata are returned;
    /// canonical-only rows (`forwarder_id`/`reader_ip` NULL) are skipped rather
    /// than fabricating keys from `stream_id`.
    pub fn load_earliest_epochs(&self) -> DbResult<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT forwarder_id, reader_ip, earliest_epoch FROM earliest_epochs
             WHERE forwarder_id IS NOT NULL AND reader_ip IS NOT NULL
             ORDER BY forwarder_id, reader_ip",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Compatibility writer keyed by `(forwarder_id, reader_ip)`. Stores a
    /// deterministic synthetic `stream_id` so the canonical-keyed table can hold
    /// display-metadata rows, and records the real metadata in the compatibility
    /// columns.
    pub fn save_earliest_epoch(&self, fwd: &str, ip: &str, epoch: i64) -> DbResult<()> {
        let stream_id = legacy_cursor_stream_id(fwd, ip);
        self.conn.execute(
            "INSERT OR REPLACE INTO earliest_epochs
             (stream_id, forwarder_endpoint_id, earliest_epoch, forwarder_id, reader_ip)
             VALUES (?1, ?2, ?3, ?2, ?4)",
            rusqlite::params![stream_id, fwd, epoch, ip],
        )?;
        Ok(())
    }

    pub fn delete_earliest_epoch(&self, fwd: &str, ip: &str) -> DbResult<()> {
        self.conn.execute(
            "DELETE FROM earliest_epochs WHERE forwarder_id = ?1 AND reader_ip = ?2",
            rusqlite::params![fwd, ip],
        )?;
        Ok(())
    }
    /// Compatibility loader returning `(forwarder_id, reader_ip)`-keyed
    /// subscriptions. Canonical-only rows (`forwarder_id`/`reader_ip` NULL) are
    /// filtered out rather than substituting `forwarder_endpoint_id`/
    /// `stream_id`, so callers never receive fabricated display keys.
    pub fn load_subscriptions(&self) -> DbResult<Vec<Subscription>> {
        let mut s = self.conn.prepare(
            "SELECT forwarder_id,
                    reader_ip,
                    local_port_override,
                    event_type
             FROM subscriptions
             WHERE forwarder_id IS NOT NULL AND reader_ip IS NOT NULL
             ORDER BY forwarder_id, reader_ip",
        )?;
        let rows = s.query_map([], |r| {
            Ok(Subscription {
                forwarder_id: r.get(0)?,
                reader_ip: r.get(1)?,
                local_port_override: r.get::<_, Option<i64>>(2)?.map(|p| p as u16),
                event_type: parse_event_type_column(r.get::<_, String>(3)?, 3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_stream_subscriptions(&self) -> DbResult<Vec<StreamSubscription>> {
        let mut s = self.conn.prepare(
            "SELECT forwarder_endpoint_id,
                    stream_id,
                    local_port_override,
                    event_type,
                    forwarder_id,
                    reader_ip
             FROM subscriptions
             ORDER BY forwarder_endpoint_id, stream_id",
        )?;
        let rows = s.query_map([], |r| {
            Ok(StreamSubscription {
                forwarder_endpoint_id: r.get(0)?,
                stream_id: r.get(1)?,
                local_port_override: r.get::<_, Option<i64>>(2)?.map(|p| p as u16),
                event_type: parse_event_type_column(r.get::<_, String>(3)?, 3)?,
                forwarder_id: r.get(4)?,
                reader_ip: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
    pub fn save_subscription(
        &self,
        fwd: &str,
        ip: &str,
        port: Option<u16>,
        event_type: Option<EventType>,
    ) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO subscriptions
             (forwarder_endpoint_id, stream_id, local_port_override, event_type, forwarder_id, reader_ip)
             VALUES (?1, ?2, ?3, ?4, ?1, ?2)",
            rusqlite::params![fwd, ip, port.map(|p| p as i64), event_type.unwrap_or(EventType::Finish).as_str()],
        )?;
        Ok(())
    }
    pub fn replace_subscriptions(&mut self, subs: &[Subscription]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM subscriptions")?;
        for s in subs {
            tx.execute(
                "INSERT INTO subscriptions
                 (forwarder_endpoint_id, stream_id, local_port_override, event_type, forwarder_id, reader_ip)
                 VALUES (?1, ?2, ?3, ?4, ?1, ?2)",
                rusqlite::params![&s.forwarder_id, &s.reader_ip, s.local_port_override.map(|p| p as i64), s.event_type.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn replace_stream_subscriptions(&mut self, subs: &[StreamSubscription]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM subscriptions")?;
        for s in subs {
            tx.execute(
                "INSERT INTO subscriptions
                 (forwarder_endpoint_id, stream_id, local_port_override, event_type, forwarder_id, reader_ip)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    &s.forwarder_endpoint_id,
                    &s.stream_id,
                    s.local_port_override.map(|p| p as i64),
                    s.event_type.as_str(),
                    s.forwarder_id.as_deref(),
                    s.reader_ip.as_deref(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace all participants with `participants` (upload-replaces-all).
    pub fn replace_participants(
        &mut self,
        participants: &[crate::participants::Participant],
    ) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM participants")?;
        for p in participants {
            tx.execute(
                "INSERT OR REPLACE INTO participants (bib, last, first, affiliation, gender, division)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![p.bib, &p.last, &p.first, &p.affiliation, &p.gender, p.division],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace all division-code -> name entries (upload-replaces-all). Only
    /// Race Director imports populate this; other import sources clear it.
    pub fn replace_divisions(&mut self, divisions: &[(i32, String)]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM divisions")?;
        for (divno, name) in divisions {
            tx.execute(
                "INSERT OR REPLACE INTO divisions (divno, name) VALUES (?1, ?2)",
                rusqlite::params![divno, name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace participants, bib->chip assignments, and divisions together in a
    /// single transaction (upload-replaces-all across all three tables). Used
    /// by the Race Director import so the three tables swap atomically: a write
    /// error rolls the whole set back rather than leaving a partially-updated
    /// mix of old and new data.
    pub fn replace_rd_data(
        &mut self,
        participants: &[crate::participants::Participant],
        chips: &[(i64, String)],
        divisions: &[(i32, String)],
    ) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM participants; DELETE FROM bib_chips; DELETE FROM divisions")?;
        for p in participants {
            tx.execute(
                "INSERT OR REPLACE INTO participants (bib, last, first, affiliation, gender, division)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![p.bib, &p.last, &p.first, &p.affiliation, &p.gender, p.division],
            )?;
        }
        for (bib, chip_id) in chips {
            tx.execute(
                "INSERT OR REPLACE INTO bib_chips (chip_id, bib) VALUES (?1, ?2)",
                rusqlite::params![chip_id, bib],
            )?;
        }
        for (divno, name) in divisions {
            tx.execute(
                "INSERT OR REPLACE INTO divisions (divno, name) VALUES (?1, ?2)",
                rusqlite::params![divno, name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace all bib->chip assignments with `chips` (upload-replaces-all).
    pub fn replace_bib_chips(&mut self, chips: &[(i64, String)]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM bib_chips")?;
        for (bib, chip_id) in chips {
            tx.execute(
                "INSERT OR REPLACE INTO bib_chips (chip_id, bib) VALUES (?1, ?2)",
                rusqlite::params![chip_id, bib],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Build the chip-id -> participant-entry lookup from `bib_chips`, keeping
    /// chip assignments even when the bib has no participant row. The bib is
    /// rendered as a canonical decimal string via its i64 value. Participant
    /// name and division are populated only when the bib joins to a participant.
    pub fn load_chip_to_participant(
        &self,
    ) -> DbResult<std::collections::HashMap<String, crate::control_api::ChipEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.chip_id, c.bib, p.first, p.last, d.name
             FROM bib_chips c
             LEFT JOIN participants p ON p.bib = c.bib
             LEFT JOIN divisions d ON d.divno = p.division",
        )?;
        let rows = stmt.query_map([], |r| {
            let chip_id: String = r.get(0)?;
            let bib: i64 = r.get(1)?;
            let first: Option<String> = r.get(2)?;
            let last: Option<String> = r.get(3)?;
            let division: Option<String> = r.get(4)?;
            let name = match (first, last) {
                (Some(first), Some(last)) => Some(format!("{first} {last}").trim().to_owned()),
                _ => None,
            };
            Ok((
                chip_id,
                crate::control_api::ChipEntry {
                    bib: bib.to_string(),
                    name,
                    division,
                },
            ))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (chip_id, value) = row?;
            map.insert(chip_id, value);
        }
        Ok(map)
    }

    /// Count participants, chip assignments, and how they overlap so the UI can
    /// show the state of the imported data. `matched_participants` is the
    /// number of participants that have at least one chip assignment; the
    /// remainder (`participants - matched_participants`) are missing chips.
    /// `resolvable_chips` is the chip-side count (chips whose bib matches a
    /// participant), matching [`Self::load_chip_to_participant`].
    pub fn data_stats(&self) -> DbResult<DataStats> {
        let participants: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM participants", [], |r| r.get(0))?;
        let chips: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM bib_chips", [], |r| r.get(0))?;
        let matched_participants: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM participants p \
             WHERE EXISTS (SELECT 1 FROM bib_chips c WHERE c.bib = p.bib)",
            [],
            |r| r.get(0),
        )?;
        let resolvable_chips: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM bib_chips c \
             JOIN participants p ON p.bib = c.bib",
            [],
            |r| r.get(0),
        )?;
        Ok(DataStats {
            participants: participants.max(0) as usize,
            chips: chips.max(0) as usize,
            matched_participants: matched_participants.max(0) as usize,
            participants_without_chips: (participants - matched_participants).max(0) as usize,
            resolvable_chips: resolvable_chips.max(0) as usize,
        })
    }

    pub fn load_resume_cursors(&self) -> DbResult<Vec<ResumeCursor>> {
        Ok(self
            .load_cursors()?
            .into_iter()
            .map(|c| ResumeCursor {
                forwarder_id: c.forwarder_id,
                reader_ip: c.reader_ip,
                stream_epoch: c.stream_epoch,
                last_seq: c.last_seq,
            })
            .collect())
    }
    pub fn load_cursors(&self) -> DbResult<Vec<CursorRecord>> {
        let mut s = self.conn.prepare(
            "SELECT forwarder_id, reader_ip, COALESCE(stream_epoch, 0), last_seq
             FROM cursors
             WHERE forwarder_id IS NOT NULL AND reader_ip IS NOT NULL
             ORDER BY forwarder_id, reader_ip",
        )?;
        let rows = s.query_map([], |r| {
            Ok(CursorRecord {
                forwarder_id: r.get(0)?,
                reader_ip: r.get(1)?,
                stream_epoch: r.get::<_, i64>(2)?,
                last_seq: r.get::<_, i64>(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_stream_cursors(&self) -> DbResult<Vec<StreamCursorRecord>> {
        let mut s = self.conn.prepare(
            "SELECT stream_id, stream_epoch, last_seq, forwarder_id, reader_ip
             FROM cursors
             ORDER BY stream_id",
        )?;
        let rows = s.query_map([], |r| {
            Ok(StreamCursorRecord {
                stream_id: r.get(0)?,
                stream_epoch: r.get(1)?,
                last_seq: r.get(2)?,
                forwarder_id: r.get(3)?,
                reader_ip: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
    pub fn save_cursor(&self, fwd: &str, ip: &str, epoch: i64, seq: i64) -> DbResult<()> {
        let stream_id = legacy_cursor_stream_id(fwd, ip);
        let existing: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT COALESCE(stream_epoch, 0), last_seq
                 FROM cursors
                 WHERE stream_id = ?1 OR (forwarder_id = ?2 AND reader_ip = ?3)
                 LIMIT 1",
                rusqlite::params![&stream_id, fwd, ip],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        match existing {
            Some((stored_epoch, stored_seq))
                if epoch > stored_epoch || (epoch == stored_epoch && seq > stored_seq) =>
            {
                self.conn.execute(
                    "UPDATE cursors
                     SET stream_id = ?1, last_seq = ?5, forwarder_id = ?2, reader_ip = ?3, stream_epoch = ?4
                     WHERE stream_id = ?1 OR (forwarder_id = ?2 AND reader_ip = ?3)",
                    rusqlite::params![stream_id, fwd, ip, epoch, seq],
                )?;
            }
            None => {
                self.conn.execute(
                    "INSERT INTO cursors (stream_id, last_seq, forwarder_id, reader_ip, stream_epoch)
                     VALUES (?1, ?5, ?2, ?3, ?4)",
                    rusqlite::params![stream_id, fwd, ip, epoch, seq],
                )?;
            }
            Some(_) => {}
        }
        Ok(())
    }
    pub fn delete_cursor(&self, fwd: &str, ip: &str) -> DbResult<()> {
        let stream_id = legacy_cursor_stream_id(fwd, ip);
        self.conn.execute(
            "DELETE FROM cursors WHERE stream_id = ?1 OR (forwarder_id = ?2 AND reader_ip = ?3)",
            rusqlite::params![stream_id, fwd, ip],
        )?;
        Ok(())
    }

    pub fn delete_stream_cursor(&self, stream_id: &str) -> DbResult<()> {
        self.conn.execute(
            "DELETE FROM cursors WHERE stream_id = ?1",
            rusqlite::params![stream_id],
        )?;
        Ok(())
    }

    /// Begin an `IMMEDIATE` write transaction on the underlying connection.
    ///
    /// Used by the P2P persist path to make each `EventBatch` (inserts +
    /// conflict checks + cursor advance) one atomic commit — one fsync per
    /// batch instead of one per row. Run the row-level operations through the
    /// `*_conn` free functions in this module.
    pub fn transaction(&mut self) -> DbResult<rusqlite::Transaction<'_>> {
        Ok(self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?)
    }

    pub fn insert_received_event(&self, event: &ReceivedEventInsert<'_>) -> DbResult<bool> {
        insert_received_event_conn(&self.conn, event)
    }

    pub fn load_received_events(&self, stream_id: &str) -> DbResult<Vec<ReceivedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms, dbf_delivered_unix_ms
             FROM received_events
             WHERE stream_id = ?1
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id], received_event_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_received_events_after(
        &self,
        stream_id: &str,
        after_seq: i64,
    ) -> DbResult<Vec<ReceivedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms, dbf_delivered_unix_ms
             FROM received_events
             WHERE stream_id = ?1 AND seq > ?2
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![stream_id, after_seq],
            received_event_from_row,
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// One-time projection seed: per-epoch counts and latest row per epoch for
    /// a stream. O(N) once at startup (or after a hint-channel overflow); the
    /// hot path never calls this.
    pub fn load_stream_projection_summary(
        &self,
        stream_id: &str,
    ) -> DbResult<Vec<crate::projection::EpochSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT epoch, COUNT(*), MAX(received_unix_ms), MAX(seq)
             FROM received_events
             WHERE stream_id = ?1
             GROUP BY epoch
             ORDER BY epoch",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id], |row| {
            Ok(crate::projection::EpochSummary {
                epoch: row.get(0)?,
                count: row.get::<_, i64>(1)?.try_into().unwrap_or_default(),
                max_received_unix_ms: row.get(2)?,
                max_seq: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Raw frames of one epoch, ordered by seq, for the one-time projection
    /// chip-set seed. Bounded to a single epoch; the hot path never calls this.
    pub fn load_epoch_raw_frames(
        &self,
        stream_id: &str,
        epoch: i64,
    ) -> DbResult<Vec<(i64, Vec<u8>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, raw_frame
             FROM received_events
             WHERE stream_id = ?1 AND epoch = ?2
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id, epoch], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Load the distinct epochs durably received for `stream_id`, newest first,
    /// with the earliest `reader_timestamp` seen in each epoch.
    ///
    /// Keyed by the canonical `stream_id` because the live P2P data plane writes
    /// `received_events` (and advances cursors) by `stream_id` alone, leaving the
    /// legacy `(forwarder_id, reader_ip)` columns NULL. A legacy-keyed lookup
    /// therefore matches nothing for currently-receiving streams.
    pub fn load_replay_target_epochs(
        &self,
        stream_id: &str,
    ) -> DbResult<Vec<(i64, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT epoch, MIN(reader_timestamp)
             FROM received_events
             WHERE stream_id = ?1
             GROUP BY epoch
             ORDER BY epoch DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Load every durable event for `stream_id` that has not yet been written to
    /// the DBF file (`dbf_delivered_unix_ms IS NULL`), ordered by `seq`.
    ///
    /// This is the source for the idempotent DBF feed: once an event is marked
    /// delivered via [`Db::mark_dbf_delivered`], it is no longer returned here, so
    /// replay/re-run never re-emits an already-written `(stream_id, seq)`.
    pub fn load_undelivered_received_events(
        &self,
        stream_id: &str,
    ) -> DbResult<Vec<ReceivedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms, dbf_delivered_unix_ms
             FROM received_events
             WHERE stream_id = ?1 AND dbf_delivered_unix_ms IS NULL
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id], received_event_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Mark a single durable event as written to the DBF file. Only updates rows
    /// whose marker is still NULL, so this is safe to call repeatedly without
    /// overwriting an earlier delivery timestamp. Returns whether a row changed.
    pub fn mark_dbf_delivered(
        &self,
        stream_id: &str,
        seq: i64,
        delivered_unix_ms: i64,
    ) -> DbResult<bool> {
        let changed = self.conn.execute(
            "UPDATE received_events
             SET dbf_delivered_unix_ms = ?3
             WHERE stream_id = ?1 AND seq = ?2 AND dbf_delivered_unix_ms IS NULL",
            rusqlite::params![stream_id, seq, delivered_unix_ms],
        )?;
        Ok(changed > 0)
    }

    /// Clear the DBF delivery markers for every event of `stream_id`, returning
    /// the number of rows reset. Used when regenerating the DBF file from the
    /// durable store so all events are re-delivered on the next feed run.
    pub fn reset_dbf_delivered(&self, stream_id: &str) -> DbResult<usize> {
        let count = self.conn.execute(
            "UPDATE received_events SET dbf_delivered_unix_ms = NULL WHERE stream_id = ?1",
            rusqlite::params![stream_id],
        )?;
        Ok(count)
    }

    /// Load every durable event for `stream_id` that has not yet been pushed to
    /// the announcer (`announcer_pushed_unix_ms IS NULL`), ordered by the
    /// announcer ordering key `received_unix_ms` (with `seq` as a stable
    /// tie-breaker).
    ///
    /// This is the source for the idempotent announcer push: once an event is
    /// marked pushed via [`Db::mark_announcer_pushed`], it is no longer returned
    /// here, so a repush never re-emits an already-sent `(stream_id, seq)`.
    pub fn load_unpushed_announcer_events(&self, stream_id: &str) -> DbResult<Vec<ReceivedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms, dbf_delivered_unix_ms
             FROM received_events
             WHERE stream_id = ?1 AND announcer_pushed_unix_ms IS NULL
             ORDER BY received_unix_ms, seq",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id], received_event_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Mark a single durable event as pushed to the announcer. Only updates rows
    /// whose marker is still NULL, so this is safe to call repeatedly without
    /// overwriting an earlier push timestamp. Returns whether a row changed.
    pub fn mark_announcer_pushed(
        &self,
        stream_id: &str,
        seq: i64,
        pushed_unix_ms: i64,
    ) -> DbResult<bool> {
        let changed = self.conn.execute(
            "UPDATE received_events
             SET announcer_pushed_unix_ms = ?3
             WHERE stream_id = ?1 AND seq = ?2 AND announcer_pushed_unix_ms IS NULL",
            rusqlite::params![stream_id, seq, pushed_unix_ms],
        )?;
        Ok(changed > 0)
    }

    /// Return the highest announcer source generation accepted for `stream_id`,
    /// or `None` if no generation has been fenced yet.
    pub fn load_announcer_fence(&self, stream_id: &str) -> DbResult<Option<i64>> {
        self.conn
            .query_row(
                "SELECT generation FROM announcer_source_fence WHERE stream_id = ?1",
                rusqlite::params![stream_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Atomically accept `generation` as current for `stream_id`, or report it
    /// stale when a higher generation has already been accepted.
    ///
    /// The stored value only ever moves forward. Equal generations are current
    /// and accepted for idempotent retries; lower generations are observable as
    /// stale so callers can avoid sending rows for an outdated source.
    pub fn accept_announcer_generation(
        &self,
        stream_id: &str,
        generation: i64,
    ) -> DbResult<AnnouncerGenerationAcceptance> {
        let changed = self.conn.execute(
            "INSERT INTO announcer_source_fence (stream_id, generation)
             VALUES (?1, ?2)
             ON CONFLICT (stream_id) DO UPDATE SET generation = excluded.generation
             WHERE excluded.generation >= announcer_source_fence.generation",
            rusqlite::params![stream_id, generation],
        )?;
        if changed > 0 {
            return Ok(AnnouncerGenerationAcceptance::Current { generation });
        }

        let current = self.load_announcer_fence(stream_id)?.unwrap_or(generation);
        Ok(AnnouncerGenerationAcceptance::Stale {
            current,
            attempted: generation,
        })
    }

    /// Clear all per-stream announcer source-generation fences.
    ///
    /// Fences are monotonic per stream, which is correct for a single server
    /// but wrong across a server change: a replacement announcer backend may
    /// start at a lower generation and would otherwise be fenced out forever.
    /// The reconcile loop calls this when the server config changes so the new
    /// server's generation is accepted fresh.
    pub fn reset_announcer_fences(&self) -> DbResult<()> {
        self.conn
            .execute_batch("DELETE FROM announcer_source_fence")?;
        Ok(())
    }

    pub fn load_received_event(
        &self,
        stream_id: &str,
        seq: i64,
    ) -> DbResult<Option<ReceivedEvent>> {
        load_received_event_conn(&self.conn, stream_id, seq)
    }

    /// Read the current durable cursor (`last_seq`) for a P2P stream, or 0 when
    /// no cursor row exists yet. The cursor is the highest contiguous seq that
    /// has been durably stored and acknowledged for `stream_id`; it is the
    /// `after_seq` a resuming `DataSubscribe` must request.
    pub fn load_stream_cursor(&self, stream_id: &str) -> DbResult<i64> {
        load_stream_cursor_conn(&self.conn, stream_id)
    }

    /// Jump the durable cursor for `stream_id` forward to `last_seq`, used when a
    /// `GapNotice` makes earlier seqs permanently unavailable (jump target is
    /// `earliest_available_seq - 1`). The cursor only ever moves forward: a jump
    /// target at or below the current cursor is ignored so a late or duplicate
    /// gap notice cannot regress progress.
    pub fn jump_stream_cursor(&self, stream_id: &str, last_seq: i64) -> DbResult<()> {
        jump_stream_cursor_conn(&self.conn, stream_id, last_seq)
    }

    pub fn advance_cursor_contiguous_prefix(&self, stream_id: &str) -> DbResult<i64> {
        advance_cursor_contiguous_prefix_conn(&self.conn, stream_id)
    }

    pub fn save_gap_marker(&self, marker: &GapMarkerInsert<'_>) -> DbResult<()> {
        save_gap_marker_conn(&self.conn, marker)
    }

    pub fn load_gap_markers(&self, stream_id: &str) -> DbResult<Vec<GapMarker>> {
        let mut stmt = self.conn.prepare(
            "SELECT stream_id, requested_after_seq, earliest_available_seq, latest_available_seq, reason, created_unix_ms
             FROM gap_markers
             WHERE stream_id = ?1
             ORDER BY created_unix_ms, id",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id], |r| {
            Ok(GapMarker {
                stream_id: r.get(0)?,
                requested_after_seq: r.get(1)?,
                earliest_available_seq: r.get(2)?,
                latest_available_seq: r.get(3)?,
                reason: r.get(4)?,
                created_unix_ms: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn apply_pragmas(&self) -> DbResult<()> {
        // `synchronous=FULL` is a durability requirement, not a tunable: the
        // receiver acks each EventBatch only after the commit, and the
        // forwarder prunes acked rows. With WAL + `synchronous=NORMAL` a commit
        // does NOT fsync, so a power loss could silently lose rows that were
        // already acked and pruned upstream — unrecoverable data loss. Do not
        // "optimize" this to NORMAL.
        //
        // The remaining pragmas target the deployment hardware (old 2-core
        // Windows laptops with slow disks): a generous busy_timeout so
        // cross-connection writes ride out group commits instead of failing,
        // a 16 MiB page cache, and in-memory temp tables.
        self.conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA wal_autocheckpoint=1000;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=10000;
             PRAGMA cache_size=-16384;
             PRAGMA temp_store=MEMORY;",
        )?;
        Ok(())
    }
    fn apply_schema(&self) -> DbResult<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        // Migration: rename the legacy profile.thin_node_url column to server_url.
        migrate_profile_server_url_column(&self.conn)?;
        // Migration: add update_mode column to existing profile tables.
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN update_mode TEXT NOT NULL DEFAULT 'check-and-download';",
            "update_mode",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN receiver_mode_json TEXT;",
            "receiver_mode_json",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN receiver_id TEXT;",
            "receiver_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN dbf_enabled INTEGER NOT NULL DEFAULT 0;",
            "dbf_enabled",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN announcer_enabled INTEGER NOT NULL DEFAULT 0;",
            "announcer_enabled",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN announcer_max_list_size INTEGER NOT NULL DEFAULT 25;",
            "announcer_max_list_size",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN device_token TEXT;",
            "device_token",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE subscriptions ADD COLUMN event_type TEXT NOT NULL DEFAULT 'finish';",
            "event_type",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE subscriptions ADD COLUMN forwarder_endpoint_id TEXT NOT NULL DEFAULT '';",
            "forwarder_endpoint_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE subscriptions ADD COLUMN stream_id TEXT NOT NULL DEFAULT '';",
            "stream_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE subscriptions ADD COLUMN local_port_override INTEGER;",
            "local_port_override",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE subscriptions ADD COLUMN forwarder_id TEXT;",
            "forwarder_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE subscriptions ADD COLUMN reader_ip TEXT;",
            "reader_ip",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE cursors ADD COLUMN stream_id TEXT;",
            "stream_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE cursors ADD COLUMN last_seq BIGINT NOT NULL DEFAULT 0;",
            "last_seq",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE cursors ADD COLUMN forwarder_id TEXT;",
            "forwarder_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE cursors ADD COLUMN reader_ip TEXT;",
            "reader_ip",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE cursors ADD COLUMN stream_epoch BIGINT;",
            "stream_epoch",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE received_events ADD COLUMN announcer_pushed_unix_ms BIGINT;",
            "announcer_pushed_unix_ms",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE earliest_epochs ADD COLUMN stream_id TEXT;",
            "stream_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE earliest_epochs ADD COLUMN forwarder_endpoint_id TEXT;",
            "forwarder_endpoint_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE earliest_epochs ADD COLUMN forwarder_id TEXT;",
            "forwarder_id",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE earliest_epochs ADD COLUMN reader_ip TEXT;",
            "reader_ip",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE participants ADD COLUMN division INTEGER;",
            "division",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN rd_import_enabled INTEGER NOT NULL DEFAULT 0;",
            "rd_import_enabled",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN rd_import_dir TEXT NOT NULL DEFAULT 'C:\\Winrace\\Files';",
            "rd_import_dir",
        )?;
        apply_add_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN rd_import_interval_secs INTEGER NOT NULL DEFAULT 15;",
            "rd_import_interval_secs",
        )?;
        migrate_subscriptions_to_endpoint_stream_shape(&self.conn)?;
        migrate_cursors_to_stream_id_shape(&self.conn)?;
        migrate_earliest_epochs_to_stream_id_shape(&self.conn)?;
        migrate_forwarder_intent(&self.conn)?;
        Ok(())
    }

    pub fn forwarder_should_connect(&self, endpoint_id: &str) -> DbResult<bool> {
        let value: Option<i64> = self
            .conn
            .query_row(
                "SELECT connect FROM forwarder_intent WHERE endpoint_id = ?1",
                [endpoint_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value != Some(0))
    }

    pub fn set_forwarder_intent(&self, endpoint_id: &str, connect: bool) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO forwarder_intent(endpoint_id, connect) VALUES(?1, ?2)
             ON CONFLICT(endpoint_id) DO UPDATE SET connect = excluded.connect",
            rusqlite::params![endpoint_id, i64::from(connect)],
        )?;
        Ok(())
    }

    pub fn load_forwarder_intents(&self) -> DbResult<std::collections::HashMap<String, bool>> {
        let mut stmt = self
            .conn
            .prepare("SELECT endpoint_id, connect FROM forwarder_intent")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (endpoint_id, connect) = row?;
            map.insert(endpoint_id, connect);
        }
        Ok(map)
    }

    pub fn delete_all_cursors(&self) -> DbResult<usize> {
        let count = self.conn.execute("DELETE FROM cursors", [])?;
        Ok(count)
    }

    pub fn delete_all_earliest_epochs(&self) -> DbResult<usize> {
        let count = self.conn.execute("DELETE FROM earliest_epochs", [])?;
        Ok(count)
    }

    pub fn update_subscription_port(
        &self,
        fwd: &str,
        ip: &str,
        port: Option<u16>,
    ) -> DbResult<bool> {
        let count = self.conn.execute(
            "UPDATE subscriptions
             SET local_port_override = ?1
             WHERE (forwarder_endpoint_id = ?2 AND stream_id = ?3)
                OR (forwarder_id = ?2 AND reader_ip = ?3)",
            rusqlite::params![port.map(|p| p as i64), fwd, ip],
        )?;
        Ok(count > 0)
    }

    pub fn update_stream_subscription_port(
        &self,
        forwarder_endpoint_id: &str,
        stream_id: &str,
        port: Option<u16>,
    ) -> DbResult<bool> {
        let count = self.conn.execute(
            "UPDATE subscriptions
             SET local_port_override = ?1
             WHERE forwarder_endpoint_id = ?2 AND stream_id = ?3",
            rusqlite::params![port.map(|p| p as i64), forwarder_endpoint_id, stream_id],
        )?;
        Ok(count > 0)
    }

    pub fn delete_all_subscriptions(&self) -> DbResult<usize> {
        let count = self.conn.execute("DELETE FROM subscriptions", [])?;
        Ok(count)
    }

    pub fn reset_profile(&self) -> DbResult<()> {
        self.conn.execute_batch("DELETE FROM profile")?;
        self.conn.execute(
            "INSERT INTO profile (server_url, token, update_mode) VALUES ('', '', ?1)",
            rusqlite::params![DEFAULT_UPDATE_MODE],
        )?;
        Ok(())
    }

    pub fn clear_data(&mut self) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM cursors")?;
        tx.execute_batch("DELETE FROM received_events")?;
        tx.execute_batch("DELETE FROM announcer_source_fence")?;
        tx.execute_batch("DELETE FROM gap_markers")?;
        tx.execute_batch("DELETE FROM earliest_epochs")?;
        tx.execute_batch("DELETE FROM subscriptions")?;
        tx.execute_batch("DELETE FROM announcer_publish_streams")?;
        tx.execute(
            "UPDATE profile SET update_mode = ?1, receiver_mode_json = NULL, dbf_enabled = 0, announcer_enabled = 0",
            rusqlite::params![DEFAULT_UPDATE_MODE],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn factory_reset(&mut self) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM cursors")?;
        tx.execute_batch("DELETE FROM received_events")?;
        tx.execute_batch("DELETE FROM announcer_source_fence")?;
        tx.execute_batch("DELETE FROM gap_markers")?;
        tx.execute_batch("DELETE FROM earliest_epochs")?;
        tx.execute_batch("DELETE FROM subscriptions")?;
        tx.execute_batch("DELETE FROM forwarder_intent")?;
        tx.execute_batch("DELETE FROM announcer_publish_streams")?;
        tx.execute_batch("DELETE FROM participants")?;
        tx.execute_batch("DELETE FROM bib_chips")?;
        tx.execute_batch("DELETE FROM profile")?;
        tx.execute(
            "INSERT INTO profile (server_url, token, update_mode) VALUES ('', '', ?1)",
            rusqlite::params![DEFAULT_UPDATE_MODE],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn load_dbf_config(&self) -> DbResult<DbfConfig> {
        let enabled: Option<i64> = self
            .conn
            .query_row("SELECT dbf_enabled FROM profile LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(DbfConfig {
            enabled: enabled.unwrap_or(0) != 0,
        })
    }

    pub fn save_dbf_config(&self, config: &DbfConfig) -> DbResult<()> {
        let changed = self.conn.execute(
            "UPDATE profile SET dbf_enabled = ?1",
            rusqlite::params![config.enabled as i64],
        )?;
        if changed == 0 {
            return Err(DbError::ProfileMissing);
        }
        Ok(())
    }

    pub fn load_rd_import_config(&self) -> DbResult<RdImportConfig> {
        let result: Option<(i64, String, i64)> = self
            .conn
            .query_row(
                "SELECT rd_import_enabled, rd_import_dir, rd_import_interval_secs \
                 FROM profile LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        Ok(match result {
            Some((enabled, dir, interval)) => RdImportConfig {
                enabled: enabled != 0,
                dir: if dir.trim().is_empty() {
                    DEFAULT_RD_IMPORT_DIR.to_owned()
                } else {
                    dir
                },
                interval_secs: u32::try_from(interval)
                    .unwrap_or(DEFAULT_RD_IMPORT_INTERVAL_SECS)
                    .max(1),
            },
            None => RdImportConfig {
                enabled: false,
                dir: DEFAULT_RD_IMPORT_DIR.to_owned(),
                interval_secs: DEFAULT_RD_IMPORT_INTERVAL_SECS,
            },
        })
    }

    pub fn save_rd_import_config(&self, config: &RdImportConfig) -> DbResult<()> {
        if let Err(msg) = config.validate() {
            return Err(DbError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                msg,
            )));
        }
        let changed = self.conn.execute(
            "UPDATE profile SET rd_import_enabled = ?1, rd_import_dir = ?2, \
             rd_import_interval_secs = ?3",
            rusqlite::params![
                i64::from(config.enabled),
                config.dir,
                i64::from(config.interval_secs),
            ],
        )?;
        if changed == 0 {
            return Err(DbError::ProfileMissing);
        }
        Ok(())
    }

    /// Whether the global announcer publish toggle is on. Defaults to `false`
    /// (opt-in) when no profile row exists.
    pub fn load_announcer_enabled(&self) -> DbResult<bool> {
        let enabled: Option<i64> = self
            .conn
            .query_row("SELECT announcer_enabled FROM profile LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(enabled.unwrap_or(0) != 0)
    }

    /// Set the global announcer publish toggle. Requires a profile row
    /// (created at receiver startup).
    pub fn set_announcer_enabled(&self, enabled: bool) -> DbResult<()> {
        let changed = self.conn.execute(
            "UPDATE profile SET announcer_enabled = ?1",
            rusqlite::params![i64::from(enabled)],
        )?;
        if changed == 0 {
            return Err(DbError::ProfileMissing);
        }
        Ok(())
    }

    /// The server-minted per-device token, if one has been persisted. This is
    /// the receiver's long-term server credential; the configured `token` is
    /// only the bootstrap voucher used to mint it.
    pub fn load_device_token(&self) -> DbResult<Option<String>> {
        let value: Option<Option<String>> = self
            .conn
            .query_row("SELECT device_token FROM profile LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(value.flatten().filter(|token| !token.trim().is_empty()))
    }

    /// Persist the server-minted per-device token. Requires a profile row
    /// (created at receiver startup).
    pub fn set_device_token(&self, device_token: &str) -> DbResult<()> {
        let changed = self.conn.execute(
            "UPDATE profile SET device_token = ?1",
            rusqlite::params![device_token],
        )?;
        if changed == 0 {
            return Err(DbError::ProfileMissing);
        }
        Ok(())
    }

    /// Clear the persisted device token (e.g. on a server change, so the next
    /// start re-bootstraps against the new server from the voucher).
    pub fn clear_device_token(&self) -> DbResult<()> {
        self.conn
            .execute("UPDATE profile SET device_token = NULL", [])?;
        Ok(())
    }

    /// Receiver-configured cap on the number of rows the server announcer feed
    /// keeps visible. Defaults to 25 when unset or out of range.
    pub fn load_announcer_max_list_size(&self) -> DbResult<u32> {
        let value: Option<i64> = self
            .conn
            .query_row(
                "SELECT announcer_max_list_size FROM profile LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let resolved = value
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(25);
        Ok(resolved)
    }

    pub fn set_announcer_max_list_size(&self, max_list_size: u32) -> DbResult<()> {
        let changed = self.conn.execute(
            "UPDATE profile SET announcer_max_list_size = ?1",
            rusqlite::params![i64::from(max_list_size)],
        )?;
        if changed == 0 {
            return Err(DbError::ProfileMissing);
        }
        Ok(())
    }

    /// Enable or disable announcer publishing for a single stream (opt-in).
    pub fn set_stream_announcer_publish(&self, stream_id: &str, publish: bool) -> DbResult<()> {
        if publish {
            self.conn.execute(
                "INSERT OR IGNORE INTO announcer_publish_streams (stream_id) VALUES (?1)",
                rusqlite::params![stream_id],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM announcer_publish_streams WHERE stream_id = ?1",
                rusqlite::params![stream_id],
            )?;
        }
        Ok(())
    }

    /// The set of stream ids opted in to announcer publishing.
    pub fn load_announcer_publish_streams(&self) -> DbResult<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT stream_id FROM announcer_publish_streams")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut set = std::collections::HashSet::new();
        for row in rows {
            set.insert(row?);
        }
        Ok(set)
    }

    pub fn update_subscription_event_type(
        &self,
        fwd: &str,
        ip: &str,
        event_type: EventType,
    ) -> DbResult<bool> {
        let count = self.conn.execute(
            "UPDATE subscriptions
             SET event_type = ?1
             WHERE (forwarder_endpoint_id = ?2 AND stream_id = ?3)
                OR (forwarder_id = ?2 AND reader_ip = ?3)",
            rusqlite::params![event_type.as_str(), fwd, ip],
        )?;
        Ok(count > 0)
    }

    pub fn update_stream_subscription_event_type(
        &self,
        forwarder_endpoint_id: &str,
        stream_id: &str,
        event_type: EventType,
    ) -> DbResult<bool> {
        let count = self.conn.execute(
            "UPDATE subscriptions
             SET event_type = ?1
             WHERE forwarder_endpoint_id = ?2 AND stream_id = ?3",
            rusqlite::params![event_type.as_str(), forwarder_endpoint_id, stream_id],
        )?;
        Ok(count > 0)
    }

    pub fn load_subscription_dbf_details(
        &self,
        fwd: &str,
        ip: &str,
    ) -> DbResult<Option<(usize, EventType)>> {
        let result: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT s1.event_type,
                        (
                            SELECT COUNT(*)
                            FROM subscriptions s2
                            WHERE COALESCE(s2.forwarder_id, s2.forwarder_endpoint_id) < COALESCE(s1.forwarder_id, s1.forwarder_endpoint_id)
                               OR (COALESCE(s2.forwarder_id, s2.forwarder_endpoint_id) = COALESCE(s1.forwarder_id, s1.forwarder_endpoint_id)
                                   AND COALESCE(s2.reader_ip, s2.stream_id) < COALESCE(s1.reader_ip, s1.stream_id))
                        ) AS subscription_index
                 FROM subscriptions s1
                 WHERE (s1.forwarder_endpoint_id = ?1 AND s1.stream_id = ?2)
                    OR (s1.forwarder_id = ?1 AND s1.reader_ip = ?2)",
                rusqlite::params![fwd, ip],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        result
            .map(|(raw_event_type, idx)| {
                let event_type = raw_event_type.parse::<EventType>().map_err(|e| {
                    DbError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;
                let idx = usize::try_from(idx).map_err(|e| {
                    DbError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;
                Ok((idx, event_type))
            })
            .transpose()
    }

    fn load_receiver_mode_json_raw(&self) -> DbResult<Option<String>> {
        let raw: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT receiver_mode_json FROM profile LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw.flatten().and_then(|json| {
            if json.trim().is_empty() {
                None
            } else {
                Some(json)
            }
        }))
    }
}

fn parse_event_type_column(raw: String, column: usize) -> rusqlite::Result<EventType> {
    match raw.parse::<EventType>() {
        Ok(et) => Ok(et),
        Err(e) => {
            tracing::error!(error = %e, value = %raw, "corrupt event_type in database");
            Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            ))
        }
    }
}

/// Deterministic stream_id used for legacy cursor rows keyed only by
/// `(forwarder_id, reader_ip)`. Unit Separator avoids ambiguity between the
/// two user-provided strings while keeping the value human-readable in SQLite.
fn legacy_cursor_stream_id(fwd: &str, ip: &str) -> String {
    format!("legacy:{fwd}\u{1f}{ip}")
}

/// Row-level operations usable both on a plain connection and inside a
/// [`rusqlite::Transaction`] (which derefs to [`Connection`]). The P2P persist
/// path runs these inside one `IMMEDIATE` transaction per `EventBatch`.
pub fn insert_received_event_conn(
    conn: &Connection,
    event: &ReceivedEventInsert<'_>,
) -> DbResult<bool> {
    let changed = conn.execute(
        "INSERT INTO received_events
         (stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms, dbf_delivered_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (stream_id, seq) DO NOTHING",
        rusqlite::params![
            event.stream_id,
            event.seq,
            event.epoch,
            event.raw_frame,
            event.read_kind,
            event.reader_timestamp,
            event.received_unix_ms,
            event.dbf_delivered_unix_ms,
        ],
    )?;
    Ok(changed > 0)
}

pub fn load_received_event_conn(
    conn: &Connection,
    stream_id: &str,
    seq: i64,
) -> DbResult<Option<ReceivedEvent>> {
    conn.query_row(
        "SELECT stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms, dbf_delivered_unix_ms
         FROM received_events
         WHERE stream_id = ?1 AND seq = ?2",
        rusqlite::params![stream_id, seq],
        received_event_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn load_stream_cursor_conn(conn: &Connection, stream_id: &str) -> DbResult<i64> {
    let last_seq: Option<i64> = conn
        .query_row(
            "SELECT last_seq FROM cursors WHERE stream_id = ?1",
            rusqlite::params![stream_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(last_seq.unwrap_or(0))
}

pub fn jump_stream_cursor_conn(conn: &Connection, stream_id: &str, last_seq: i64) -> DbResult<()> {
    conn.execute(
        "INSERT INTO cursors (stream_id, last_seq) VALUES (?1, ?2)
         ON CONFLICT (stream_id) DO UPDATE SET last_seq = excluded.last_seq
         WHERE excluded.last_seq > cursors.last_seq",
        rusqlite::params![stream_id, last_seq],
    )?;
    Ok(())
}

pub fn advance_cursor_contiguous_prefix_conn(conn: &Connection, stream_id: &str) -> DbResult<i64> {
    let current: Option<i64> = conn
        .query_row(
            "SELECT last_seq FROM cursors WHERE stream_id = ?1",
            rusqlite::params![&stream_id],
            |r| r.get(0),
        )
        .optional()?;
    let mut last_seq = current.unwrap_or(0);

    let mut stmt = conn.prepare(
        "SELECT seq FROM received_events WHERE stream_id = ?1 AND seq > ?2 ORDER BY seq",
    )?;
    let rows = stmt.query_map(rusqlite::params![stream_id, last_seq], |r| {
        r.get::<_, i64>(0)
    })?;
    for row in rows {
        let seq = row?;
        if seq == last_seq + 1 {
            last_seq = seq;
        } else if seq > last_seq + 1 {
            break;
        }
    }

    let updated = conn.execute(
        "UPDATE cursors SET last_seq = ?2 WHERE stream_id = ?1 AND last_seq < ?2",
        rusqlite::params![stream_id, last_seq],
    )?;
    if updated == 0 && current.is_none() {
        conn.execute(
            "INSERT INTO cursors (stream_id, last_seq) VALUES (?1, ?2)",
            rusqlite::params![stream_id, last_seq],
        )?;
    }
    Ok(last_seq)
}

pub fn save_gap_marker_conn(conn: &Connection, marker: &GapMarkerInsert<'_>) -> DbResult<()> {
    conn.execute(
        "INSERT INTO gap_markers
         (stream_id, requested_after_seq, earliest_available_seq, latest_available_seq, reason, created_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            marker.stream_id,
            marker.requested_after_seq,
            marker.earliest_available_seq,
            marker.latest_available_seq,
            marker.reason,
            marker.created_unix_ms,
        ],
    )?;
    Ok(())
}

fn received_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReceivedEvent> {
    Ok(ReceivedEvent {
        stream_id: row.get(0)?,
        seq: row.get(1)?,
        epoch: row.get(2)?,
        raw_frame: row.get(3)?,
        read_kind: row.get(4)?,
        reader_timestamp: row.get(5)?,
        received_unix_ms: row.get(6)?,
        dbf_delivered_unix_ms: row.get(7)?,
    })
}

fn apply_add_column_migration(conn: &Connection, sql: &str, column_name: &str) -> DbResult<()> {
    match conn.execute_batch(sql) {
        Ok(()) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if is_duplicate_column_error(&message, column_name) =>
        {
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn migrate_subscriptions_to_endpoint_stream_shape(conn: &Connection) -> DbResult<()> {
    let columns = load_table_columns(conn, "subscriptions")?;
    if has_column_pk_notnull(&columns, "forwarder_endpoint_id", 1, true)
        && has_column_pk_notnull(&columns, "stream_id", 2, true)
        && !legacy_column_has_pk_or_notnull(&columns, "forwarder_id")
        && !legacy_column_has_pk_or_notnull(&columns, "reader_ip")
    {
        return Ok(());
    }

    conn.execute_batch(
        "SAVEPOINT migrate_subscriptions_to_v2;
         CREATE TABLE subscriptions_v2 (
             forwarder_endpoint_id TEXT NOT NULL,
             stream_id             TEXT NOT NULL,
             local_port_override   INTEGER,
             event_type            TEXT NOT NULL DEFAULT 'finish',
             forwarder_id          TEXT,
             reader_ip             TEXT,
             PRIMARY KEY (forwarder_endpoint_id, stream_id)
         );
         INSERT OR REPLACE INTO subscriptions_v2
             (forwarder_endpoint_id, stream_id, local_port_override, event_type, forwarder_id, reader_ip)
         SELECT
             COALESCE(NULLIF(forwarder_endpoint_id, ''), forwarder_id),
             COALESCE(NULLIF(stream_id, ''), reader_ip),
             local_port_override,
             COALESCE(event_type, 'finish'),
             forwarder_id,
             reader_ip
         FROM subscriptions;
         DROP TABLE subscriptions;
         ALTER TABLE subscriptions_v2 RENAME TO subscriptions;
         RELEASE migrate_subscriptions_to_v2;",
    )?;
    Ok(())
}

fn migrate_earliest_epochs_to_stream_id_shape(conn: &Connection) -> DbResult<()> {
    let columns = load_table_columns(conn, "earliest_epochs")?;
    if has_column_pk_notnull(&columns, "stream_id", 1, false)
        && has_column_notnull(&columns, "forwarder_endpoint_id", true)
        && !legacy_column_has_pk_or_notnull(&columns, "forwarder_id")
        && !legacy_column_has_pk_or_notnull(&columns, "reader_ip")
    {
        return Ok(());
    }

    conn.execute_batch(
        "SAVEPOINT migrate_earliest_epochs_to_v2;
         CREATE TABLE earliest_epochs_v2 (
             stream_id             TEXT PRIMARY KEY,
             forwarder_endpoint_id TEXT NOT NULL,
             earliest_epoch        BIGINT NOT NULL,
             forwarder_id          TEXT,
             reader_ip             TEXT
         );
         INSERT OR REPLACE INTO earliest_epochs_v2
             (stream_id, forwarder_endpoint_id, earliest_epoch, forwarder_id, reader_ip)
         SELECT
             COALESCE(NULLIF(stream_id, ''), 'legacy:' || forwarder_id || char(31) || reader_ip),
             COALESCE(NULLIF(forwarder_endpoint_id, ''), forwarder_id),
             earliest_epoch,
             forwarder_id,
             reader_ip
         FROM earliest_epochs;
         DROP TABLE earliest_epochs;
         ALTER TABLE earliest_epochs_v2 RENAME TO earliest_epochs;
         RELEASE migrate_earliest_epochs_to_v2;",
    )?;
    Ok(())
}

fn migrate_cursors_to_stream_id_shape(conn: &Connection) -> DbResult<()> {
    let columns = load_table_columns(conn, "cursors")?;
    if has_column_pk_notnull(&columns, "stream_id", 1, false)
        && has_column_notnull(&columns, "last_seq", true)
        && !legacy_column_has_pk_or_notnull(&columns, "forwarder_id")
        && !legacy_column_has_pk_or_notnull(&columns, "reader_ip")
    {
        return Ok(());
    }

    conn.execute_batch(
        "SAVEPOINT migrate_cursors_to_v2;
         CREATE TABLE cursors_v2 (
             stream_id    TEXT PRIMARY KEY,
             last_seq     BIGINT NOT NULL,
             forwarder_id TEXT,
             reader_ip    TEXT,
             stream_epoch BIGINT
         );
         INSERT OR REPLACE INTO cursors_v2
             (stream_id, last_seq, forwarder_id, reader_ip, stream_epoch)
         SELECT
             COALESCE(NULLIF(stream_id, ''), 'legacy:' || forwarder_id || char(31) || reader_ip),
             acked_through_seq,
             forwarder_id,
             reader_ip,
             stream_epoch
         FROM cursors;
         DROP TABLE cursors;
         ALTER TABLE cursors_v2 RENAME TO cursors;
         RELEASE migrate_cursors_to_v2;",
    )?;
    Ok(())
}

fn migrate_forwarder_intent(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS forwarder_intent (
             endpoint_id TEXT PRIMARY KEY,
             connect     INTEGER NOT NULL
         );",
    )?;
    Ok(())
}

#[derive(Debug)]
struct TableColumn {
    name: String,
    notnull: bool,
    pk: i64,
}

/// Rename the legacy `profile.thin_node_url` column to `server_url`.
///
/// Idempotent: only renames when the legacy column exists and the new column
/// does not, so it is a no-op on fresh databases (created with `server_url`)
/// and on databases already migrated.
fn migrate_profile_server_url_column(conn: &Connection) -> DbResult<()> {
    let columns = load_table_columns(conn, "profile")?;
    let has_legacy = columns.iter().any(|column| column.name == "thin_node_url");
    let has_new = columns.iter().any(|column| column.name == "server_url");
    if has_legacy && !has_new {
        conn.execute_batch("ALTER TABLE profile RENAME COLUMN thin_node_url TO server_url;")?;
    }
    Ok(())
}

fn load_table_columns(conn: &Connection, table: &str) -> DbResult<Vec<TableColumn>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| {
        Ok(TableColumn {
            name: row.get(1)?,
            notnull: row.get::<_, i64>(3)? != 0,
            pk: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn has_column_pk_notnull(
    columns: &[TableColumn],
    column_name: &str,
    expected_pk: i64,
    expected_notnull: bool,
) -> bool {
    columns.iter().any(|column| {
        column.name == column_name && column.pk == expected_pk && column.notnull == expected_notnull
    })
}

fn has_column_notnull(columns: &[TableColumn], column_name: &str, expected_notnull: bool) -> bool {
    columns
        .iter()
        .any(|column| column.name == column_name && column.notnull == expected_notnull)
}

fn legacy_column_has_pk_or_notnull(columns: &[TableColumn], column_name: &str) -> bool {
    columns
        .iter()
        .any(|column| column.name == column_name && (column.pk != 0 || column.notnull))
}

fn is_duplicate_column_error(message: &str, column_name: &str) -> bool {
    message.contains(&format!("duplicate column name: {column_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcer_flags_persist() {
        let mut db = Db::open_in_memory().unwrap();
        // A profile row is required for the global toggle (created at startup).
        db.save_profile("http://x", "t", DEFAULT_UPDATE_MODE, None)
            .unwrap();
        assert!(!db.load_announcer_enabled().unwrap(), "defaults to off");
        db.set_announcer_enabled(true).unwrap();
        assert!(db.load_announcer_enabled().unwrap());

        // Saving the profile again must preserve the announcer toggle.
        db.save_profile("http://y", "t2", DEFAULT_UPDATE_MODE, None)
            .unwrap();
        assert!(
            db.load_announcer_enabled().unwrap(),
            "announcer_enabled must survive a profile save"
        );

        // Per-stream publish is opt-in and tracked independently.
        assert!(db.load_announcer_publish_streams().unwrap().is_empty());
        db.set_stream_announcer_publish("127.0.0.1:1", true)
            .unwrap();
        assert!(
            db.load_announcer_publish_streams()
                .unwrap()
                .contains("127.0.0.1:1")
        );
        db.set_stream_announcer_publish("127.0.0.1:1", false)
            .unwrap();
        assert!(db.load_announcer_publish_streams().unwrap().is_empty());
    }

    #[test]
    fn announcer_max_list_size_persists_and_defaults() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("http://x", "t", DEFAULT_UPDATE_MODE, None)
            .unwrap();
        assert_eq!(
            db.load_announcer_max_list_size().unwrap(),
            25,
            "defaults to 25"
        );

        db.set_announcer_max_list_size(60).unwrap();
        assert_eq!(db.load_announcer_max_list_size().unwrap(), 60);

        // Must survive a profile save (delete+insert).
        db.save_profile("http://y", "t2", DEFAULT_UPDATE_MODE, None)
            .unwrap();
        assert_eq!(
            db.load_announcer_max_list_size().unwrap(),
            60,
            "announcer_max_list_size must survive a profile save"
        );
    }

    #[test]
    fn replace_and_load_participants_and_chips() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_participants(&[crate::participants::Participant {
            bib: 1,
            last: "A".to_owned(),
            first: "B".to_owned(),
            affiliation: String::new(),
            gender: "M".to_owned(),
            division: None,
        }])
        .unwrap();
        db.replace_bib_chips(&[(1i64, "0580".to_owned())]).unwrap();
        let map = db.load_chip_to_participant().unwrap();
        let entry = map.get("0580").unwrap();
        assert_eq!(entry.bib, "1");
        assert_eq!(entry.name.as_deref(), Some("B A"));
        assert_eq!(entry.division, None);
    }

    #[test]
    fn unmatched_chip_resolves_to_bib_without_participant_and_replace_clears() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_bib_chips(&[(99, "deadbeef".to_owned())])
            .unwrap();
        let map = db.load_chip_to_participant().unwrap();
        let entry = map.get("deadbeef").expect("bib-only chip resolves");
        assert_eq!(entry.bib, "99");
        assert_eq!(entry.name, None);
        assert_eq!(entry.division, None);
        // Re-import replaces wholesale.
        db.replace_participants(&[crate::participants::Participant {
            bib: 5,
            last: "Last".to_owned(),
            first: "First".to_owned(),
            affiliation: "Team".to_owned(),
            gender: "X".to_owned(),
            division: None,
        }])
        .unwrap();
        db.replace_bib_chips(&[(5, "abc".to_owned())]).unwrap();
        let map = db.load_chip_to_participant().unwrap();
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key("deadbeef"));
        let entry = map.get("abc").unwrap();
        assert_eq!(entry.bib, "5");
        assert_eq!(entry.name.as_deref(), Some("First Last"));
    }

    #[test]
    fn migrates_legacy_thin_node_url_column_to_server_url() {
        let conn = Connection::open_in_memory().unwrap();
        // Pre-rename profile schema, created with the legacy column name.
        conn.execute_batch(
            "CREATE TABLE profile (
                 thin_node_url TEXT NOT NULL,
                 token TEXT NOT NULL,
                 update_mode TEXT NOT NULL DEFAULT 'check-and-download'
             );
             INSERT INTO profile (thin_node_url, token)
             VALUES ('https://legacy.example', 'legacy-token');",
        )
        .unwrap();

        let db = Db { conn };
        db.apply_pragmas().unwrap();
        db.apply_schema().unwrap();

        let columns = load_table_columns(&db.conn, "profile").unwrap();
        assert!(columns.iter().any(|column| column.name == "server_url"));
        assert!(!columns.iter().any(|column| column.name == "thin_node_url"));

        let profile = db.load_profile().unwrap().expect("profile row preserved");
        assert_eq!(profile.server_url, "https://legacy.example");
        assert_eq!(profile.token, "legacy-token");
    }

    #[test]
    fn profile_round_trip_with_update_mode() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-only", None)
            .unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.update_mode, "check-only");
    }

    #[test]
    fn profile_update_mode_defaults_for_existing_db() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-and-download", None)
            .unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.update_mode, "check-and-download");
    }

    #[test]
    fn duplicate_column_message_detection_matches_expected_error() {
        assert!(is_duplicate_column_error(
            "duplicate column name: update_mode",
            "update_mode"
        ));
        assert!(!is_duplicate_column_error(
            "near \"ALTER\": syntax error",
            "update_mode"
        ));
    }

    #[test]
    fn forwarder_intent_defaults_to_connect_and_persists_disconnect() {
        let db = Db::open_in_memory().unwrap();
        // Unknown forwarder defaults to connect (true).
        assert!(db.forwarder_should_connect("fwd-1").unwrap());
        db.set_forwarder_intent("fwd-1", false).unwrap();
        assert!(!db.forwarder_should_connect("fwd-1").unwrap());
        let intents = db.load_forwarder_intents().unwrap();
        assert_eq!(intents.get("fwd-1"), Some(&false));
    }

    #[test]
    fn forwarder_intent_overwrite_and_independent_endpoints() {
        let db = Db::open_in_memory().unwrap();
        // Overwrite/update path: false then true reads back true.
        db.set_forwarder_intent("fwd-1", false).unwrap();
        assert!(!db.forwarder_should_connect("fwd-1").unwrap());
        db.set_forwarder_intent("fwd-1", true).unwrap();
        assert!(db.forwarder_should_connect("fwd-1").unwrap());

        // Independence: setting one endpoint does not affect another.
        db.set_forwarder_intent("fwd-1", false).unwrap();
        db.set_forwarder_intent("fwd-2", true).unwrap();
        assert!(!db.forwarder_should_connect("fwd-1").unwrap());
        assert!(db.forwarder_should_connect("fwd-2").unwrap());
        let intents = db.load_forwarder_intents().unwrap();
        assert_eq!(intents.get("fwd-1"), Some(&false));
        assert_eq!(intents.get("fwd-2"), Some(&true));
    }

    #[test]
    fn receiver_mode_round_trip() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-and-download", None)
            .unwrap();
        let mode = ReceiverMode::Live {
            streams: vec![],
            earliest_epochs: vec![],
        };
        db.save_receiver_mode(&mode).unwrap();

        let loaded = db.load_receiver_mode().unwrap().unwrap();
        assert_eq!(loaded, mode);
    }

    #[test]
    fn targeted_replay_mode_round_trips_with_targets() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-and-download", None)
            .unwrap();
        let targeted = ReceiverMode::TargetedReplay {
            targets: vec![rt_domain::ReplayTarget {
                forwarder_id: "f1".to_owned(),
                reader_ip: "10.0.0.1".to_owned(),
                stream_epoch: 3,
                from_seq: 1,
            }],
        };

        db.save_receiver_mode(&targeted).unwrap();
        assert_eq!(db.load_receiver_mode().unwrap().unwrap(), targeted);
    }

    #[test]
    fn save_profile_tolerates_invalid_stored_receiver_mode_json() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-and-download", None)
            .unwrap();
        db.conn
            .execute(
                "UPDATE profile SET receiver_mode_json = ?1",
                rusqlite::params!["{invalid-json"],
            )
            .unwrap();

        let result = db.save_profile("https://example.org", "tok-2", "check-only", None);
        assert!(
            result.is_ok(),
            "profile updates should not fail due to malformed stored receiver_mode_json: {result:?}"
        );
    }

    #[test]
    fn earliest_epoch_round_trip() {
        let db = Db::open_in_memory().unwrap();
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
        assert_eq!(
            db.load_earliest_epochs().unwrap(),
            vec![("f1".to_owned(), "10.0.0.1".to_owned(), 7)]
        );

        db.delete_earliest_epoch("f1", "10.0.0.1").unwrap();
        assert!(db.load_earliest_epochs().unwrap().is_empty());
    }

    #[test]
    fn save_stream_earliest_epoch_does_not_fabricate_legacy_reader_ip() {
        let db = Db::open_in_memory().unwrap();
        db.save_stream_earliest_epoch("endpoint-1", "22222222-2222-2222-2222-222222222222", 7)
            .unwrap();

        // Canonical view round-trips by stream_id.
        assert_eq!(
            db.load_stream_earliest_epochs().unwrap(),
            vec![StreamEarliestEpoch {
                stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                earliest_epoch: 7,
            }]
        );

        // The raw row must NOT store stream_id in reader_ip or
        // forwarder_endpoint_id in forwarder_id.
        let (stream_id, fwd_endpoint, forwarder_id, reader_ip): (
            String,
            String,
            Option<String>,
            Option<String>,
        ) = db
            .conn
            .query_row(
                "SELECT stream_id, forwarder_endpoint_id, forwarder_id, reader_ip FROM earliest_epochs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(stream_id, "22222222-2222-2222-2222-222222222222");
        assert_eq!(fwd_endpoint, "endpoint-1");
        assert_eq!(forwarder_id, None);
        assert_eq!(reader_ip, None);

        // Legacy view must skip canonical-only rows (no fabricated keys).
        assert!(db.load_earliest_epochs().unwrap().is_empty());
    }

    #[test]
    fn delete_stream_earliest_epoch_removes_by_stream_id() {
        let db = Db::open_in_memory().unwrap();
        db.save_stream_earliest_epoch("endpoint-1", "22222222-2222-2222-2222-222222222222", 7)
            .unwrap();
        db.save_stream_earliest_epoch("endpoint-2", "33333333-3333-3333-3333-333333333333", 9)
            .unwrap();

        db.delete_stream_earliest_epoch("22222222-2222-2222-2222-222222222222")
            .unwrap();

        assert_eq!(
            db.load_stream_earliest_epochs().unwrap(),
            vec![StreamEarliestEpoch {
                stream_id: "33333333-3333-3333-3333-333333333333".to_owned(),
                forwarder_endpoint_id: "endpoint-2".to_owned(),
                earliest_epoch: 9,
            }]
        );
    }

    #[test]
    fn load_subscriptions_excludes_canonical_only_rows() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[
            // Canonical-only row: must NOT appear in the legacy view.
            StreamSubscription {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
                local_port_override: Some(9000),
                event_type: EventType::Start,
                forwarder_id: None,
                reader_ip: None,
            },
            // Row carrying real legacy metadata: must appear with those values.
            StreamSubscription {
                forwarder_endpoint_id: "endpoint-2".to_owned(),
                stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
                local_port_override: Some(9100),
                event_type: EventType::Finish,
                forwarder_id: Some("legacy-fwd".to_owned()),
                reader_ip: Some("10.0.0.1:10000".to_owned()),
            },
        ])
        .unwrap();

        let legacy = db.load_subscriptions().unwrap();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].forwarder_id, "legacy-fwd");
        assert_eq!(legacy[0].reader_ip, "10.0.0.1:10000");
        assert_eq!(legacy[0].local_port_override, Some(9100));
        assert_eq!(legacy[0].event_type, EventType::Finish);
        // The canonical-only stream_id must never be surfaced as a reader_ip.
        assert!(
            !legacy
                .iter()
                .any(|s| s.reader_ip == "11111111-1111-1111-1111-111111111111")
        );
    }

    #[test]
    fn delete_cursor_removes_only_matching_stream() {
        let db = Db::open_in_memory().unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 7, 42).unwrap();
        db.save_cursor("f2", "10.0.0.2:10000", 3, 9).unwrap();

        db.delete_cursor("f1", "10.0.0.1:10000").unwrap();

        let rows = db.load_cursors().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].forwarder_id, "f2");
        assert_eq!(rows[0].reader_ip, "10.0.0.2:10000");
    }

    #[test]
    fn save_receiver_id_on_empty_db_creates_minimal_profile() {
        let db = Db::open_in_memory().unwrap();
        db.save_receiver_id("recv-test1234").unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, Some("recv-test1234".to_owned()));
        assert_eq!(p.server_url, "");
        assert_eq!(p.token, "");
        assert_eq!(p.update_mode, "check-and-download");
    }

    #[test]
    fn save_receiver_id_on_existing_profile_updates_only_receiver_id() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-only", Some("recv-old"))
            .unwrap();
        db.save_receiver_id("recv-new").unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, Some("recv-new".to_owned()));
        assert_eq!(p.server_url, "https://example.com");
        assert_eq!(p.token, "tok");
        assert_eq!(p.update_mode, "check-only");
    }

    #[test]
    fn save_profile_round_trips_receiver_id() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile(
            "https://thin.test",
            "t",
            "check-and-download",
            Some("recv-roundtrip"),
        )
        .unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, Some("recv-roundtrip".to_owned()));
    }

    #[test]
    fn save_profile_with_none_receiver_id_stores_null() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://thin.test", "t", "check-and-download", None)
            .unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, None);
    }

    #[test]
    fn device_token_load_set_clear_roundtrip() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://thin.test", "voucher", "check-and-download", None)
            .unwrap();
        // Unset by default.
        assert_eq!(db.load_device_token().unwrap(), None);
        // Persisted token round-trips.
        db.set_device_token("rtk_id_secret").unwrap();
        assert_eq!(
            db.load_device_token().unwrap().as_deref(),
            Some("rtk_id_secret")
        );
        // Blank is treated as unset.
        db.set_device_token("   ").unwrap();
        assert_eq!(db.load_device_token().unwrap(), None);
        // Clear removes it.
        db.set_device_token("rtk_id_secret").unwrap();
        db.clear_device_token().unwrap();
        assert_eq!(db.load_device_token().unwrap(), None);
    }

    #[test]
    fn delete_all_cursors_removes_every_row() {
        let db = Db::open_in_memory().unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 7, 42).unwrap();
        db.save_cursor("f2", "10.0.0.2:10000", 3, 9).unwrap();
        let count = db.delete_all_cursors().unwrap();
        assert_eq!(count, 2);
        assert!(db.load_cursors().unwrap().is_empty());
    }

    #[test]
    fn delete_all_cursors_on_empty_table_returns_zero() {
        let db = Db::open_in_memory().unwrap();
        let count = db.delete_all_cursors().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_all_earliest_epochs_removes_every_row() {
        let db = Db::open_in_memory().unwrap();
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
        db.save_earliest_epoch("f2", "10.0.0.2", 3).unwrap();
        let count = db.delete_all_earliest_epochs().unwrap();
        assert_eq!(count, 2);
        assert!(db.load_earliest_epochs().unwrap().is_empty());
    }

    #[test]
    fn delete_all_earliest_epochs_on_empty_table_returns_zero() {
        let db = Db::open_in_memory().unwrap();
        let count = db.delete_all_earliest_epochs().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_all_subscriptions_removes_every_row() {
        let db = Db::open_in_memory().unwrap();
        db.save_subscription("f1", "10.0.0.1", None, None).unwrap();
        db.save_subscription("f2", "10.0.0.2", Some(9000), None)
            .unwrap();
        let count = db.delete_all_subscriptions().unwrap();
        assert_eq!(count, 2);
        assert!(db.load_subscriptions().unwrap().is_empty());
    }

    #[test]
    fn reset_profile_clears_to_defaults() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile(
            "https://example.com",
            "secret-tok",
            "check-only",
            Some("recv-1"),
        )
        .unwrap();
        db.reset_profile().unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.server_url, "");
        assert_eq!(p.token, "");
        assert_eq!(p.update_mode, "check-and-download");
        assert_eq!(p.receiver_id, None);
    }

    #[test]
    fn reset_profile_on_empty_db_is_ok() {
        let db = Db::open_in_memory().unwrap();
        db.reset_profile().unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.server_url, "");
        assert_eq!(p.token, "");
    }

    #[test]
    fn factory_reset_clears_all_tables() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-only", Some("recv-1"))
            .unwrap();
        db.save_subscription("f1", "10.0.0.1", None, None).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 7, 42).unwrap();
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
        db.set_forwarder_intent("f1", false).unwrap();
        db.set_announcer_enabled(true).unwrap();
        db.set_stream_announcer_publish("10.0.0.1:10000", true)
            .unwrap();
        db.replace_participants(&[crate::participants::Participant {
            bib: 1,
            last: "Last".to_owned(),
            first: "First".to_owned(),
            affiliation: String::new(),
            gender: "X".to_owned(),
            division: None,
        }])
        .unwrap();
        db.replace_bib_chips(&[(1, "0a1b".to_owned())]).unwrap();
        db.factory_reset().unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.server_url, "");
        assert_eq!(p.token, "");
        assert_eq!(p.receiver_id, None);
        assert!(db.load_subscriptions().unwrap().is_empty());
        assert!(db.load_cursors().unwrap().is_empty());
        assert!(db.load_earliest_epochs().unwrap().is_empty());
        // forwarder_intent is cleared, so the default-true contract is restored.
        assert!(db.load_forwarder_intents().unwrap().is_empty());
        assert!(db.forwarder_should_connect("f1").unwrap());
        // New tables and the global toggle must also be wiped.
        assert!(!db.load_announcer_enabled().unwrap());
        assert!(db.load_announcer_publish_streams().unwrap().is_empty());
        assert!(db.load_chip_to_participant().unwrap().is_empty());
    }

    #[test]
    fn reset_announcer_fences_allows_lower_generation_after_server_change() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "11111111-1111-1111-1111-111111111111";
        // Fence advances to 10 against the original server.
        db.accept_announcer_generation(stream_id, 10).unwrap();
        // A replacement server starting at a lower generation would be fenced.
        assert!(matches!(
            db.accept_announcer_generation(stream_id, 3).unwrap(),
            AnnouncerGenerationAcceptance::Stale { .. }
        ));
        // Resetting the fence (done on a server change) accepts it fresh.
        db.reset_announcer_fences().unwrap();
        assert_eq!(db.load_announcer_fence(stream_id).unwrap(), None);
        assert_eq!(
            db.accept_announcer_generation(stream_id, 3).unwrap(),
            AnnouncerGenerationAcceptance::Current { generation: 3 }
        );
    }

    #[test]
    fn clear_data_preserves_profile_connection_fields() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("https://example.com", "tok", "check-only", Some("recv-1"))
            .unwrap();
        db.save_dbf_config(&DbfConfig { enabled: true }).unwrap();
        db.save_subscription("f1", "10.0.0.1", None, None).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 7, 42).unwrap();
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
        db.clear_data().unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.server_url, "https://example.com");
        assert_eq!(p.token, "tok");
        assert_eq!(p.receiver_id, Some("recv-1".to_owned()));
        // Non-profile fields should be reset
        let dbf = db.load_dbf_config().unwrap();
        assert!(!dbf.enabled);
        assert!(db.load_subscriptions().unwrap().is_empty());
        assert!(db.load_cursors().unwrap().is_empty());
        assert!(db.load_earliest_epochs().unwrap().is_empty());
    }

    #[test]
    fn replace_stream_subscriptions_persists_canonical_identity_without_fake_legacy_keys() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            local_port_override: Some(9000),
            event_type: EventType::Start,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();

        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].forwarder_endpoint_id, "endpoint-1");
        assert_eq!(subs[0].stream_id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(subs[0].local_port_override, Some(9000));
        assert_eq!(subs[0].event_type, EventType::Start);
        assert_eq!(subs[0].forwarder_id, None);
        assert_eq!(subs[0].reader_ip, None);

        let raw: (String, String, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT forwarder_endpoint_id, stream_id, forwarder_id, reader_ip FROM subscriptions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(raw.0, "endpoint-1");
        assert_eq!(raw.1, "11111111-1111-1111-1111-111111111111");
        assert_eq!(raw.2, None);
        assert_eq!(raw.3, None);
    }

    #[test]
    fn update_stream_subscription_port_uses_canonical_identity() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();

        let updated = db
            .update_stream_subscription_port(
                "endpoint-1",
                "11111111-1111-1111-1111-111111111111",
                Some(9100),
            )
            .unwrap();

        assert!(updated);
        assert_eq!(
            db.load_stream_subscriptions().unwrap()[0].local_port_override,
            Some(9100)
        );
    }

    #[test]
    fn update_subscription_port_changes_existing() {
        let db = Db::open_in_memory().unwrap();
        db.save_subscription("f1", "10.0.0.1", None, None).unwrap();
        let updated = db
            .update_subscription_port("f1", "10.0.0.1", Some(9000))
            .unwrap();
        assert!(updated);
        let subs = db.load_subscriptions().unwrap();
        assert_eq!(subs[0].local_port_override, Some(9000));
    }

    #[test]
    fn update_subscription_port_clears_override() {
        let db = Db::open_in_memory().unwrap();
        db.save_subscription("f1", "10.0.0.1", Some(9000), None)
            .unwrap();
        let updated = db.update_subscription_port("f1", "10.0.0.1", None).unwrap();
        assert!(updated);
        let subs = db.load_subscriptions().unwrap();
        assert_eq!(subs[0].local_port_override, None);
    }

    #[test]
    fn update_subscription_port_returns_false_for_missing() {
        let db = Db::open_in_memory().unwrap();
        let updated = db
            .update_subscription_port("f1", "10.0.0.1", Some(9000))
            .unwrap();
        assert!(!updated);
    }

    #[test]
    fn save_receiver_id_on_empty_db_does_not_create_duplicate_rows() {
        let db = Db::open_in_memory().unwrap();
        // First call on empty DB creates exactly one row.
        db.save_receiver_id("id-1").unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM profile", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "expected exactly 1 profile row after first save_receiver_id"
        );

        // Second call must update the existing row, not insert another.
        db.save_receiver_id("id-2").unwrap();
        let count2: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM profile", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count2, 1,
            "expected still exactly 1 profile row after second save_receiver_id"
        );

        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, Some("id-2".to_owned()));
    }

    #[test]
    fn save_cursor_rejects_same_epoch_lower_seq() {
        let db = Db::open_in_memory().unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 5, 10).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 5, 5).unwrap();
        let rows = db.load_cursors().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].stream_epoch, 5);
        assert_eq!(rows[0].last_seq, 10, "cursor must not regress to lower seq");
    }

    #[test]
    fn save_cursor_rejects_lower_epoch() {
        let db = Db::open_in_memory().unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 5, 10).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 4, 100).unwrap();
        let rows = db.load_cursors().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].stream_epoch, 5,
            "cursor must not regress to lower epoch"
        );
        assert_eq!(rows[0].last_seq, 10);
    }

    #[test]
    fn save_cursor_accepts_same_epoch_higher_seq() {
        let db = Db::open_in_memory().unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 5, 10).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 5, 15).unwrap();
        let rows = db.load_cursors().unwrap();
        assert_eq!(rows[0].stream_epoch, 5);
        assert_eq!(rows[0].last_seq, 15, "cursor must advance to higher seq");
    }

    #[test]
    fn save_cursor_accepts_higher_epoch() {
        let db = Db::open_in_memory().unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 5, 10).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 6, 1).unwrap();
        let rows = db.load_cursors().unwrap();
        assert_eq!(
            rows[0].stream_epoch, 6,
            "cursor must advance to higher epoch"
        );
        assert_eq!(rows[0].last_seq, 1);
    }

    #[test]
    fn announcer_generation_acceptance_reports_current_and_stale() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "11111111-1111-1111-1111-111111111111";

        assert_eq!(
            db.accept_announcer_generation(stream_id, 3).unwrap(),
            AnnouncerGenerationAcceptance::Current { generation: 3 }
        );
        assert_eq!(
            db.accept_announcer_generation(stream_id, 5).unwrap(),
            AnnouncerGenerationAcceptance::Current { generation: 5 }
        );
        assert_eq!(
            db.accept_announcer_generation(stream_id, 5).unwrap(),
            AnnouncerGenerationAcceptance::Current { generation: 5 }
        );
        assert_eq!(
            db.accept_announcer_generation(stream_id, 4).unwrap(),
            AnnouncerGenerationAcceptance::Stale {
                current: 5,
                attempted: 4
            }
        );
        assert_eq!(db.load_announcer_fence(stream_id).unwrap(), Some(5));
    }

    #[test]
    fn rd_import_config_defaults_to_race_director_files_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(dir.path().join("test.db").as_path()).unwrap();
        db.save_profile("https://example.com", "tok", "check-and-download", None)
            .unwrap();

        let config = db.load_rd_import_config().unwrap();
        assert!(!config.enabled);
        assert_eq!(config.dir, r"C:\Winrace\Files");
        assert_eq!(config.interval_secs, DEFAULT_RD_IMPORT_INTERVAL_SECS);
    }

    #[test]
    fn dbf_config_defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(dir.path().join("test.db").as_path()).unwrap();
        db.save_profile("https://example.com", "tok", "check-and-download", None)
            .unwrap();
        let config = db.load_dbf_config().unwrap();
        assert!(!config.enabled);
        db.save_dbf_config(&DbfConfig { enabled: true }).unwrap();
        let config = db.load_dbf_config().unwrap();
        assert!(config.enabled);
        db.save_profile("https://new.com", "tok2", "check-and-download", None)
            .unwrap();
        let config = db.load_dbf_config().unwrap();
        assert!(config.enabled);
    }

    #[test]
    fn subscription_event_type_defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(dir.path().join("test.db").as_path()).unwrap();
        db.save_subscription("fwd1", "10.0.0.1", None, None)
            .unwrap();
        let subs = db.load_subscriptions().unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].event_type, EventType::Finish);
        db.replace_subscriptions(&[Subscription {
            forwarder_id: "fwd1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
            local_port_override: None,
            event_type: EventType::Start,
        }])
        .unwrap();
        let subs = db.load_subscriptions().unwrap();
        assert_eq!(subs[0].event_type, EventType::Start);
    }

    #[test]
    fn migrates_legacy_cursors_to_stream_id_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE cursors (
                    forwarder_id TEXT NOT NULL,
                    reader_ip TEXT NOT NULL,
                    stream_epoch BIGINT NOT NULL,
                    acked_through_seq BIGINT NOT NULL,
                    PRIMARY KEY (forwarder_id, reader_ip)
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["fwd-1", "10.0.0.1:10000", 7i64, 42i64],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();

        let columns = table_info(&db.conn, "cursors");
        assert_eq!(columns.get("stream_id"), Some(&(1, 0)));
        assert_eq!(columns.get("last_seq"), Some(&(0, 1)));
        assert_eq!(columns.get("forwarder_id"), Some(&(0, 0)));
        assert_eq!(columns.get("reader_ip"), Some(&(0, 0)));

        let rows = db.load_cursors().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].forwarder_id, "fwd-1");
        assert_eq!(rows[0].reader_ip, "10.0.0.1:10000");
        assert_eq!(rows[0].stream_epoch, 7);
        assert_eq!(rows[0].last_seq, 42);

        let stream_id = "44444444-4444-4444-4444-444444444444";
        assert_eq!(db.advance_cursor_contiguous_prefix(stream_id).unwrap(), 0);
    }

    #[test]
    fn migrates_legacy_subscriptions_to_endpoint_stream_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE subscriptions (
                    forwarder_id TEXT NOT NULL,
                    reader_ip TEXT NOT NULL,
                    local_port_override INTEGER,
                    event_type TEXT NOT NULL DEFAULT 'finish',
                    PRIMARY KEY (forwarder_id, reader_ip)
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO subscriptions (forwarder_id, reader_ip, local_port_override, event_type)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["fwd-1", "10.0.0.1:10000", 10001i64, "start"],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();

        let columns = table_info(&db.conn, "subscriptions");
        assert_eq!(columns.get("forwarder_endpoint_id"), Some(&(1, 1)));
        assert_eq!(columns.get("stream_id"), Some(&(2, 1)));
        assert_eq!(columns.get("forwarder_id"), Some(&(0, 0)));
        assert_eq!(columns.get("reader_ip"), Some(&(0, 0)));

        let subs = db.load_subscriptions().unwrap();
        assert_eq!(
            subs,
            vec![Subscription {
                forwarder_id: "fwd-1".to_owned(),
                reader_ip: "10.0.0.1:10000".to_owned(),
                local_port_override: Some(10001),
                event_type: EventType::Start,
            }]
        );

        // A canonical-only row (no legacy forwarder_id/reader_ip metadata) must
        // NOT surface in the legacy (forwarder_id, reader_ip) view: the legacy
        // loader filters it out rather than fabricating legacy keys from
        // forwarder_endpoint_id/stream_id.
        db.conn
            .execute(
                "INSERT INTO subscriptions (forwarder_endpoint_id, stream_id, event_type)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    "endpoint-2",
                    "22222222-2222-2222-2222-222222222222",
                    "finish"
                ],
            )
            .unwrap();
        assert_eq!(db.load_subscriptions().unwrap().len(), 1);
        // It is, however, visible through the canonical loader.
        assert_eq!(db.load_stream_subscriptions().unwrap().len(), 2);
    }

    #[test]
    fn migrates_legacy_earliest_epochs_to_stream_id_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE earliest_epochs (
                    forwarder_id TEXT NOT NULL,
                    reader_ip TEXT NOT NULL,
                    earliest_epoch BIGINT NOT NULL,
                    PRIMARY KEY (forwarder_id, reader_ip)
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO earliest_epochs (forwarder_id, reader_ip, earliest_epoch)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params!["fwd-1", "10.0.0.1:10000", 7i64],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();

        // Canonical shape is in place after migration.
        let columns = table_info(&db.conn, "earliest_epochs");
        assert_eq!(columns.get("stream_id"), Some(&(1, 0)));
        assert_eq!(columns.get("forwarder_endpoint_id"), Some(&(0, 1)));
        assert_eq!(columns.get("forwarder_id"), Some(&(0, 0)));
        assert_eq!(columns.get("reader_ip"), Some(&(0, 0)));

        // Legacy view still returns the real legacy metadata row.
        assert_eq!(
            db.load_earliest_epochs().unwrap(),
            vec![("fwd-1".to_owned(), "10.0.0.1:10000".to_owned(), 7)]
        );

        // Canonical view returns a synthetic stream_id + forwarder_endpoint_id
        // without storing stream_id in reader_ip.
        assert_eq!(
            db.load_stream_earliest_epochs().unwrap(),
            vec![StreamEarliestEpoch {
                stream_id: "legacy:fwd-1\u{1f}10.0.0.1:10000".to_owned(),
                forwarder_endpoint_id: "fwd-1".to_owned(),
                earliest_epoch: 7,
            }]
        );

        // Canonical saves work afterwards and stay canonical-only.
        db.save_stream_earliest_epoch("endpoint-2", "22222222-2222-2222-2222-222222222222", 11)
            .unwrap();
        let mut stream_rows = db.load_stream_earliest_epochs().unwrap();
        stream_rows.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
        assert_eq!(
            stream_rows,
            vec![
                StreamEarliestEpoch {
                    stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
                    forwarder_endpoint_id: "endpoint-2".to_owned(),
                    earliest_epoch: 11,
                },
                StreamEarliestEpoch {
                    stream_id: "legacy:fwd-1\u{1f}10.0.0.1:10000".to_owned(),
                    forwarder_endpoint_id: "fwd-1".to_owned(),
                    earliest_epoch: 7,
                },
            ]
        );
        // The canonical row must not surface in the legacy view.
        assert_eq!(
            db.load_earliest_epochs().unwrap(),
            vec![("fwd-1".to_owned(), "10.0.0.1:10000".to_owned(), 7)]
        );
    }

    fn table_info(conn: &Connection, table: &str) -> std::collections::HashMap<String, (i64, i64)> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    (row.get::<_, i64>(5)?, row.get::<_, i64>(3)?),
                ))
            })
            .unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    }

    #[test]
    fn load_subscription_dbf_details_returns_latest_index_and_event_type() {
        let db = Db::open_in_memory().unwrap();
        db.save_subscription("fwd2", "10.0.0.2", None, Some(EventType::Finish))
            .unwrap();
        db.save_subscription("fwd1", "10.0.0.1", None, Some(EventType::Start))
            .unwrap();

        let details = db
            .load_subscription_dbf_details("fwd2", "10.0.0.2")
            .unwrap()
            .unwrap();
        assert_eq!(details, (1, EventType::Finish));
    }

    #[test]
    fn insert_on_conflict_do_nothing() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "11111111-1111-1111-1111-111111111111";
        let event = ReceivedEventInsert {
            stream_id,
            seq: 7,
            epoch: 3,
            raw_frame: b"frame-one",
            read_kind: "chip",
            reader_timestamp: Some("12:34:56.789"),
            received_unix_ms: 1_700_000_000_123,
            dbf_delivered_unix_ms: None,
        };

        assert!(db.insert_received_event(&event).unwrap());

        let duplicate = ReceivedEventInsert {
            raw_frame: b"different-frame",
            received_unix_ms: 1_700_000_000_999,
            ..event
        };
        assert!(!db.insert_received_event(&duplicate).unwrap());

        let rows = db.load_received_events(stream_id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_frame, b"frame-one");
        assert_eq!(rows[0].received_unix_ms, 1_700_000_000_123);
    }

    #[test]
    fn load_replay_target_epochs_uses_local_received_events() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_id = "stream-a";
        // Canonical live-mode subscription: legacy (forwarder_id, reader_ip)
        // columns are NULL, exactly as the P2P data plane persists them.
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: stream_id.to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();

        for (seq, epoch, timestamp) in [
            (1, 1, "2026-02-01T10:00:00Z"),
            (2, 2, "2026-02-01T11:00:00Z"),
            (3, 2, "2026-02-01T11:05:00Z"),
        ] {
            db.insert_received_event(&ReceivedEventInsert {
                stream_id,
                seq,
                epoch,
                raw_frame: b"frame",
                read_kind: "chip",
                reader_timestamp: Some(timestamp),
                received_unix_ms: 1_700_000_000_000 + seq,
                dbf_delivered_unix_ms: None,
            })
            .unwrap();
        }

        let epochs = db.load_replay_target_epochs(stream_id).unwrap();
        assert_eq!(
            epochs,
            vec![
                (2, Some("2026-02-01T11:00:00Z".to_owned())),
                (1, Some("2026-02-01T10:00:00Z".to_owned())),
            ]
        );
        assert!(
            db.load_replay_target_epochs("other-stream")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cursor_advances_contiguous_prefix() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "22222222-2222-2222-2222-222222222222";

        for seq in [1, 2, 4] {
            let event = ReceivedEventInsert {
                stream_id,
                seq,
                epoch: 9,
                raw_frame: b"frame",
                read_kind: "chip",
                reader_timestamp: None,
                received_unix_ms: 1_700_000_000_000 + seq,
                dbf_delivered_unix_ms: None,
            };
            assert!(db.insert_received_event(&event).unwrap());
        }

        assert_eq!(db.advance_cursor_contiguous_prefix(stream_id).unwrap(), 2);

        let event = ReceivedEventInsert {
            stream_id,
            seq: 3,
            epoch: 9,
            raw_frame: b"frame",
            read_kind: "chip",
            reader_timestamp: None,
            received_unix_ms: 1_700_000_000_003,
            dbf_delivered_unix_ms: None,
        };
        assert!(db.insert_received_event(&event).unwrap());

        assert_eq!(db.advance_cursor_contiguous_prefix(stream_id).unwrap(), 4);
    }

    #[test]
    fn load_stream_cursor_defaults_to_zero_then_reflects_advance() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "55555555-5555-5555-5555-555555555555";
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 0);
        for seq in [1, 2, 3] {
            db.insert_received_event(&ReceivedEventInsert {
                stream_id,
                seq,
                epoch: 1,
                raw_frame: b"frame",
                read_kind: "chip",
                reader_timestamp: None,
                received_unix_ms: seq,
                dbf_delivered_unix_ms: None,
            })
            .unwrap();
        }
        assert_eq!(db.advance_cursor_contiguous_prefix(stream_id).unwrap(), 3);
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 3);
    }

    #[test]
    fn jump_stream_cursor_moves_forward_only() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "66666666-6666-6666-6666-666666666666";
        db.jump_stream_cursor(stream_id, 14).unwrap();
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 14);
        // A regressing jump target is ignored.
        db.jump_stream_cursor(stream_id, 5).unwrap();
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 14);
        // A forward jump advances the cursor.
        db.jump_stream_cursor(stream_id, 20).unwrap();
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 20);
    }

    #[test]
    fn non_uuid_stream_id_persists_and_advances_cursor() {
        // A real forwarder P2P stream_id is an arbitrary UTF-8 journal key such
        // as `ip:port`, not a parseable UUID. It must persist, dedup, and
        // advance the contiguous cursor like any other stream_id.
        let db = Db::open_in_memory().unwrap();
        let stream_id = "127.0.0.1:10000";
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 0);
        for seq in [1, 2, 3] {
            assert!(
                db.insert_received_event(&ReceivedEventInsert {
                    stream_id,
                    seq,
                    epoch: 1,
                    raw_frame: b"frame",
                    read_kind: "chip",
                    reader_timestamp: None,
                    received_unix_ms: 1_700_000_000_000 + seq,
                    dbf_delivered_unix_ms: None,
                })
                .unwrap()
            );
        }
        let rows = db.load_received_events(stream_id).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].stream_id, stream_id);
        assert_eq!(db.advance_cursor_contiguous_prefix(stream_id).unwrap(), 3);
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 3);

        // Exactly-16-byte id (the legacy UUID byte length) must also work as a
        // plain string, proving we never reinterpret it as raw UUID bytes.
        let sixteen = "100.64.0.1:10000";
        assert_eq!(sixteen.len(), 16);
        assert!(
            db.insert_received_event(&ReceivedEventInsert {
                stream_id: sixteen,
                seq: 1,
                epoch: 1,
                raw_frame: b"frame",
                read_kind: "chip",
                reader_timestamp: None,
                received_unix_ms: 1,
                dbf_delivered_unix_ms: None,
            })
            .unwrap()
        );
        assert_eq!(db.advance_cursor_contiguous_prefix(sixteen).unwrap(), 1);
        db.accept_announcer_generation(sixteen, 1).unwrap();
        assert_eq!(db.load_announcer_fence(sixteen).unwrap(), Some(1));
    }

    #[test]
    fn gap_marker_persists() {
        let db = Db::open_in_memory().unwrap();
        let stream_id = "33333333-3333-3333-3333-333333333333";
        let marker = GapMarkerInsert {
            stream_id,
            requested_after_seq: 10,
            earliest_available_seq: 15,
            latest_available_seq: 20,
            reason: "retention-window",
            created_unix_ms: 1_700_000_001_000,
        };

        db.save_gap_marker(&marker).unwrap();

        let rows = db.load_gap_markers(stream_id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].stream_id, stream_id);
        assert_eq!(rows[0].requested_after_seq, 10);
        // `stream_id` here is `&str`; `rows[0].stream_id` is `String`.
        assert_eq!(rows[0].earliest_available_seq, 15);
        assert_eq!(rows[0].latest_available_seq, 20);
        assert_eq!(rows[0].reason, "retention-window");
        assert_eq!(rows[0].created_unix_ms, 1_700_000_001_000);
    }
}

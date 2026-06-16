//! SQLite-backed device registry for the thin node.
//!
//! Tracks forwarder/receiver endpoints, their approval state, hashed
//! per-device bearer tokens, and a backup of the forwarder stream catalog.
//! New devices self-register under a TOFU (trust-on-first-use) model: the
//! first valid registration creates a `pending` record, and an admin later
//! approves and names it to mark it `active`.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::num::TryFromIntError;

/// Kind of device that can register with the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Forwarder,
    Receiver,
}

impl DeviceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceKind::Forwarder => "forwarder",
            DeviceKind::Receiver => "receiver",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "forwarder" => Some(DeviceKind::Forwarder),
            "receiver" => Some(DeviceKind::Receiver),
            _ => None,
        }
    }
}

/// Approval state of a registered device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalState {
    Pending,
    Active,
}

impl ApprovalState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalState::Pending => "pending",
            ApprovalState::Active => "active",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(ApprovalState::Pending),
            "active" => Some(ApprovalState::Active),
            _ => None,
        }
    }
}

/// A registered device record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceRecord {
    pub endpoint_id: String,
    pub device_kind: DeviceKind,
    pub display_name: Option<String>,
    pub approval_state: ApprovalState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncerRowRecord {
    pub announcer_source_generation: u64,
    pub stream_id: String,
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_unix_ms: i64,
}

#[derive(Debug)]
pub enum AnnouncerStorageError {
    Sqlite(rusqlite::Error),
    StaleGeneration { current_generation: u64 },
    ValueOutOfRange(&'static str),
}

impl From<rusqlite::Error> for AnnouncerStorageError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

/// Hash a raw bearer token for storage/comparison. Tokens are never persisted
/// in plaintext.
#[must_use]
pub fn hash_token(raw_token: &str) -> Vec<u8> {
    Sha256::digest(raw_token.as_bytes()).to_vec()
}

/// Create the registry tables. Idempotent and safe to call on every open.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS devices (
             endpoint_id TEXT PRIMARY KEY,
             device_kind TEXT NOT NULL,
             display_name TEXT,
             approval_state TEXT NOT NULL,
             token_hash BLOB NOT NULL,
             created_unix_ms INTEGER NOT NULL,
             updated_unix_ms INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS forwarder_streams (
             stream_id TEXT PRIMARY KEY,
             endpoint_id TEXT NOT NULL,
             epoch INTEGER NOT NULL,
             next_seq INTEGER NOT NULL,
             FOREIGN KEY(endpoint_id) REFERENCES devices(endpoint_id)
         );",
    )
}

/// Register (or re-register) a device under the TOFU model.
///
/// A brand-new endpoint is recorded as `pending`. Re-registration of an
/// existing endpoint refreshes its kind and hashed token while preserving any
/// existing approval state and admin-assigned display name (idempotent for an
/// already-approved device).
pub fn register_device(
    conn: &Connection,
    endpoint_id: &str,
    device_kind: DeviceKind,
    raw_token: &str,
) -> rusqlite::Result<DeviceRecord> {
    let token_hash = hash_token(raw_token);
    let now = Utc::now().timestamp_millis();

    conn.execute(
        "INSERT INTO devices (
             endpoint_id, device_kind, display_name, approval_state,
             token_hash, created_unix_ms, updated_unix_ms
         )
         VALUES (?1, ?2, NULL, 'pending', ?3, ?4, ?4)
         ON CONFLICT(endpoint_id) DO UPDATE SET
             device_kind = excluded.device_kind,
             token_hash = excluded.token_hash,
             updated_unix_ms = excluded.updated_unix_ms",
        params![endpoint_id, device_kind.as_str(), token_hash, now],
    )?;

    get_device(conn, endpoint_id)?.ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
}

/// Approve a device and assign its display name, marking it `active`.
///
/// Returns `None` if no device with the given endpoint id exists.
pub fn approve_device(
    conn: &Connection,
    endpoint_id: &str,
    display_name: &str,
) -> rusqlite::Result<Option<DeviceRecord>> {
    let now = Utc::now().timestamp_millis();
    let changed = conn.execute(
        "UPDATE devices
         SET approval_state = 'active', display_name = ?2, updated_unix_ms = ?3
         WHERE endpoint_id = ?1",
        params![endpoint_id, display_name, now],
    )?;

    if changed == 0 {
        return Ok(None);
    }

    get_device(conn, endpoint_id)
}

/// Fetch a device record by endpoint id.
pub fn get_device(conn: &Connection, endpoint_id: &str) -> rusqlite::Result<Option<DeviceRecord>> {
    conn.query_row(
        "SELECT endpoint_id, device_kind, display_name, approval_state
         FROM devices
         WHERE endpoint_id = ?1",
        [endpoint_id],
        |row| {
            let kind_str: String = row.get(1)?;
            let approval_str: String = row.get(3)?;
            let device_kind = DeviceKind::parse(&kind_str).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    format!("invalid device_kind: {kind_str}").into(),
                )
            })?;
            let approval_state = ApprovalState::parse(&approval_str).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    format!("invalid approval_state: {approval_str}").into(),
                )
            })?;
            Ok(DeviceRecord {
                endpoint_id: row.get(0)?,
                device_kind,
                display_name: row.get(2)?,
                approval_state,
            })
        },
    )
    .optional()
}

pub fn current_announcer_source_generation(
    conn: &Connection,
) -> Result<u64, AnnouncerStorageError> {
    let generation = conn.query_row(
        "SELECT generation FROM announcer_source_state WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    u64::try_from(generation).map_err(|_| AnnouncerStorageError::ValueOutOfRange("generation"))
}

pub fn takeover_announcer_source(conn: &Connection) -> Result<u64, AnnouncerStorageError> {
    conn.execute(
        "UPDATE announcer_source_state SET generation = generation + 1 WHERE id = 1",
        [],
    )?;
    current_announcer_source_generation(conn)
}

pub fn upsert_announcer_row(
    conn: &Connection,
    row: &AnnouncerRowRecord,
) -> Result<(), AnnouncerStorageError> {
    let current_generation = current_announcer_source_generation(conn)?;
    if row.announcer_source_generation < current_generation {
        return Err(AnnouncerStorageError::StaleGeneration { current_generation });
    }

    conn.execute(
        "INSERT INTO announcer_rows (
             stream_id, seq, source_generation, chip_id, bib, display_name,
             reader_timestamp, received_unix_ms
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(stream_id, seq) DO UPDATE SET
             source_generation = excluded.source_generation,
             chip_id = excluded.chip_id,
             bib = excluded.bib,
             display_name = excluded.display_name,
             reader_timestamp = excluded.reader_timestamp,
             received_unix_ms = excluded.received_unix_ms",
        params![
            &row.stream_id,
            u64_to_i64(row.seq, "seq")?,
            u64_to_i64(
                row.announcer_source_generation,
                "announcer_source_generation"
            )?,
            &row.chip_id,
            row.bib,
            &row.display_name,
            &row.reader_timestamp,
            row.received_unix_ms,
        ],
    )?;
    Ok(())
}

pub fn list_announcer_rows_ordered(
    conn: &Connection,
) -> Result<Vec<AnnouncerRowRecord>, AnnouncerStorageError> {
    let mut stmt = conn.prepare(
        "SELECT source_generation, stream_id, seq, chip_id, bib, display_name,
                reader_timestamp, received_unix_ms
         FROM announcer_rows
         ORDER BY received_unix_ms, stream_id, seq",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AnnouncerRowRecord {
            announcer_source_generation: sql_i64_to_u64(row.get(0)?, 0)?,
            stream_id: row.get(1)?,
            seq: sql_i64_to_u64(row.get(2)?, 2)?,
            chip_id: row.get(3)?,
            bib: row.get(4)?,
            display_name: row.get(5)?,
            reader_timestamp: row.get(6)?,
            received_unix_ms: row.get(7)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AnnouncerStorageError::Sqlite)
}

fn u64_to_i64(value: u64, field: &'static str) -> Result<i64, AnnouncerStorageError> {
    i64::try_from(value).map_err(|_| AnnouncerStorageError::ValueOutOfRange(field))
}

fn sql_i64_to_u64(value: i64, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|err: TryFromIntError| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(err),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn register_then_approve_roundtrip() {
        let conn = test_conn();

        let pending = register_device(&conn, "ep-1", DeviceKind::Forwarder, "tok").unwrap();
        assert_eq!(pending.approval_state, ApprovalState::Pending);
        assert_eq!(pending.device_kind, DeviceKind::Forwarder);
        assert!(pending.display_name.is_none());

        let active = approve_device(&conn, "ep-1", "Start Line")
            .unwrap()
            .expect("device exists");
        assert_eq!(active.approval_state, ApprovalState::Active);
        assert_eq!(active.display_name.as_deref(), Some("Start Line"));
    }

    #[test]
    fn reregistration_preserves_approval_and_name() {
        let conn = test_conn();

        register_device(&conn, "ep-2", DeviceKind::Receiver, "tok-a").unwrap();
        approve_device(&conn, "ep-2", "Finish").unwrap();

        let reregistered = register_device(&conn, "ep-2", DeviceKind::Receiver, "tok-b").unwrap();
        assert_eq!(reregistered.approval_state, ApprovalState::Active);
        assert_eq!(reregistered.display_name.as_deref(), Some("Finish"));
    }

    #[test]
    fn approve_missing_device_returns_none() {
        let conn = test_conn();
        assert!(approve_device(&conn, "missing", "name").unwrap().is_none());
    }

    #[test]
    fn tokens_are_hashed_not_plaintext() {
        let conn = test_conn();
        register_device(&conn, "ep-3", DeviceKind::Forwarder, "super-secret").unwrap();

        let stored: Vec<u8> = conn
            .query_row(
                "SELECT token_hash FROM devices WHERE endpoint_id = 'ep-3'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, hash_token("super-secret"));
        assert_ne!(stored, b"super-secret".to_vec());
    }
}

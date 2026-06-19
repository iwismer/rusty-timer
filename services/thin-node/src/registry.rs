//! SQLite-backed device registry for the thin node.
//!
//! Tracks forwarder/receiver endpoints, their approval state, hashed
//! per-device bearer tokens, and a backup of the forwarder stream catalog.
//! New devices self-register under a TOFU (trust-on-first-use) model: the
//! first valid registration creates a `pending` record, and an admin later
//! approves and names it to mark it `active`.

use chrono::Utc;
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::num::TryFromIntError;
use subtle::ConstantTimeEq;

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

/// A registered forwarder's latest pushed identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForwarderRecord {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<String>,
    pub last_seen_unix_ms: i64,
    pub approval_state: ApprovalState,
}

/// A stream row from a pushed forwarder catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwarderCatalogStreamRecord {
    pub stream_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

/// A backup row from the forwarder stream catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForwarderStreamRecord {
    pub stream_id: String,
    pub endpoint_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

/// One stream entry of an approved forwarder, for receiver discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredForwarderStream {
    pub stream_id: String,
    pub epoch: u64,
    pub next_seq: u64,
}

/// An approved forwarder joined with its stream catalog, returned by
/// `GET /forwarders` so receivers can discover dialable forwarders and the
/// streams each exposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovedForwarderWithStreams {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub direct_addrs: Vec<String>,
    pub streams: Vec<DiscoveredForwarderStream>,
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

/// Length in bytes of the random per-token salt.
const TOKEN_SALT_LEN: usize = 16;

/// Hash a raw bearer token for storage/comparison. Tokens are never persisted
/// in plaintext.
///
/// Each call generates a fresh random salt and returns the UTF-8 bytes of
/// `"<salt_hex>$<sha256(salt || token)_hex>"`. Salting means identical tokens
/// produce distinct stored hashes and defeats precomputed (rainbow-table)
/// attacks against a leaked registry. Use [`verify_token`] to check a candidate
/// token against a stored hash — never compare hashes with `==`, which is not
/// constant-time.
#[must_use]
pub fn hash_token(raw_token: &str) -> Vec<u8> {
    let mut salt = [0u8; TOKEN_SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    encode_salted_hash(&salt, raw_token).into_bytes()
}

/// Verify a candidate raw token against a salted hash produced by [`hash_token`].
///
/// Returns `false` for any malformed stored hash. The final digest comparison
/// is constant-time to avoid leaking how many leading bytes matched.
#[must_use]
pub fn verify_token(raw_token: &str, stored_hash: &[u8]) -> bool {
    let Ok(stored) = std::str::from_utf8(stored_hash) else {
        return false;
    };
    let Some((salt_hex, expected_digest_hex)) = stored.split_once('$') else {
        return false;
    };
    let Some(salt) = decode_hex(salt_hex) else {
        return false;
    };
    let Some(expected_digest) = decode_hex(expected_digest_hex) else {
        return false;
    };
    let actual_digest = Sha256::digest(salted_input(&salt, raw_token));
    actual_digest.as_slice().ct_eq(&expected_digest).into()
}

fn salted_input(salt: &[u8], raw_token: &str) -> Vec<u8> {
    let mut input = Vec::with_capacity(salt.len() + raw_token.len());
    input.extend_from_slice(salt);
    input.extend_from_slice(raw_token.as_bytes());
    input
}

fn encode_salted_hash(salt: &[u8], raw_token: &str) -> String {
    let digest = Sha256::digest(salted_input(salt, raw_token));
    format!("{}${}", encode_hex(salt), encode_hex(&digest))
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
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
         CREATE TABLE IF NOT EXISTS forwarders (
             endpoint_id TEXT PRIMARY KEY,
             display_name TEXT,
             direct_addrs TEXT NOT NULL,
             last_seen_unix_ms INTEGER NOT NULL,
             FOREIGN KEY(endpoint_id) REFERENCES devices(endpoint_id)
         );
         CREATE TABLE IF NOT EXISTS forwarder_streams (
             endpoint_id TEXT NOT NULL,
             stream_id TEXT NOT NULL,
             epoch INTEGER NOT NULL,
             next_seq INTEGER NOT NULL,
             PRIMARY KEY(endpoint_id, stream_id),
             FOREIGN KEY(endpoint_id) REFERENCES devices(endpoint_id)
         );",
    )?;

    if forwarder_streams_needs_composite_pk(conn)? {
        // Wrap the table reshape in a SAVEPOINT so a failure mid-migration
        // rolls back atomically instead of leaving a half-renamed/dropped
        // table behind (mirrors the receiver's SAVEPOINT-guarded reshapes).
        conn.execute_batch(
            "SAVEPOINT reshape_forwarder_streams;
             ALTER TABLE forwarder_streams RENAME TO forwarder_streams_old;
             CREATE TABLE forwarder_streams (
                 endpoint_id TEXT NOT NULL,
                 stream_id TEXT NOT NULL,
                 epoch INTEGER NOT NULL,
                 next_seq INTEGER NOT NULL,
                 PRIMARY KEY(endpoint_id, stream_id),
                 FOREIGN KEY(endpoint_id) REFERENCES devices(endpoint_id)
             );
             INSERT OR REPLACE INTO forwarder_streams (endpoint_id, stream_id, epoch, next_seq)
             SELECT endpoint_id, stream_id, epoch, next_seq FROM forwarder_streams_old;
             DROP TABLE forwarder_streams_old;
             RELEASE reshape_forwarder_streams;",
        )?;
    }

    Ok(())
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

/// Upsert one forwarder's pushed identity and replace its stream catalog.
///
/// Forwarders are expected to pre-register via `POST /register`. For robustness,
/// a catalog push from an unknown endpoint creates a pending forwarder device
/// using the in-process provisioning token hash; existing approval state and
/// admin-assigned device names are preserved.
pub fn upsert_forwarder_catalog(
    conn: &Connection,
    endpoint_id: &str,
    display_name: Option<&str>,
    direct_addrs: &[String],
    streams: &[ForwarderCatalogStreamRecord],
    pending_token_hash: &[u8],
) -> rusqlite::Result<()> {
    let now = Utc::now().timestamp_millis();
    let direct_addrs_json = serde_json::to_string(direct_addrs)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "INSERT INTO devices (
             endpoint_id, device_kind, display_name, approval_state,
             token_hash, created_unix_ms, updated_unix_ms
         )
         VALUES (?1, 'forwarder', NULL, 'pending', ?2, ?3, ?3)
         ON CONFLICT(endpoint_id) DO UPDATE SET
             device_kind = 'forwarder',
             updated_unix_ms = excluded.updated_unix_ms",
        params![endpoint_id, pending_token_hash, now],
    )?;

    tx.execute(
        "INSERT INTO forwarders (endpoint_id, display_name, direct_addrs, last_seen_unix_ms)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(endpoint_id) DO UPDATE SET
             display_name = excluded.display_name,
             direct_addrs = excluded.direct_addrs,
             last_seen_unix_ms = excluded.last_seen_unix_ms",
        params![endpoint_id, display_name, direct_addrs_json, now],
    )?;

    tx.execute(
        "DELETE FROM forwarder_streams WHERE endpoint_id = ?1",
        [endpoint_id],
    )?;
    for stream in streams {
        tx.execute(
            "INSERT INTO forwarder_streams (endpoint_id, stream_id, epoch, next_seq)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                endpoint_id,
                &stream.stream_id,
                sql_u64_to_i64(stream.epoch)?,
                sql_u64_to_i64(stream.next_seq)?,
            ],
        )?;
    }

    tx.commit()
}

/// List all registered forwarder identities, ordered by endpoint id.
pub fn list_forwarders(conn: &Connection) -> rusqlite::Result<Vec<ForwarderRecord>> {
    let mut stmt = conn.prepare(
        "SELECT f.endpoint_id, f.display_name, f.direct_addrs, f.last_seen_unix_ms,
                d.approval_state
         FROM forwarders f
         JOIN devices d ON d.endpoint_id = f.endpoint_id
         ORDER BY f.endpoint_id",
    )?;
    let rows = stmt.query_map([], |row| {
        let approval_str: String = row.get(4)?;
        let approval_state = ApprovalState::parse(&approval_str).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                format!("invalid approval_state: {approval_str}").into(),
            )
        })?;
        let direct_addrs_json: String = row.get(2)?;
        let direct_addrs = serde_json::from_str(&direct_addrs_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
        })?;
        Ok(ForwarderRecord {
            endpoint_id: row.get(0)?,
            display_name: row.get(1)?,
            direct_addrs,
            last_seen_unix_ms: row.get(3)?,
            approval_state,
        })
    })?;

    rows.collect()
}

/// List approved (`active`) forwarder devices joined with their stream catalog.
///
/// Only devices of kind `forwarder` whose approval state is `active` and which
/// have a pushed forwarder identity row are returned, ordered by endpoint id;
/// each carries its `forwarder_streams` rows ordered by stream id. Pending or
/// receiver devices are excluded.
pub fn list_approved_forwarders_with_streams(
    conn: &Connection,
) -> rusqlite::Result<Vec<ApprovedForwarderWithStreams>> {
    let mut stmt = conn.prepare(
        "SELECT f.endpoint_id, f.display_name, f.direct_addrs
         FROM forwarders f
         JOIN devices d ON d.endpoint_id = f.endpoint_id
         WHERE d.device_kind = 'forwarder' AND d.approval_state = 'active'
         ORDER BY f.endpoint_id",
    )?;
    let forwarders = stmt
        .query_map([], |row| {
            let direct_addrs_json: String = row.get(2)?;
            let direct_addrs = serde_json::from_str(&direct_addrs_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                direct_addrs,
            ))
        })?
        .collect::<rusqlite::Result<Vec<(String, Option<String>, Vec<String>)>>>()?;

    let mut result = Vec::with_capacity(forwarders.len());
    for (endpoint_id, display_name, direct_addrs) in forwarders {
        let mut stream_stmt = conn.prepare(
            "SELECT stream_id, epoch, next_seq
             FROM forwarder_streams
             WHERE endpoint_id = ?1
             ORDER BY stream_id",
        )?;
        let streams = stream_stmt
            .query_map([&endpoint_id], |row| {
                Ok(DiscoveredForwarderStream {
                    stream_id: row.get(0)?,
                    epoch: sql_i64_to_u64(row.get(1)?, 1)?,
                    next_seq: sql_i64_to_u64(row.get(2)?, 2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.push(ApprovedForwarderWithStreams {
            endpoint_id,
            display_name,
            direct_addrs,
            streams,
        });
    }
    Ok(result)
}

/// List all registered devices, ordered by endpoint id.
pub fn list_devices(conn: &Connection) -> rusqlite::Result<Vec<DeviceRecord>> {
    let mut stmt = conn.prepare(
        "SELECT endpoint_id, device_kind, display_name, approval_state
         FROM devices
         ORDER BY endpoint_id",
    )?;
    let rows = stmt.query_map([], |row| {
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
    })?;

    rows.collect()
}

/// List the backup forwarder stream catalog rows, ordered by stream id.
pub fn list_forwarder_streams(conn: &Connection) -> rusqlite::Result<Vec<ForwarderStreamRecord>> {
    let mut stmt = conn.prepare(
        "SELECT stream_id, endpoint_id, epoch, next_seq
         FROM forwarder_streams
         ORDER BY endpoint_id, stream_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ForwarderStreamRecord {
            stream_id: row.get(0)?,
            endpoint_id: row.get(1)?,
            epoch: sql_i64_to_u64(row.get(2)?, 2)?,
            next_seq: sql_i64_to_u64(row.get(3)?, 3)?,
        })
    })?;

    rows.collect()
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

fn sql_u64_to_i64(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
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

fn forwarder_streams_needs_composite_pk(conn: &Connection) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(forwarder_streams)")?;
    let mut rows = stmt.query([])?;
    let mut pk_columns = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        let pk_position: i64 = row.get(5)?;
        if pk_position > 0 {
            pk_columns.push((pk_position, name));
        }
    }
    pk_columns.sort_by_key(|(position, _)| *position);
    Ok(pk_columns
        .into_iter()
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        != ["endpoint_id", "stream_id"])
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
    fn list_devices_returns_all_sorted() {
        let conn = test_conn();
        register_device(&conn, "ep-b", DeviceKind::Receiver, "tok-b").unwrap();
        register_device(&conn, "ep-a", DeviceKind::Forwarder, "tok-a").unwrap();
        approve_device(&conn, "ep-a", "Start").unwrap();

        let devices = list_devices(&conn).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].endpoint_id, "ep-a");
        assert_eq!(devices[0].approval_state, ApprovalState::Active);
        assert_eq!(devices[1].endpoint_id, "ep-b");
        assert_eq!(devices[1].approval_state, ApprovalState::Pending);
    }

    #[test]
    fn list_forwarder_streams_returns_backup_rows() {
        let conn = test_conn();
        register_device(&conn, "ep-fwd", DeviceKind::Forwarder, "tok").unwrap();
        conn.execute(
            "INSERT INTO forwarder_streams (stream_id, endpoint_id, epoch, next_seq)
             VALUES ('finish-line', 'ep-fwd', 3, 42)",
            [],
        )
        .unwrap();

        let streams = list_forwarder_streams(&conn).unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].stream_id, "finish-line");
        assert_eq!(streams[0].endpoint_id, "ep-fwd");
        assert_eq!(streams[0].epoch, 3);
        assert_eq!(streams[0].next_seq, 42);
    }

    #[test]
    fn upsert_forwarder_catalog_creates_pending_device_and_replaces_streams() {
        let conn = test_conn();
        let token_hash = hash_token("prov-secret");

        upsert_forwarder_catalog(
            &conn,
            "ep-fwd",
            Some("Start Line"),
            &["127.0.0.1:12345".to_owned()],
            &[
                ForwarderCatalogStreamRecord {
                    stream_id: "reader-a".to_owned(),
                    epoch: 1,
                    next_seq: 10,
                },
                ForwarderCatalogStreamRecord {
                    stream_id: "reader-b".to_owned(),
                    epoch: 2,
                    next_seq: 20,
                },
            ],
            &token_hash,
        )
        .unwrap();

        let device = get_device(&conn, "ep-fwd").unwrap().expect("device exists");
        assert_eq!(device.device_kind, DeviceKind::Forwarder);
        assert_eq!(device.approval_state, ApprovalState::Pending);

        let forwarders = list_forwarders(&conn).unwrap();
        assert_eq!(forwarders.len(), 1);
        assert_eq!(forwarders[0].endpoint_id, "ep-fwd");
        assert_eq!(forwarders[0].display_name.as_deref(), Some("Start Line"));
        assert_eq!(forwarders[0].direct_addrs, vec!["127.0.0.1:12345"]);
        assert_eq!(forwarders[0].approval_state, ApprovalState::Pending);
        assert!(forwarders[0].last_seen_unix_ms > 0);

        upsert_forwarder_catalog(
            &conn,
            "ep-fwd",
            Some("Start Line Updated"),
            &["10.0.0.7:54321".to_owned()],
            &[ForwarderCatalogStreamRecord {
                stream_id: "reader-b".to_owned(),
                epoch: 3,
                next_seq: 30,
            }],
            &token_hash,
        )
        .unwrap();

        let streams = list_forwarder_streams(&conn).unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].endpoint_id, "ep-fwd");
        assert_eq!(streams[0].stream_id, "reader-b");
        assert_eq!(streams[0].epoch, 3);
        assert_eq!(streams[0].next_seq, 30);
        assert_eq!(
            list_forwarders(&conn).unwrap()[0].direct_addrs,
            vec!["10.0.0.7:54321"]
        );
    }

    #[test]
    fn list_approved_forwarders_with_streams_joins_and_filters() {
        let conn = test_conn();
        let token_hash = hash_token("prov-secret");

        // Approved forwarder with two streams.
        upsert_forwarder_catalog(
            &conn,
            "ep-approved",
            Some("Start Line"),
            &["127.0.0.1:5000".to_owned(), "10.0.0.7:5000".to_owned()],
            &[
                ForwarderCatalogStreamRecord {
                    stream_id: "reader-a".to_owned(),
                    epoch: 1,
                    next_seq: 10,
                },
                ForwarderCatalogStreamRecord {
                    stream_id: "reader-b".to_owned(),
                    epoch: 2,
                    next_seq: 20,
                },
            ],
            &token_hash,
        )
        .unwrap();
        approve_device(&conn, "ep-approved", "Start Line")
            .unwrap()
            .unwrap();

        // Pending (unapproved) forwarder — must be excluded.
        upsert_forwarder_catalog(
            &conn,
            "ep-pending",
            Some("Pending"),
            &["127.0.0.1:6000".to_owned()],
            &[ForwarderCatalogStreamRecord {
                stream_id: "reader-c".to_owned(),
                epoch: 1,
                next_seq: 1,
            }],
            &token_hash,
        )
        .unwrap();

        // Approved receiver — must be excluded (wrong device kind).
        register_device(&conn, "ep-receiver", DeviceKind::Receiver, "tok").unwrap();
        approve_device(&conn, "ep-receiver", "Finish").unwrap();

        let forwarders = list_approved_forwarders_with_streams(&conn).unwrap();
        assert_eq!(forwarders.len(), 1);
        let fwd = &forwarders[0];
        assert_eq!(fwd.endpoint_id, "ep-approved");
        assert_eq!(fwd.display_name.as_deref(), Some("Start Line"));
        assert_eq!(fwd.direct_addrs, vec!["127.0.0.1:5000", "10.0.0.7:5000"]);
        assert_eq!(fwd.streams.len(), 2);
        assert_eq!(fwd.streams[0].stream_id, "reader-a");
        assert_eq!(fwd.streams[0].epoch, 1);
        assert_eq!(fwd.streams[0].next_seq, 10);
        assert_eq!(fwd.streams[1].stream_id, "reader-b");
        assert_eq!(fwd.streams[1].epoch, 2);
        assert_eq!(fwd.streams[1].next_seq, 20);
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
        assert!(verify_token("super-secret", &stored));
        assert!(!verify_token("wrong-secret", &stored));
        assert_ne!(stored, b"super-secret".to_vec());
    }

    #[test]
    fn hash_token_is_salted_and_verifiable() {
        // Two hashes of the same token differ (random salt) but both verify.
        let a = hash_token("same-token");
        let b = hash_token("same-token");
        assert_ne!(a, b, "salted hashes of the same token must differ");
        assert!(verify_token("same-token", &a));
        assert!(verify_token("same-token", &b));
        assert!(!verify_token("same-token ", &a));
    }

    #[test]
    fn verify_token_rejects_malformed_stored_hash() {
        assert!(!verify_token("x", b"not-hex"));
        assert!(!verify_token("x", b"deadbeef"));
        assert!(!verify_token("x", b"zz$zz"));
        assert!(!verify_token("x", &[0xff, 0xfe]));
    }
}

//! SQLite-backed device registry for the server.
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
    pub approval_state: ApprovalState,
    /// Human-friendly name for the device, if known. Prefers the admin-assigned
    /// name from the enrollment token used to enroll it, falling back to a
    /// forwarder's self-pushed catalog name. `None` for legacy devices that
    /// enrolled without an enrollment token and never pushed a name.
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentTokenStatus {
    Active,
    Used,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrollmentTokenRecord {
    pub token_id: String,
    pub device_kind: DeviceKind,
    pub display_name: Option<String>,
    pub status: EnrollmentTokenStatus,
    pub created_unix_ms: i64,
    pub used_unix_ms: Option<i64>,
    pub used_endpoint_id: Option<String>,
    pub revoked_unix_ms: Option<i64>,
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
    /// Division display name resolved by the receiver, when known.
    pub division: Option<String>,
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
    // Decode byte-wise (never slice the `str`, which would panic on a
    // multi-byte UTF-8 boundary). A malformed stored hash must fail closed by
    // returning `None`, never by panicking.
    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| Some((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Prefix that identifies a minted per-device bearer token.
const DEVICE_TOKEN_PREFIX: &str = "rtk_";
/// Byte length of the random per-device `token_id` (hex-encoded, ~128-bit).
const DEVICE_TOKEN_ID_LEN: usize = 16;
/// Byte length of the random per-device token secret.
const DEVICE_TOKEN_SECRET_LEN: usize = 32;

fn random_hex(len_bytes: usize) -> String {
    let mut buf = vec![0u8; len_bytes];
    rand::rng().fill_bytes(&mut buf);
    encode_hex(&buf)
}

/// Format a minted device token as `rtk_<token_id>_<secret>`.
fn format_device_token(token_id: &str, secret: &str) -> String {
    format!("{DEVICE_TOKEN_PREFIX}{token_id}_{secret}")
}

/// Split a raw bearer into `(token_id, secret)` iff it is a well-formed device
/// token. The `token_id` is the underscore-free hex segment after the prefix.
fn parse_device_token(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix(DEVICE_TOKEN_PREFIX)?;
    let (token_id, secret) = rest.split_once('_')?;
    if token_id.is_empty() || secret.is_empty() {
        return None;
    }
    Some((token_id, secret))
}

/// Resolve a raw bearer token to its device record via the indexed `token_id`,
/// verifying the secret in constant time.
///
/// Returns `None` for any token that is not a well-formed device token, has no
/// matching `devices` row, or fails verification — i.e. fail-closed. The caller
/// asserts device kind / approval state per endpoint.
pub fn authenticate_device(
    conn: &Connection,
    raw_token: &str,
) -> rusqlite::Result<Option<DeviceRecord>> {
    let Some((token_id, secret)) = parse_device_token(raw_token) else {
        return Ok(None);
    };
    let row = conn
        .query_row(
            "SELECT endpoint_id, device_kind, approval_state, token_hash
             FROM devices WHERE token_id = ?1",
            [token_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((endpoint_id, kind_raw, approval_raw, token_hash)) = row else {
        return Ok(None);
    };
    if !verify_token(secret, &token_hash) {
        return Ok(None);
    }
    // Reject a corrupt row fail-closed (deny) rather than erroring the auth path.
    if DeviceKind::parse(&kind_raw).is_none() || ApprovalState::parse(&approval_raw).is_none() {
        return Ok(None);
    }
    // Return the canonical record so the resolved `display_name` is populated
    // consistently with `get_device`/`list_devices`.
    get_device(conn, &endpoint_id)
}

/// Create the registry tables. Idempotent and safe to call on every open.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS devices (
             endpoint_id TEXT PRIMARY KEY,
             device_kind TEXT NOT NULL,
             approval_state TEXT NOT NULL,
             token_hash BLOB NOT NULL,
             created_unix_ms INTEGER NOT NULL,
             updated_unix_ms INTEGER NOT NULL,
             display_name TEXT
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
         );
         CREATE TABLE IF NOT EXISTS enrollment_tokens (
             token_id TEXT PRIMARY KEY,
             device_kind TEXT NOT NULL,
             display_name TEXT,
             token_hash BLOB NOT NULL,
             created_unix_ms INTEGER NOT NULL,
             used_unix_ms INTEGER,
             used_endpoint_id TEXT,
             revoked_unix_ms INTEGER
         );",
    )?;

    if forwarder_streams_needs_composite_pk(conn)? {
        reshape_forwarder_streams_pk(conn)?;
    }

    if !devices_has_display_name_column(conn)? {
        conn.execute_batch("ALTER TABLE devices ADD COLUMN display_name TEXT")?;
    }

    // Per-device minted-token id (nullable until a device is minted). A UNIQUE
    // index gives an indexed lookup in `authenticate_device`; SQLite treats
    // NULLs as distinct, so pre-mint rows coexist freely.
    if !column_exists(conn, "devices", "token_id")? {
        conn.execute_batch(
            "ALTER TABLE devices ADD COLUMN token_id TEXT;
             CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_token_id ON devices(token_id);",
        )?;
    }

    Ok(())
}

/// Whether the `devices` table already has the `display_name` column.
///
/// Older databases created before self-reported device names were added lack
/// the column; [`migrate`] adds it via `ALTER TABLE` when this returns false.
fn devices_has_display_name_column(conn: &Connection) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(devices)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == "display_name" {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Rebuild `forwarder_streams` with the composite `(endpoint_id, stream_id)`
/// primary key.
///
/// The reshape runs inside a `SAVEPOINT` so it is atomic: on any error the
/// partial work is explicitly rolled back to the savepoint and the savepoint
/// released, leaving the connection clean (and the original table intact)
/// rather than aborting mid-batch with a half-renamed/dropped table.
fn reshape_forwarder_streams_pk(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("SAVEPOINT reshape_forwarder_streams")?;

    let reshape = conn.execute_batch(
        "ALTER TABLE forwarder_streams RENAME TO forwarder_streams_old;
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
         DROP TABLE forwarder_streams_old;",
    );

    match reshape {
        Ok(()) => conn.execute_batch("RELEASE reshape_forwarder_streams"),
        Err(reshape_err) => {
            // Roll the partial reshape back and release the savepoint so the
            // connection is left usable; surface the original error (a cleanup
            // failure takes precedence via `?` since the connection state is
            // then unknown).
            conn.execute_batch(
                "ROLLBACK TO reshape_forwarder_streams; RELEASE reshape_forwarder_streams;",
            )?;
            Err(reshape_err)
        }
    }
}

fn enrollment_token_status(
    used_unix_ms: Option<i64>,
    revoked_unix_ms: Option<i64>,
) -> EnrollmentTokenStatus {
    if revoked_unix_ms.is_some() {
        EnrollmentTokenStatus::Revoked
    } else if used_unix_ms.is_some() {
        EnrollmentTokenStatus::Used
    } else {
        EnrollmentTokenStatus::Active
    }
}

fn enrollment_token_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EnrollmentTokenRecord> {
    let device_kind_raw: String = row.get(1)?;
    let used_unix_ms = row.get(5)?;
    let revoked_unix_ms = row.get(7)?;
    let Some(device_kind) = DeviceKind::parse(&device_kind_raw) else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    Ok(EnrollmentTokenRecord {
        token_id: row.get(0)?,
        device_kind,
        display_name: row.get(2)?,
        status: enrollment_token_status(used_unix_ms, revoked_unix_ms),
        created_unix_ms: row.get(4)?,
        used_unix_ms,
        used_endpoint_id: row.get(6)?,
        revoked_unix_ms,
    })
}

pub fn create_enrollment_token(
    conn: &Connection,
    token_id: &str,
    device_kind: DeviceKind,
    display_name: Option<&str>,
    raw_token: &str,
) -> rusqlite::Result<EnrollmentTokenRecord> {
    let now = Utc::now().timestamp_millis();
    let token_hash = hash_token(raw_token);
    conn.execute(
        "INSERT INTO enrollment_tokens (
             token_id, device_kind, display_name, token_hash, created_unix_ms
         )
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            token_id,
            device_kind.as_str(),
            display_name,
            token_hash,
            now
        ],
    )?;
    get_enrollment_token(conn, token_id)?.ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
}

fn get_enrollment_token(
    conn: &Connection,
    token_id: &str,
) -> rusqlite::Result<Option<EnrollmentTokenRecord>> {
    conn.query_row(
        "SELECT token_id, device_kind, display_name, token_hash, created_unix_ms,
                used_unix_ms, used_endpoint_id, revoked_unix_ms
         FROM enrollment_tokens
         WHERE token_id = ?1",
        [token_id],
        enrollment_token_from_row,
    )
    .optional()
}

pub fn list_enrollment_tokens(conn: &Connection) -> rusqlite::Result<Vec<EnrollmentTokenRecord>> {
    let mut stmt = conn.prepare(
        "SELECT token_id, device_kind, display_name, token_hash, created_unix_ms,
                used_unix_ms, used_endpoint_id, revoked_unix_ms
         FROM enrollment_tokens
         ORDER BY created_unix_ms DESC, token_id",
    )?;
    stmt.query_map([], enrollment_token_from_row)?.collect()
}

pub fn revoke_enrollment_token(
    conn: &Connection,
    token_id: &str,
) -> rusqlite::Result<Option<EnrollmentTokenRecord>> {
    let now = Utc::now().timestamp_millis();
    let changed = conn.execute(
        "UPDATE enrollment_tokens
         SET revoked_unix_ms = COALESCE(revoked_unix_ms, ?2)
         WHERE token_id = ?1",
        params![token_id, now],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    get_enrollment_token(conn, token_id)
}

/// Test-only: seed a device row with a hashed token (no `token_id`, so it does
/// not authenticate via [`authenticate_device`]). Tests use it to populate the
/// registry; production registration goes through [`register_device_with_voucher`].
#[cfg(test)]
pub(crate) fn register_device(
    conn: &Connection,
    endpoint_id: &str,
    device_kind: DeviceKind,
    raw_token: &str,
) -> rusqlite::Result<DeviceRecord> {
    let token_hash = hash_token(raw_token);
    let now = Utc::now().timestamp_millis();

    conn.execute(
        "INSERT INTO devices (
             endpoint_id, device_kind, approval_state,
             token_hash, created_unix_ms, updated_unix_ms
         )
         VALUES (?1, ?2, 'pending', ?3, ?4, ?4)
         ON CONFLICT(endpoint_id) DO UPDATE SET
             device_kind = excluded.device_kind,
             token_hash = excluded.token_hash,
             updated_unix_ms = excluded.updated_unix_ms",
        params![endpoint_id, device_kind.as_str(), token_hash, now],
    )?;

    get_device(conn, endpoint_id)?.ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
}

/// A freshly registered device plus its minted bearer token.
///
/// The token secret is returned exactly once; the server persists only its
/// salted hash and the lookup `token_id`.
#[derive(Clone)]
pub struct MintedRegistration {
    pub record: DeviceRecord,
    pub device_token: String,
}

// Manual `Debug` so the minted secret is never leaked through formatting.
impl std::fmt::Debug for MintedRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedRegistration")
            .field("record", &self.record)
            .field("device_token", &"<redacted>")
            .finish()
    }
}

/// Mint (rotate) the per-device token for an existing `devices` row, setting its
/// indexed `token_id` and the salted hash of a fresh secret. Returns the full
/// raw token (`rtk_<token_id>_<secret>`). The row must already exist.
fn mint_device_token_into(tx: &Connection, endpoint_id: &str) -> rusqlite::Result<String> {
    let now = Utc::now().timestamp_millis();
    let mut last_err = None;
    // `token_id` collisions on the UNIQUE index are astronomically unlikely;
    // retry a few times rather than surfacing a spurious constraint error.
    for _ in 0..8 {
        let token_id = random_hex(DEVICE_TOKEN_ID_LEN);
        let secret = random_hex(DEVICE_TOKEN_SECRET_LEN);
        let token_hash = hash_token(&secret);
        match tx.execute(
            "UPDATE devices
             SET token_id = ?2, token_hash = ?3, updated_unix_ms = ?4
             WHERE endpoint_id = ?1",
            params![endpoint_id, token_id, token_hash, now],
        ) {
            // 0 rows means the device row does not exist; callers always create
            // it first, so treat that as an error rather than minting a token
            // for a non-existent device.
            Ok(0) => return Err(rusqlite::Error::QueryReturnedNoRows),
            Ok(_) => return Ok(format_device_token(&token_id, &secret)),
            Err(err @ rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                last_err = Some(err);
            }
            Err(other) => return Err(other),
        }
    }
    Err(last_err.unwrap_or(rusqlite::Error::QueryReturnedNoRows))
}

/// Test-only: register-or-keep a device and mint its bearer token, returning the
/// raw token. Tests use it to obtain a working device token; production minting
/// happens via [`register_device_with_voucher`] on the `/register` path.
#[cfg(test)]
pub(crate) fn register_device_minted(
    conn: &Connection,
    endpoint_id: &str,
    device_kind: DeviceKind,
) -> rusqlite::Result<MintedRegistration> {
    let tx = conn.unchecked_transaction()?;
    let now = Utc::now().timestamp_millis();
    tx.execute(
        "INSERT INTO devices (
             endpoint_id, device_kind, approval_state,
             token_hash, created_unix_ms, updated_unix_ms
         )
         VALUES (?1, ?2, 'pending', ?3, ?4, ?4)
         ON CONFLICT(endpoint_id) DO UPDATE SET
             device_kind = excluded.device_kind,
             updated_unix_ms = excluded.updated_unix_ms",
        params![endpoint_id, device_kind.as_str(), Vec::<u8>::new(), now],
    )?;
    let device_token = mint_device_token_into(&tx, endpoint_id)?;
    tx.commit()?;
    let record = get_device(conn, endpoint_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    Ok(MintedRegistration {
        record,
        device_token,
    })
}

/// Register (or recover) a device by presenting an enrollment **voucher**, then
/// mint its bearer token. Returns `None` (→ `401`) when no usable voucher of the
/// matching kind verifies.
///
/// Recovery semantics (single atomic transaction so a voucher can't be
/// double-spent across endpoints):
/// - Same-`endpoint_id` re-presentation of its own already-used voucher rotates
///   the token and **keeps** the existing approval state (covers a crash before
///   the client persisted its first token).
/// - A different, unused voucher rebinding an *existing* endpoint resets
///   `approval_state` to `pending` and requires the existing `device_kind` to
///   match, so a stolen voucher cannot silently take over an active device.
/// - An unused voucher for an unknown endpoint creates a new `pending` device.
pub fn register_device_with_voucher(
    conn: &Connection,
    endpoint_id: &str,
    device_kind: DeviceKind,
    raw_voucher: &str,
) -> rusqlite::Result<Option<MintedRegistration>> {
    let tx = conn.unchecked_transaction()?;

    let existing_kind = tx
        .query_row(
            "SELECT device_kind FROM devices WHERE endpoint_id = ?1",
            [endpoint_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|raw| DeviceKind::parse(&raw));
    let device_exists = existing_kind.is_some();

    // Find a non-revoked voucher of this kind whose secret verifies.
    let mut matched: Option<(String, Option<String>, bool)> = None;
    {
        let mut stmt = tx.prepare(
            "SELECT token_id, token_hash, used_unix_ms, used_endpoint_id
             FROM enrollment_tokens
             WHERE device_kind = ?1 AND revoked_unix_ms IS NULL
             ORDER BY created_unix_ms, token_id",
        )?;
        let rows = stmt.query_map([device_kind.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (voucher_id, token_hash, used_unix_ms, used_endpoint_id) = row?;
            if verify_token(raw_voucher, &token_hash) {
                matched = Some((voucher_id, used_endpoint_id, used_unix_ms.is_some()));
                break;
            }
        }
    }
    let Some((voucher_id, used_endpoint_id, used)) = matched else {
        return Ok(None);
    };

    // A used voucher is acceptable only when re-presented by the same endpoint.
    let same_endpoint_reuse = used && used_endpoint_id.as_deref() == Some(endpoint_id);
    if used && !same_endpoint_reuse {
        return Ok(None);
    }

    // A voucher kind must match an existing device's kind (no cross-kind rebind).
    if let Some(existing_kind) = existing_kind
        && existing_kind != device_kind
    {
        return Ok(None);
    }

    let now = Utc::now().timestamp_millis();

    // Consume a fresh voucher (idempotent no-op for same-endpoint reuse).
    if !used {
        tx.execute(
            "UPDATE enrollment_tokens
             SET used_unix_ms = ?2, used_endpoint_id = ?3
             WHERE token_id = ?1 AND used_unix_ms IS NULL AND revoked_unix_ms IS NULL",
            params![voucher_id, now, endpoint_id],
        )?;
    }

    if device_exists {
        if same_endpoint_reuse {
            // Rotate only: keep approval state.
            tx.execute(
                "UPDATE devices SET device_kind = ?2, updated_unix_ms = ?3 WHERE endpoint_id = ?1",
                params![endpoint_id, device_kind.as_str(), now],
            )?;
        } else {
            // Fresh-voucher rebind of an existing endpoint: reset to pending.
            tx.execute(
                "UPDATE devices
                 SET device_kind = ?2, approval_state = 'pending', updated_unix_ms = ?3
                 WHERE endpoint_id = ?1",
                params![endpoint_id, device_kind.as_str(), now],
            )?;
        }
    } else {
        tx.execute(
            "INSERT INTO devices (
                 endpoint_id, device_kind, approval_state,
                 token_hash, created_unix_ms, updated_unix_ms
             )
             VALUES (?1, ?2, 'pending', ?3, ?4, ?4)",
            params![endpoint_id, device_kind.as_str(), Vec::<u8>::new(), now],
        )?;
    }

    let device_token = mint_device_token_into(&tx, endpoint_id)?;
    tx.commit()?;
    let record = get_device(conn, endpoint_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    Ok(Some(MintedRegistration {
        record,
        device_token,
    }))
}

/// Test-only: insert an `active` device with a caller-chosen `token_id`/secret
/// so tests can authenticate with a deterministic `rtk_<token_id>_<secret>`
/// bearer instead of a randomly minted one.
#[cfg(test)]
pub(crate) fn seed_active_device(
    conn: &Connection,
    endpoint_id: &str,
    device_kind: DeviceKind,
    token_id: &str,
    secret: &str,
) {
    let now = Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO devices (
             endpoint_id, device_kind, approval_state,
             token_hash, token_id, created_unix_ms, updated_unix_ms
         )
         VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?5)",
        params![
            endpoint_id,
            device_kind.as_str(),
            hash_token(secret),
            token_id,
            now
        ],
    )
    .expect("seed_active_device insert");
}

/// Approve a device, marking it `active`.
///
/// Returns `None` if no device with the given endpoint id exists.
pub fn approve_device(
    conn: &Connection,
    endpoint_id: &str,
) -> rusqlite::Result<Option<DeviceRecord>> {
    let now = Utc::now().timestamp_millis();
    let changed = conn.execute(
        "UPDATE devices
         SET approval_state = 'active', updated_unix_ms = ?2
         WHERE endpoint_id = ?1",
        params![endpoint_id, now],
    )?;

    if changed == 0 {
        return Ok(None);
    }

    get_device(conn, endpoint_id)
}

/// Upsert one forwarder's pushed identity and replace its stream catalog.
///
/// The forwarder device must already exist (it registers and mints its token
/// via `POST /register` before pushing a catalog, and the catalog endpoint only
/// authorizes an existing forwarder). Existing approval state and admin-assigned
/// names are preserved; this only refreshes the pushed identity and streams.
pub fn upsert_forwarder_catalog(
    conn: &Connection,
    endpoint_id: &str,
    display_name: Option<&str>,
    direct_addrs: &[String],
    streams: &[ForwarderCatalogStreamRecord],
) -> rusqlite::Result<()> {
    let now = Utc::now().timestamp_millis();
    let direct_addrs_json = serde_json::to_string(direct_addrs)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    let tx = conn.unchecked_transaction()?;

    // Ensure a device row exists for the FK (it normally already does, since the
    // catalog endpoint only authorizes an already-registered forwarder). A
    // token-less `pending` row is a harmless backstop; it carries no `token_id`
    // so it cannot authenticate until the forwarder mints one via `/register`.
    tx.execute(
        "INSERT INTO devices (
             endpoint_id, device_kind, approval_state,
             token_hash, created_unix_ms, updated_unix_ms
         )
         VALUES (?1, 'forwarder', 'pending', ?2, ?3, ?3)
         ON CONFLICT(endpoint_id) DO UPDATE SET
             device_kind = 'forwarder',
             updated_unix_ms = excluded.updated_unix_ms",
        params![endpoint_id, Vec::<u8>::new(), now],
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
        "SELECT f.endpoint_id, f.display_name, f.direct_addrs,
                f.last_seen_unix_ms, d.approval_state
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
/// Map a row of `(endpoint_id, device_kind, approval_state, display_name)` into
/// a [`DeviceRecord`]. Shared by the device list and single-device fetch so
/// both surface the same human-friendly name resolution.
fn device_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceRecord> {
    let kind_str: String = row.get(1)?;
    let approval_str: String = row.get(2)?;
    let device_kind = DeviceKind::parse(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            format!("invalid device_kind: {kind_str}").into(),
        )
    })?;
    let approval_state = ApprovalState::parse(&approval_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("invalid approval_state: {approval_str}").into(),
        )
    })?;
    Ok(DeviceRecord {
        endpoint_id: row.get(0)?,
        device_kind,
        approval_state,
        display_name: row.get(3)?,
    })
}

/// SQL selecting device columns plus a resolved human-friendly `display_name`.
///
/// The name prefers the admin-assigned name from the most recently used
/// enrollment token, then the device's self-reported name (e.g. a receiver's
/// configured receiver ID, sent at registration), then the forwarder's
/// self-pushed catalog name.
const DEVICE_SELECT_WITH_NAME: &str = "SELECT d.endpoint_id, d.device_kind, d.approval_state,
            COALESCE(
                (SELECT et.display_name FROM enrollment_tokens et
                 WHERE et.used_endpoint_id = d.endpoint_id
                   AND et.display_name IS NOT NULL
                 ORDER BY et.used_unix_ms DESC LIMIT 1),
                d.display_name,
                f.display_name
            ) AS display_name
     FROM devices d
     LEFT JOIN forwarders f ON f.endpoint_id = d.endpoint_id";

/// Persist a device's self-reported display name.
///
/// Called from the registration path when a device sends a non-empty name
/// (e.g. a receiver's configured receiver ID). A blank name is ignored so an
/// unnamed re-registration never clears a previously stored name.
pub fn set_device_display_name(
    conn: &Connection,
    endpoint_id: &str,
    display_name: &str,
) -> rusqlite::Result<()> {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE devices SET display_name = ?2 WHERE endpoint_id = ?1",
        params![endpoint_id, trimmed],
    )?;
    Ok(())
}

pub fn list_devices(conn: &Connection) -> rusqlite::Result<Vec<DeviceRecord>> {
    let sql = format!("{DEVICE_SELECT_WITH_NAME}\n         ORDER BY d.endpoint_id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], device_from_row)?;

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
    let sql = format!("{DEVICE_SELECT_WITH_NAME}\n         WHERE d.endpoint_id = ?1");
    conn.query_row(&sql, [endpoint_id], device_from_row)
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
             reader_timestamp, received_unix_ms, division
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(stream_id, seq) DO UPDATE SET
             source_generation = excluded.source_generation,
             chip_id = excluded.chip_id,
             bib = excluded.bib,
             display_name = excluded.display_name,
             reader_timestamp = excluded.reader_timestamp,
             received_unix_ms = excluded.received_unix_ms,
             division = excluded.division",
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
            &row.division,
        ],
    )?;
    Ok(())
}

pub fn list_announcer_rows_ordered(
    conn: &Connection,
) -> Result<Vec<AnnouncerRowRecord>, AnnouncerStorageError> {
    let mut stmt = conn.prepare(
        "SELECT source_generation, stream_id, seq, chip_id, bib, display_name,
                reader_timestamp, received_unix_ms, division
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
            division: row.get(8)?,
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

/// Returns whether `table` has a column named `column`.
fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
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

        let active = approve_device(&conn, "ep-1")
            .unwrap()
            .expect("device exists");
        assert_eq!(active.approval_state, ApprovalState::Active);
    }

    #[test]
    fn authenticate_device_roundtrips_and_fails_closed() {
        let conn = test_conn();
        let minted = register_device_minted(&conn, "ep-1", DeviceKind::Forwarder).unwrap();
        assert!(minted.device_token.starts_with("rtk_"));
        let record = authenticate_device(&conn, &minted.device_token)
            .unwrap()
            .expect("token resolves");
        assert_eq!(record.endpoint_id, "ep-1");
        assert_eq!(record.device_kind, DeviceKind::Forwarder);
        assert_eq!(record.approval_state, ApprovalState::Pending);
        // Tampered secret, and non-device-token shapes, fail closed.
        assert!(
            authenticate_device(&conn, &format!("{}x", minted.device_token))
                .unwrap()
                .is_none()
        );
        assert!(authenticate_device(&conn, "not-a-token").unwrap().is_none());
        assert!(authenticate_device(&conn, "rtk_onlyid").unwrap().is_none());
    }

    #[test]
    fn register_device_minted_preserves_approval_and_rotates() {
        let conn = test_conn();
        let first = register_device_minted(&conn, "ep-1", DeviceKind::Receiver).unwrap();
        approve_device(&conn, "ep-1").unwrap().unwrap();
        let second = register_device_minted(&conn, "ep-1", DeviceKind::Receiver).unwrap();
        assert_eq!(second.record.approval_state, ApprovalState::Active);
        // Re-mint rotates: the old token no longer authenticates.
        assert!(
            authenticate_device(&conn, &first.device_token)
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate_device(&conn, &second.device_token)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn voucher_register_mints_and_consumes() {
        let conn = test_conn();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher").unwrap();
        let minted = register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
            .unwrap()
            .expect("voucher accepted");
        assert_eq!(minted.record.approval_state, ApprovalState::Pending);
        assert!(
            authenticate_device(&conn, &minted.device_token)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            list_enrollment_tokens(&conn).unwrap()[0].status,
            EnrollmentTokenStatus::Used
        );
    }

    #[test]
    fn voucher_double_spend_by_second_endpoint_rejected() {
        let conn = test_conn();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher").unwrap();
        register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
            .unwrap()
            .unwrap();
        assert!(
            register_device_with_voucher(&conn, "ep-2", DeviceKind::Forwarder, "voucher")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn same_endpoint_voucher_reuse_rotates_and_keeps_approval() {
        let conn = test_conn();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher").unwrap();
        let first = register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
            .unwrap()
            .unwrap();
        approve_device(&conn, "ep-1").unwrap().unwrap();
        let second = register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
            .unwrap()
            .unwrap();
        assert_eq!(second.record.approval_state, ApprovalState::Active);
        assert!(
            authenticate_device(&conn, &first.device_token)
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate_device(&conn, &second.device_token)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn fresh_voucher_rebind_resets_approval_and_requires_kind_match() {
        let conn = test_conn();
        register_device_minted(&conn, "ep-1", DeviceKind::Forwarder).unwrap();
        approve_device(&conn, "ep-1").unwrap().unwrap();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher-f").unwrap();
        let rebound =
            register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher-f")
                .unwrap()
                .unwrap();
        assert_eq!(rebound.record.approval_state, ApprovalState::Pending);
        // A receiver voucher must not rebind a forwarder endpoint.
        create_enrollment_token(&conn, "tok-2", DeviceKind::Receiver, None, "voucher-r").unwrap();
        assert!(
            register_device_with_voucher(&conn, "ep-1", DeviceKind::Receiver, "voucher-r")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn reregistration_preserves_approval() {
        let conn = test_conn();

        register_device(&conn, "ep-2", DeviceKind::Receiver, "tok-a").unwrap();
        approve_device(&conn, "ep-2").unwrap();

        let reregistered = register_device(&conn, "ep-2", DeviceKind::Receiver, "tok-b").unwrap();
        assert_eq!(reregistered.approval_state, ApprovalState::Active);
    }

    #[test]
    fn approve_missing_device_returns_none() {
        let conn = test_conn();
        assert!(approve_device(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn forwarder_listings_use_pushed_catalog_name() {
        let conn = test_conn();
        upsert_forwarder_catalog(
            &conn,
            "ep-fwd",
            Some("Pushed Name"),
            &["127.0.0.1:5000".to_owned()],
            &[ForwarderCatalogStreamRecord {
                stream_id: "reader-a".to_owned(),
                epoch: 1,
                next_seq: 10,
            }],
        )
        .unwrap();
        approve_device(&conn, "ep-fwd").unwrap();

        let forwarders = list_forwarders(&conn).unwrap();
        assert_eq!(forwarders[0].display_name.as_deref(), Some("Pushed Name"));

        let approved = list_approved_forwarders_with_streams(&conn).unwrap();
        assert_eq!(approved[0].display_name.as_deref(), Some("Pushed Name"));
    }

    #[test]
    fn list_devices_returns_all_sorted() {
        let conn = test_conn();
        register_device(&conn, "ep-b", DeviceKind::Receiver, "tok-b").unwrap();
        register_device(&conn, "ep-a", DeviceKind::Forwarder, "tok-a").unwrap();
        approve_device(&conn, "ep-a").unwrap();

        let devices = list_devices(&conn).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].endpoint_id, "ep-a");
        assert_eq!(devices[0].approval_state, ApprovalState::Active);
        assert_eq!(devices[1].endpoint_id, "ep-b");
        assert_eq!(devices[1].approval_state, ApprovalState::Pending);
    }

    #[test]
    fn device_display_name_prefers_enrollment_token_then_catalog() {
        let conn = test_conn();

        // Receiver enrolled via an admin-named enrollment token.
        create_enrollment_token(
            &conn,
            "et-rx",
            DeviceKind::Receiver,
            Some("Finish Line"),
            "rx-token",
        )
        .unwrap();
        register_device_with_voucher(&conn, "ep-rx", DeviceKind::Receiver, "rx-token").unwrap();

        // Forwarder with an admin token name AND a self-pushed catalog name:
        // the admin-assigned token name wins.
        create_enrollment_token(
            &conn,
            "et-fwd",
            DeviceKind::Forwarder,
            Some("Admin Name"),
            "fwd-token",
        )
        .unwrap();
        register_device_with_voucher(&conn, "ep-fwd", DeviceKind::Forwarder, "fwd-token").unwrap();
        upsert_forwarder_catalog(
            &conn,
            "ep-fwd",
            Some("Pushed Name"),
            &["127.0.0.1:5000".to_owned()],
            &[],
        )
        .unwrap();

        // Receiver with a self-reported name (e.g. its receiver ID) set at
        // registration: surfaced when there is no enrollment-token name.
        register_device(&conn, "ep-self", DeviceKind::Receiver, "tok").unwrap();
        set_device_display_name(&conn, "ep-self", "dev-receiver").unwrap();

        // Forwarder with only a self-pushed catalog name (no enrollment token):
        // falls back to the pushed name.
        upsert_forwarder_catalog(
            &conn,
            "ep-pushed",
            Some("Pushed Only"),
            &["127.0.0.1:5001".to_owned()],
            &[],
        )
        .unwrap();

        // Legacy device with no name source at all.
        register_device(&conn, "ep-legacy", DeviceKind::Receiver, "tok").unwrap();

        let by_id: std::collections::HashMap<_, _> = list_devices(&conn)
            .unwrap()
            .into_iter()
            .map(|d| (d.endpoint_id.clone(), d))
            .collect();
        assert_eq!(by_id["ep-rx"].display_name.as_deref(), Some("Finish Line"));
        assert_eq!(by_id["ep-fwd"].display_name.as_deref(), Some("Admin Name"));
        assert_eq!(
            by_id["ep-self"].display_name.as_deref(),
            Some("dev-receiver")
        );
        assert_eq!(
            by_id["ep-pushed"].display_name.as_deref(),
            Some("Pushed Only")
        );
        assert_eq!(by_id["ep-legacy"].display_name, None);

        // A blank self-reported name never clears an existing name.
        set_device_display_name(&conn, "ep-self", "   ").unwrap();
        let still_named = get_device(&conn, "ep-self").unwrap().unwrap();
        assert_eq!(still_named.display_name.as_deref(), Some("dev-receiver"));

        // Single-device fetch resolves the same name.
        let fetched = get_device(&conn, "ep-rx").unwrap().unwrap();
        assert_eq!(fetched.display_name.as_deref(), Some("Finish Line"));
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
        )
        .unwrap();
        approve_device(&conn, "ep-approved").unwrap().unwrap();

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
        )
        .unwrap();

        // Approved receiver — must be excluded (wrong device kind).
        register_device(&conn, "ep-receiver", DeviceKind::Receiver, "tok").unwrap();
        approve_device(&conn, "ep-receiver").unwrap();

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
    fn enrollment_tokens_are_hashed_not_plaintext() {
        let conn = test_conn();

        create_enrollment_token(
            &conn,
            "tok-1",
            DeviceKind::Forwarder,
            Some("Start Line"),
            "super-secret",
        )
        .unwrap();

        let stored: Vec<u8> = conn
            .query_row(
                "SELECT token_hash FROM enrollment_tokens WHERE token_id = 'tok-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(verify_token("super-secret", &stored));
        assert!(!verify_token("wrong-secret", &stored));
        assert_ne!(stored, b"super-secret".to_vec());
    }

    #[test]
    fn list_enrollment_tokens_omits_plaintext_secret() {
        let conn = test_conn();

        create_enrollment_token(
            &conn,
            "tok-1",
            DeviceKind::Forwarder,
            Some("Start Line"),
            "super-secret",
        )
        .unwrap();

        let tokens = list_enrollment_tokens(&conn).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_id, "tok-1");
        assert_eq!(tokens[0].device_kind, DeviceKind::Forwarder);
        assert_eq!(tokens[0].display_name.as_deref(), Some("Start Line"));
        assert_eq!(tokens[0].status, EnrollmentTokenStatus::Active);
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
        // Valid UTF-8 but multi-byte hex parts must fail closed, not panic
        // (would have panicked when slicing the str by byte index).
        assert!(!verify_token("x", "é$é".as_bytes()));
        assert!(!verify_token("x", "00$é".as_bytes()));
        assert!(!verify_token("x", "abcd€xyz$0011".as_bytes()));
    }

    #[test]
    fn forwarder_streams_reshape_rolls_back_on_failure() {
        let conn = Connection::open_in_memory().unwrap();
        // Legacy single-column-PK shape (triggers the composite-PK reshape),
        // plus the `devices` FK target. The row references an `endpoint_id`
        // that does NOT exist in `devices`.
        conn.execute_batch(
            "CREATE TABLE devices (endpoint_id TEXT PRIMARY KEY);
             CREATE TABLE forwarder_streams (
                 endpoint_id TEXT PRIMARY KEY,
                 stream_id TEXT NOT NULL,
                 epoch INTEGER NOT NULL,
                 next_seq INTEGER NOT NULL
             );
             INSERT INTO forwarder_streams (endpoint_id, stream_id, epoch, next_seq)
             VALUES ('orphan', 'reader-a', 1, 5);",
        )
        .unwrap();

        // Enforce FKs (must be set outside a transaction) so the reshape's
        // INSERT...SELECT fails *after* the RENAME and CREATE have already been
        // applied inside the savepoint — the exact mid-batch failure the
        // explicit rollback must recover from.
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        assert!(forwarder_streams_needs_composite_pk(&conn).unwrap());

        let err = reshape_forwarder_streams_pk(&conn).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("foreign key"),
            "expected a foreign-key failure, got: {err}"
        );

        // Rollback restored the original table and row; the temp *_old table is
        // gone; the legacy PK shape is intact (reshape did not partially apply).
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM forwarder_streams", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let old_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'forwarder_streams_old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_exists, 0);
        assert!(forwarder_streams_needs_composite_pk(&conn).unwrap());

        // No dangling savepoint/transaction: the connection is still usable.
        conn.execute_batch("CREATE TABLE probe (x); DROP TABLE probe;")
            .unwrap();
    }
}

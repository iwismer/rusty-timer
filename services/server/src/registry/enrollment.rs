use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::{DeviceKind, hash_token};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentTokenStatus {
    Active,
    Used,
    Revoked,
    Expired,
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
    /// When the voucher stops being usable. `None` for legacy rows created
    /// before the TTL was introduced (no expiry).
    pub expires_unix_ms: Option<i64>,
}

/// Enrollment vouchers are valid for 24 hours from creation. NULL (legacy
/// rows) means no expiry.
const ENROLLMENT_TOKEN_TTL_MS: i64 = 24 * 60 * 60 * 1000;

fn enrollment_token_status(
    used_unix_ms: Option<i64>,
    revoked_unix_ms: Option<i64>,
    expires_unix_ms: Option<i64>,
    now_unix_ms: i64,
) -> EnrollmentTokenStatus {
    if revoked_unix_ms.is_some() {
        EnrollmentTokenStatus::Revoked
    } else if used_unix_ms.is_some() {
        // A used voucher past its expiry still displays as `Used`; expiry only
        // matters for reuse, which the registration query blocks directly.
        EnrollmentTokenStatus::Used
    } else if expires_unix_ms.is_some_and(|expires| expires <= now_unix_ms) {
        EnrollmentTokenStatus::Expired
    } else {
        EnrollmentTokenStatus::Active
    }
}

fn enrollment_token_from_row(
    row: &rusqlite::Row<'_>,
    now_unix_ms: i64,
) -> rusqlite::Result<EnrollmentTokenRecord> {
    let device_kind_raw: String = row.get(1)?;
    let used_unix_ms = row.get(5)?;
    let revoked_unix_ms = row.get(7)?;
    let expires_unix_ms = row.get(8)?;
    let Some(device_kind) = DeviceKind::parse(&device_kind_raw) else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    Ok(EnrollmentTokenRecord {
        token_id: row.get(0)?,
        device_kind,
        display_name: row.get(2)?,
        status: enrollment_token_status(
            used_unix_ms,
            revoked_unix_ms,
            expires_unix_ms,
            now_unix_ms,
        ),
        created_unix_ms: row.get(4)?,
        used_unix_ms,
        used_endpoint_id: row.get(6)?,
        revoked_unix_ms,
        expires_unix_ms,
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
             token_id, device_kind, display_name, token_hash, created_unix_ms,
             expires_unix_ms
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            token_id,
            device_kind.as_str(),
            display_name,
            token_hash,
            now,
            now + ENROLLMENT_TOKEN_TTL_MS
        ],
    )?;
    get_enrollment_token(conn, token_id)?.ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
}

fn get_enrollment_token(
    conn: &Connection,
    token_id: &str,
) -> rusqlite::Result<Option<EnrollmentTokenRecord>> {
    let now = Utc::now().timestamp_millis();
    conn.query_row(
        "SELECT token_id, device_kind, display_name, token_hash, created_unix_ms,
                used_unix_ms, used_endpoint_id, revoked_unix_ms, expires_unix_ms
         FROM enrollment_tokens
         WHERE token_id = ?1",
        [token_id],
        |row| enrollment_token_from_row(row, now),
    )
    .optional()
}

pub fn list_enrollment_tokens(conn: &Connection) -> rusqlite::Result<Vec<EnrollmentTokenRecord>> {
    let now = Utc::now().timestamp_millis();
    let mut stmt = conn.prepare(
        "SELECT token_id, device_kind, display_name, token_hash, created_unix_ms,
                used_unix_ms, used_endpoint_id, revoked_unix_ms, expires_unix_ms
         FROM enrollment_tokens
         ORDER BY created_unix_ms DESC, token_id",
    )?;
    stmt.query_map([], |row| enrollment_token_from_row(row, now))?
        .collect()
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

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::registry::{migrate, verify_token};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    /// Force a voucher's expiry into the past directly in SQL.
    pub(super) fn force_expire_token(conn: &Connection, token_id: &str) {
        conn.execute(
            "UPDATE enrollment_tokens SET expires_unix_ms = 1 WHERE token_id = ?1",
            [token_id],
        )
        .unwrap();
    }

    #[test]
    fn enrollment_token_listing_reports_expired_status() {
        let conn = test_conn();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher").unwrap();
        force_expire_token(&conn, "tok-1");
        assert_eq!(
            list_enrollment_tokens(&conn).unwrap()[0].status,
            EnrollmentTokenStatus::Expired
        );
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
}

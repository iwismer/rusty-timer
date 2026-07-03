use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub use rt_server_api::device::{ApprovalState, DeviceKind};

use super::tokens::{
    DEVICE_TOKEN_ID_LEN, DEVICE_TOKEN_SECRET_LEN, format_device_token, random_hex,
};
use super::{DEVICE_SELECT_WITH_NAME, device_from_row, hash_token, verify_token};

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

/// Test-only: seed a device row with a hashed token (no `token_id`, so it does
/// not authenticate via [`authenticate_device`]). Tests use it to populate the
/// registry; production registration goes through [`register_device_with_voucher`].
///
/// [`authenticate_device`]: super::authenticate_device
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
///   the token and **resets** approval to `pending` (covers a crash before the
///   client persisted its first token; a voucher secret leaked after use must
///   not silently yield an active token — the admin re-approves).
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

    let now = Utc::now().timestamp_millis();

    // Find a non-revoked, non-expired voucher of this kind whose secret
    // verifies. The expiry filter applies to fresh use and same-endpoint reuse
    // alike (both flow through this query); NULL expiry (legacy rows) never
    // expires.
    let mut matched: Option<(String, Option<String>, bool)> = None;
    {
        let mut stmt = tx.prepare(
            "SELECT token_id, token_hash, used_unix_ms, used_endpoint_id
             FROM enrollment_tokens
             WHERE device_kind = ?1 AND revoked_unix_ms IS NULL
               AND (expires_unix_ms IS NULL OR expires_unix_ms > ?2)
             ORDER BY created_unix_ms, token_id",
        )?;
        let rows = stmt.query_map(params![device_kind.as_str(), now], |row| {
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
        // Any voucher-based (re)registration of an existing endpoint resets to
        // pending: fresh-voucher rebind for the usual takeover reasons, and
        // same-endpoint reuse because a voucher secret leaked after use must
        // not silently yield an active token.
        tx.execute(
            "UPDATE devices
             SET device_kind = ?2, approval_state = 'pending', updated_unix_ms = ?3
             WHERE endpoint_id = ?1",
            params![endpoint_id, device_kind.as_str(), now],
        )?;
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

/// List all registered devices, ordered by endpoint id.
pub fn list_devices(conn: &Connection) -> rusqlite::Result<Vec<DeviceRecord>> {
    let sql = format!("{DEVICE_SELECT_WITH_NAME}\n         ORDER BY d.endpoint_id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], device_from_row)?;

    rows.collect()
}

/// Fetch a device record by endpoint id.
pub fn get_device(conn: &Connection, endpoint_id: &str) -> rusqlite::Result<Option<DeviceRecord>> {
    let sql = format!("{DEVICE_SELECT_WITH_NAME}\n         WHERE d.endpoint_id = ?1");
    conn.query_row(&sql, [endpoint_id], device_from_row)
        .optional()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rusqlite::Connection;

    use super::*;
    use crate::registry::{
        EnrollmentTokenStatus, ForwarderCatalogStreamRecord, authenticate_device,
        create_enrollment_token, list_enrollment_tokens, migrate, upsert_forwarder_catalog,
    };

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    /// Force a voucher's expiry into the past directly in SQL.
    fn force_expire_token(conn: &Connection, token_id: &str) {
        conn.execute(
            "UPDATE enrollment_tokens SET expires_unix_ms = 1 WHERE token_id = ?1",
            [token_id],
        )
        .unwrap();
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
    fn expired_voucher_cannot_register() {
        let conn = test_conn();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher").unwrap();
        force_expire_token(&conn, "tok-1");
        assert!(
            register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn expired_voucher_cannot_be_reused_by_same_endpoint() {
        let conn = test_conn();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher").unwrap();
        register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
            .unwrap()
            .unwrap();
        force_expire_token(&conn, "tok-1");
        assert!(
            register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
                .unwrap()
                .is_none()
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
    fn same_endpoint_voucher_reuse_rotates_and_resets_to_pending() {
        let conn = test_conn();
        create_enrollment_token(&conn, "tok-1", DeviceKind::Forwarder, None, "voucher").unwrap();
        let first = register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
            .unwrap()
            .unwrap();
        approve_device(&conn, "ep-1").unwrap().unwrap();
        let second = register_device_with_voucher(&conn, "ep-1", DeviceKind::Forwarder, "voucher")
            .unwrap()
            .unwrap();
        assert_eq!(second.record.approval_state, ApprovalState::Pending);
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

        let by_id: HashMap<_, _> = list_devices(&conn)
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

        let forwarders = crate::registry::list_forwarders(&conn).unwrap();
        assert_eq!(forwarders[0].display_name.as_deref(), Some("Pushed Name"));

        let approved = crate::registry::list_approved_forwarders_with_streams(&conn).unwrap();
        assert_eq!(approved[0].display_name.as_deref(), Some("Pushed Name"));
    }
}

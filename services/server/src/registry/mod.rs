//! SQLite-backed device registry for the server.
//!
//! Tracks forwarder/receiver endpoints, their approval state, hashed
//! per-device bearer tokens, and a backup of the forwarder stream catalog.
//! New devices self-register under a TOFU (trust-on-first-use) model: the
//! first valid registration creates a `pending` record, and an admin later
//! approves and names it to mark it `active`.

mod announcer_store;
mod devices;
mod enrollment;
mod forwarders;
mod schema;
mod tokens;

pub use announcer_store::{
    AnnouncerRowRecord, AnnouncerStorageError, current_announcer_source_generation,
    list_announcer_rows_ordered, takeover_announcer_source, upsert_announcer_row,
};
pub use devices::{
    ApprovalState, DeviceKind, DeviceRecord, MintedRegistration, approve_device, get_device,
    list_devices, register_device_with_voucher, set_device_display_name,
};
#[cfg(test)]
pub(crate) use devices::{register_device, register_device_minted, seed_active_device};
pub use enrollment::{
    EnrollmentTokenRecord, EnrollmentTokenStatus, create_enrollment_token, list_enrollment_tokens,
    revoke_enrollment_token,
};
pub use forwarders::{
    ApprovedForwarderWithStreams, DiscoveredForwarderStream, ForwarderCatalogStreamRecord,
    ForwarderRecord, ForwarderStreamRecord, list_approved_forwarders_with_streams,
    list_forwarder_streams, list_forwarders, upsert_forwarder_catalog,
};
pub use schema::migrate;
pub use tokens::{authenticate_device, hash_token, verify_token};

/// Map a row of `(endpoint_id, device_kind, approval_state, display_name)` into
/// a [`DeviceRecord`]. Shared by the device list and single-device fetch so
/// both surface the same human-friendly name resolution.
pub(crate) fn device_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceRecord> {
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
pub(crate) const DEVICE_SELECT_WITH_NAME: &str =
    "SELECT d.endpoint_id, d.device_kind, d.approval_state,
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

pub(crate) fn sql_u64_to_i64(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
}

pub(crate) fn sql_i64_to_u64(value: i64, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|err: std::num::TryFromIntError| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(err),
        )
    })
}

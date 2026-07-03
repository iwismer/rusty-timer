use chrono::Utc;
use rusqlite::{Connection, params};
use serde::Serialize;

use super::{ApprovalState, sql_i64_to_u64, sql_u64_to_i64};

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

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::registry::{DeviceKind, approve_device, get_device, migrate, register_device};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
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
        assert_eq!(
            device.approval_state,
            crate::registry::ApprovalState::Pending
        );

        let forwarders = list_forwarders(&conn).unwrap();
        assert_eq!(forwarders.len(), 1);
        assert_eq!(forwarders[0].endpoint_id, "ep-fwd");
        assert_eq!(forwarders[0].display_name.as_deref(), Some("Start Line"));
        assert_eq!(forwarders[0].direct_addrs, vec!["127.0.0.1:12345"]);
        assert_eq!(
            forwarders[0].approval_state,
            crate::registry::ApprovalState::Pending
        );
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
}

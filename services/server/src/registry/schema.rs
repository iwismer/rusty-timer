use rusqlite::Connection;

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
             revoked_unix_ms INTEGER,
             expires_unix_ms INTEGER
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
    // NULLs as distinct, so pre-mint rows coexist freely. The index creation is
    // deliberately outside the column gate so a partially applied migration
    // (column without index) self-heals on the next open.
    if !column_exists(conn, "devices", "token_id")? {
        conn.execute_batch("ALTER TABLE devices ADD COLUMN token_id TEXT;")?;
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_token_id ON devices(token_id);",
    )?;

    // Enrollment voucher expiry. NULL = no expiry (legacy rows created before
    // the TTL was introduced).
    if !column_exists(conn, "enrollment_tokens", "expires_unix_ms")? {
        conn.execute_batch("ALTER TABLE enrollment_tokens ADD COLUMN expires_unix_ms INTEGER;")?;
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
    use rusqlite::Connection;

    use super::*;

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

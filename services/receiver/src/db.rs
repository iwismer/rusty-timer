use rt_protocol::{ReceiverMode, ResumeCursor};
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
const SCHEMA_SQL: &str = include_str!("storage/schema.sql");
pub const DEFAULT_UPDATE_MODE: &str = "check-and-download";
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
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRecord {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub stream_epoch: i64,
    pub last_seq: i64,
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
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM profile")?;
        tx.execute(
            "INSERT INTO profile (server_url, token, update_mode, receiver_mode_json, receiver_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![url, tok, update_mode, receiver_mode_json, receiver_id],
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

    pub fn load_earliest_epochs(&self) -> DbResult<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT forwarder_id, reader_ip, earliest_epoch FROM earliest_epochs ORDER BY forwarder_id, reader_ip",
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

    pub fn save_earliest_epoch(&self, fwd: &str, ip: &str, epoch: i64) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO earliest_epochs (forwarder_id, reader_ip, earliest_epoch) VALUES (?1, ?2, ?3)",
            rusqlite::params![fwd, ip, epoch],
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
    pub fn load_subscriptions(&self) -> DbResult<Vec<Subscription>> {
        let mut s = self.conn.prepare("SELECT forwarder_id, reader_ip, local_port_override FROM subscriptions ORDER BY forwarder_id, reader_ip")?;
        let rows = s.query_map([], |r| {
            Ok(Subscription {
                forwarder_id: r.get(0)?,
                reader_ip: r.get(1)?,
                local_port_override: r.get::<_, Option<i64>>(2)?.map(|p| p as u16),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
    pub fn save_subscription(&self, fwd: &str, ip: &str, port: Option<u16>) -> DbResult<()> {
        self.conn.execute("INSERT OR IGNORE INTO subscriptions (forwarder_id, reader_ip, local_port_override) VALUES (?1, ?2, ?3)", rusqlite::params![fwd, ip, port.map(|p| p as i64)])?;
        Ok(())
    }
    pub fn replace_subscriptions(&mut self, subs: &[Subscription]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM subscriptions")?;
        for s in subs {
            tx.execute("INSERT INTO subscriptions (forwarder_id, reader_ip, local_port_override) VALUES (?1, ?2, ?3)", rusqlite::params![&s.forwarder_id, &s.reader_ip, s.local_port_override.map(|p| p as i64)])?;
        }
        tx.commit()?;
        Ok(())
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
        let mut s = self.conn.prepare("SELECT forwarder_id, reader_ip, stream_epoch, acked_through_seq FROM cursors ORDER BY forwarder_id, reader_ip")?;
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
    pub fn save_cursor(&self, fwd: &str, ip: &str, epoch: i64, seq: i64) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (forwarder_id, reader_ip) DO UPDATE SET
                 stream_epoch = ?3,
                 acked_through_seq = ?4
             WHERE excluded.stream_epoch > cursors.stream_epoch
                OR (excluded.stream_epoch = cursors.stream_epoch AND excluded.acked_through_seq > cursors.acked_through_seq)",
            rusqlite::params![fwd, ip, epoch, seq],
        )?;
        Ok(())
    }
    pub fn delete_cursor(&self, fwd: &str, ip: &str) -> DbResult<()> {
        self.conn.execute(
            "DELETE FROM cursors WHERE forwarder_id = ?1 AND reader_ip = ?2",
            rusqlite::params![fwd, ip],
        )?;
        Ok(())
    }
    fn apply_pragmas(&self) -> DbResult<()> {
        self.conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=1000; PRAGMA foreign_keys=ON;")?;
        Ok(())
    }
    fn apply_schema(&self) -> DbResult<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        // Migration: add update_mode column to existing profile tables.
        apply_profile_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN update_mode TEXT NOT NULL DEFAULT 'check-and-download';",
            "update_mode",
        )?;
        apply_profile_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN receiver_mode_json TEXT;",
            "receiver_mode_json",
        )?;
        apply_profile_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN receiver_id TEXT;",
            "receiver_id",
        )?;
        Ok(())
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
            "UPDATE subscriptions SET local_port_override = ?1 WHERE forwarder_id = ?2 AND reader_ip = ?3",
            rusqlite::params![port.map(|p| p as i64), fwd, ip],
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

    pub fn factory_reset(&mut self) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM cursors")?;
        tx.execute_batch("DELETE FROM earliest_epochs")?;
        tx.execute_batch("DELETE FROM subscriptions")?;
        tx.execute_batch("DELETE FROM profile")?;
        tx.execute(
            "INSERT INTO profile (server_url, token, update_mode) VALUES ('', '', ?1)",
            rusqlite::params![DEFAULT_UPDATE_MODE],
        )?;
        tx.commit()?;
        Ok(())
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

fn apply_profile_column_migration(conn: &Connection, sql: &str, column_name: &str) -> DbResult<()> {
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

fn is_duplicate_column_error(message: &str, column_name: &str) -> bool {
    message.contains(&format!("duplicate column name: {column_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_round_trip_with_update_mode() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-only", None)
            .unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.update_mode, "check-only");
    }

    #[test]
    fn profile_update_mode_defaults_for_existing_db() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-and-download", None)
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
    fn receiver_mode_round_trip() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-and-download", None)
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
        db.save_profile("wss://example.com", "tok", "check-and-download", None)
            .unwrap();
        let targeted = ReceiverMode::TargetedReplay {
            targets: vec![rt_protocol::ReplayTarget {
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
        db.save_profile("wss://example.com", "tok", "check-and-download", None)
            .unwrap();
        db.conn
            .execute(
                "UPDATE profile SET receiver_mode_json = ?1",
                rusqlite::params!["{invalid-json"],
            )
            .unwrap();

        let result = db.save_profile("wss://example.org", "tok-2", "check-only", None);
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
        db.save_profile("wss://example.com", "tok", "check-only", Some("recv-old"))
            .unwrap();
        db.save_receiver_id("recv-new").unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, Some("recv-new".to_owned()));
        assert_eq!(p.server_url, "wss://example.com");
        assert_eq!(p.token, "tok");
        assert_eq!(p.update_mode, "check-only");
    }

    #[test]
    fn save_profile_round_trips_receiver_id() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile(
            "wss://s.com",
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
        db.save_profile("wss://s.com", "t", "check-and-download", None)
            .unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, None);
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
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
        db.save_subscription("f2", "10.0.0.2", Some(9000)).unwrap();
        let count = db.delete_all_subscriptions().unwrap();
        assert_eq!(count, 2);
        assert!(db.load_subscriptions().unwrap().is_empty());
    }

    #[test]
    fn reset_profile_clears_to_defaults() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile(
            "wss://example.com",
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
        db.save_profile("wss://example.com", "tok", "check-only", Some("recv-1"))
            .unwrap();
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 7, 42).unwrap();
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
        db.factory_reset().unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.server_url, "");
        assert_eq!(p.token, "");
        assert_eq!(p.receiver_id, None);
        assert!(db.load_subscriptions().unwrap().is_empty());
        assert!(db.load_cursors().unwrap().is_empty());
        assert!(db.load_earliest_epochs().unwrap().is_empty());
    }

    #[test]
    fn update_subscription_port_changes_existing() {
        let db = Db::open_in_memory().unwrap();
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
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
        db.save_subscription("f1", "10.0.0.1", Some(9000)).unwrap();
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
}

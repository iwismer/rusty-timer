use rt_protocol::{ReceiverMode, ResumeCursor};
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
const SCHEMA_SQL: &str = include_str!("storage/schema.sql");
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
    pub stream_epoch: u64,
    pub last_seq: u64,
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
            .prepare("SELECT server_url, token, update_mode FROM profile LIMIT 1")?;
        let mut rows = s.query_map([], |r| {
            Ok(Profile {
                server_url: r.get(0)?,
                token: r.get(1)?,
                update_mode: r.get(2)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }
    pub fn save_profile(&self, url: &str, tok: &str, update_mode: &str) -> DbResult<()> {
        let receiver_mode_json = self
            .load_receiver_mode()?
            .map(|mode| serde_json::to_string(&mode))
            .transpose()?;
        self.conn.execute_batch("DELETE FROM profile")?;
        self.conn.execute(
            "INSERT INTO profile (server_url, token, update_mode, receiver_mode_json) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![url, tok, update_mode, receiver_mode_json],
        )?;
        Ok(())
    }

    pub fn load_receiver_mode(&self) -> DbResult<Option<ReceiverMode>> {
        let raw: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT receiver_mode_json FROM profile LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(Some(raw_json)) = raw else {
            return Ok(None);
        };
        if raw_json.trim().is_empty() {
            return Ok(None);
        }
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
                stream_epoch: r.get::<_, i64>(2)? as u64,
                last_seq: r.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
    pub fn save_cursor(&self, fwd: &str, ip: &str, epoch: u64, seq: u64) -> DbResult<()> {
        let existing: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT stream_epoch, acked_through_seq FROM cursors WHERE forwarder_id = ?1 AND reader_ip = ?2",
                rusqlite::params![fwd, ip],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;

        if let Some((current_epoch, current_seq)) = existing {
            let new_epoch = epoch as i64;
            let new_seq = seq as i64;
            let is_stale =
                new_epoch < current_epoch || (new_epoch == current_epoch && new_seq < current_seq);
            if is_stale {
                return Ok(());
            }
        }

        self.conn.execute("INSERT OR REPLACE INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![fwd, ip, epoch as i64, seq as i64])?;
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
        Ok(())
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
        let db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-only")
            .unwrap();
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.update_mode, "check-only");
    }

    #[test]
    fn profile_update_mode_defaults_for_existing_db() {
        let db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-and-download")
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
        let db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-and-download")
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
        let db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-and-download")
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
}

use rt_protocol::{
    ReceiverSelection, ReceiverSetSelection, ReplayPolicy, ReplayTarget, ResumeCursor,
};
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
    #[error("Invalid receiver selection: {0}")]
    InvalidReceiverSelection(String),
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

fn default_selection() -> ReceiverSelection {
    ReceiverSelection::Manual {
        streams: Vec::new(),
    }
}

fn default_selection_json() -> String {
    serde_json::to_string(&default_selection()).expect("manual selection is serializable")
}

fn default_replay_policy() -> ReplayPolicy {
    ReplayPolicy::Resume
}

fn replay_policy_to_string(policy: ReplayPolicy) -> String {
    serde_json::to_string(&policy)
        .unwrap_or_else(|_| "\"resume\"".to_owned())
        .trim_matches('"')
        .to_owned()
}

fn parse_replay_policy(raw: &str) -> ReplayPolicy {
    serde_json::from_str::<ReplayPolicy>(&format!("\"{raw}\""))
        .unwrap_or_else(|_| default_replay_policy())
}

fn normalize_receiver_selection(
    selection: ReceiverSetSelection,
) -> Result<ReceiverSetSelection, DbError> {
    let replay_targets =
        match selection.replay_policy {
            ReplayPolicy::Targeted => match selection.replay_targets {
                Some(targets) if !targets.is_empty() => Some(targets),
                _ => return Err(DbError::InvalidReceiverSelection(
                    "replay_targets must be present and non-empty when replay_policy is targeted"
                        .to_owned(),
                )),
            },
            _ => None,
        };

    Ok(ReceiverSetSelection {
        selection: selection.selection,
        replay_policy: selection.replay_policy,
        replay_targets,
    })
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
        let existing_selection = self.load_receiver_selection_raw()?;
        let (selection_json, replay_policy, replay_targets_json) = existing_selection
            .map(|s| (s.selection_json, s.replay_policy, s.replay_targets_json))
            .unwrap_or_else(|| (default_selection_json(), "resume".to_owned(), None));
        self.conn.execute_batch("DELETE FROM profile")?;
        self.conn.execute(
            "INSERT INTO profile (server_url, token, update_mode, selection_json, replay_policy, replay_targets_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![url, tok, update_mode, selection_json, replay_policy, replay_targets_json],
        )?;
        Ok(())
    }
    pub fn load_receiver_selection(&self) -> DbResult<ReceiverSetSelection> {
        let Some(raw) = self.load_receiver_selection_raw()? else {
            return Ok(ReceiverSetSelection {
                selection: default_selection(),
                replay_policy: default_replay_policy(),
                replay_targets: None,
            });
        };

        let selection = serde_json::from_str::<ReceiverSelection>(&raw.selection_json)
            .unwrap_or_else(|_| default_selection());
        let replay_policy = parse_replay_policy(&raw.replay_policy);
        let replay_targets = raw
            .replay_targets_json
            .as_deref()
            .map(serde_json::from_str::<Vec<ReplayTarget>>)
            .transpose()?;

        normalize_receiver_selection(ReceiverSetSelection {
            selection,
            replay_policy,
            replay_targets,
        })
    }
    pub fn save_receiver_selection(&self, selection: &ReceiverSetSelection) -> DbResult<()> {
        let normalized = normalize_receiver_selection(selection.clone())?;
        let selection_json = serde_json::to_string(&normalized.selection)?;
        let replay_policy = replay_policy_to_string(normalized.replay_policy);
        let replay_targets_json = normalized
            .replay_targets
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        let updated = self.conn.execute(
            "UPDATE profile SET selection_json = ?1, replay_policy = ?2, replay_targets_json = ?3",
            rusqlite::params![selection_json, replay_policy, replay_targets_json],
        )?;
        if updated == 0 {
            return Err(DbError::ProfileMissing);
        }
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
            "ALTER TABLE profile ADD COLUMN selection_json TEXT NOT NULL DEFAULT '{\"mode\":\"manual\",\"streams\":[]}';",
            "selection_json",
        )?;
        apply_profile_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN replay_policy TEXT NOT NULL DEFAULT 'resume';",
            "replay_policy",
        )?;
        apply_profile_column_migration(
            &self.conn,
            "ALTER TABLE profile ADD COLUMN replay_targets_json TEXT;",
            "replay_targets_json",
        )?;
        Ok(())
    }

    fn load_receiver_selection_raw(&self) -> DbResult<Option<ReceiverSelectionRow>> {
        self.conn
            .query_row(
                "SELECT selection_json, replay_policy, replay_targets_json FROM profile LIMIT 1",
                [],
                |row| {
                    Ok(ReceiverSelectionRow {
                        selection_json: row.get(0)?,
                        replay_policy: row.get(1)?,
                        replay_targets_json: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
    }
}

#[derive(Debug)]
struct ReceiverSelectionRow {
    selection_json: String,
    replay_policy: String,
    replay_targets_json: Option<String>,
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
    fn selection_defaults_to_manual_resume() {
        let db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-and-download")
            .unwrap();
        let selection = db.load_receiver_selection().unwrap();
        assert_eq!(
            selection.selection,
            ReceiverSelection::Manual {
                streams: Vec::new()
            }
        );
        assert_eq!(selection.replay_policy, ReplayPolicy::Resume);
        assert!(selection.replay_targets.is_none());
    }

    #[test]
    fn load_receiver_selection_rejects_targeted_without_targets() {
        let db = Db::open_in_memory().unwrap();
        db.save_profile("wss://example.com", "tok", "check-and-download")
            .unwrap();
        db.conn
            .execute(
                "UPDATE profile SET replay_policy = 'targeted', replay_targets_json = NULL",
                [],
            )
            .unwrap();

        let err = db.load_receiver_selection().unwrap_err();
        assert!(matches!(err, DbError::InvalidReceiverSelection(_)));
    }
}

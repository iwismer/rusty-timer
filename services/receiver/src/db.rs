use rt_protocol::ResumeCursor;
use rusqlite::Connection;
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
}
pub type DbResult<T> = Result<T, DbError>;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub server_url: String,
    pub token: String,
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
            .prepare("SELECT server_url, token FROM profile LIMIT 1")?;
        let mut rows = s.query_map([], |r| {
            Ok(Profile {
                server_url: r.get(0)?,
                token: r.get(1)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }
    pub fn save_profile(&self, url: &str, tok: &str) -> DbResult<()> {
        self.conn.execute_batch("DELETE FROM profile")?;
        self.conn.execute(
            "INSERT INTO profile (server_url, token) VALUES (?1, ?2)",
            rusqlite::params![url, tok],
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
        self.conn.execute("INSERT OR REPLACE INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![fwd, ip, epoch as i64, seq as i64])?;
        Ok(())
    }
    fn apply_pragmas(&self) -> DbResult<()> {
        self.conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=1000; PRAGMA foreign_keys=ON;")?;
        Ok(())
    }
    fn apply_schema(&self) -> DbResult<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }
}

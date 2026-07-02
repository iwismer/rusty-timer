//! Hand-rolled read-only SQLite connection pool for hot readers.
//!
//! WAL readers never block the writer (and vice versa), so proxy replay,
//! projection rebuilds, and status reads can run concurrently with group
//! commits instead of serializing on the cold `Arc<Mutex<Db>>` connection.
//!
//! Opening gotcha: a read-only connection cannot open a WAL database until a
//! read-write connection has created the `-shm`/`-wal` sidecars. Production
//! ordering guarantees this (`Db::open` runs before [`ReadPool::open`]).

use std::path::Path;
use std::sync::Arc;

use crate::db::{Db, DbError, DbResult};

/// Fixed-size pool of read-only connections handed out via a semaphore.
pub struct ReadPool {
    /// Idle connections, wrapped in [`Db`] so callers use ordinary query
    /// methods. Guarded by the semaphore: a permit guarantees a connection.
    conns: std::sync::Mutex<Vec<Db>>,
    semaphore: tokio::sync::Semaphore,
}

impl ReadPool {
    /// Open `size` read-only connections against `db_path`.
    pub fn open(db_path: &Path, size: usize) -> DbResult<Arc<Self>> {
        let size = size.max(1);
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = rusqlite::Connection::open_with_flags(
                db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            conn.execute_batch("PRAGMA query_only=1; PRAGMA busy_timeout=10000;")?;
            conns.push(Db::from_read_only_connection(conn));
        }
        Ok(Arc::new(Self {
            conns: std::sync::Mutex::new(conns),
            semaphore: tokio::sync::Semaphore::new(size),
        }))
    }

    /// Run a read-only closure on a pooled connection via `spawn_blocking`.
    pub async fn run<T, F>(self: &Arc<Self>, f: F) -> DbResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Db) -> DbResult<T> + Send + 'static,
    {
        let permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| DbError::IntegrityCheckFailed("read pool closed".to_owned()))?;
        let conn = self
            .conns
            .lock()
            .expect("read pool mutex poisoned")
            .pop()
            .expect("semaphore permit guarantees an idle connection");
        // Return the connection on drop, so a panicking closure cannot leak
        // it and break the permit⇔connection accounting (the next `run`
        // would otherwise panic on an empty pool despite holding a permit).
        struct ReturnOnDrop {
            pool: Arc<ReadPool>,
            conn: Option<Db>,
        }
        impl Drop for ReturnOnDrop {
            fn drop(&mut self) {
                if let Some(conn) = self.conn.take() {
                    self.pool
                        .conns
                        .lock()
                        .expect("read pool mutex poisoned")
                        .push(conn);
                }
            }
        }
        let guard = ReturnOnDrop {
            pool: Arc::clone(self),
            conn: Some(conn),
        };
        let result = tokio::task::spawn_blocking(move || {
            let guard = guard;
            f(guard.conn.as_ref().expect("connection present until drop"))
        })
        .await
        .map_err(|e| DbError::IntegrityCheckFailed(format!("read task join error: {e}")))?;
        drop(permit);
        result
    }
}

/// Read access for hot paths: pooled read-only connections in production, or
/// the shared mutex connection where no file-backed pool exists (in-memory
/// test states).
#[derive(Clone)]
pub enum ReadSource {
    Pool(Arc<ReadPool>),
    Mutex(Arc<tokio::sync::Mutex<Db>>),
}

impl std::fmt::Debug for ReadPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadPool").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ReadSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadSource::Pool(_) => f.write_str("ReadSource::Pool"),
            ReadSource::Mutex(_) => f.write_str("ReadSource::Mutex"),
        }
    }
}

impl ReadSource {
    /// Run a read-only closure against this source.
    pub async fn run<T, F>(&self, f: F) -> DbResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Db) -> DbResult<T> + Send + 'static,
    {
        match self {
            ReadSource::Pool(pool) => pool.run(f).await,
            ReadSource::Mutex(db) => {
                let db = db.lock().await;
                f(&db)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_reads_proceed_while_writer_holds_a_write_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool-test.sqlite3");
        // Read-write open first: creates the schema and the WAL sidecars the
        // read-only pool needs.
        let mut db = Db::open(&path).unwrap();
        db.insert_received_event(&crate::db::ReceivedEventInsert {
            stream_id: "s1",
            seq: 1,
            epoch: 1,
            raw_frame: b"frame",
            read_kind: "raw",
            reader_timestamp: None,
            received_unix_ms: 1,
            dbf_delivered_unix_ms: None,
            chip_id: None,
        })
        .unwrap();

        let pool = ReadPool::open(&path, 2).unwrap();

        // Hold an open write transaction while both reads run.
        let tx = db.transaction().unwrap();
        crate::db::insert_received_event_conn(
            &tx,
            &crate::db::ReceivedEventInsert {
                stream_id: "s1",
                seq: 2,
                epoch: 1,
                raw_frame: b"uncommitted",
                read_kind: "raw",
                reader_timestamp: None,
                received_unix_ms: 2,
                dbf_delivered_unix_ms: None,
                chip_id: None,
            },
        )
        .unwrap();

        let read = |pool: &Arc<ReadPool>| {
            let pool = Arc::clone(pool);
            async move {
                pool.run(|db| db.load_received_events("s1"))
                    .await
                    .unwrap()
                    .len()
            }
        };
        let (a, b) = tokio::time::timeout(
            Duration::from_secs(5),
            futures_util::future::join(read(&pool), read(&pool)),
        )
        .await
        .expect("reads must not block on the open write transaction");
        assert_eq!(a, 1, "snapshot excludes the uncommitted row");
        assert_eq!(b, 1);

        tx.commit().unwrap();
        let after = pool
            .run(|db| db.load_received_events("s1"))
            .await
            .unwrap()
            .len();
        assert_eq!(after, 2, "committed row visible to subsequent reads");
    }

    #[tokio::test]
    async fn panicking_read_closure_does_not_leak_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool-panic.sqlite3");
        drop(Db::open(&path).unwrap());
        let pool = ReadPool::open(&path, 1).unwrap();

        let panicked = pool
            .run(|_db| -> DbResult<()> { panic!("boom in read closure") })
            .await;
        assert!(panicked.is_err(), "the panic surfaces as a join error");

        // The single pooled connection was returned by the drop guard: the
        // next read must succeed instead of panicking on an empty pool.
        let rows = pool
            .run(|db| db.load_received_events("s1"))
            .await
            .expect("pool usable after a panicking closure");
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn pool_writes_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool-ro.sqlite3");
        drop(Db::open(&path).unwrap());
        let pool = ReadPool::open(&path, 1).unwrap();
        let result = pool
            .run(|db| {
                db.insert_received_event(&crate::db::ReceivedEventInsert {
                    stream_id: "s1",
                    seq: 1,
                    epoch: 1,
                    raw_frame: b"x",
                    read_kind: "raw",
                    reader_timestamp: None,
                    received_unix_ms: 1,
                    dbf_delivered_unix_ms: None,
                    chip_id: None,
                })
            })
            .await;
        assert!(result.is_err(), "read-only pool must reject writes");
    }
}

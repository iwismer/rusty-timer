use receiver::stream_key::LocalStreamKey;
use rt_domain::ReceiverMode;
use rusqlite::Connection;
use std::path::Path;

const SCHEMA_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/storage/schema.sql");

fn open_db(path: &Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=1000; PRAGMA foreign_keys=ON;").unwrap();
    c.execute_batch(&std::fs::read_to_string(SCHEMA_PATH).unwrap())
        .unwrap();
    c
}

#[test]
fn profile_stored_and_loaded() {
    let d = tempfile::tempdir().unwrap();
    let c = open_db(&d.path().join("r.db"));
    c.execute(
        "INSERT INTO profile (server_url, token) VALUES (?1, ?2)",
        rusqlite::params!["https://thin.test", "t"],
    )
    .unwrap();
    let url: String = c
        .query_row("SELECT server_url FROM profile", [], |r| r.get(0))
        .unwrap();
    assert_eq!(url, "https://thin.test");
}

#[test]
fn profile_schema_drops_legacy_selection_columns() {
    let d = tempfile::tempdir().unwrap();
    let c = open_db(&d.path().join("r.db"));

    let mut stmt = c
        .prepare("SELECT name FROM pragma_table_info('profile') ORDER BY cid")
        .unwrap();
    let column_names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(!column_names.iter().any(|col| col == "selection_json"));
    assert!(!column_names.iter().any(|col| col == "replay_policy"));
    assert!(!column_names.iter().any(|col| col == "replay_targets_json"));
    assert!(column_names.iter().any(|col| col == "receiver_mode_json"));
}

#[test]
fn receiver_mode_persists_via_receiver_db() {
    let d = tempfile::tempdir().unwrap();
    let db_path = d.path().join("receiver.db");

    let mut db = receiver::db::Db::open(&db_path).unwrap();
    db.save_profile("https://persist.example", "tok", "check-and-download", None)
        .unwrap();
    db.save_receiver_mode(&ReceiverMode::Race {
        race_id: "race-1".to_owned(),
    })
    .unwrap();

    let reopened = receiver::db::Db::open(&db_path).unwrap();
    assert_eq!(
        reopened.load_receiver_mode().unwrap(),
        Some(ReceiverMode::Race {
            race_id: "race-1".to_owned()
        })
    );
}

#[test]
fn earliest_epochs_survive_reopen() {
    let d = tempfile::tempdir().unwrap();
    let db_path = d.path().join("receiver.db");

    {
        let db = receiver::db::Db::open(&db_path).unwrap();
        db.save_stream_earliest_epoch("endpoint-1", "10.0.0.1", 3)
            .unwrap();
    }

    let reopened = receiver::db::Db::open(&db_path).unwrap();
    assert_eq!(
        reopened.load_stream_earliest_epochs().unwrap(),
        vec![receiver::db::StreamEarliestEpoch {
            stream_id: LocalStreamKey::new("endpoint-1", "10.0.0.1")
                .as_str()
                .to_owned(),
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            earliest_epoch: 3,
        }]
    );
}

#[test]
fn profile_persists_across_db_reopen() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("r.db");
    {
        let c = open_db(&p);
        c.execute(
            "INSERT INTO profile (server_url, token) VALUES (?1, ?2)",
            rusqlite::params!["https://p.com", "t"],
        )
        .unwrap();
    }
    let c = Connection::open(&p).unwrap();
    let url: String = c
        .query_row("SELECT server_url FROM profile", [], |r| r.get(0))
        .unwrap();
    assert_eq!(url, "https://p.com");
}

#[test]
fn subscriptions_stored_and_loaded() {
    let d = tempfile::tempdir().unwrap();
    let c = open_db(&d.path().join("r.db"));
    c.execute(
        "INSERT INTO subscriptions (forwarder_endpoint_id, stream_id, forwarder_id, reader_ip) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["endpoint-1", "stream-100", "f", "192.168.1.100"],
    )
    .unwrap();
    c.execute(
        "INSERT INTO subscriptions (forwarder_endpoint_id, stream_id, forwarder_id, reader_ip) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["endpoint-1", "stream-200", "f", "192.168.1.200"],
    )
    .unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM subscriptions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn cursor_stored_and_loaded() {
    let d = tempfile::tempdir().unwrap();
    let c = open_db(&d.path().join("r.db"));
    c.execute(
        "INSERT INTO cursors (stream_id, stream_epoch, last_seq) VALUES (?1, ?2, ?3)",
        rusqlite::params!["stream-1", 3i64, 17i64],
    )
    .unwrap();
    let (e, s): (i64, i64) = c
        .query_row(
            "SELECT stream_epoch, last_seq FROM cursors WHERE stream_id='stream-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(e, 3);
    assert_eq!(s, 17);
}

use std::path::Path;
use rusqlite::Connection;
use rt_protocol::*;
use rt_test_utils::MockWsServer;

const SCHEMA_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/storage/schema.sql");
fn open_db(path: &Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=1000; PRAGMA foreign_keys=ON;").unwrap();
    c.execute_batch(&std::fs::read_to_string(SCHEMA_PATH).unwrap()).unwrap(); c
}
#[test] fn profile_stored_and_loaded() {
    let d = tempfile::tempdir().unwrap(); let c = open_db(&d.path().join("r.db"));
    c.execute("INSERT INTO profile (server_url, token) VALUES (?1, ?2)", rusqlite::params!["wss://s.com","t"]).unwrap();
    let url: String = c.query_row("SELECT server_url FROM profile", [], |r| r.get(0)).unwrap();
    assert_eq!(url,"wss://s.com");
}
#[test] fn profile_persists_across_db_reopen() {
    let d = tempfile::tempdir().unwrap(); let p = d.path().join("r.db");
    { let c = open_db(&p); c.execute("INSERT INTO profile (server_url, token) VALUES (?1, ?2)", rusqlite::params!["wss://p.com","t"]).unwrap(); }
    let c = Connection::open(&p).unwrap();
    let url: String = c.query_row("SELECT server_url FROM profile", [], |r| r.get(0)).unwrap();
    assert_eq!(url,"wss://p.com");
}
#[test] fn subscriptions_stored_and_loaded() {
    let d = tempfile::tempdir().unwrap(); let c = open_db(&d.path().join("r.db"));
    c.execute("INSERT INTO subscriptions (forwarder_id, reader_ip) VALUES (?1, ?2)", rusqlite::params!["f","192.168.1.100"]).unwrap();
    c.execute("INSERT INTO subscriptions (forwarder_id, reader_ip) VALUES (?1, ?2)", rusqlite::params!["f","192.168.1.200"]).unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM subscriptions", [], |r| r.get(0)).unwrap();
    assert_eq!(n,2);
}
#[test] fn subscriptions_survive_reopen() {
    let d = tempfile::tempdir().unwrap(); let p = d.path().join("r.db");
    { let c = open_db(&p); c.execute("INSERT INTO subscriptions (forwarder_id, reader_ip) VALUES (?1, ?2)", rusqlite::params!["f","10.0.0.1"]).unwrap(); }
    let c = Connection::open(&p).unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM subscriptions WHERE forwarder_id='f'", [], |r| r.get(0)).unwrap();
    assert_eq!(n,1);
}
#[test] fn cursor_stored_and_loaded() {
    let d = tempfile::tempdir().unwrap(); let c = open_db(&d.path().join("r.db"));
    c.execute("INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params!["f","i",3i64,17i64]).unwrap();
    let (e,s): (i64,i64) = c.query_row("SELECT stream_epoch, acked_through_seq FROM cursors WHERE forwarder_id='f' AND reader_ip='i'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(e,3); assert_eq!(s,17);
}
#[test] fn cursor_survives_reopen() {
    let d = tempfile::tempdir().unwrap(); let p = d.path().join("r.db");
    { let c = open_db(&p); c.execute("INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params!["f","i",2i64,1024i64]).unwrap(); }
    let c = Connection::open(&p).unwrap();
    let s: i64 = c.query_row("SELECT acked_through_seq FROM cursors WHERE forwarder_id='f' AND reader_ip='i'", [], |r| r.get(0)).unwrap();
    assert_eq!(s,1024);
}
#[test] fn cursor_upsert_advances_position() {
    let d = tempfile::tempdir().unwrap(); let c = open_db(&d.path().join("r.db"));
    c.execute("INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params!["f","i",1i64,5i64]).unwrap();
    c.execute("INSERT OR REPLACE INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params!["f","i",1i64,25i64]).unwrap();
    let s: i64 = c.query_row("SELECT acked_through_seq FROM cursors WHERE forwarder_id='f' AND reader_ip='i'", [], |r| r.get(0)).unwrap();
    assert_eq!(s,25);
}
#[test] fn multiple_cursors_for_different_streams() {
    let d = tempfile::tempdir().unwrap(); let c = open_db(&d.path().join("r.db"));
    for (f,i,e,s) in [("f","i1",1i64,100i64),("f","i2",2,50),("f2","i1",1,200)] { c.execute("INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![f,i,e,s]).unwrap(); }
    let n: i64 = c.query_row("SELECT COUNT(*) FROM cursors", [], |r| r.get(0)).unwrap();
    assert_eq!(n,3);
}
#[test] fn load_resume_cursors_from_db() {
    let d = tempfile::tempdir().unwrap(); let c = open_db(&d.path().join("r.db"));
    c.execute("INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params!["f","i1",3i64,150i64]).unwrap();
    c.execute("INSERT INTO cursors (forwarder_id, reader_ip, stream_epoch, acked_through_seq) VALUES (?1, ?2, ?3, ?4)", rusqlite::params!["f","i2",1i64,40i64]).unwrap();
    let mut stmt = c.prepare("SELECT forwarder_id, reader_ip, stream_epoch, acked_through_seq FROM cursors").unwrap();
    let loaded: Vec<ResumeCursor> = stmt.query_map([], |r| Ok(ResumeCursor{forwarder_id:r.get(0)?,reader_ip:r.get(1)?,stream_epoch:r.get::<_,i64>(2)? as u64,last_seq:r.get::<_,i64>(3)? as u64})).unwrap().map(|r| r.unwrap()).collect();
    assert_eq!(loaded.len(),2);
}
#[tokio::test] async fn ws_session_sends_receiver_hello_with_resume_cursors() {
    use rt_test_utils::MockWsClient;
    let s = MockWsServer::start().await.unwrap();
    let mut c = MockWsClient::connect(&format!("ws://{}",s.local_addr())).await.unwrap();
    c.send_message(&WsMessage::ReceiverHello(ReceiverHello{receiver_id:"rcv-001".to_owned(),resume:vec![ResumeCursor{forwarder_id:"f".to_owned(),reader_ip:"i".to_owned(),stream_epoch:2,last_seq:99}]})).await.unwrap();
    match c.recv_message().await.unwrap() { WsMessage::Heartbeat(h) => { assert!(!h.session_id.is_empty()); assert_eq!(h.device_id,"rcv-001"); } other => panic!("{:?}",other) }
}
#[tokio::test] async fn ws_session_sends_receiver_hello_empty_resume_on_first_connect() {
    use rt_test_utils::MockWsClient;
    let s = MockWsServer::start().await.unwrap();
    let mut c = MockWsClient::connect(&format!("ws://{}",s.local_addr())).await.unwrap();
    c.send_message(&WsMessage::ReceiverHello(ReceiverHello{receiver_id:"rcv-fresh".to_owned(),resume:vec![]})).await.unwrap();
    assert!(matches!(c.recv_message().await.unwrap(), WsMessage::Heartbeat(_)));
}

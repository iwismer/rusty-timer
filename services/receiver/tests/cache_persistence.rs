use receiver::{Db, Subscription};

#[test]
fn db_opens_in_memory() {
    Db::open_in_memory().unwrap();
}
#[test]
fn db_integrity_check_passes_on_fresh_db() {
    assert!(Db::open_in_memory().unwrap().integrity_check().is_ok());
}
#[test]
fn profile_save_and_load() {
    let db = Db::open_in_memory().unwrap();
    db.save_profile("wss://e.com", "t", "info").unwrap();
    let p = db.load_profile().unwrap().unwrap();
    assert_eq!(p.server_url, "wss://e.com");
    assert_eq!(p.token, "t");
    assert_eq!(p.log_level, "info");
}
#[test]
fn profile_update_replaces_existing() {
    let db = Db::open_in_memory().unwrap();
    db.save_profile("wss://old", "old", "debug").unwrap();
    db.save_profile("wss://new", "new", "warn").unwrap();
    assert_eq!(db.load_profile().unwrap().unwrap().server_url, "wss://new");
}
#[test]
fn profile_load_returns_none_when_empty() {
    assert!(Db::open_in_memory()
        .unwrap()
        .load_profile()
        .unwrap()
        .is_none());
}
#[test]
fn subscriptions_save_and_load() {
    let db = Db::open_in_memory().unwrap();
    db.save_subscription("f", "192.168.1.100", Some(10100))
        .unwrap();
    db.save_subscription("f", "192.168.1.200", None).unwrap();
    let s = db.load_subscriptions().unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(
        s.iter()
            .find(|x| x.reader_ip == "192.168.1.100")
            .unwrap()
            .local_port_override,
        Some(10100)
    );
}
#[test]
fn subscriptions_replace_all_replaces_existing() {
    let db = Db::open_in_memory().unwrap();
    db.save_subscription("f", "192.168.1.100", None).unwrap();
    db.replace_subscriptions(&[Subscription {
        forwarder_id: "f2".to_owned(),
        reader_ip: "10.0.0.1".to_owned(),
        local_port_override: Some(9900),
    }])
    .unwrap();
    let s = db.load_subscriptions().unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].forwarder_id, "f2");
}
#[test]
fn cursor_save_and_load() {
    let db = Db::open_in_memory().unwrap();
    db.save_cursor("f", "192.168.1.100", 3, 99).unwrap();
    let c = db.load_cursors().unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].stream_epoch, 3);
    assert_eq!(c[0].last_seq, 99);
}
#[test]
fn cursor_upsert_advances_position() {
    let db = Db::open_in_memory().unwrap();
    db.save_cursor("f", "192.168.1.100", 1, 10).unwrap();
    db.save_cursor("f", "192.168.1.100", 1, 50).unwrap();
    let c = db.load_cursors().unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].last_seq, 50);
}
#[test]
fn cursor_epoch_advance() {
    let db = Db::open_in_memory().unwrap();
    db.save_cursor("f", "192.168.1.100", 1, 100).unwrap();
    db.save_cursor("f", "192.168.1.100", 2, 5).unwrap();
    assert_eq!(db.load_cursors().unwrap()[0].stream_epoch, 2);
}
#[test]
fn cursors_as_resume_list() {
    let db = Db::open_in_memory().unwrap();
    db.save_cursor("f", "192.168.1.100", 2, 77).unwrap();
    db.save_cursor("f", "192.168.1.200", 1, 33).unwrap();
    assert_eq!(db.load_resume_cursors().unwrap().len(), 2);
}
#[test]
fn db_profile_persists_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.sqlite3");
    {
        let db = Db::open(&p).unwrap();
        db.save_profile("wss://p.com", "t", "info").unwrap();
        db.save_cursor("f", "i", 5, 200).unwrap();
    }
    {
        let db = Db::open(&p).unwrap();
        let pr = db.load_profile().unwrap().unwrap();
        assert_eq!(pr.server_url, "wss://p.com");
        assert_eq!(db.load_cursors().unwrap()[0].last_seq, 200);
    }
}

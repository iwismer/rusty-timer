use receiver::db::StreamSubscription;
use receiver::stream_key::LocalStreamKey;
use receiver::{Db, EventType};

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
    let mut db = Db::open_in_memory().unwrap();
    db.save_profile("https://e.com", "t", "check-and-download", None)
        .unwrap();
    let p = db.load_profile().unwrap().unwrap();
    assert_eq!(p.server_url, "https://e.com");
    assert_eq!(p.token, "t");
}
#[test]
fn profile_update_replaces_existing() {
    let mut db = Db::open_in_memory().unwrap();
    db.save_profile("https://old", "old", "check-and-download", None)
        .unwrap();
    db.save_profile("https://new", "new", "check-and-download", None)
        .unwrap();
    assert_eq!(
        db.load_profile().unwrap().unwrap().server_url,
        "https://new"
    );
}
#[test]
fn profile_load_returns_none_when_empty() {
    assert!(
        Db::open_in_memory()
            .unwrap()
            .load_profile()
            .unwrap()
            .is_none()
    );
}
fn stream_sub(endpoint: &str, stream_id: &str, port: Option<u16>) -> StreamSubscription {
    StreamSubscription {
        forwarder_endpoint_id: endpoint.to_owned(),
        stream_id: stream_id.to_owned(),
        local_port_override: port,
        event_type: EventType::Finish,
        forwarder_id: None,
        reader_ip: None,
    }
}

#[test]
fn subscriptions_save_and_load() {
    let mut db = Db::open_in_memory().unwrap();
    db.replace_stream_subscriptions(&[
        stream_sub("endpoint-1", "192.168.1.100:10000", Some(10100)),
        stream_sub("endpoint-1", "192.168.1.200:10000", None),
    ])
    .unwrap();
    let s = db.load_stream_subscriptions().unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(
        s.iter()
            .find(|x| x.stream_id == "192.168.1.100:10000")
            .unwrap()
            .local_port_override,
        Some(10100)
    );
}
#[test]
fn subscriptions_replace_all_replaces_existing() {
    let mut db = Db::open_in_memory().unwrap();
    db.replace_stream_subscriptions(&[stream_sub("endpoint-1", "192.168.1.100:10000", None)])
        .unwrap();
    db.replace_stream_subscriptions(&[stream_sub("endpoint-2", "10.0.0.1:10000", Some(9900))])
        .unwrap();
    let s = db.load_stream_subscriptions().unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].forwarder_endpoint_id, "endpoint-2");
}

#[test]
fn replace_stream_subscriptions_is_atomic_on_duplicate_input() {
    let mut db = Db::open_in_memory().unwrap();
    let baseline = vec![
        stream_sub("endpoint-1", "10.0.0.1", None),
        stream_sub("endpoint-2", "10.0.0.2", Some(9900)),
    ];
    db.replace_stream_subscriptions(&baseline).unwrap();

    let duplicate_payload = vec![
        stream_sub("dup", "10.0.0.3", None),
        stream_sub("dup", "10.0.0.3", Some(9950)),
    ];

    assert!(db.replace_stream_subscriptions(&duplicate_payload).is_err());
    let after = db.load_stream_subscriptions().unwrap();
    assert_eq!(after, baseline);
}
#[test]
fn cursor_save_and_load() {
    let db = Db::open_in_memory().unwrap();
    let key = LocalStreamKey::new("endpoint-1", "192.168.1.100:10000");
    db.jump_stream_cursor(key.as_str(), 99).unwrap();
    assert_eq!(db.load_stream_cursor(key.as_str()).unwrap(), 99);
}
#[test]
fn cursor_upsert_advances_position() {
    let db = Db::open_in_memory().unwrap();
    let key = LocalStreamKey::new("endpoint-1", "192.168.1.100:10000");
    db.jump_stream_cursor(key.as_str(), 10).unwrap();
    db.jump_stream_cursor(key.as_str(), 50).unwrap();
    assert_eq!(db.load_stream_cursor(key.as_str()).unwrap(), 50);
}

#[test]
fn cursor_upsert_does_not_regress() {
    let db = Db::open_in_memory().unwrap();
    let key = LocalStreamKey::new("endpoint-1", "192.168.1.100:10000");

    db.jump_stream_cursor(key.as_str(), 5).unwrap();
    db.jump_stream_cursor(key.as_str(), 4).unwrap();

    assert_eq!(db.load_stream_cursor(key.as_str()).unwrap(), 5);
}

#[test]
fn cursors_load_as_stream_rows() {
    let db = Db::open_in_memory().unwrap();
    db.jump_stream_cursor(
        LocalStreamKey::new("endpoint-1", "192.168.1.100:10000").as_str(),
        77,
    )
    .unwrap();
    db.jump_stream_cursor(
        LocalStreamKey::new("endpoint-1", "192.168.1.200:10000").as_str(),
        33,
    )
    .unwrap();
    assert_eq!(db.load_stream_cursors().unwrap().len(), 2);
}
#[test]
fn db_profile_persists_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.sqlite3");
    {
        let mut db = Db::open(&p).unwrap();
        db.save_profile("https://p.com", "t", "check-and-download", None)
            .unwrap();
        db.jump_stream_cursor(LocalStreamKey::new("endpoint-1", "i").as_str(), 200)
            .unwrap();
    }
    {
        let db = Db::open(&p).unwrap();
        let pr = db.load_profile().unwrap().unwrap();
        assert_eq!(pr.server_url, "https://p.com");
        assert_eq!(
            db.load_stream_cursor(LocalStreamKey::new("endpoint-1", "i").as_str())
                .unwrap(),
            200
        );
    }
}

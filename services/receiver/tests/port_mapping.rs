use receiver::{Db, Subscription};
use receiver::ports::{default_port, last_octet, resolve_ports, stream_key, PortAssignment};

#[test]
fn default_port_mapping_100() { assert_eq!(default_port("192.168.1.100"), Some(10100)); }
#[test]
fn default_port_mapping_1() { assert_eq!(default_port("10.0.0.1"), Some(10001)); }
#[test]
fn default_port_mapping_255() { assert_eq!(default_port("10.0.0.255"), Some(10255)); }
#[test]
fn default_port_mapping_0() { assert_eq!(default_port("10.0.0.0"), Some(10000)); }
#[test]
fn last_octet_parses_correctly() {
    assert_eq!(last_octet("192.168.1.100"), Some(100));
    assert_eq!(last_octet("10.0.0.1"), Some(1));
}
#[test]
fn last_octet_invalid_returns_none() {
    assert_eq!(last_octet("not-ip"), None);
    assert_eq!(last_octet("192.168.1"), None);
}
#[test]
fn override_port_used_instead_of_default() {
    let subs = vec![Subscription{forwarder_id:"f".to_owned(),reader_ip:"192.168.1.100".to_owned(),local_port_override:Some(9999)}];
    let r = resolve_ports(&subs);
    assert_eq!(r[&stream_key("f","192.168.1.100")], PortAssignment::Assigned(9999));
}
#[test]
fn two_streams_no_collision() {
    let subs = vec![
        Subscription{forwarder_id:"f".to_owned(),reader_ip:"10.0.0.1".to_owned(),local_port_override:None},
        Subscription{forwarder_id:"f".to_owned(),reader_ip:"10.0.0.2".to_owned(),local_port_override:None},
    ];
    let r = resolve_ports(&subs);
    assert_eq!(r[&stream_key("f","10.0.0.1")], PortAssignment::Assigned(10001));
    assert_eq!(r[&stream_key("f","10.0.0.2")], PortAssignment::Assigned(10002));
}
#[test]
fn collision_marks_both_streams_degraded() {
    let subs = vec![
        Subscription{forwarder_id:"f1".to_owned(),reader_ip:"192.168.1.100".to_owned(),local_port_override:None},
        Subscription{forwarder_id:"f2".to_owned(),reader_ip:"10.0.0.100".to_owned(),local_port_override:None},
    ];
    let r = resolve_ports(&subs);
    assert!(matches!(r[&stream_key("f1","192.168.1.100")], PortAssignment::Collision{wanted:10100,..}));
    assert!(matches!(r[&stream_key("f2","10.0.0.100")], PortAssignment::Collision{wanted:10100,..}));
}
#[test]
fn non_colliding_streams_unaffected_by_collision() {
    let subs = vec![
        Subscription{forwarder_id:"f".to_owned(),reader_ip:"10.0.0.1".to_owned(),local_port_override:None},
        Subscription{forwarder_id:"f1".to_owned(),reader_ip:"10.0.0.1".to_owned(),local_port_override:None},
        Subscription{forwarder_id:"f".to_owned(),reader_ip:"10.0.0.2".to_owned(),local_port_override:None},
    ];
    let r = resolve_ports(&subs);
    assert!(matches!(r[&stream_key("f","10.0.0.2")], PortAssignment::Assigned(10002)));
    assert!(matches!(r[&stream_key("f","10.0.0.1")], PortAssignment::Collision{..}));
    assert!(matches!(r[&stream_key("f1","10.0.0.1")], PortAssignment::Collision{..}));
}
#[test]
fn collision_via_override_ports() {
    let subs = vec![
        Subscription{forwarder_id:"f".to_owned(),reader_ip:"10.0.0.1".to_owned(),local_port_override:Some(8000)},
        Subscription{forwarder_id:"f".to_owned(),reader_ip:"10.0.0.2".to_owned(),local_port_override:Some(8000)},
    ];
    let r = resolve_ports(&subs);
    assert!(matches!(r[&stream_key("f","10.0.0.1")], PortAssignment::Collision{wanted:8000,..}));
    assert!(matches!(r[&stream_key("f","10.0.0.2")], PortAssignment::Collision{wanted:8000,..}));
}
#[test]
fn port_assignments_loaded_from_db() {
    let db = Db::open_in_memory().unwrap();
    db.save_subscription("f","192.168.1.100",None).unwrap();
    db.save_subscription("f","192.168.1.200",None).unwrap();
    let subs = db.load_subscriptions().unwrap();
    let r = resolve_ports(&subs);
    assert_eq!(r.len(), 2);
    assert!(matches!(r[&stream_key("f","192.168.1.100")], PortAssignment::Assigned(10100)));
    assert!(matches!(r[&stream_key("f","192.168.1.200")], PortAssignment::Assigned(10200)));
}
#[test]
fn port_override_from_db_subscription() {
    let db = Db::open_in_memory().unwrap();
    db.save_subscription("f","192.168.1.100",Some(7777)).unwrap();
    let subs = db.load_subscriptions().unwrap();
    let r = resolve_ports(&subs);
    assert_eq!(r[&stream_key("f","192.168.1.100")], PortAssignment::Assigned(7777));
}

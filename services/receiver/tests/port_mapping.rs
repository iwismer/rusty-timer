use receiver::ports::{default_port, last_octet};

#[test]
fn default_port_mapping_100() {
    assert_eq!(default_port("192.168.1.100:10000"), Some(10100));
}
#[test]
fn default_port_mapping_1() {
    assert_eq!(default_port("10.0.0.1:10000"), Some(10001));
}
#[test]
fn default_port_mapping_255() {
    assert_eq!(default_port("10.0.0.255:10000"), Some(10255));
}
#[test]
fn default_port_mapping_0() {
    assert_eq!(default_port("10.0.0.0:10000"), Some(10000));
}
#[test]
fn last_octet_parses_correctly() {
    assert_eq!(last_octet("192.168.1.100:10000"), Some(100));
    assert_eq!(last_octet("10.0.0.1:10000"), Some(1));
}
#[test]
fn last_octet_invalid_returns_none() {
    assert_eq!(last_octet("not-ip"), None);
    assert_eq!(last_octet("192.168.1"), None);
}
#[test]
fn same_ip_different_reader_ports_get_distinct_defaults() {
    let p1 = default_port("10.0.0.1:10001").expect("parse :10001");
    let p2 = default_port("10.0.0.1:10002").expect("parse :10002");
    assert_ne!(p1, p2, "same IP streams must not collide by default");
    assert!(p1 >= 12000);
    assert!(p2 >= 12000);
}

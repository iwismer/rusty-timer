use receiver::stream_key::LocalStreamKey;

#[test]
fn local_stream_key_round_trips_endpoint_and_wire_stream_id() {
    let key = LocalStreamKey::new("endpoint-abc123", "127.0.0.1:10000");

    assert_eq!(key.endpoint_id(), "endpoint-abc123");
    assert_eq!(key.wire_stream_id(), "127.0.0.1:10000");
    assert_eq!(key.as_str(), "endpoint-abc123\u{1f}127.0.0.1:10000");
}

#[test]
fn local_stream_key_decodes_wire_stream_id_containing_separator() {
    let key = LocalStreamKey::new("endpoint-abc123", "reader\u{1f}with\u{1f}separator");

    assert_eq!(key.endpoint_id(), "endpoint-abc123");
    assert_eq!(key.wire_stream_id(), "reader\u{1f}with\u{1f}separator");
}

#[test]
fn local_stream_key_display_uses_visible_separator() {
    let key = LocalStreamKey::new("endpoint-abc123", "127.0.0.1:10000");

    assert_eq!(key.to_string(), "endpoint-abc123␟127.0.0.1:10000");
    assert_eq!(key.as_str(), "endpoint-abc123\u{1f}127.0.0.1:10000");
}

#[test]
#[should_panic(expected = "endpoint_id must not be empty")]
fn local_stream_key_rejects_empty_endpoint_id() {
    let _ = LocalStreamKey::new("", "stream");
}

#[test]
#[should_panic(expected = "endpoint_id must not contain separator")]
fn local_stream_key_rejects_endpoint_id_containing_separator() {
    let _ = LocalStreamKey::new("endpoint\u{1f}abc123", "stream");
}

#[test]
#[should_panic(expected = "wire_stream_id must not be empty")]
fn local_stream_key_rejects_empty_wire_stream_id() {
    let _ = LocalStreamKey::new("endpoint", "");
}

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
#[should_panic(expected = "missing endpoint/stream separator")]
fn local_stream_key_decode_without_separator_is_a_hard_error() {
    let _ = LocalStreamKey::from_encoded("no-separator-here".to_owned());
}

#[test]
fn local_stream_key_decodes_encoded_round_trip() {
    let original = LocalStreamKey::new("endpoint-abc123", "127.0.0.1:10000");
    let decoded = LocalStreamKey::from_encoded(original.as_str().to_owned());

    assert_eq!(decoded, original);
    assert_eq!(decoded.endpoint_id(), "endpoint-abc123");
    assert_eq!(decoded.wire_stream_id(), "127.0.0.1:10000");
}

#[test]
#[should_panic(expected = "endpoint_id must not be empty")]
fn local_stream_key_rejects_empty_endpoint_id() {
    let _ = LocalStreamKey::new("", "stream");
}

#[test]
#[should_panic(expected = "wire_stream_id must not be empty")]
fn local_stream_key_rejects_empty_wire_stream_id() {
    let _ = LocalStreamKey::new("endpoint", "");
}

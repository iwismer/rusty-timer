//! Prost round-trip tests: `encode` -> `decode` must equal the original.

use prost::Message;
use rt_p2p_protocol::{Hello, ProtocolError, ReadRecord};

// A stream_id is a UUID represented as 16 bytes on the wire.
const STREAM_ID: [u8; 16] = [
    0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
];

#[test]
fn read_record_round_trips() {
    let original = ReadRecord {
        stream_id: STREAM_ID.to_vec(),
        seq: 42,
        epoch: 7,
        raw_frame: vec![0xde, 0xad, 0xbe, 0xef],
        read_kind: "chip".to_string(),
        reader_timestamp: 1_700_000_000_123,
        received_unix_ms: 1_700_000_000_456,
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ReadRecord");

    let decoded = ReadRecord::decode(buf.as_slice()).expect("decode ReadRecord");

    assert_eq!(decoded, original);
}

#[test]
fn hello_round_trips() {
    let original = Hello {
        min_minor: 1,
        max_minor: 3,
        capabilities: vec!["replay".to_string(), "ups".to_string()],
        max_frame_bytes: 1024 * 1024,
        catalog_generation: 7,
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode Hello");

    let decoded = Hello::decode(buf.as_slice()).expect("decode Hello");

    assert_eq!(decoded, original);
}

#[test]
fn protocol_error_round_trips() {
    let original = ProtocolError {
        code: 42,
        message: "stream closed".to_string(),
        retryable: true,
        stream_id: Some(STREAM_ID.to_vec()),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ProtocolError");

    let decoded = ProtocolError::decode(buf.as_slice()).expect("decode ProtocolError");

    assert_eq!(decoded, original);
}

#[test]
fn read_record_default_round_trips() {
    let original = ReadRecord::default();

    let mut buf = Vec::new();
    original
        .encode(&mut buf)
        .expect("encode default ReadRecord");

    let decoded = ReadRecord::decode(buf.as_slice()).expect("decode default ReadRecord");

    assert_eq!(decoded, original);
}

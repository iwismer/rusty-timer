//! Prost round-trip tests: `encode` -> `decode` must equal the original.

use prost::Message;
use rt_p2p_protocol::{
    ConfigGetRequest, ConfigGetResponse, ConfigSetRequest, ConfigSetResponse, ControlC2F,
    ControlF2C, Hello, ReadRecord, RestartRequest, RestartResponse, WireProtocolError, control_c2f,
    control_f2c,
};

// A stream_id is opaque bytes on the wire; this test uses a 16-byte sample.
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
    let original = WireProtocolError {
        code: 42,
        message: "stream closed".to_string(),
        retryable: true,
        stream_id: Some(STREAM_ID.to_vec()),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ProtocolError");

    let decoded = WireProtocolError::decode(buf.as_slice()).expect("decode ProtocolError");

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

#[test]
fn config_get_request_control_c2f_round_trips() {
    let original = ControlC2F {
        msg: Some(control_c2f::Msg::ConfigGetRequest(ConfigGetRequest {
            request_id: "config-get-1".to_string(),
        })),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ControlC2F");

    let decoded = ControlC2F::decode(buf.as_slice()).expect("decode ControlC2F");

    assert_eq!(decoded, original);
}

#[test]
fn config_set_request_control_c2f_round_trips() {
    let original = ControlC2F {
        msg: Some(control_c2f::Msg::ConfigSetRequest(ConfigSetRequest {
            request_id: "config-set-1".to_string(),
            config_json: r#"{"reader":"alpha"}"#.to_string(),
        })),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ControlC2F");

    let decoded = ControlC2F::decode(buf.as_slice()).expect("decode ControlC2F");

    assert_eq!(decoded, original);
}

#[test]
fn restart_request_control_c2f_round_trips() {
    let original = ControlC2F {
        msg: Some(control_c2f::Msg::RestartRequest(RestartRequest {
            request_id: "restart-1".to_string(),
        })),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ControlC2F");

    let decoded = ControlC2F::decode(buf.as_slice()).expect("decode ControlC2F");

    assert_eq!(decoded, original);
}

#[test]
fn config_get_response_control_f2c_round_trips() {
    let original = ControlF2C {
        msg: Some(control_f2c::Msg::ConfigGetResponse(ConfigGetResponse {
            request_id: "config-get-1".to_string(),
            config_json: r#"{"reader":"alpha"}"#.to_string(),
            restart_needed: true,
        })),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ControlF2C");

    let decoded = ControlF2C::decode(buf.as_slice()).expect("decode ControlF2C");

    assert_eq!(decoded, original);
}

#[test]
fn config_set_response_control_f2c_round_trips() {
    let original = ControlF2C {
        msg: Some(control_f2c::Msg::ConfigSetResponse(ConfigSetResponse {
            request_id: "config-set-1".to_string(),
            ok: false,
            restart_needed: true,
            error: "invalid reader port".to_string(),
        })),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ControlF2C");

    let decoded = ControlF2C::decode(buf.as_slice()).expect("decode ControlF2C");

    assert_eq!(decoded, original);
}

#[test]
fn restart_response_control_f2c_round_trips() {
    let original = ControlF2C {
        msg: Some(control_f2c::Msg::RestartResponse(RestartResponse {
            request_id: "restart-1".to_string(),
            accepted: false,
            error: "restart already pending".to_string(),
        })),
    };

    let mut buf = Vec::new();
    original.encode(&mut buf).expect("encode ControlF2C");

    let decoded = ControlF2C::decode(buf.as_slice()).expect("decode ControlF2C");

    assert_eq!(decoded, original);
}

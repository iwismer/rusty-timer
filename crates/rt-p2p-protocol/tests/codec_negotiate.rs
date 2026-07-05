use bytes::{BufMut, BytesMut};
use prost::Message;
use rt_p2p_protocol::{
    CAP_CONTROL_EVENTS, CAP_REMOTE_CONFIG, Hello, Ping, ProtocolError, decode_frame,
    decode_frame_payload, decode_message_frame, encode_frame, has_capability, negotiate,
};

#[test]
fn roundtrip_frame() {
    let original = Ping { nonce: 42 };

    let encoded = encode_frame(&original);

    assert_eq!(
        u32::from_le_bytes(encoded[..4].try_into().expect("length prefix")) as usize,
        original.encoded_len()
    );

    let mut buf = BytesMut::from(encoded.as_ref());
    let frame = decode_frame(&mut buf)
        .expect("decode frame")
        .expect("complete frame");
    let decoded = Ping::decode(frame).expect("decode Ping");

    assert!(buf.is_empty());
    assert_eq!(decoded, original);
}

#[test]
fn decode_frame_payload_roundtrips_encoded_message() {
    let original = Ping { nonce: 42 };
    let encoded = encode_frame(&original);
    let len_buf = encoded[..4].try_into().expect("length prefix");

    let decoded = decode_frame_payload::<Ping>(len_buf, &encoded[4..]).expect("decode payload");

    assert_eq!(decoded, original);
}

#[test]
fn decode_frame_payload_rejects_oversize_prefix() {
    let len_buf = u32::try_from(8 * 1024 * 1024 + 1)
        .expect("fits u32")
        .to_le_bytes();

    let err = decode_frame_payload::<Ping>(len_buf, &[]).expect_err("oversize frame");

    assert!(matches!(err, ProtocolError::FrameTooLarge { .. }));
}

#[test]
fn decode_frame_payload_rejects_payload_shorter_than_prefix() {
    let encoded = encode_frame(&Ping { nonce: 42 });
    let len_buf = encoded[..4].try_into().expect("length prefix");

    let err = decode_frame_payload::<Ping>(len_buf, &encoded[4..encoded.len() - 1])
        .expect_err("truncated payload");

    assert!(matches!(err, ProtocolError::ProtocolViolation { .. }));
}

#[test]
fn partial_read_returns_none() {
    let encoded = encode_frame(&Ping { nonce: 7 });

    let mut partial_prefix = BytesMut::from(&encoded[..2]);
    let original_prefix = partial_prefix.clone();
    assert_eq!(
        decode_frame(&mut partial_prefix).expect("partial prefix"),
        None
    );
    assert_eq!(partial_prefix, original_prefix);

    let mut partial_payload = BytesMut::from(&encoded[..encoded.len() - 1]);
    let original_payload = partial_payload.clone();
    assert_eq!(
        decode_frame(&mut partial_payload).expect("partial payload"),
        None
    );
    assert_eq!(partial_payload, original_payload);
}

#[test]
fn oversize_frame_rejected() {
    let mut buf = BytesMut::new();
    buf.put_u32_le(8 * 1024 * 1024 + 1);

    let err = decode_frame(&mut buf).expect_err("oversize frame");

    assert!(matches!(err, ProtocolError::FrameTooLarge { .. }));
}

#[test]
fn garbage_is_decode_error() {
    let mut buf = BytesMut::new();
    buf.put_u32_le(1);
    buf.put_u8(0xff);

    let err = decode_message_frame::<Ping>(&mut buf).expect_err("invalid protobuf");

    assert!(matches!(err, ProtocolError::DecodeError { .. }));
}

#[test]
fn old_minor_peer_refuses_to_pair_with_current_minor_in_both_directions() {
    // A peer still speaking minor 1 (pre epoch-metadata protocol) must fail
    // negotiation against the current PROTOCOL_MINOR, whichever side is old.
    let old = Hello {
        min_minor: 1,
        max_minor: 1,
        capabilities: vec!["data".to_owned()],
        max_frame_bytes: 0,
        catalog_generation: 0,
    };
    let current = Hello {
        min_minor: rt_p2p_protocol::PROTOCOL_MINOR,
        max_minor: rt_p2p_protocol::PROTOCOL_MINOR,
        capabilities: vec!["data".to_owned()],
        max_frame_bytes: 0,
        catalog_generation: 0,
    };

    assert!(
        negotiate(&old, &current).is_err(),
        "old client vs current server must refuse to pair"
    );
    assert!(
        negotiate(&current, &old).is_err(),
        "current client vs old server must refuse to pair"
    );
}

#[test]
fn negotiate_picks_min_minor_and_capability_intersection() {
    let client = Hello {
        min_minor: 1,
        max_minor: 5,
        capabilities: vec![
            "client-only".to_string(),
            "shared-b".to_string(),
            "shared-a".to_string(),
        ],
        max_frame_bytes: 4 * 1024 * 1024,
        catalog_generation: 3,
    };
    let server = Hello {
        min_minor: 2,
        max_minor: 3,
        capabilities: vec![
            "shared-b".to_string(),
            "server-only".to_string(),
            "shared-a".to_string(),
        ],
        max_frame_bytes: 1024 * 1024,
        catalog_generation: 9,
    };

    let negotiated = negotiate(&client, &server).expect("negotiate");

    assert_eq!(negotiated.protocol_minor, 3);
    assert_eq!(
        negotiated.capabilities,
        vec!["shared-a".to_string(), "shared-b".to_string()]
    );
    assert_eq!(negotiated.max_frame_bytes, 1024 * 1024);
    assert_eq!(negotiated.catalog_generation, 9);

    let unsupported = Hello {
        min_minor: 4,
        max_minor: 5,
        ..client
    };
    let err = negotiate(&unsupported, &server).expect_err("unsupported version");

    assert!(matches!(err, ProtocolError::UnsupportedVersion { .. }));
}

#[test]
fn negotiate_keeps_named_capability_only_when_both_advertise() {
    let base_client = Hello {
        min_minor: 1,
        max_minor: 5,
        capabilities: vec![CAP_CONTROL_EVENTS.to_string()],
        max_frame_bytes: 4 * 1024 * 1024,
        catalog_generation: 3,
    };
    let server_with = Hello {
        min_minor: 1,
        max_minor: 5,
        capabilities: vec![CAP_CONTROL_EVENTS.to_string()],
        max_frame_bytes: 4 * 1024 * 1024,
        catalog_generation: 3,
    };

    let both = negotiate(&base_client, &server_with).expect("negotiate");
    assert!(has_capability(&both.capabilities, CAP_CONTROL_EVENTS));
    assert!(!has_capability(&both.capabilities, CAP_REMOTE_CONFIG));

    let server_without = Hello {
        capabilities: vec![CAP_REMOTE_CONFIG.to_string()],
        ..server_with
    };
    let one_sided = negotiate(&base_client, &server_without).expect("negotiate");
    assert!(!has_capability(&one_sided.capabilities, CAP_CONTROL_EVENTS));
    assert!(!has_capability(&one_sided.capabilities, CAP_REMOTE_CONFIG));
}

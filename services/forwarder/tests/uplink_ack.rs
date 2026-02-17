/// Tests for the uplink WS client: hello handshake, event batch sending, ack processing.
///
/// Uses MockWsServer from rt-test-utils to simulate a server.
use forwarder::uplink::{UplinkConfig, UplinkSession};
use rt_protocol::{AckEntry, ForwarderAck, ForwarderHello, WsMessage};
use rt_test_utils::MockWsServer;

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// Test: uplink session connects to server, sends ForwarderHello, receives Heartbeat.
#[tokio::test]
async fn uplink_sends_forwarder_hello_on_connect() {
    let server = MockWsServer::start().await.unwrap();
    let url = format!("ws://{}", server.local_addr());

    let cfg = UplinkConfig {
        server_url: url.clone(),
        token: "test-token".to_owned(),
        forwarder_id: "fwd-test-001".to_owned(),
        batch_mode: "immediate".to_owned(),
        batch_flush_ms: 100,
        batch_max_events: 50,
    };

    let mut session = UplinkSession::connect(cfg).await.expect("connect");
    let session_id = session.session_id().to_owned();
    assert!(
        !session_id.is_empty(),
        "session_id must be assigned after hello"
    );
    assert!(
        !session.device_id().is_empty(),
        "device_id must be assigned from heartbeat"
    );
}

/// Test: uplink session returns correct device_id after handshake.
#[tokio::test]
async fn uplink_device_id_matches_hello_forwarder_id() {
    let server = MockWsServer::start().await.unwrap();
    let url = format!("ws://{}", server.local_addr());

    let cfg = UplinkConfig {
        server_url: url.clone(),
        token: "test-token".to_owned(),
        forwarder_id: "fwd-device-check".to_owned(),
        batch_mode: "immediate".to_owned(),
        batch_flush_ms: 100,
        batch_max_events: 50,
    };

    let session = UplinkSession::connect(cfg).await.expect("connect");
    assert_eq!(session.device_id(), "fwd-device-check");
}

// ---------------------------------------------------------------------------
// Event batch sending and ack
// ---------------------------------------------------------------------------

/// Test: sending a batch of events results in a ForwarderAck.
#[tokio::test]
async fn uplink_receives_ack_for_event_batch() {
    let server = MockWsServer::start().await.unwrap();
    let url = format!("ws://{}", server.local_addr());

    let cfg = UplinkConfig {
        server_url: url,
        token: "test-token".to_owned(),
        forwarder_id: "fwd-ack-test".to_owned(),
        batch_mode: "immediate".to_owned(),
        batch_flush_ms: 100,
        batch_max_events: 50,
    };

    let mut session = UplinkSession::connect(cfg).await.expect("connect");

    let events = vec![rt_protocol::ReadEvent {
        forwarder_id: "fwd-ack-test".to_owned(),
        reader_ip: "192.168.2.100".to_owned(),
        stream_epoch: 1,
        seq: 1,
        reader_timestamp: "2026-01-01T00:00:00Z".to_owned(),
        raw_read_line: "aa400000000123450a2a01123018455927a7".to_owned(),
        read_type: "RAW".to_owned(),
    }];

    let ack = session.send_batch(events).await.expect("send_batch");
    assert_eq!(ack.entries.len(), 1);
    assert_eq!(ack.entries[0].reader_ip, "192.168.2.100");
    assert_eq!(ack.entries[0].stream_epoch, 1);
    assert_eq!(ack.entries[0].last_seq, 1);
}

/// Test: multi-event batch produces ack with high-water marks per stream.
#[tokio::test]
async fn uplink_ack_contains_high_water_marks() {
    let server = MockWsServer::start().await.unwrap();
    let url = format!("ws://{}", server.local_addr());

    let cfg = UplinkConfig {
        server_url: url,
        token: "test-token".to_owned(),
        forwarder_id: "fwd-hwm".to_owned(),
        batch_mode: "immediate".to_owned(),
        batch_flush_ms: 100,
        batch_max_events: 50,
    };

    let mut session = UplinkSession::connect(cfg).await.expect("connect");

    let events = vec![
        rt_protocol::ReadEvent {
            forwarder_id: "fwd-hwm".to_owned(),
            reader_ip: "192.168.2.1".to_owned(),
            stream_epoch: 1,
            seq: 3,
            reader_timestamp: "2026-01-01T00:00:00Z".to_owned(),
            raw_read_line: "line1".to_owned(),
            read_type: "RAW".to_owned(),
        },
        rt_protocol::ReadEvent {
            forwarder_id: "fwd-hwm".to_owned(),
            reader_ip: "192.168.2.1".to_owned(),
            stream_epoch: 1,
            seq: 7,
            reader_timestamp: "2026-01-01T00:00:01Z".to_owned(),
            raw_read_line: "line2".to_owned(),
            read_type: "RAW".to_owned(),
        },
        rt_protocol::ReadEvent {
            forwarder_id: "fwd-hwm".to_owned(),
            reader_ip: "192.168.2.2".to_owned(),
            stream_epoch: 1,
            seq: 5,
            reader_timestamp: "2026-01-01T00:00:02Z".to_owned(),
            raw_read_line: "line3".to_owned(),
            read_type: "RAW".to_owned(),
        },
    ];

    let ack = session.send_batch(events).await.expect("send_batch");
    assert_eq!(ack.entries.len(), 2);

    let e1 = ack
        .entries
        .iter()
        .find(|e| e.reader_ip == "192.168.2.1")
        .unwrap();
    assert_eq!(e1.last_seq, 7, "high water mark for .2.1 must be 7");

    let e2 = ack
        .entries
        .iter()
        .find(|e| e.reader_ip == "192.168.2.2")
        .unwrap();
    assert_eq!(e2.last_seq, 5);
}

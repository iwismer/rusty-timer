// rt-test-utils: Shared test utilities for the remote forwarding suite.
//
// Provides mock WebSocket server and client for integration testing of
// forwarder, server, and receiver components.

pub mod mock_ws_client;
pub mod mock_ws_server;

pub use mock_ws_client::MockWsClient;
pub use mock_ws_server::MockWsServer;

#[cfg(test)]
mod tests {
    use super::*;
    use rt_protocol::*;

    // -----------------------------------------------------------------------
    // Mock WS Server tests
    // -----------------------------------------------------------------------

    /// Test: server starts, binds to a random port, and reports a valid address.
    #[tokio::test]
    async fn mock_server_starts_and_reports_port() {
        let server = MockWsServer::start().await.unwrap();
        let addr = server.local_addr();
        assert_ne!(addr.port(), 0, "should bind to a real port");
    }

    /// Test: forwarder hello handshake flow.
    ///
    /// 1. Client connects to mock server
    /// 2. Client sends forwarder_hello
    /// 3. Server validates hello and responds with heartbeat (session_id + device_id)
    #[tokio::test]
    async fn mock_server_forwarder_hello_handshake() {
        let server = MockWsServer::start().await.unwrap();
        let url = format!("ws://{}", server.local_addr());

        let mut client = MockWsClient::connect(&url).await.unwrap();

        // Send a forwarder_hello
        let hello = WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-test-001".to_owned(),
            reader_ips: vec!["192.168.1.10".to_owned()],
            display_name: None,
        });
        client.send_message(&hello).await.unwrap();

        // Server should respond with a heartbeat carrying session_id and device_id
        let response = client.recv_message().await.unwrap();
        match response {
            WsMessage::Heartbeat(hb) => {
                assert!(!hb.session_id.is_empty(), "session_id must not be empty");
                assert!(!hb.device_id.is_empty(), "device_id must not be empty");
            }
            other => panic!("expected Heartbeat, got {:?}", other),
        }
    }

    /// Test: receiver hello handshake flow.
    #[tokio::test]
    async fn mock_server_receiver_hello_handshake() {
        let server = MockWsServer::start().await.unwrap();
        let url = format!("ws://{}", server.local_addr());

        let mut client = MockWsClient::connect(&url).await.unwrap();

        let hello = WsMessage::ReceiverHelloV12(ReceiverHelloV12 {
            receiver_id: "rcv-test-001".to_owned(),
            mode: ReceiverMode::Live {
                streams: vec![],
                earliest_epochs: vec![],
            },
            resume: vec![],
        });
        client.send_message(&hello).await.unwrap();

        let response = client.recv_message().await.unwrap();
        match response {
            WsMessage::Heartbeat(hb) => {
                assert!(!hb.session_id.is_empty(), "session_id must not be empty");
                assert!(!hb.device_id.is_empty(), "device_id must not be empty");
            }
            other => panic!("expected Heartbeat, got {:?}", other),
        }
    }

    /// Test: forwarder event batch -> ack flow.
    ///
    /// 1. Client sends forwarder_hello, receives heartbeat with session_id
    /// 2. Client sends forwarder_event_batch with that session_id
    /// 3. Server responds with forwarder_ack
    #[tokio::test]
    async fn mock_server_forwarder_event_batch_ack() {
        let server = MockWsServer::start().await.unwrap();
        let url = format!("ws://{}", server.local_addr());

        let mut client = MockWsClient::connect(&url).await.unwrap();

        // Handshake
        let hello = WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-test-001".to_owned(),
            reader_ips: vec!["192.168.1.10".to_owned()],
            display_name: None,
        });
        client.send_message(&hello).await.unwrap();
        let hb = client.recv_message().await.unwrap();
        let session_id = match hb {
            WsMessage::Heartbeat(hb) => hb.session_id,
            other => panic!("expected Heartbeat, got {:?}", other),
        };

        // Send event batch
        let batch = WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "batch-001".to_owned(),
            events: vec![ReadEvent {
                forwarder_id: "fwd-test-001".to_owned(),
                reader_ip: "192.168.1.10".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_frame: b"09001234567890123 12:00:00.000 1".to_vec(),
                read_type: "RAW".to_owned(),
            }],
        });
        client.send_message(&batch).await.unwrap();

        // Expect ack
        let ack = client.recv_message().await.unwrap();
        match ack {
            WsMessage::ForwarderAck(ack) => {
                assert_eq!(ack.session_id, session_id);
                assert_eq!(ack.entries.len(), 1);
                assert_eq!(ack.entries[0].forwarder_id, "fwd-test-001");
                assert_eq!(ack.entries[0].reader_ip, "192.168.1.10");
                assert_eq!(ack.entries[0].stream_epoch, 1);
                assert_eq!(ack.entries[0].last_seq, 1);
            }
            other => panic!("expected ForwarderAck, got {:?}", other),
        }
    }

    /// Test: server sends error for non-hello first message.
    #[tokio::test]
    async fn mock_server_rejects_non_hello_first_message() {
        let server = MockWsServer::start().await.unwrap();
        let url = format!("ws://{}", server.local_addr());

        let mut client = MockWsClient::connect(&url).await.unwrap();

        // Send an event batch without hello first -- protocol violation
        let batch = WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: "fake-session".to_owned(),
            batch_id: "batch-001".to_owned(),
            events: vec![],
        });
        client.send_message(&batch).await.unwrap();

        let response = client.recv_message().await.unwrap();
        match response {
            WsMessage::Error(err) => {
                assert_eq!(err.code, rt_protocol::error_codes::PROTOCOL_ERROR);
                assert!(!err.retryable);
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Mock WS Client tests
    // -----------------------------------------------------------------------

    /// Test: mock client connects to a server and can send/receive messages.
    #[tokio::test]
    async fn mock_client_send_recv_roundtrip() {
        let server = MockWsServer::start().await.unwrap();
        let url = format!("ws://{}", server.local_addr());

        let mut client = MockWsClient::connect(&url).await.unwrap();

        // Send hello
        let hello = WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-roundtrip".to_owned(),
            reader_ips: vec!["10.0.0.1".to_owned()],
            display_name: None,
        });
        client.send_message(&hello).await.unwrap();

        // Should get heartbeat back
        let msg = client.recv_message().await.unwrap();
        assert!(matches!(msg, WsMessage::Heartbeat(_)));
    }

    /// Test: multiple clients can connect to the same server independently.
    #[tokio::test]
    async fn mock_server_handles_multiple_clients() {
        let server = MockWsServer::start().await.unwrap();
        let url = format!("ws://{}", server.local_addr());

        let mut client1 = MockWsClient::connect(&url).await.unwrap();
        let mut client2 = MockWsClient::connect(&url).await.unwrap();

        // Both send hello
        let hello1 = WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-001".to_owned(),
            reader_ips: vec![],
            display_name: None,
        });
        let hello2 = WsMessage::ReceiverHelloV12(ReceiverHelloV12 {
            receiver_id: "rcv-001".to_owned(),
            mode: ReceiverMode::Live {
                streams: vec![],
                earliest_epochs: vec![],
            },
            resume: vec![],
        });

        client1.send_message(&hello1).await.unwrap();
        client2.send_message(&hello2).await.unwrap();

        // Both should get heartbeats with different session IDs
        let hb1 = client1.recv_message().await.unwrap();
        let hb2 = client2.recv_message().await.unwrap();

        let sid1 = match hb1 {
            WsMessage::Heartbeat(hb) => hb.session_id,
            other => panic!("expected Heartbeat, got {:?}", other),
        };
        let sid2 = match hb2 {
            WsMessage::Heartbeat(hb) => hb.session_id,
            other => panic!("expected Heartbeat, got {:?}", other),
        };

        assert_ne!(sid1, sid2, "each client should get a unique session_id");
    }

    /// Test: multi-event batch produces ack with correct high-water marks.
    #[tokio::test]
    async fn mock_server_acks_multi_event_batch() {
        let server = MockWsServer::start().await.unwrap();
        let url = format!("ws://{}", server.local_addr());

        let mut client = MockWsClient::connect(&url).await.unwrap();

        // Handshake
        let hello = WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-multi".to_owned(),
            reader_ips: vec!["10.0.0.1".to_owned(), "10.0.0.2".to_owned()],
            display_name: None,
        });
        client.send_message(&hello).await.unwrap();
        let hb = client.recv_message().await.unwrap();
        let session_id = match hb {
            WsMessage::Heartbeat(hb) => hb.session_id,
            other => panic!("expected Heartbeat, got {:?}", other),
        };

        // Send batch with events from two different streams
        let batch = WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "batch-multi".to_owned(),
            events: vec![
                ReadEvent {
                    forwarder_id: "fwd-multi".to_owned(),
                    reader_ip: "10.0.0.1".to_owned(),
                    stream_epoch: 1,
                    seq: 5,
                    reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                    raw_frame: b"line1".to_vec(),
                    read_type: "RAW".to_owned(),
                },
                ReadEvent {
                    forwarder_id: "fwd-multi".to_owned(),
                    reader_ip: "10.0.0.1".to_owned(),
                    stream_epoch: 1,
                    seq: 6,
                    reader_timestamp: "2026-02-17T10:00:01.000Z".to_owned(),
                    raw_frame: b"line2".to_vec(),
                    read_type: "RAW".to_owned(),
                },
                ReadEvent {
                    forwarder_id: "fwd-multi".to_owned(),
                    reader_ip: "10.0.0.2".to_owned(),
                    stream_epoch: 1,
                    seq: 10,
                    reader_timestamp: "2026-02-17T10:00:02.000Z".to_owned(),
                    raw_frame: b"line3".to_vec(),
                    read_type: "RAW".to_owned(),
                },
            ],
        });
        client.send_message(&batch).await.unwrap();

        let ack = client.recv_message().await.unwrap();
        match ack {
            WsMessage::ForwarderAck(ack) => {
                assert_eq!(ack.session_id, session_id);
                // Should have two entries: one for each (forwarder_id, reader_ip, epoch)
                assert_eq!(ack.entries.len(), 2);

                // Find the entry for 10.0.0.1 -- should have last_seq=6 (high water)
                let entry_1 = ack
                    .entries
                    .iter()
                    .find(|e| e.reader_ip == "10.0.0.1")
                    .expect("should have entry for 10.0.0.1");
                assert_eq!(entry_1.last_seq, 6);

                // Find the entry for 10.0.0.2 -- should have last_seq=10
                let entry_2 = ack
                    .entries
                    .iter()
                    .find(|e| e.reader_ip == "10.0.0.2")
                    .expect("should have entry for 10.0.0.2");
                assert_eq!(entry_2.last_seq, 10);
            }
            other => panic!("expected ForwarderAck, got {:?}", other),
        }
    }
}

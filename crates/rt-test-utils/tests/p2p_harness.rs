//! Self-test for the iroh peer harness (`MockForwarderPeer` / `MockReceiverPeer`).
//!
//! Exercises the loopback flow a future forwarder/receiver P2P test will rely
//! on: a `Hello` negotiation on the control plane plus a `DataSubscribe` on the
//! data plane, followed by a scripted `EventBatch` and a client `Ack`.

use std::time::Duration;

use prost::Message;
use rt_iroh::{EndpointBuilder, RecvStream, SendStream};
use rt_p2p_protocol::{
    Ack, ControlC2F, ControlF2C, DataC2F, DataSubscribe, EventBatch, Hello, MAX_FRAME_BYTES,
    ReadRecord, StreamCatalog, StreamEntry, SubscribeMode, SubscribeOk, control_c2f, data_c2f,
    encode_frame,
};
use rt_test_utils::p2p::{ConnectivityFault, ForwarderScript, MockForwarderPeer, MockReceiverPeer};
use rt_test_utils::poll_until;

#[tokio::test]
async fn p2p_harness_exchanges_hello_and_subscribe_over_loopback() {
    let result = tokio::time::timeout(Duration::from_secs(15), run_harness_self_test()).await;
    result.expect("harness self-test timed out");
}

async fn run_harness_self_test() {
    let stream_id = vec![7u8; 16];

    let server_hello = Hello {
        min_minor: 1,
        max_minor: 3,
        capabilities: vec!["shared".to_owned(), "server-only".to_owned()],
        max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
        catalog_generation: 5,
    };
    let catalog = StreamCatalog {
        generation: 5,
        entries: vec![StreamEntry {
            stream_id: stream_id.clone(),
            display_name: "Finish".to_owned(),
            network_addr: "10.0.0.1:10000".to_owned(),
            reader_connected: true,
            hardware_reader_id: "R1".to_owned(),
        }],
    };
    let subscribe_ok = SubscribeOk {
        stream_id: stream_id.clone(),
        earliest_available_seq: 1,
        latest_seq_at_open: 2,
    };
    let batch = EventBatch {
        records: vec![ReadRecord {
            stream_id: stream_id.clone(),
            seq: 1,
            epoch: 1,
            raw_frame: b"frame-one".to_vec(),
            read_kind: "chip".to_owned(),
            reader_timestamp: 0,
            received_unix_ms: 0,
        }],
        replay: false,
    };

    let script = ForwarderScript {
        server_hello,
        catalog,
        subscribe_ok,
        batches: vec![batch],
        caught_up_through: Some(1),
        gap_notice: None,
        data_fault: ConnectivityFault::healthy(),
        echo_subscribed_stream_id: false,
        close_connection_after_data: false,
        control_events: Vec::new(),
        control_pings: 0,
        control_ping_interval: std::time::Duration::from_millis(50),
        config_get_json: String::new(),
        config_restart_needed: false,
        respond_to_config_requests: true,
        reader_control_info_json: None,
        respond_to_reader_control_requests: true,
    };

    let forwarder = MockForwarderPeer::start([1; 32], script)
        .await
        .expect("start forwarder peer");
    let receiver = MockReceiverPeer::start([2; 32])
        .await
        .expect("start receiver peer");

    let client_hello = Hello {
        min_minor: 1,
        max_minor: 5,
        capabilities: vec!["shared".to_owned(), "client-only".to_owned()],
        max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
        catalog_generation: 0,
    };

    let session = receiver
        .hello(forwarder.node_addr(), client_hello)
        .await
        .expect("hello negotiation");

    assert_eq!(session.hello_ok.protocol_minor, 3);
    assert_eq!(session.hello_ok.capabilities, vec!["shared".to_owned()]);
    assert_eq!(session.catalog.generation, 5);
    assert_eq!(session.catalog.entries.len(), 1);
    assert_eq!(session.catalog.entries[0].stream_id, stream_id);

    let subscribe = DataSubscribe {
        stream_id: stream_id.clone(),
        after_seq: 0,
        mode: SubscribeMode::Live as i32,
    };
    let mut subscription = session.subscribe(subscribe).await.expect("subscribe");

    assert_eq!(subscription.subscribe_ok.latest_seq_at_open, 2);

    let batches = subscription
        .collect_batches(1)
        .await
        .expect("collect batches");
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].records.len(), 1);
    assert_eq!(batches[0].records[0].seq, 1);
    assert_eq!(batches[0].records[0].raw_frame, b"frame-one");

    subscription
        .ack(Ack {
            stream_id: stream_id.clone(),
            through_seq: 1,
        })
        .await
        .expect("send ack");

    poll_until(
        || async { !forwarder.acks().is_empty() },
        Duration::from_secs(5),
    )
    .await;

    let acks = forwarder.acks();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].through_seq, 1);
    assert_eq!(acks[0].stream_id, stream_id);

    forwarder.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn connectivity_fault_delay_waits_for_configured_duration() {
    let fault = ConnectivityFault::delayed(Duration::from_secs(5));
    let started = tokio::time::Instant::now();

    fault.apply_delay().await;

    assert_eq!(
        tokio::time::Instant::now() - started,
        Duration::from_secs(5)
    );
}

#[tokio::test]
async fn p2p_harness_data_fault_drop_outbound_suppresses_subscribe_response() {
    let stream_id = vec![8u8; 16];
    let script = ForwarderScript {
        server_hello: Hello {
            min_minor: 1,
            max_minor: 1,
            capabilities: vec!["shared".to_owned()],
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
            catalog_generation: 1,
        },
        catalog: StreamCatalog {
            generation: 1,
            entries: vec![StreamEntry {
                stream_id: stream_id.clone(),
                display_name: "Faulted".to_owned(),
                network_addr: "10.0.0.2:10000".to_owned(),
                reader_connected: true,
                hardware_reader_id: "R2".to_owned(),
            }],
        },
        subscribe_ok: SubscribeOk {
            stream_id: stream_id.clone(),
            earliest_available_seq: 1,
            latest_seq_at_open: 1,
        },
        batches: vec![EventBatch {
            records: vec![ReadRecord {
                stream_id: stream_id.clone(),
                seq: 1,
                epoch: 1,
                raw_frame: b"dropped-frame".to_vec(),
                read_kind: "chip".to_owned(),
                reader_timestamp: 0,
                received_unix_ms: 0,
            }],
            replay: false,
        }],
        caught_up_through: Some(1),
        gap_notice: None,
        data_fault: ConnectivityFault::dropping(),
        echo_subscribed_stream_id: false,
        close_connection_after_data: false,
        control_events: Vec::new(),
        control_pings: 0,
        control_ping_interval: std::time::Duration::from_millis(50),
        config_get_json: String::new(),
        config_restart_needed: false,
        respond_to_config_requests: true,
        reader_control_info_json: None,
        respond_to_reader_control_requests: true,
    };

    let forwarder = MockForwarderPeer::start([3; 32], script)
        .await
        .expect("start forwarder peer");
    let receiver = MockReceiverPeer::start([4; 32])
        .await
        .expect("start receiver peer");

    let session = receiver
        .hello(
            forwarder.node_addr(),
            Hello {
                min_minor: 1,
                max_minor: 1,
                capabilities: vec!["shared".to_owned()],
                max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
                catalog_generation: 0,
            },
        )
        .await
        .expect("hello negotiation should not be faulted");

    let subscribe = DataSubscribe {
        stream_id,
        after_seq: 0,
        mode: SubscribeMode::Live as i32,
    };
    let result =
        tokio::time::timeout(Duration::from_millis(100), session.subscribe(subscribe)).await;

    assert!(
        result.is_err(),
        "dropped outbound data frame should withhold SubscribeOk"
    );
    assert!(forwarder.acks().is_empty());

    forwarder.shutdown().await;
}

#[tokio::test]
async fn p2p_harness_partition_suppresses_inbound_acks() {
    let result = tokio::time::timeout(Duration::from_secs(10), run_partition_ack_test()).await;
    result.expect("partition ack test timed out");
}

async fn run_partition_ack_test() {
    let stream_id = vec![9u8; 16];
    let script = ForwarderScript {
        server_hello: Hello {
            min_minor: 1,
            max_minor: 1,
            capabilities: vec!["shared".to_owned()],
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
            catalog_generation: 1,
        },
        catalog: StreamCatalog {
            generation: 1,
            entries: vec![StreamEntry {
                stream_id: stream_id.clone(),
                display_name: "Partitioned".to_owned(),
                network_addr: "10.0.0.3:10000".to_owned(),
                reader_connected: true,
                hardware_reader_id: "R3".to_owned(),
            }],
        },
        subscribe_ok: SubscribeOk {
            stream_id: stream_id.clone(),
            earliest_available_seq: 1,
            latest_seq_at_open: 1,
        },
        batches: Vec::new(),
        caught_up_through: None,
        gap_notice: None,
        data_fault: ConnectivityFault::partitioned(),
        echo_subscribed_stream_id: false,
        close_connection_after_data: false,
        control_events: Vec::new(),
        control_pings: 0,
        control_ping_interval: std::time::Duration::from_millis(50),
        config_get_json: String::new(),
        config_restart_needed: false,
        respond_to_config_requests: true,
        reader_control_info_json: None,
        respond_to_reader_control_requests: true,
    };

    let forwarder = MockForwarderPeer::start([5; 32], script)
        .await
        .expect("start forwarder peer");
    let client = EndpointBuilder::test([6; 32])
        .bind()
        .await
        .expect("start client endpoint");
    let forwarder_addr = forwarder.node_addr();
    let connection = client
        .connect(forwarder_addr)
        .await
        .expect("connect to forwarder");

    let (mut control_send, mut control_recv) = connection.open_bi().await.expect("open control");
    write_proto_frame(
        &mut control_send,
        &ControlC2F {
            msg: Some(control_c2f::Msg::Hello(Hello {
                min_minor: 1,
                max_minor: 1,
                capabilities: vec!["shared".to_owned()],
                max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
                catalog_generation: 0,
            })),
        },
    )
    .await;
    read_proto_frame::<ControlF2C>(&mut control_recv).await;
    read_proto_frame::<ControlF2C>(&mut control_recv).await;

    let (mut data_send, mut data_recv) = connection.open_bi().await.expect("open data");
    write_proto_frame(
        &mut data_send,
        &DataC2F {
            msg: Some(data_c2f::Msg::DataSubscribe(DataSubscribe {
                stream_id: stream_id.clone(),
                after_seq: 0,
                mode: SubscribeMode::Live as i32,
            })),
        },
    )
    .await;
    write_proto_frame(
        &mut data_send,
        &DataC2F {
            msg: Some(data_c2f::Msg::Ack(Ack {
                stream_id,
                through_seq: 1,
            })),
        },
    )
    .await;
    data_send.finish().expect("finish data writes");

    let _ = data_recv.read_to_end(MAX_FRAME_BYTES + 4).await;
    assert!(
        forwarder.acks().is_empty(),
        "partitioned data plane should not read inbound acks"
    );

    connection.close(0u8.into(), b"done");
    client.close().await;
    forwarder.shutdown().await;
}

async fn write_proto_frame(send: &mut SendStream, message: &impl Message) {
    send.write_all(&encode_frame(message))
        .await
        .expect("write frame");
}

async fn read_proto_frame<M>(recv: &mut RecvStream) -> M
where
    M: Message + Default,
{
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .expect("read frame length");
    let len = u32::from_le_bytes(len_buf) as usize;
    assert!(len <= MAX_FRAME_BYTES);

    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload)
        .await
        .expect("read frame payload");
    M::decode(payload.as_slice()).expect("decode frame")
}

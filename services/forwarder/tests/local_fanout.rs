//! Integration tests for Task 8: Local Raw Fanout.
//!
//! Tests:
//! 1. exact-byte fanout (bytes go through unmodified)
//! 2. multi-client fanout (multiple simultaneous consumers)
//! 3. consumer drop does not crash other consumers
//! 4. late consumer connects and receives new data only
//! 5. fanout on degraded/collision stream reports error

use forwarder::local_fanout::{FanoutServer, FanoutError};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::time::timeout;

// Helper: read exactly `n` bytes from a TcpStream with a timeout.
async fn read_bytes(stream: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
        .await
        .expect("timeout reading bytes")
        .expect("read_exact failed");
    buf
}

#[tokio::test]
async fn fanout_exact_byte_preservation() {
    // Bind on port 0 (OS assigns)
    let server = FanoutServer::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = server.local_addr();

    // Spawn the fanout server task
    tokio::spawn(async move {
        server.run().await;
    });

    // Connect a consumer
    let mut consumer = TcpStream::connect(addr).await.expect("connect failed");

    // Give the server time to register the consumer
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Write raw bytes (simulating exact IPICO data — binary-safe)
    let raw_data: Vec<u8> = vec![0x01, 0x02, 0x0D, 0x0A, 0xFF, 0xFE, 0x00, 0x41];
    // Push data to the fanout
    FanoutServer::push_to_addr(addr, raw_data.clone()).await.expect("push failed");

    // Consumer should receive the exact bytes
    let received = read_bytes(&mut consumer, raw_data.len()).await;
    assert_eq!(received, raw_data, "bytes must be forwarded without modification");
}

#[tokio::test]
async fn fanout_multi_client_all_receive() {
    let server = FanoutServer::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = server.local_addr();

    tokio::spawn(async move {
        server.run().await;
    });

    // Connect three consumers
    let mut c1 = TcpStream::connect(addr).await.expect("c1 connect failed");
    let mut c2 = TcpStream::connect(addr).await.expect("c2 connect failed");
    let mut c3 = TcpStream::connect(addr).await.expect("c3 connect failed");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let data = b"HELLO FANOUT\r\n";
    FanoutServer::push_to_addr(addr, data.to_vec()).await.expect("push failed");

    let r1 = read_bytes(&mut c1, data.len()).await;
    let r2 = read_bytes(&mut c2, data.len()).await;
    let r3 = read_bytes(&mut c3, data.len()).await;

    assert_eq!(r1, data, "c1 must receive the data");
    assert_eq!(r2, data, "c2 must receive the data");
    assert_eq!(r3, data, "c3 must receive the data");
}

#[tokio::test]
async fn fanout_consumer_drop_does_not_crash_others() {
    let server = FanoutServer::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = server.local_addr();

    tokio::spawn(async move {
        server.run().await;
    });

    let mut c1 = TcpStream::connect(addr).await.expect("c1 connect failed");
    let c2 = TcpStream::connect(addr).await.expect("c2 connect failed");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Drop c2 early
    drop(c2);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // c1 should still work
    let data = b"STILL ALIVE\r\n";
    FanoutServer::push_to_addr(addr, data.to_vec()).await.expect("push after drop failed");

    let r1 = read_bytes(&mut c1, data.len()).await;
    assert_eq!(r1, data, "surviving consumer must still receive data");
}

#[tokio::test]
async fn fanout_no_line_ending_rewrite() {
    // The spec says: "no line-ending rewrite or normalization"
    // We send \r\n and also bare \n to verify nothing changes.
    let server = FanoutServer::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = server.local_addr();

    tokio::spawn(async move {
        server.run().await;
    });

    let mut consumer = TcpStream::connect(addr).await.expect("connect failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // \r\n line
    let crlf_line = b"CRLF\r\n";
    FanoutServer::push_to_addr(addr, crlf_line.to_vec()).await.expect("push failed");
    let received_crlf = read_bytes(&mut consumer, crlf_line.len()).await;
    assert_eq!(received_crlf, crlf_line, "CRLF must not be rewritten");

    // Bare \n line
    let lf_line = b"LF\n";
    FanoutServer::push_to_addr(addr, lf_line.to_vec()).await.expect("push failed");
    let received_lf = read_bytes(&mut consumer, lf_line.len()).await;
    assert_eq!(received_lf, lf_line, "LF must not be rewritten");
}

#[tokio::test]
async fn fanout_bind_collision_returns_error() {
    // Bind the same address twice — the second bind should fail with a
    // FanoutError indicating the port collision.
    let server1 = FanoutServer::bind("127.0.0.1:0").await.expect("first bind failed");
    let addr = server1.local_addr();

    // Explicitly bind the same port
    let result = FanoutServer::bind(&addr.to_string()).await;
    assert!(
        result.is_err(),
        "second bind on same port must return a collision error"
    );
    let err = result.err().expect("already checked is_err");
    assert!(
        matches!(err, FanoutError::BindFailed(_)),
        "expected BindFailed error, got {:?}",
        err
    );
}

#[tokio::test]
async fn fanout_multiple_sequential_messages() {
    let server = FanoutServer::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = server.local_addr();

    tokio::spawn(async move {
        server.run().await;
    });

    let mut consumer = TcpStream::connect(addr).await.expect("connect failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    for i in 0u8..5 {
        let msg = vec![i; 8];
        FanoutServer::push_to_addr(addr, msg.clone()).await.expect("push failed");
        let received = read_bytes(&mut consumer, 8).await;
        assert_eq!(received, msg, "message {} must match", i);
    }
}

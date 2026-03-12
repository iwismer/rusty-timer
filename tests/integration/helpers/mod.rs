//! Shared integration test helpers.
//!
//! Included in each test file via:
//! ```rust,ignore
//! #[path = "helpers/mod.rs"] mod helpers;
//! ```
//!
//! Provides common harness functions used across the integration test suite.

// Not every helper is used by every test binary — suppress dead_code warnings.
#![allow(dead_code)]

use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;

/// Insert a device token into the server DB for testing.
pub async fn insert_token(
    pool: &sqlx::PgPool,
    device_id: &str,
    device_type: &str,
    raw_token: &[u8],
) {
    let hash = Sha256::digest(raw_token);
    let hash_bytes: Vec<u8> = hash.as_slice().to_vec();
    sqlx::query(
        "INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3)",
    )
    .bind(hash_bytes)
    .bind(device_type)
    .bind(device_id)
    .execute(pool)
    .await
    .unwrap();
}

/// Spin up an in-process server against the given Postgres pool.
/// Returns the local address the server is bound to.
pub async fn start_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let state = server::AppState::new(pool);
    let router = server::build_router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind server");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });
    // Give the server a moment to start accepting connections.
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
}

/// Perform the forwarder hello handshake and return the session_id.
pub async fn forwarder_handshake(
    client: &mut MockWsClient,
    forwarder_id: &str,
    reader_ips: Vec<String>,
) -> String {
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: forwarder_id.to_owned(),
            reader_ips,
            display_name: None,
        }))
        .await
        .unwrap();
    match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(hb) => {
            assert!(!hb.session_id.is_empty(), "session_id must not be empty");
            hb.session_id
        }
        other => panic!("expected Heartbeat after forwarder hello, got {:?}", other),
    }
}

/// Perform the receiver hello handshake with a resume cursor list.
/// Returns the session_id.
pub async fn receiver_handshake(
    client: &mut MockWsClient,
    receiver_id: &str,
    resume: Vec<ResumeCursor>,
) -> String {
    let mut seen = std::collections::HashSet::new();
    let streams: Vec<StreamRef> = resume
        .iter()
        .filter_map(|cursor| {
            let key = (cursor.forwarder_id.clone(), cursor.reader_ip.clone());
            if seen.insert(key.clone()) {
                Some(StreamRef {
                    forwarder_id: key.0,
                    reader_ip: key.1,
                })
            } else {
                None
            }
        })
        .collect();
    client
        .send_message(&WsMessage::ReceiverHelloV12(ReceiverHelloV12 {
            receiver_id: receiver_id.to_owned(),
            mode: ReceiverMode::Live {
                streams,
                earliest_epochs: vec![],
            },
            resume,
        }))
        .await
        .unwrap();
    loop {
        match client.recv_message().await.unwrap() {
            WsMessage::Heartbeat(hb) => {
                assert!(!hb.session_id.is_empty(), "session_id must not be empty");
                break hb.session_id;
            }
            WsMessage::ReceiverModeApplied(_) => {}
            other => panic!("expected Heartbeat or ReceiverModeApplied, got {:?}", other),
        }
    }
}

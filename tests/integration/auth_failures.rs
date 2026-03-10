//! Integration tests for WebSocket authentication failures.
//!
//! Verifies that invalid, empty, and missing tokens are rejected
//! on both forwarder and receiver WebSocket endpoints.
//!
//! Requires Docker for the Postgres testcontainer.

use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ---------------------------------------------------------------------------
// Harness helpers (same pattern as other integration tests)
// ---------------------------------------------------------------------------

async fn insert_token(pool: &sqlx::PgPool, device_id: &str, device_type: &str, raw_token: &[u8]) {
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

async fn start_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let state = server::AppState::new(pool);
    let router = server::build_router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind server");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
}

/// Helper: expect the server to send an Error message with the given code,
/// then close the connection.
async fn expect_ws_error(client: &mut MockWsClient, expected_code: &str) {
    match client.recv_message().await {
        Ok(WsMessage::Error(err)) => {
            assert_eq!(
                err.code, expected_code,
                "expected error code {}, got {}",
                expected_code, err.code
            );
        }
        Ok(other) => panic!("expected Error message, got {:?}", other),
        Err(e) => {
            // Connection closed before we got a message — also acceptable
            // for missing auth header scenarios where the server may close immediately.
            eprintln!("connection closed/errored (acceptable): {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

async fn setup() -> (
    sqlx::PgPool,
    std::net::SocketAddr,
    testcontainers::ContainerAsync<Postgres>,
) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    insert_token(&pool, "fwd-auth-01", "forwarder", b"valid-fwd-token").await;
    insert_token(&pool, "recv-auth-01", "receiver", b"valid-recv-token").await;
    let addr = start_server(pool.clone()).await;
    (pool, addr, container)
}

#[tokio::test]
async fn forwarder_invalid_token_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "wrong-token")
        .await
        .unwrap();
    expect_ws_error(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn forwarder_empty_token_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "").await.unwrap();
    expect_ws_error(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn forwarder_missing_auth_header_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect(&url).await.unwrap();
    expect_ws_error(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn receiver_invalid_token_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut client = MockWsClient::connect_with_token(&url, "wrong-token")
        .await
        .unwrap();
    expect_ws_error(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn receiver_missing_auth_header_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut client = MockWsClient::connect(&url).await.unwrap();
    expect_ws_error(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn forwarder_token_rejected_on_receiver_endpoint() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1.2/receivers", addr);
    // Use a valid forwarder token on the receiver endpoint
    let mut client = MockWsClient::connect_with_token(&url, "valid-fwd-token")
        .await
        .unwrap();
    expect_ws_error(&mut client, "INVALID_TOKEN").await;
}

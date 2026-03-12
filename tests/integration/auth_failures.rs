//! Integration tests for WebSocket authentication failures.
//!
//! Verifies that invalid, empty, and missing tokens are rejected
//! on both forwarder and receiver WebSocket endpoints.
//!
//! Requires Docker for the Postgres testcontainer.

#[path = "helpers/mod.rs"]
mod helpers;
use helpers::{insert_token, start_server};

use rt_protocol::*;
use rt_test_utils::MockWsClient;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

/// Expect the server to send an Error message with the given code.
/// Panics if the connection closes without an error frame or a different message arrives.
async fn expect_ws_error_strict(client: &mut MockWsClient, expected_code: &str) {
    match client.recv_message().await {
        Ok(WsMessage::Error(err)) => {
            assert_eq!(
                err.code, expected_code,
                "expected error code {}, got {}",
                expected_code, err.code
            );
        }
        Ok(other) => panic!("expected Error message, got {:?}", other),
        Err(e) => panic!("connection closed before receiving Error message: {e}"),
    }
}

/// Expect the server to reject the connection — either via an Error message
/// or by closing the connection immediately (acceptable for missing-header scenarios).
async fn expect_ws_error_or_close(client: &mut MockWsClient, expected_code: &str) {
    match client.recv_message().await {
        Ok(WsMessage::Error(err)) => {
            assert_eq!(
                err.code, expected_code,
                "expected error code {}, got {}",
                expected_code, err.code
            );
        }
        Ok(other) => panic!("expected Error message, got {:?}", other),
        Err(_) => {
            // Connection closed before we got a message — acceptable
            // for missing auth header scenarios where the server may close immediately.
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
    expect_ws_error_strict(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn forwarder_empty_token_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "").await.unwrap();
    expect_ws_error_strict(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn forwarder_missing_auth_header_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect(&url).await.unwrap();
    expect_ws_error_or_close(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn receiver_invalid_token_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut client = MockWsClient::connect_with_token(&url, "wrong-token")
        .await
        .unwrap();
    expect_ws_error_strict(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn receiver_empty_token_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut client = MockWsClient::connect_with_token(&url, "").await.unwrap();
    expect_ws_error_strict(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn receiver_missing_auth_header_rejected() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut client = MockWsClient::connect(&url).await.unwrap();
    expect_ws_error_or_close(&mut client, "INVALID_TOKEN").await;
}

#[tokio::test]
async fn forwarder_token_rejected_on_receiver_endpoint() {
    let (_pool, addr, _container) = setup().await;
    let url = format!("ws://{}/ws/v1.2/receivers", addr);
    // Use a valid forwarder token on the receiver endpoint
    let mut client = MockWsClient::connect_with_token(&url, "valid-fwd-token")
        .await
        .unwrap();
    expect_ws_error_strict(&mut client, "INVALID_TOKEN").await;
}

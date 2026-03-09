//! Integration tests for server reader-control proxy behavior.

use rt_protocol::{
    ForwarderHello, ReaderConnectionState, ReaderControlAction, ReaderControlResponse, ReaderInfo,
    ReaderInfoUpdate, WsMessage,
};
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn insert_token(pool: &sqlx::PgPool, device_id: &str, device_type: &str, raw_token: &[u8]) {
    let hash = Sha256::digest(raw_token);
    sqlx::query!(
        "INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3)",
        hash.as_slice(),
        device_type,
        device_id
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn make_server(pool: sqlx::PgPool) -> (std::net::SocketAddr, server::AppState) {
    let app_state = server::AppState::new(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state_clone = app_state.clone();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });
    (addr, state_clone)
}

async fn setup_pg() -> (testcontainers::ContainerAsync<Postgres>, sqlx::PgPool) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    (container, pool)
}

async fn connect_forwarder(
    addr: std::net::SocketAddr,
    device_id: &str,
    token: &str,
    reader_ips: Vec<String>,
) -> MockWsClient {
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, token)
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: device_id.to_owned(),
        reader_ips,
        display_name: Some("Test Forwarder".to_owned()),
    }))
    .await
    .unwrap();
    // Consume initial heartbeat
    let _ = fwd.recv_message().await.unwrap();
    fwd
}

#[tokio::test]
async fn reader_control_returns_200_on_success() {
    let (_container, pool) = setup_pg().await;
    let (addr, _state) = make_server(pool.clone()).await;

    let device_id = "fwd-rc-200";
    let reader_ip = "10.0.1.1:10000";
    insert_token(&pool, device_id, "forwarder", b"fwd-rc-200-token").await;
    let mut fwd = connect_forwarder(
        addr,
        device_id,
        "fwd-rc-200-token",
        vec![reader_ip.to_owned()],
    )
    .await;

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .get(format!(
                "http://{}/api/v1/forwarders/{}/readers/{}/info",
                addr, device_id, reader_ip
            ))
            .send()
            .await
            .unwrap()
    });

    let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await
        .expect("timed out waiting for proxied control request")
        .unwrap();
    let request_id = match proxied {
        WsMessage::ReaderControlRequest(req) => {
            assert_eq!(req.action, ReaderControlAction::GetInfo);
            assert_eq!(req.reader_ip, reader_ip);
            req.request_id
        }
        other => panic!("expected ReaderControlRequest, got {:?}", other),
    };

    fwd.send_message(&WsMessage::ReaderControlResponse(ReaderControlResponse {
        request_id,
        reader_ip: reader_ip.to_owned(),
        success: true,
        error: None,
        reader_info: Some(ReaderInfo {
            banner: Some("IPICO Reader".to_owned()),
            hardware: None,
            config: None,
            tto_enabled: None,
            clock: None,
            estimated_stored_reads: None,
            recording: None,
            connect_failures: 0,
        }),
    }))
    .await
    .unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(body["reader_info"].is_object());

    fwd.close().await.unwrap();
}

#[tokio::test]
async fn reader_control_returns_502_on_forwarder_error() {
    let (_container, pool) = setup_pg().await;
    let (addr, _state) = make_server(pool.clone()).await;

    let device_id = "fwd-rc-502-err";
    let reader_ip = "10.0.2.1:10000";
    insert_token(&pool, device_id, "forwarder", b"fwd-rc-502-err-token").await;
    let mut fwd = connect_forwarder(
        addr,
        device_id,
        "fwd-rc-502-err-token",
        vec![reader_ip.to_owned()],
    )
    .await;

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .get(format!(
                "http://{}/api/v1/forwarders/{}/readers/{}/info",
                addr, device_id, reader_ip
            ))
            .send()
            .await
            .unwrap()
    });

    let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await
        .expect("timed out waiting for proxied control request")
        .unwrap();
    let request_id = match proxied {
        WsMessage::ReaderControlRequest(req) => req.request_id,
        other => panic!("expected ReaderControlRequest, got {:?}", other),
    };

    fwd.send_message(&WsMessage::ReaderControlResponse(ReaderControlResponse {
        request_id,
        reader_ip: reader_ip.to_owned(),
        success: false,
        error: Some("reader not connected".to_owned()),
        reader_info: None,
    }))
    .await
    .unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "READER_CONTROL_ERROR");

    fwd.close().await.unwrap();
}

#[tokio::test]
async fn reader_control_returns_502_on_forwarder_disconnect() {
    let (_container, pool) = setup_pg().await;
    let (addr, _state) = make_server(pool.clone()).await;

    let device_id = "fwd-rc-502-dc";
    let reader_ip = "10.0.3.1:10000";
    insert_token(&pool, device_id, "forwarder", b"fwd-rc-502-dc-token").await;
    let mut fwd = connect_forwarder(
        addr,
        device_id,
        "fwd-rc-502-dc-token",
        vec![reader_ip.to_owned()],
    )
    .await;

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .get(format!(
                "http://{}/api/v1/forwarders/{}/readers/{}/info",
                addr, device_id, reader_ip
            ))
            .send()
            .await
            .unwrap()
    });

    let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await
        .expect("timed out waiting for proxied control request")
        .unwrap();
    match proxied {
        WsMessage::ReaderControlRequest(req) => {
            assert_eq!(req.action, ReaderControlAction::GetInfo);
        }
        other => panic!("expected ReaderControlRequest, got {:?}", other),
    }

    // Close without replying
    fwd.close().await.unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "FORWARDER_DISCONNECTED");
}

#[tokio::test]
async fn fire_and_forget_returns_202() {
    let (_container, pool) = setup_pg().await;
    let (addr, _state) = make_server(pool.clone()).await;

    let device_id = "fwd-rc-202-faf";
    let reader_ip = "10.0.4.1:10000";
    insert_token(&pool, device_id, "forwarder", b"fwd-rc-202-faf-token").await;
    let mut fwd = connect_forwarder(
        addr,
        device_id,
        "fwd-rc-202-faf-token",
        vec![reader_ip.to_owned()],
    )
    .await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}/api/v1/forwarders/{}/readers/{}/clear-records",
            addr, device_id, reader_ip
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);

    // Verify the forwarder received the command
    let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await
        .expect("timed out waiting for fire-and-forget command")
        .unwrap();
    match proxied {
        WsMessage::ReaderControlRequest(req) => {
            assert_eq!(req.action, ReaderControlAction::ClearRecords);
            assert_eq!(req.reader_ip, reader_ip);
        }
        other => panic!("expected ReaderControlRequest, got {:?}", other),
    }

    fwd.close().await.unwrap();
}

#[tokio::test]
async fn reader_states_cleaned_up_on_forwarder_disconnect() {
    let (_container, pool) = setup_pg().await;
    let (addr, state) = make_server(pool.clone()).await;

    let device_id = "fwd-rc-cleanup";
    let reader_ip = "10.0.5.1:10000";
    insert_token(&pool, device_id, "forwarder", b"fwd-rc-cleanup-token").await;
    let mut fwd = connect_forwarder(
        addr,
        device_id,
        "fwd-rc-cleanup-token",
        vec![reader_ip.to_owned()],
    )
    .await;

    // Send a ReaderInfoUpdate to populate the cache
    fwd.send_message(&WsMessage::ReaderInfoUpdate(ReaderInfoUpdate {
        reader_ip: reader_ip.to_owned(),
        state: ReaderConnectionState::Connected,
        reader_info: None,
    }))
    .await
    .unwrap();

    // Wait briefly for the server to process the update
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify cache is populated
    {
        let cache = state.reader_states.read().await;
        let key = format!("{}:{}", device_id, reader_ip);
        assert!(
            cache.contains_key(&key),
            "reader state should be cached after ReaderInfoUpdate"
        );
    }

    // Disconnect forwarder
    fwd.close().await.unwrap();

    // Wait for cleanup
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify cache is cleaned up
    {
        let cache = state.reader_states.read().await;
        let key = format!("{}:{}", device_id, reader_ip);
        assert!(
            !cache.contains_key(&key),
            "reader state should be removed after forwarder disconnect"
        );
    }
}

#[tokio::test]
async fn reader_control_returns_404_when_forwarder_not_connected() {
    let (_container, pool) = setup_pg().await;
    let (addr, _state) = make_server(pool.clone()).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!(
            "http://{}/api/v1/forwarders/{}/readers/{}/info",
            addr, "nonexistent", "10.0.0.1:10000"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reader_control_returns_400_for_invalid_reader_ip() {
    let (_container, pool) = setup_pg().await;
    let (addr, _state) = make_server(pool.clone()).await;

    let device_id = "fwd-rc-400-ip";
    let reader_ip = "10.0.6.1:10000";
    insert_token(&pool, device_id, "forwarder", b"fwd-rc-400-ip-token").await;
    let _fwd = connect_forwarder(
        addr,
        device_id,
        "fwd-rc-400-ip-token",
        vec![reader_ip.to_owned()],
    )
    .await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!(
            "http://{}/api/v1/forwarders/{}/readers/{}/info",
            addr, device_id, "bad-ip"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "INVALID_READER_IP");
}

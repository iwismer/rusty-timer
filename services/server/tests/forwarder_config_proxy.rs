//! Integration tests for server forwarder-config proxy behavior.

use rt_protocol::{ForwarderHello, WsMessage};
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

async fn make_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let app_state = server::AppState::new(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state))
            .await
            .unwrap();
    });
    addr
}

#[tokio::test]
async fn get_forwarder_config_returns_bad_gateway_when_forwarder_disconnects_before_reply() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-cfg-proxy", "forwarder", b"fwd-cfg-proxy-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-cfg-proxy-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-cfg-proxy".to_owned(),
        reader_ips: vec!["10.10.0.1:10000".to_owned()],
        resume: vec![],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .get(format!(
                "http://{}/api/v1/forwarders/{}/config",
                addr, "fwd-cfg-proxy"
            ))
            .send()
            .await
            .unwrap()
    });

    let proxied = fwd.recv_message().await.unwrap();
    match proxied {
        WsMessage::ConfigGetRequest(req) => {
            assert!(!req.request_id.is_empty());
        }
        other => panic!("expected ConfigGetRequest, got {:?}", other),
    }

    fwd.close().await.unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::BAD_GATEWAY,
        "disconnect before reply should surface as 502"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "FORWARDER_DISCONNECTED");
}

#[tokio::test]
async fn get_forwarder_config_returns_bad_gateway_when_forwarder_reports_config_error() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-cfg-proxy-error",
        "forwarder",
        b"fwd-cfg-proxy-error-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-cfg-proxy-error-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-cfg-proxy-error".to_owned(),
        reader_ips: vec!["10.10.0.2:10000".to_owned()],
        resume: vec![],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .get(format!(
                "http://{}/api/v1/forwarders/{}/config",
                addr, "fwd-cfg-proxy-error"
            ))
            .send()
            .await
            .unwrap()
    });

    let proxied = fwd.recv_message().await.unwrap();
    let request_id = match proxied {
        WsMessage::ConfigGetRequest(req) => req.request_id,
        other => panic!("expected ConfigGetRequest, got {:?}", other),
    };

    fwd.send_message(&WsMessage::ConfigGetResponse(
        rt_protocol::ConfigGetResponse {
            request_id,
            ok: false,
            error: Some("File read error: permission denied".to_owned()),
            config: serde_json::Value::Null,
            restart_needed: false,
        },
    ))
    .await
    .unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::BAD_GATEWAY,
        "forwarder config-get failure should surface as 502"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "FORWARDER_CONFIG_ERROR");
}

#[tokio::test]
async fn get_forwarder_config_returns_gateway_timeout_when_forwarder_never_replies() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-cfg-proxy-timeout",
        "forwarder",
        b"fwd-cfg-proxy-timeout-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-cfg-proxy-timeout-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-cfg-proxy-timeout".to_owned(),
        reader_ips: vec!["10.10.0.3:10000".to_owned()],
        resume: vec![],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .get(format!(
                "http://{}/api/v1/forwarders/{}/config",
                addr, "fwd-cfg-proxy-timeout"
            ))
            .send()
            .await
            .unwrap()
    });

    let proxied = fwd.recv_message().await.unwrap();
    match proxied {
        WsMessage::ConfigGetRequest(req) => {
            assert!(!req.request_id.is_empty());
        }
        other => panic!("expected ConfigGetRequest, got {:?}", other),
    }

    // Keep the connection open and intentionally do not reply.
    let response = request_task.await.unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::GATEWAY_TIMEOUT,
        "no reply should surface as 504 timeout"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "TIMEOUT");

    fwd.close().await.unwrap();
}

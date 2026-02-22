//! Integration tests for server forwarder-config proxy behavior.

use rt_protocol::{ForwarderHello, WsMessage};
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::sync::{mpsc, oneshot};

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
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });
    addr
}

#[tokio::test]
async fn get_forwarder_config_returns_gateway_timeout_when_command_queue_is_saturated() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let (tx, mut rx) = mpsc::channel::<server::state::ForwarderCommand>(1);
    let (reply_tx, _reply_rx) = oneshot::channel();
    tx.send(server::state::ForwarderCommand::Restart {
        request_id: "prefill".to_owned(),
        reply: reply_tx,
    })
    .await
    .unwrap();
    {
        let mut senders = app_state.forwarder_command_senders.write().await;
        senders.insert("fwd-saturated".to_owned(), tx);
    }

    // Keep receiver alive but do not drain it, so the sender remains saturated.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let _ = rx.recv().await;
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap();
    let response = client
        .get(format!(
            "http://{}/api/v1/forwarders/{}/config",
            addr, "fwd-saturated"
        ))
        .send()
        .await
        .expect("request should complete with timeout response");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::GATEWAY_TIMEOUT,
        "saturated command queue should surface as 504 timeout"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "TIMEOUT");
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

    let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await
        .expect("timed out waiting for proxied control request")
        .unwrap();
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

    let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await
        .expect("timed out waiting for proxied control request")
        .unwrap();
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

#[tokio::test]
async fn set_forwarder_config_returns_bad_request_for_forwarder_validation_error() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-cfg-set-validation",
        "forwarder",
        b"fwd-cfg-set-validation-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-cfg-set-validation-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-cfg-set-validation".to_owned(),
        reader_ips: vec!["10.10.0.4:10000".to_owned()],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .post(format!(
                "http://{}/api/v1/forwarders/{}/config/{}",
                addr, "fwd-cfg-set-validation", "general"
            ))
            .json(&serde_json::json!({"display_name":"New Name"}))
            .send()
            .await
            .unwrap()
    });

    let proxied = fwd.recv_message().await.unwrap();
    let request_id = match proxied {
        WsMessage::ConfigSetRequest(req) => req.request_id,
        other => panic!("expected ConfigSetRequest, got {:?}", other),
    };

    fwd.send_message(&WsMessage::ConfigSetResponse(
        rt_protocol::ConfigSetResponse {
            request_id,
            ok: false,
            error: Some("display_name must not be empty".to_owned()),
            restart_needed: false,
            status_code: Some(400),
        },
    ))
    .await
    .unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn set_forwarder_config_returns_bad_gateway_for_forwarder_internal_error() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-cfg-set-internal",
        "forwarder",
        b"fwd-cfg-set-internal-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-cfg-set-internal-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-cfg-set-internal".to_owned(),
        reader_ips: vec!["10.10.0.5:10000".to_owned()],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .post(format!(
                "http://{}/api/v1/forwarders/{}/config/{}",
                addr, "fwd-cfg-set-internal", "general"
            ))
            .json(&serde_json::json!({"display_name":"New Name"}))
            .send()
            .await
            .unwrap()
    });

    let proxied = fwd.recv_message().await.unwrap();
    let request_id = match proxied {
        WsMessage::ConfigSetRequest(req) => req.request_id,
        other => panic!("expected ConfigSetRequest, got {:?}", other),
    };

    fwd.send_message(&WsMessage::ConfigSetResponse(
        rt_protocol::ConfigSetResponse {
            request_id,
            ok: false,
            error: Some("file write error".to_owned()),
            restart_needed: false,
            status_code: Some(500),
        },
    ))
    .await
    .unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn control_restart_device_proxies_as_config_set_control_action() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-control-proxy",
        "forwarder",
        b"fwd-control-proxy-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-control-proxy-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-control-proxy".to_owned(),
        reader_ips: vec!["10.10.0.6:10000".to_owned()],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .post(format!(
                "http://{}/api/v1/forwarders/{}/control/restart-device",
                addr, "fwd-control-proxy"
            ))
            .send()
            .await
            .unwrap()
    });

    let request_id = loop {
        let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
            .await
            .expect("timed out waiting for proxied control request")
            .unwrap();
        match proxied {
            WsMessage::Heartbeat(_) => continue,
            WsMessage::ConfigSetRequest(req) => {
                assert_eq!(req.section, "control");
                assert_eq!(req.payload["action"], "restart_device");
                break req.request_id;
            }
            other => panic!("expected ConfigSetRequest, got {:?}", other),
        }
    };

    fwd.send_message(&WsMessage::ConfigSetResponse(
        rt_protocol::ConfigSetResponse {
            request_id,
            ok: true,
            error: None,
            restart_needed: false,
            status_code: None,
        },
    ))
    .await
    .unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn control_restart_device_preserves_forwarder_403_status() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-control-403",
        "forwarder",
        b"fwd-control-403-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-control-403-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-control-403".to_owned(),
        reader_ips: vec!["10.10.0.8:10000".to_owned()],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .post(format!(
                "http://{}/api/v1/forwarders/{}/control/restart-device",
                addr, "fwd-control-403"
            ))
            .send()
            .await
            .unwrap()
    });

    let request_id = loop {
        let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
            .await
            .expect("timed out waiting for proxied control request")
            .unwrap();
        match proxied {
            WsMessage::Heartbeat(_) => continue,
            WsMessage::ConfigSetRequest(req) => {
                assert_eq!(req.section, "control");
                assert_eq!(req.payload["action"], "restart_device");
                break req.request_id;
            }
            other => panic!("expected ConfigSetRequest, got {:?}", other),
        }
    };

    fwd.send_message(&WsMessage::ConfigSetResponse(
        rt_protocol::ConfigSetResponse {
            request_id,
            ok: false,
            error: Some("power actions disabled".to_owned()),
            restart_needed: false,
            status_code: Some(403),
        },
    ))
    .await
    .unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "power actions disabled");
    assert_eq!(body["status_code"], 403);
}

#[tokio::test]
async fn control_restart_device_returns_bad_gateway_when_forwarder_disconnects_before_reply() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-control-disconnect",
        "forwarder",
        b"fwd-control-disconnect-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-control-disconnect-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-control-disconnect".to_owned(),
        reader_ips: vec!["10.10.0.7:10000".to_owned()],
        display_name: Some("Proxy Target".to_owned()),
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap(); // initial heartbeat

    let client = reqwest::Client::new();
    let request_task = tokio::spawn(async move {
        client
            .post(format!(
                "http://{}/api/v1/forwarders/{}/control/restart-device",
                addr, "fwd-control-disconnect"
            ))
            .send()
            .await
            .unwrap()
    });

    loop {
        let proxied = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
            .await
            .expect("timed out waiting for proxied control request")
            .unwrap();
        match proxied {
            WsMessage::Heartbeat(_) => continue,
            WsMessage::ConfigSetRequest(req) => {
                assert_eq!(req.section, "control");
                assert_eq!(req.payload["action"], "restart_device");
                break;
            }
            other => panic!("expected ConfigSetRequest, got {:?}", other),
        }
    }

    fwd.close().await.unwrap();

    let response = request_task.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "FORWARDER_DISCONNECTED");
}

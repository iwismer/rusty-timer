use rt_protocol::{ForwarderEventBatch, ForwarderHello, ReadEvent, WsMessage};
use rt_test_utils::MockWsClient;
use server::announcer::AnnouncerInputEvent;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

async fn start_server(
    pool: sqlx::PgPool,
) -> (
    server::AppState,
    std::net::SocketAddr,
    tokio::task::JoinHandle<()>,
) {
    let app_state = server::AppState::new(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = app_state.clone();
    let handle = tokio::spawn(async move {
        axum::serve(listener, server::build_router(state, None))
            .await
            .unwrap();
    });
    (app_state, addr, handle)
}

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

#[tokio::test]
async fn enable_requires_non_empty_streams() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (_app_state, addr, _server_task) = start_server(pool).await;

    let resp = reqwest::Client::new()
        .put(format!("http://{addr}/api/v1/announcer/config"))
        .json(&serde_json::json!({
            "enabled": true,
            "selected_stream_ids": [],
            "max_list_size": 25
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "BAD_REQUEST");
}

#[tokio::test]
async fn reset_clears_rows_and_finisher_count() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (app_state, addr, _server_task) = start_server(pool).await;

    {
        let mut runtime = app_state.announcer_runtime.write().await;
        let _ = runtime.ingest(
            AnnouncerInputEvent {
                stream_id: Uuid::new_v4(),
                seq: 1,
                chip_id: "000000123456".to_owned(),
                bib: Some(101),
                display_name: "Runner".to_owned(),
                reader_timestamp: Some("10:00:00".to_owned()),
                received_at: chrono::Utc::now(),
            },
            25,
        );
        assert_eq!(runtime.finisher_count(), 1);
        assert_eq!(runtime.rows().len(), 1);
    }

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/announcer/reset"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);

    let runtime = app_state.announcer_runtime.read().await;
    assert_eq!(runtime.finisher_count(), 0);
    assert!(runtime.rows().is_empty());
}

#[tokio::test]
async fn disabled_state_reports_public_disabled() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (_app_state, addr, _server_task) = start_server(pool).await;

    let resp = reqwest::get(format!("http://{addr}/api/v1/announcer/state"))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["public_enabled"], serde_json::Value::Bool(false));
    assert_eq!(body["finisher_count"], serde_json::Value::Number(0.into()));
    assert_eq!(body["rows"], serde_json::json!([]));
}

#[tokio::test]
async fn disabled_public_state_redacts_seeded_runtime_rows() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (app_state, addr, _server_task) = start_server(pool).await;

    {
        let mut runtime = app_state.announcer_runtime.write().await;
        let _ = runtime.ingest(
            AnnouncerInputEvent {
                stream_id: Uuid::new_v4(),
                seq: 1,
                chip_id: "000000999999".to_owned(),
                bib: Some(99),
                display_name: "Seed Runner".to_owned(),
                reader_timestamp: Some("10:00:00".to_owned()),
                received_at: chrono::Utc::now(),
            },
            25,
        );
        assert_eq!(runtime.finisher_count(), 1);
        assert_eq!(runtime.rows().len(), 1);
    }

    let resp = reqwest::get(format!("http://{addr}/api/v1/announcer/state"))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["public_enabled"], serde_json::json!(false));
    assert_eq!(body["finisher_count"], serde_json::json!(0));
    assert_eq!(body["rows"], serde_json::json!([]));
}

#[tokio::test]
async fn public_state_endpoint_redacts_internal_fields_when_disabled() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (_app_state, addr, _server_task) = start_server(pool).await;

    let resp = reqwest::get(format!("http://{addr}/api/v1/public/announcer/state"))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    let obj = body
        .as_object()
        .expect("public announcer state should be object");

    assert_eq!(obj.get("public_enabled"), Some(&serde_json::json!(false)));
    assert_eq!(obj.get("finisher_count"), Some(&serde_json::json!(0)));
    assert_eq!(obj.get("rows"), Some(&serde_json::json!([])));
    assert_eq!(obj.get("max_list_size"), Some(&serde_json::json!(25)));
    assert_eq!(obj.len(), 4, "public state must only contain public fields");

    for forbidden in [
        "enabled",
        "enabled_until",
        "selected_stream_ids",
        "updated_at",
    ] {
        assert!(
            !obj.contains_key(forbidden),
            "public state leaked internal field: {forbidden}"
        );
    }
}

#[tokio::test]
async fn public_state_endpoint_returns_sanitized_rows_when_enabled() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, online) VALUES ($1, $2, $3, $4)",
    )
    .bind(stream_id)
    .bind("fwd-public-state")
    .bind("10.200.0.1:10000")
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE announcer_config SET enabled = true, selected_stream_ids = $1, max_list_size = 10",
    )
    .bind(vec![stream_id])
    .execute(&pool)
    .await
    .unwrap();

    let (app_state, addr, _server_task) = start_server(pool).await;
    {
        let mut runtime = app_state.announcer_runtime.write().await;
        let _ = runtime.ingest(
            AnnouncerInputEvent {
                stream_id,
                seq: 55,
                chip_id: "000000777777".to_owned(),
                bib: Some(777),
                display_name: "Public Runner".to_owned(),
                reader_timestamp: Some("10:00:55".to_owned()),
                received_at: chrono::Utc::now(),
            },
            25,
        );
    }

    let resp = reqwest::get(format!("http://{addr}/api/v1/public/announcer/state"))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();

    assert_eq!(body["public_enabled"], serde_json::json!(true));
    assert_eq!(body["finisher_count"], serde_json::json!(1));
    assert_eq!(body["max_list_size"], serde_json::json!(10));
    let rows = body["rows"]
        .as_array()
        .expect("public state rows should be array");
    assert_eq!(rows.len(), 1);
    let row = rows[0]
        .as_object()
        .expect("public state row should be object");
    assert_eq!(
        row.get("display_name"),
        Some(&serde_json::json!("Public Runner"))
    );
    assert_eq!(row.get("bib"), Some(&serde_json::json!(777)));
    assert_eq!(
        row.get("reader_timestamp"),
        Some(&serde_json::json!("10:00:55"))
    );
    assert!(
        row.get("announcement_id")
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "announcement_id should be present and numeric"
    );
    assert_eq!(row.len(), 4, "public row must only contain public fields");

    for forbidden in ["chip_id", "stream_id", "seq", "received_at"] {
        assert!(
            !row.contains_key(forbidden),
            "public row leaked internal field: {forbidden}"
        );
    }
}

#[tokio::test]
async fn config_update_with_different_stream_selection_resets_runtime() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_a = Uuid::new_v4();
    let stream_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, online) VALUES
         ($1, 'fwd-a', '10.100.0.1:10000', false),
         ($2, 'fwd-b', '10.100.0.2:10000', false)",
    )
    .bind(stream_a)
    .bind(stream_b)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "UPDATE announcer_config
         SET enabled = true, selected_stream_ids = $1, max_list_size = 25",
    )
    .bind(vec![stream_a])
    .execute(&pool)
    .await
    .unwrap();

    let (app_state, addr, _server_task) = start_server(pool).await;
    {
        let mut runtime = app_state.announcer_runtime.write().await;
        let _ = runtime.ingest(
            AnnouncerInputEvent {
                stream_id: stream_a,
                seq: 1,
                chip_id: "000000777777".to_owned(),
                bib: Some(7),
                display_name: "Runner A".to_owned(),
                reader_timestamp: Some("10:00:00".to_owned()),
                received_at: chrono::Utc::now(),
            },
            25,
        );
        assert_eq!(runtime.finisher_count(), 1);
    }

    let resp = reqwest::Client::new()
        .put(format!("http://{addr}/api/v1/announcer/config"))
        .json(&serde_json::json!({
            "enabled": true,
            "selected_stream_ids": [stream_b],
            "max_list_size": 25
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let runtime = app_state.announcer_runtime.read().await;
    assert_eq!(runtime.finisher_count(), 0);
    assert!(runtime.rows().is_empty());
}

#[tokio::test]
async fn config_update_with_max_list_size_change_emits_resync_without_resetting_runtime() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, online) VALUES
         ($1, 'fwd-a', '10.100.0.1:10000', false)",
    )
    .bind(stream_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "UPDATE announcer_config
         SET enabled = true, selected_stream_ids = $1, max_list_size = 25",
    )
    .bind(vec![stream_id])
    .execute(&pool)
    .await
    .unwrap();

    let (app_state, addr, _server_task) = start_server(pool).await;
    {
        let mut runtime = app_state.announcer_runtime.write().await;
        let _ = runtime.ingest(
            AnnouncerInputEvent {
                stream_id,
                seq: 1,
                chip_id: "000000777777".to_owned(),
                bib: Some(7),
                display_name: "Runner A".to_owned(),
                reader_timestamp: Some("10:00:00".to_owned()),
                received_at: chrono::Utc::now(),
            },
            25,
        );
        assert_eq!(runtime.finisher_count(), 1);
        assert_eq!(runtime.rows().len(), 1);
    }

    let sse_url = format!("http://{addr}/api/v1/announcer/events");
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), reqwest::StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp = reqwest::Client::new()
        .put(format!("http://{addr}/api/v1/announcer/config"))
        .json(&serde_json::json!({
            "enabled": true,
            "selected_stream_ids": [stream_id],
            "max_list_size": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let runtime = app_state.announcer_runtime.read().await;
    assert_eq!(runtime.finisher_count(), 1);
    assert_eq!(runtime.rows().len(), 1);

    let mut collected = String::new();
    let mut saw_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                if collected.contains("event: resync") {
                    saw_resync = true;
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("error reading SSE chunk: {:?}", e),
            Err(_) => break,
        }
    }

    assert!(
        saw_resync,
        "expected 'event: resync' in announcer SSE after max_list_size update, got:\n{}",
        collected
    );
}

#[tokio::test]
async fn disabling_announcer_allows_stale_selected_stream_ids() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, online) VALUES ($1, $2, $3, $4)",
    )
    .bind(stream_id)
    .bind("fwd-stale")
    .bind("10.102.0.1:10000")
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("UPDATE announcer_config SET enabled = true, selected_stream_ids = $1")
        .bind(vec![stream_id])
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DELETE FROM streams WHERE stream_id = $1")
        .bind(stream_id)
        .execute(&pool)
        .await
        .unwrap();

    let (_app_state, addr, _server_task) = start_server(pool).await;
    let resp = reqwest::Client::new()
        .put(format!("http://{addr}/api/v1/announcer/config"))
        .json(&serde_json::json!({
            "enabled": false,
            "selected_stream_ids": [stream_id],
            "max_list_size": 25
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn expired_enable_stops_accepting_new_reads() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, online) VALUES ($1, $2, $3, $4)",
    )
    .bind(stream_id)
    .bind("fwd-expired")
    .bind("10.101.0.1:10000")
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO stream_metrics (stream_id) VALUES ($1)")
        .bind(stream_id)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE announcer_config
         SET enabled = true,
             enabled_until = now() - interval '1 minute',
             selected_stream_ids = $1",
    )
    .bind(vec![stream_id])
    .execute(&pool)
    .await
    .unwrap();

    let (app_state, addr, _server_task) = start_server(pool.clone()).await;
    insert_token(&pool, "fwd-expired", "forwarder", b"fwd-expired-token").await;

    let ws_url = format!("ws://{addr}/ws/v1/forwarders");
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "fwd-expired-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-expired".to_owned(),
        reader_ips: vec!["10.101.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let session_id = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id,
        batch_id: "b-expired".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-expired".to_owned(),
            reader_ip: "10.101.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-26T10:00:00.000Z".to_owned(),
            raw_frame: "aa400000000123450a2a01123018455927a7".as_bytes().to_vec(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let runtime = app_state.announcer_runtime.read().await;
    assert_eq!(runtime.finisher_count(), 0);
    assert!(runtime.rows().is_empty());
}

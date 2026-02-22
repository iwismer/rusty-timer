//! Integration test: SSE dashboard events are emitted when a forwarder connects and sends events.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

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

fn find_sse_event_data(collected: &str, event_name: &str) -> Option<String> {
    for block in collected.split("\n\n") {
        let mut name: Option<&str> = None;
        let mut data: Option<&str> = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event: ") {
                name = Some(rest);
            } else if let Some(rest) = line.strip_prefix("data: ") {
                data = Some(rest);
            }
        }
        if name == Some(event_name) {
            return data.map(ToOwned::to_owned);
        }
    }
    None
}

fn find_all_sse_event_data(collected: &str, event_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in collected.split("\n\n") {
        let mut name: Option<&str> = None;
        let mut data: Option<&str> = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event: ") {
                name = Some(rest);
            } else if let Some(rest) = line.strip_prefix("data: ") {
                data = Some(rest);
            }
        }
        if name == Some(event_name) {
            if let Some(d) = data {
                out.push(d.to_owned());
            }
        }
    }
    out
}

#[tokio::test]
async fn test_sse_emits_stream_created_and_metrics_updated() {
    // 1. Start Postgres container
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    // 2. Build state, start server on ephemeral port
    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    // 3. Insert a forwarder token
    insert_token(&pool, "fwd-sse", "forwarder", b"sse-test-token").await;

    // Seed an existing stream so StreamCreated should reflect persisted fields.
    let expected_stream_id = Uuid::new_v4();
    let expected_created_at = "2026-01-01T00:00:00+00:00";
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, display_alias, forwarder_display_name, stream_epoch, online, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz)",
    )
    .bind(expected_stream_id)
    .bind("fwd-sse")
    .bind("192.168.100.1:10000")
    .bind("Desk Reader")
    .bind("My Forwarder")
    .bind(7_i64)
    .bind(false)
    .bind(expected_created_at)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO stream_metrics (stream_id) VALUES ($1)")
        .bind(expected_stream_id)
        .execute(&pool)
        .await
        .unwrap();

    // 4. Connect SSE client FIRST (before forwarder) so it doesn't miss events
    let sse_url = format!("http://{}/api/v1/events", addr);
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), 200);

    // Give the SSE subscription a moment to be fully established
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 5. Connect forwarder via WebSocket
    let ws_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "sse-test-token")
        .await
        .unwrap();

    // 6. Send ForwarderHello with one reader_ip
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse".to_owned(),
        reader_ips: vec!["192.168.100.1:10000".to_owned()],
        display_name: Some("My Forwarder".to_owned()),
    }))
    .await
    .unwrap();

    // 7. Wait for heartbeat response
    let session_id = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected Heartbeat, got {:?}", other),
    };

    // 8. Send a ForwarderEventBatch with one read event
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session_id.clone(),
        batch_id: "b001".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-sse".to_owned(),
            reader_ip: "192.168.100.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-17T12:00:00.000Z".to_owned(),
            raw_read_line: "SSE_TEST_LINE".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();

    // 9. Wait for the ack
    match fwd.recv_message().await.unwrap() {
        WsMessage::ForwarderAck(a) => {
            assert_eq!(a.session_id, session_id);
            assert_eq!(a.entries.len(), 1);
        }
        other => panic!("expected ForwarderAck, got {:?}", other),
    }

    // 10. Read SSE chunks with a timeout, looking for stream_created and metrics_updated
    let mut collected = String::new();
    let mut saw_stream_created = false;
    let mut saw_metrics_updated = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                collected.push_str(&text);
                if collected.contains("event: stream_created") {
                    saw_stream_created = true;
                }
                if collected.contains("event: metrics_updated") {
                    saw_metrics_updated = true;
                }
                if saw_stream_created && saw_metrics_updated {
                    break;
                }
            }
            Ok(Ok(None)) => {
                // Stream ended unexpectedly
                break;
            }
            Ok(Err(e)) => {
                panic!("error reading SSE chunk: {:?}", e);
            }
            Err(_) => {
                // Timeout reading a chunk, check if we have what we need
                break;
            }
        }
    }

    // 11. Assert both events were received
    assert!(
        saw_stream_created,
        "expected 'event: stream_created' in SSE stream, got:\n{}",
        collected
    );
    assert!(
        saw_metrics_updated,
        "expected 'event: metrics_updated' in SSE stream, got:\n{}",
        collected
    );

    let stream_created_data = find_sse_event_data(&collected, "stream_created")
        .expect("missing stream_created data payload");
    let stream_created_json: serde_json::Value =
        serde_json::from_str(&stream_created_data).unwrap();
    let expected_stream_id_str = expected_stream_id.to_string();
    assert_eq!(
        stream_created_json["stream_id"].as_str(),
        Some(expected_stream_id_str.as_str())
    );
    assert_eq!(
        stream_created_json["display_alias"].as_str(),
        Some("Desk Reader")
    );
    assert_eq!(stream_created_json["stream_epoch"].as_i64(), Some(7));
    assert_eq!(
        stream_created_json["created_at"].as_str(),
        Some(expected_created_at)
    );
    assert_eq!(
        stream_created_json["forwarder_display_name"].as_str(),
        Some("My Forwarder")
    );

    let metrics_updated_data = find_sse_event_data(&collected, "metrics_updated")
        .expect("missing metrics_updated payload");
    let metrics_updated_json: serde_json::Value =
        serde_json::from_str(&metrics_updated_data).unwrap();
    assert_eq!(metrics_updated_json["raw_count"].as_i64(), Some(1));
    assert_eq!(metrics_updated_json["dedup_count"].as_i64(), Some(1));
    assert_eq!(metrics_updated_json["retransmit_count"].as_i64(), Some(0));
    assert_eq!(metrics_updated_json["epoch_raw_count"].as_i64(), Some(0));
    assert_eq!(metrics_updated_json["epoch_dedup_count"].as_i64(), Some(0));
    assert_eq!(
        metrics_updated_json["epoch_retransmit_count"].as_i64(),
        Some(0)
    );
    assert_eq!(metrics_updated_json["epoch_lag_ms"].as_i64(), None);
    assert_eq!(
        metrics_updated_json["epoch_last_received_at"].as_str(),
        None
    );
    assert_eq!(metrics_updated_json["unique_chips"].as_i64(), Some(0));

    // Keep the container alive until the end of the test
    std::mem::forget(container);
}

#[tokio::test]
async fn test_sse_emits_stream_updated_on_forwarder_display_name_change() {
    // 1. Start Postgres container
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    // 2. Build state, start server on ephemeral port
    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    // 3. Insert a forwarder token
    insert_token(&pool, "fwd-sse-updated", "forwarder", b"sse-updated-token").await;

    // 4. Connect SSE client FIRST (before forwarder) so it doesn't miss events
    let sse_url = format!("http://{}/api/v1/events", addr);
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), 200);

    // Give the SSE subscription a moment to be fully established
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 5. Connect forwarder via WebSocket
    let ws_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "sse-updated-token")
        .await
        .unwrap();

    // 6. Initial hello creates stream with first display_name
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse-updated".to_owned(),
        reader_ips: vec!["192.168.100.2".to_owned()],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();

    // Wait for heartbeat response
    match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("expected Heartbeat, got {:?}", other),
    }

    // 7. Send updated hello with a new display_name
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse-updated".to_owned(),
        reader_ips: vec!["192.168.100.2".to_owned()],
        display_name: Some("Finish Line".to_owned()),
    }))
    .await
    .unwrap();

    match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("expected Heartbeat, got {:?}", other),
    }

    // 8. Read SSE until a stream_updated payload contains the updated display_name.
    let mut collected = String::new();
    let mut saw_finish_line_update = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                collected.push_str(&text);
                for data in find_all_sse_event_data(&collected, "stream_updated") {
                    let payload: serde_json::Value = serde_json::from_str(&data).unwrap();
                    if payload["forwarder_display_name"] == "Finish Line" {
                        saw_finish_line_update = true;
                        break;
                    }
                }
                if saw_finish_line_update {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("error reading SSE chunk: {:?}", e),
            Err(_) => break,
        }
    }

    assert!(
        saw_finish_line_update,
        "expected stream_updated with forwarder_display_name='Finish Line', got:\n{}",
        collected
    );

    let stream_updated_json = find_all_sse_event_data(&collected, "stream_updated")
        .into_iter()
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .find(|payload| payload["forwarder_display_name"] == "Finish Line")
        .expect("missing stream_updated payload for updated display name");
    assert_eq!(
        stream_updated_json["forwarder_display_name"].as_str(),
        Some("Finish Line")
    );

    // Keep the container alive until the end of the test
    std::mem::forget(container);
}

#[tokio::test]
async fn test_sse_emits_stream_updated_with_null_forwarder_display_name_on_clear() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    insert_token(
        &pool,
        "fwd-sse-clear-name",
        "forwarder",
        b"sse-clear-name-token",
    )
    .await;

    let sse_url = format!("http://{}/api/v1/events", addr);
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ws_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "sse-clear-name-token")
        .await
        .unwrap();

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse-clear-name".to_owned(),
        reader_ips: vec!["192.168.100.3".to_owned()],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse-clear-name".to_owned(),
        reader_ips: vec!["192.168.100.3".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    let mut collected = String::new();
    let mut saw_clear_update = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                collected.push_str(&text);
                for data in find_all_sse_event_data(&collected, "stream_updated") {
                    let payload: serde_json::Value = serde_json::from_str(&data).unwrap();
                    if payload.get("forwarder_display_name").is_some()
                        && payload["forwarder_display_name"].is_null()
                    {
                        saw_clear_update = true;
                        break;
                    }
                }
                if saw_clear_update {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("error reading SSE chunk: {:?}", e),
            Err(_) => break,
        }
    }

    assert!(
        saw_clear_update,
        "expected stream_updated with forwarder_display_name=null, got:\n{}",
        collected
    );

    let stream_updated_json = find_all_sse_event_data(&collected, "stream_updated")
        .into_iter()
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .find(|payload| {
            payload.get("forwarder_display_name").is_some()
                && payload["forwarder_display_name"].is_null()
        })
        .expect("missing stream_updated payload for cleared display name");
    assert!(
        stream_updated_json.get("forwarder_display_name").is_some(),
        "stream_updated must include forwarder_display_name field when clearing display name"
    );
    assert!(
        stream_updated_json["forwarder_display_name"].is_null(),
        "forwarder_display_name should be explicit null when display name is cleared"
    );

    std::mem::forget(container);
}

#[tokio::test]
async fn test_sse_emits_stream_updated_for_all_forwarder_streams_on_display_name_change() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    insert_token(
        &pool,
        "fwd-sse-update-all",
        "forwarder",
        b"sse-update-all-token",
    )
    .await;

    // Seed a historical stream for this forwarder that won't be in this session's reader_ips.
    let historical_stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, display_alias, forwarder_display_name, stream_epoch, online, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
    )
    .bind(historical_stream_id)
    .bind("fwd-sse-update-all")
    .bind("192.168.101.99")
    .bind(Option::<String>::None)
    .bind("Start Line")
    .bind(1_i64)
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO stream_metrics (stream_id) VALUES ($1)")
        .bind(historical_stream_id)
        .execute(&pool)
        .await
        .unwrap();

    let sse_url = format!("http://{}/api/v1/events", addr);
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ws_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "sse-update-all-token")
        .await
        .unwrap();

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse-update-all".to_owned(),
        reader_ips: vec!["192.168.101.1".to_owned()],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    let streams_resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    let streams_body: serde_json::Value = streams_resp.json().await.unwrap();
    let stream_ids: std::collections::HashSet<String> = streams_body["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["forwarder_id"] == "fwd-sse-update-all")
        .map(|s| s["stream_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(stream_ids.len(), 2);

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse-update-all".to_owned(),
        reader_ips: vec!["192.168.101.1".to_owned()],
        display_name: Some("Finish Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut updated_ids = std::collections::HashSet::new();

    while tokio::time::Instant::now() < deadline && updated_ids.len() < stream_ids.len() {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                collected.push_str(&text);
                for data in find_all_sse_event_data(&collected, "stream_updated") {
                    let payload: serde_json::Value = serde_json::from_str(&data).unwrap();
                    if payload["forwarder_display_name"] == "Finish Line" {
                        if let Some(id) = payload["stream_id"].as_str() {
                            updated_ids.insert(id.to_owned());
                        }
                    }
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("error reading SSE chunk: {:?}", e),
            Err(_) => break,
        }
    }

    assert_eq!(
        updated_ids, stream_ids,
        "expected stream_updated for all streams; collected:\n{}",
        collected
    );

    std::mem::forget(container);
}

#[tokio::test]
async fn test_sse_emits_stream_updated_for_all_forwarder_streams_on_initial_hello_display_name() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    insert_token(
        &pool,
        "fwd-sse-initial-update-all",
        "forwarder",
        b"sse-initial-update-all-token",
    )
    .await;

    // Seed a historical stream for this forwarder that is not in initial hello reader_ips.
    let historical_stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, display_alias, forwarder_display_name, stream_epoch, online, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
    )
    .bind(historical_stream_id)
    .bind("fwd-sse-initial-update-all")
    .bind("192.168.111.99")
    .bind(Option::<String>::None)
    .bind("Old Name")
    .bind(1_i64)
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO stream_metrics (stream_id) VALUES ($1)")
        .bind(historical_stream_id)
        .execute(&pool)
        .await
        .unwrap();

    let sse_url = format!("http://{}/api/v1/events", addr);
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ws_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "sse-initial-update-all-token")
        .await
        .unwrap();

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse-initial-update-all".to_owned(),
        reader_ips: vec!["192.168.111.1".to_owned()],
        display_name: Some("Finish Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    let mut collected = String::new();
    let mut updated_ids = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                collected.push_str(&text);
                for data in find_all_sse_event_data(&collected, "stream_updated") {
                    let payload: serde_json::Value = serde_json::from_str(&data).unwrap();
                    if payload["forwarder_display_name"] == "Finish Line" {
                        if let Some(id) = payload["stream_id"].as_str() {
                            updated_ids.insert(id.to_owned());
                        }
                    }
                }

                if updated_ids.contains(&historical_stream_id.to_string()) {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("error reading SSE chunk: {:?}", e),
            Err(_) => break,
        }
    }

    assert!(
        updated_ids.contains(&historical_stream_id.to_string()),
        "expected stream_updated for historical stream on initial hello display-name set; collected:\n{}",
        collected
    );

    std::mem::forget(container);
}

#[tokio::test]
async fn test_sse_emits_resync_after_admin_stream_delete() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    let stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, stream_epoch, online, created_at)
         VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(stream_id)
    .bind("fwd-admin-delete")
    .bind("192.168.200.1:10000")
    .bind(1_i64)
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO stream_metrics (stream_id) VALUES ($1)")
        .bind(stream_id)
        .execute(&pool)
        .await
        .unwrap();

    let sse_url = format!("http://{}/api/v1/events", addr);
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let delete_resp = reqwest::Client::new()
        .delete(format!(
            "http://{}/api/v1/admin/streams/{}",
            addr, stream_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), 204);

    let mut collected = String::new();
    let mut saw_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                collected.push_str(&text);
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
        "expected 'event: resync' in SSE stream after admin delete, got:\n{}",
        collected
    );

    std::mem::forget(container);
}

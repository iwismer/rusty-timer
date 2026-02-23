//! Integration tests for stream-epoch race mapping and activate-next APIs.
use rt_protocol::{ForwarderHello, WsMessage};
use rt_test_utils::MockWsClient;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

async fn make_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let app_state = server::AppState::new(pool);
    make_server_with_state(app_state).await
}

async fn make_server_with_state(app_state: server::AppState) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });
    addr
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

async fn insert_stream(pool: &sqlx::PgPool, forwarder_id: &str, reader_ip: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO streams (forwarder_id, reader_ip) VALUES ($1, $2) RETURNING stream_id",
    )
    .bind(forwarder_id)
    .bind(reader_ip)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn create_race(client: &reqwest::Client, addr: std::net::SocketAddr, name: &str) -> Uuid {
    let response = client
        .post(format!("http://{addr}/api/v1/races"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    body["race_id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn stream_epoch_race_mapping_crud_and_listing() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;
    let client = reqwest::Client::new();

    let stream_id = insert_stream(&pool, "fwd-map", "10.10.0.1:10000").await;
    let race_id = create_race(&client, addr, "Mapping Race").await;

    let put_resp = client
        .put(format!(
            "http://{addr}/api/v1/streams/{stream_id}/epochs/1/race"
        ))
        .json(&serde_json::json!({ "race_id": race_id.to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), 200);

    let get_resp = client
        .get(format!(
            "http://{addr}/api/v1/races/{race_id}/stream-epochs"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 200);
    let body: Value = get_resp.json().await.unwrap();
    let mappings = body["mappings"].as_array().unwrap();
    assert_eq!(mappings.len(), 1);
    assert_eq!(mappings[0]["stream_id"], stream_id.to_string());
    assert_eq!(mappings[0]["forwarder_id"], "fwd-map");
    assert_eq!(mappings[0]["reader_ip"], "10.10.0.1:10000");
    assert_eq!(mappings[0]["stream_epoch"], 1);
    assert_eq!(mappings[0]["race_id"], race_id.to_string());

    let clear_resp = client
        .put(format!(
            "http://{addr}/api/v1/streams/{stream_id}/epochs/1/race"
        ))
        .json(&serde_json::json!({ "race_id": Value::Null }))
        .send()
        .await
        .unwrap();
    assert_eq!(clear_resp.status(), 200);

    let get_resp_after = client
        .get(format!(
            "http://{addr}/api/v1/races/{race_id}/stream-epochs"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp_after.status(), 200);
    let body_after: Value = get_resp_after.json().await.unwrap();
    assert_eq!(body_after["mappings"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn activate_next_creates_next_mapping_sends_command_and_keeps_current_epoch() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;
    let client = reqwest::Client::new();

    insert_token(&pool, "fwd-online", "forwarder", b"fwd-online-token").await;
    let mut fwd = MockWsClient::connect_with_token(
        &format!("ws://{addr}/ws/v1/forwarders"),
        "fwd-online-token",
    )
    .await
    .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-online".to_owned(),
        reader_ips: vec!["10.20.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let streams_resp = client
        .get(format!("http://{addr}/api/v1/streams"))
        .send()
        .await
        .unwrap();
    let streams_body: Value = streams_resp.json().await.unwrap();
    let stream_id: Uuid = streams_body["streams"][0]["stream_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let race_id = create_race(&client, addr, "Activate Next").await;

    let map_resp = client
        .put(format!(
            "http://{addr}/api/v1/streams/{stream_id}/epochs/1/race"
        ))
        .json(&serde_json::json!({ "race_id": race_id.to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(map_resp.status(), 200);

    let activate_resp = client
        .post(format!(
            "http://{addr}/api/v1/races/{race_id}/streams/{stream_id}/epochs/activate-next"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(activate_resp.status(), 204);

    let command = fwd.recv_message().await.unwrap();
    let reset = match command {
        WsMessage::EpochResetCommand(cmd) => cmd,
        other => panic!("unexpected ws message after activate-next: {other:?}"),
    };
    assert_eq!(reset.forwarder_id, "fwd-online");
    assert_eq!(reset.reader_ip, "10.20.0.1:10000");
    assert_eq!(reset.new_stream_epoch, 2);

    let mapped_race = sqlx::query(
        "SELECT race_id FROM stream_epoch_races WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(2_i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mapped_race_id: Uuid = mapped_race.get("race_id");
    assert_eq!(mapped_race_id, race_id);

    let stream_row = sqlx::query("SELECT stream_epoch FROM streams WHERE stream_id = $1")
        .bind(stream_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let stream_epoch: i64 = stream_row.get("stream_epoch");
    assert_eq!(stream_epoch, 1);
}

#[tokio::test]
async fn activate_next_returns_404_when_stream_not_mapped_to_race() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;
    let client = reqwest::Client::new();

    let stream_id = insert_stream(&pool, "fwd-missing-map", "10.30.0.1:10000").await;
    let race_id = create_race(&client, addr, "Missing Mapping").await;

    let resp = client
        .post(format!(
            "http://{addr}/api/v1/races/{race_id}/streams/{stream_id}/epochs/activate-next"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
async fn activate_next_returns_409_when_forwarder_offline() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;
    let client = reqwest::Client::new();

    let stream_id = insert_stream(&pool, "fwd-offline", "10.40.0.1:10000").await;
    let race_id = create_race(&client, addr, "Offline Forwarder").await;

    let map_resp = client
        .put(format!(
            "http://{addr}/api/v1/streams/{stream_id}/epochs/1/race"
        ))
        .json(&serde_json::json!({ "race_id": race_id.to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(map_resp.status(), 200);

    let activate_resp = client
        .post(format!(
            "http://{addr}/api/v1/races/{race_id}/streams/{stream_id}/epochs/activate-next"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(activate_resp.status(), 409);
    let body: Value = activate_resp.json().await.unwrap();
    assert_eq!(body["code"], "CONFLICT");

    let next_mapping = sqlx::query(
        "SELECT race_id FROM stream_epoch_races WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(2_i64)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(next_mapping.is_none());
}

#[tokio::test]
async fn activate_next_returns_post_commit_conflict_when_forwarder_disconnects_during_send() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<server::state::ForwarderCommand>(1);
    drop(cmd_rx);
    {
        let mut senders = app_state.forwarder_command_senders.write().await;
        senders.insert("fwd-closed-at-send".to_owned(), cmd_tx);
    }

    let addr = make_server_with_state(app_state).await;
    let client = reqwest::Client::new();

    let stream_id = insert_stream(&pool, "fwd-closed-at-send", "10.50.0.1:10000").await;
    let race_id = create_race(&client, addr, "Closed During Send").await;

    let map_resp = client
        .put(format!(
            "http://{addr}/api/v1/streams/{stream_id}/epochs/1/race"
        ))
        .json(&serde_json::json!({ "race_id": race_id.to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(map_resp.status(), 200);

    let activate_resp = client
        .post(format!(
            "http://{addr}/api/v1/races/{race_id}/streams/{stream_id}/epochs/activate-next"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(activate_resp.status(), 409);
    let body: Value = activate_resp.json().await.unwrap();
    assert_eq!(body["code"], "CONFLICT");
    assert_eq!(
        body["message"],
        "race activation committed, but failed to deliver epoch reset command"
    );

    let mapped_race = sqlx::query(
        "SELECT race_id FROM stream_epoch_races WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(2_i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mapped_race_id: Uuid = mapped_race.get("race_id");
    assert_eq!(mapped_race_id, race_id);
}

#[tokio::test]
async fn activate_next_returns_504_timeout_when_forwarder_command_queue_saturated() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<server::state::ForwarderCommand>(1);
    cmd_tx
        .send(server::state::ForwarderCommand::EpochReset(
            rt_protocol::EpochResetCommand {
                session_id: String::new(),
                forwarder_id: "fwd-saturated".to_owned(),
                reader_ip: "10.60.0.1:10000".to_owned(),
                new_stream_epoch: 999,
            },
        ))
        .await
        .unwrap();
    {
        let mut senders = app_state.forwarder_command_senders.write().await;
        senders.insert("fwd-saturated".to_owned(), cmd_tx);
    }

    let addr = make_server_with_state(app_state).await;
    let client = reqwest::Client::new();

    let stream_id = insert_stream(&pool, "fwd-saturated", "10.60.0.1:10000").await;
    let race_id = create_race(&client, addr, "Saturated Queue").await;

    let map_resp = client
        .put(format!(
            "http://{addr}/api/v1/streams/{stream_id}/epochs/1/race"
        ))
        .json(&serde_json::json!({ "race_id": race_id.to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(map_resp.status(), 200);

    let activate_resp = client
        .post(format!(
            "http://{addr}/api/v1/races/{race_id}/streams/{stream_id}/epochs/activate-next"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(activate_resp.status(), 504);
    let body: Value = activate_resp.json().await.unwrap();
    assert_eq!(body["code"], "TIMEOUT");
    assert_eq!(body["message"], "forwarder command queue is saturated");

    let mapped_race = sqlx::query(
        "SELECT race_id FROM stream_epoch_races WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(2_i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mapped_race_id: Uuid = mapped_race.get("race_id");
    assert_eq!(mapped_race_id, race_id);
}

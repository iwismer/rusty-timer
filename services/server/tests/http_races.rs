//! Integration tests for race management HTTP API.
use std::net::SocketAddr;
use std::time::Duration;

use serde_json::Value;
use server::state::{ReceiverSelectionSnapshot, ReceiverSessionProtocol};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn make_server(pool: sqlx::PgPool) -> (SocketAddr, server::AppState) {
    let app_state = server::AppState::new(pool);
    let app_state_for_test = app_state.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });
    (addr, app_state_for_test)
}

fn multipart_body(boundary: &str, filename: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn upload_file(
    client: &reqwest::Client,
    addr: SocketAddr,
    race_id: &str,
    endpoint: &str,
    filename: &str,
    bytes: &[u8],
) -> reqwest::Response {
    let boundary = "rt-boundary-123";
    let body = multipart_body(boundary, filename, bytes);
    client
        .post(format!("http://{addr}/api/v1/races/{race_id}/{endpoint}"))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap()
}

async fn create_race(client: &reqwest::Client, addr: SocketAddr, name: &str) -> String {
    let response = client
        .post(format!("http://{addr}/api/v1/races"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    body["race_id"].as_str().unwrap().to_owned()
}

async fn wait_for_delete_status(
    client: &reqwest::Client,
    addr: SocketAddr,
    race_id: &str,
    expected: reqwest::StatusCode,
    timeout: Duration,
) -> reqwest::Response {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let response = client
            .delete(format!("http://{addr}/api/v1/races/{race_id}"))
            .send()
            .await
            .unwrap();
        if response.status() == expected {
            return response;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "DELETE /api/v1/races/{race_id} did not return {expected}; last status was {}",
            response.status()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn create_race_rejects_blank_name() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (addr, _state) = make_server(pool).await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{addr}/api/v1/races"))
        .json(&serde_json::json!({ "name": "   " }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn invalid_participant_upload_is_rejected_without_wiping_existing_data() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (addr, _state) = make_server(pool).await;

    let client = reqwest::Client::new();
    let race_id = create_race(&client, addr, "Race A").await;

    let valid_ppl = b"1,Smith,John,Team A,,M\n";
    let valid_response = upload_file(
        &client,
        addr,
        &race_id,
        "participants/upload",
        "valid.ppl",
        valid_ppl,
    )
    .await;
    assert_eq!(valid_response.status(), 200);

    let before: Value = client
        .get(format!("http://{addr}/api/v1/races/{race_id}/participants"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["participants"].as_array().unwrap().len(), 1);

    let invalid_ppl = b"1\n";
    let invalid_response = upload_file(
        &client,
        addr,
        &race_id,
        "participants/upload",
        "invalid.ppl",
        invalid_ppl,
    )
    .await;
    assert_eq!(invalid_response.status(), 400);

    let after: Value = client
        .get(format!("http://{addr}/api/v1/races/{race_id}/participants"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["participants"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_chip_upload_is_rejected_without_wiping_existing_data() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (addr, _state) = make_server(pool).await;

    let client = reqwest::Client::new();
    let race_id = create_race(&client, addr, "Race B").await;

    let valid_ppl = b"1,Smith,John,Team A,,M\n";
    let valid_ppl_response = upload_file(
        &client,
        addr,
        &race_id,
        "participants/upload",
        "valid.ppl",
        valid_ppl,
    )
    .await;
    assert_eq!(valid_ppl_response.status(), 200);

    let valid_chips = b"BIB,CHIP\n1,058003700001\n";
    let valid_chip_response = upload_file(
        &client,
        addr,
        &race_id,
        "chips/upload",
        "valid.txt",
        valid_chips,
    )
    .await;
    assert_eq!(valid_chip_response.status(), 200);

    let before: Value = client
        .get(format!("http://{addr}/api/v1/races/{race_id}/participants"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        before["participants"][0]["chip_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let invalid_chips = b"1x,058003700001\n";
    let invalid_response = upload_file(
        &client,
        addr,
        &race_id,
        "chips/upload",
        "invalid.txt",
        invalid_chips,
    )
    .await;
    assert_eq!(invalid_response.status(), 400);

    let after: Value = client
        .get(format!("http://{addr}/api/v1/races/{race_id}/participants"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        after["participants"][0]["chip_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn delete_race_returns_conflict_while_actively_selected_by_receiver() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (addr, state) = make_server(pool).await;

    let client = reqwest::Client::new();
    let race_id = create_race(&client, addr, "Delete Guard Race").await;

    state
        .register_receiver_session(
            "session-delete-guard",
            "receiver-a",
            ReceiverSessionProtocol::V12,
            ReceiverSelectionSnapshot::Mode {
                mode_summary: format!("race ({race_id})"),
            },
        )
        .await;

    let response = client
        .delete(format!("http://{addr}/api/v1/races/{race_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_race_returns_conflict_for_equivalent_noncanonical_uuid_selection() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (addr, state) = make_server(pool).await;

    let client = reqwest::Client::new();
    let race_id = create_race(&client, addr, "Delete Guard Race Noncanonical").await;

    state
        .register_receiver_session(
            "session-delete-guard-noncanonical",
            "receiver-a",
            ReceiverSessionProtocol::V12,
            ReceiverSelectionSnapshot::Mode {
                mode_summary: format!("race ({})", race_id.to_uppercase()),
            },
        )
        .await;

    let response = client
        .delete(format!("http://{addr}/api/v1/races/{race_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_race_succeeds_after_receiver_disconnects_and_keeps_unrelated_behavior() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let (addr, state) = make_server(pool).await;

    let client = reqwest::Client::new();
    let guarded_race_id = create_race(&client, addr, "Guarded Race").await;
    let unrelated_race_id = create_race(&client, addr, "Unrelated Race").await;

    state
        .register_receiver_session(
            "session-disconnect",
            "receiver-b",
            ReceiverSessionProtocol::V12,
            ReceiverSelectionSnapshot::Mode {
                mode_summary: format!("race ({guarded_race_id})"),
            },
        )
        .await;

    let blocked = client
        .delete(format!("http://{addr}/api/v1/races/{guarded_race_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), reqwest::StatusCode::CONFLICT);

    let unrelated_deleted = client
        .delete(format!("http://{addr}/api/v1/races/{unrelated_race_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(unrelated_deleted.status(), reqwest::StatusCode::NO_CONTENT);

    state
        .unregister_receiver_session("session-disconnect")
        .await;

    let deleted = wait_for_delete_status(
        &client,
        addr,
        &guarded_race_id,
        reqwest::StatusCode::NO_CONTENT,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(deleted.status(), reqwest::StatusCode::NO_CONTENT);

    let not_found_after_delete = client
        .delete(format!("http://{addr}/api/v1/races/{guarded_race_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        not_found_after_delete.status(),
        reqwest::StatusCode::NOT_FOUND
    );
}

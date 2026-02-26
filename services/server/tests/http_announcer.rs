use server::announcer::AnnouncerInputEvent;
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

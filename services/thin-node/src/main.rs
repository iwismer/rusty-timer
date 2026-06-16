use std::env;

use axum::{Router, routing::get};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(log_level))
        .init();

    let db_path = env::var("THIN_NODE_DB_PATH").unwrap_or_else(|_| "thin-node.sqlite3".to_owned());
    let conn = thin_node::db::open(&db_path).expect("failed to open thin-node SQLite database");

    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let mut router = Router::new().route("/healthz", get(healthz));
    if let Ok(provisioning_token) = env::var("THIN_NODE_PROVISIONING_TOKEN") {
        let state = thin_node::http::AppState::new(conn, &provisioning_token);
        router = router.merge(thin_node::http::router(state));
    } else {
        warn!("THIN_NODE_PROVISIONING_TOKEN not set; /register disabled");
    }
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");

    info!(%bind_addr, %db_path, "thin-node listening");
    axum::serve(listener, router)
        .await
        .expect("thin-node server error");
}

async fn healthz() -> &'static str {
    "ok"
}

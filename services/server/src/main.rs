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

    let db_path = env::var("SERVER_DB_PATH").unwrap_or_else(|_| "server.sqlite3".to_owned());
    let conn = server::db::open(&db_path).expect("failed to open server SQLite database");

    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    // Fail-closed admin guard: only trust the upstream-injected Remote-User
    // header when the operator explicitly asserts a header-stripping reverse
    // proxy (Caddy/Authelia) sits in front of the node. Without this, any
    // direct client could forge the header and self-authorize admin routes.
    let admin_proxy_trusted = env_flag("SERVER_TRUSTED_PROXY");
    if !admin_proxy_trusted {
        warn!(
            "SERVER_TRUSTED_PROXY not set; /admin/* routes are disabled (fail-closed). \
             Set SERVER_TRUSTED_PROXY=1 only when behind a trusted, header-stripping proxy."
        );
    }

    // Devices authenticate with server-minted per-device tokens (bootstrapped
    // from admin-issued enrollment vouchers); there is no shared provisioning
    // secret. Admin routes are gated by the upstream Remote-User header.
    let state = server::http::AppState::new(conn, admin_proxy_trusted);
    let router = Router::new()
        .route("/healthz", get(healthz))
        .merge(server::http::router(state));
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");

    info!(%bind_addr, %db_path, "server listening");
    axum::serve(listener, router).await.expect("server error");
}

async fn healthz() -> &'static str {
    "ok"
}

/// Read a boolean-ish environment flag. Treats `1`, `true`, `yes`, and `on`
/// (case-insensitive) as enabled; everything else (including unset) is
/// disabled, keeping the admin guard fail-closed by default.
fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

use server::db;
use server::state::AppState;
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(log_level))
        .init();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());

    info!("connecting to database...");
    let pool = db::create_pool(&database_url).await;
    db::run_migrations(&pool).await;
    info!("migrations applied");

    // No forwarders are connected at startup, so mark all streams offline.
    // This cleans up stale online=true from previous unclean shutdowns.
    sqlx::query("UPDATE streams SET online = false WHERE online = true")
        .execute(&pool)
        .await
        .expect("failed to reset stream online status");

    let state = AppState::new(pool);
    let router = server::build_router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");
    info!(addr = %bind_addr, "server listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
    info!("server shut down gracefully");
}

/// Waits for SIGTERM or Ctrl-C (SIGINT) and returns to trigger graceful shutdown.
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { info!("received Ctrl+C, shutting down"); },
        _ = terminate => { info!("received SIGTERM, shutting down"); },
    }
}

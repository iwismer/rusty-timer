use server::db;
use server::state::AppState;
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt().with_env_filter(EnvFilter::new(log_level)).init();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());

    info!("connecting to database...");
    let pool = db::create_pool(&database_url).await;
    db::run_migrations(&pool).await;
    info!("migrations applied");

    let state = AppState::new(pool);
    let router = server::build_router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.expect("failed to bind");
    info!(addr = %bind_addr, "server listening");
    axum::serve(listener, router).await.expect("server error");
}

pub mod auth;
pub mod dashboard_events;
pub mod db;
pub mod http;
pub mod repo;
pub mod state;
pub mod ws_forwarder;
pub mod ws_receiver;

pub use state::AppState;

use std::path::PathBuf;

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, patch, post},
    Router,
};
use tower_http::services::{ServeDir, ServeFile};

pub fn build_router(state: AppState, dashboard_dir: Option<PathBuf>) -> Router {
    let router = Router::new()
        .route("/ws/v1/forwarders", get(ws_forwarder::ws_forwarder_handler))
        .route("/ws/v1/receivers", get(ws_receiver::ws_receiver_handler))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1/streams", get(http::streams::list_streams))
        .route(
            "/api/v1/streams/:stream_id",
            patch(http::streams::patch_stream),
        )
        .route(
            "/api/v1/streams/:stream_id/metrics",
            get(http::metrics::get_metrics),
        )
        .route(
            "/api/v1/streams/:stream_id/export.txt",
            get(http::export::export_raw),
        )
        .route(
            "/api/v1/streams/:stream_id/export.csv",
            get(http::export::export_csv),
        )
        .route(
            "/api/v1/streams/:stream_id/reset-epoch",
            post(http::streams::reset_epoch),
        )
        .route("/api/v1/events", get(http::sse::dashboard_sse))
        .route(
            "/api/v1/forwarders/:forwarder_id/config",
            get(http::forwarder_config::get_forwarder_config),
        )
        .route(
            "/api/v1/forwarders/:forwarder_id/config/:section",
            post(http::forwarder_config::set_forwarder_config),
        )
        .route(
            "/api/v1/forwarders/:forwarder_id/restart",
            post(http::forwarder_config::restart_forwarder),
        );

    let router = match dashboard_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => router.fallback(fallback_404),
    };

    router.with_state(state)
}

async fn fallback_404() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Html(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>404 – Not Found</title>
  <style>
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body {
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #0f1117;
      color: #e1e4e8;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
    }
    .container { text-align: center; }
    .code {
      font-size: 8rem;
      font-weight: 700;
      letter-spacing: -0.04em;
      line-height: 1;
      background: linear-gradient(135deg, #667eea, #764ba2);
      -webkit-background-clip: text;
      -webkit-text-fill-color: transparent;
    }
    .message {
      margin-top: 0.5rem;
      font-size: 1.25rem;
      color: #8b949e;
    }
    .home-link {
      display: inline-block;
      margin-top: 2rem;
      padding: 0.6rem 1.5rem;
      border: 1px solid #30363d;
      border-radius: 6px;
      color: #c9d1d9;
      text-decoration: none;
      transition: border-color 0.15s, color 0.15s;
    }
    .home-link:hover { border-color: #667eea; color: #fff; }
  </style>
</head>
<body>
  <div class="container">
    <div class="code">404</div>
    <p class="message">This page doesn't exist.</p>
    <a class="home-link" href="/">← Back to home</a>
  </div>
</body>
</html>"#,
        ),
    )
}

mod health {
    use axum::response::IntoResponse;
    pub async fn healthz() -> impl IntoResponse {
        "ok"
    }
    pub async fn readyz() -> impl IntoResponse {
        "ok"
    }
}

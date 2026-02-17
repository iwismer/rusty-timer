pub mod auth;
pub mod db;
pub mod http;
pub mod repo;
pub mod state;
pub mod ws_forwarder;
pub mod ws_receiver;

pub use state::AppState;

use axum::{routing::{get, patch, post}, Router};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/ws/v1/forwarders", get(ws_forwarder::ws_forwarder_handler))
        .route("/ws/v1/receivers", get(ws_receiver::ws_receiver_handler))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1/streams", get(http::streams::list_streams))
        .route("/api/v1/streams/:stream_id", patch(http::streams::patch_stream))
        .route("/api/v1/streams/:stream_id/metrics", get(http::metrics::get_metrics))
        .route("/api/v1/streams/:stream_id/export.raw", get(http::export::export_raw))
        .route("/api/v1/streams/:stream_id/export.csv", get(http::export::export_csv))
        .route("/api/v1/streams/:stream_id/reset-epoch", post(http::streams::reset_epoch))
        .with_state(state)
}

mod health {
    use axum::response::IntoResponse;
    pub async fn healthz() -> impl IntoResponse { "ok" }
    pub async fn readyz() -> impl IntoResponse { "ok" }
}

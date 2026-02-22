#![cfg(feature = "embed-ui")]

use axum::http::StatusCode;
use receiver::control_api::{AppState, build_router};
use std::sync::Arc;

fn make_state() -> Arc<AppState> {
    let db = receiver::Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    state
}

#[tokio::test]
async fn api_root_returns_not_found() {
    let app = build_router(make_state());
    let req = axum::http::Request::builder()
        .uri("/api")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn spa_route_serves_index() {
    let app = build_router(make_state());
    let req = axum::http::Request::builder()
        .uri("/settings")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

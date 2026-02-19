//! Verify that the UI fallback handler responds when embed-ui is not enabled.

use axum::http::StatusCode;
use http_body_util::BodyExt;
use receiver::control_api::{build_router, AppState};
use std::sync::Arc;

fn make_state() -> Arc<AppState> {
    let db = receiver::Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    state
}

#[tokio::test]
async fn root_returns_html() {
    let app = build_router(make_state());
    let req = axum::http::Request::builder()
        .uri("/")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("<html>"));
}

#[tokio::test]
async fn api_routes_still_work() {
    let app = build_router(make_state());
    let req = axum::http::Request::builder()
        .uri("/api/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

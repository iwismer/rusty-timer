use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use receiver::control_api::{build_router, AppState};
use receiver::Db;
use serde_json::{json, Value};
use tower::ServiceExt;
fn setup() -> axum::Router {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    build_router(state)
}
async fn get_json(app: axum::Router, path: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let val = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, val)
}
async fn put_json(app: axum::Router, path: &str, body: Value) -> StatusCode {
    let req = Request::builder()
        .method(Method::PUT)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}
async fn post_empty(app: axum::Router, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}
#[tokio::test]
async fn get_profile_returns_404_when_no_profile() {
    let (status, _) = get_json(setup(), "/api/v1/profile").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn put_profile_stores_and_get_returns_it() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok","log_level":"info"})
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let (status, val) = get_json(app, "/api/v1/profile").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["server_url"], "wss://s.com");
    assert_eq!(val["token"], "tok");
}
#[tokio::test]
async fn put_profile_updates_existing() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    put_json(
        app.clone(),
        "/api/v1/profile",
        json!({"server_url":"wss://old","token":"t1","log_level":"debug"}),
    )
    .await;
    put_json(
        app.clone(),
        "/api/v1/profile",
        json!({"server_url":"wss://new","token":"t2","log_level":"warn"}),
    )
    .await;
    let (_, val) = get_json(app, "/api/v1/profile").await;
    assert_eq!(val["server_url"], "wss://new");
}
#[tokio::test]
async fn get_streams_returns_empty_list() {
    let (status, val) = get_json(setup(), "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["streams"].as_array().unwrap().len(), 0);
}
#[tokio::test]
async fn get_streams_degraded_when_no_profile() {
    let (_, val) = get_json(setup(), "/api/v1/streams").await;
    assert_eq!(val["degraded"], true);
    assert!(val["upstream_error"].is_string());
}
#[tokio::test]
async fn get_streams_degraded_when_disconnected_with_profile() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    state
        .db
        .lock()
        .await
        .save_profile("wss://s.com", "tok", "info")
        .unwrap();
    *state.upstream_url.write().await = Some("wss://s.com".to_owned());
    let (_, val) = get_json(build_router(state), "/api/v1/streams").await;
    assert_eq!(val["degraded"], true);
    assert!(val["upstream_error"].is_string());
}
#[tokio::test]
async fn put_subscriptions_and_get_streams() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    let body = json!({"subscriptions":[{"forwarder_id":"f","reader_ip":"192.168.1.100","local_port_override":null},{"forwarder_id":"f","reader_ip":"192.168.1.200","local_port_override":9900}]});
    assert_eq!(
        put_json(app.clone(), "/api/v1/subscriptions", body).await,
        StatusCode::NO_CONTENT
    );
    let (status, val) = get_json(app, "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    let streams = val["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    let s1 = streams
        .iter()
        .find(|s| s["reader_ip"] == "192.168.1.100")
        .unwrap();
    assert_eq!(s1["local_port"], 10100);
    assert_eq!(s1["subscribed"], true);
    // In degraded mode (no server connection), online/display_alias are absent.
    assert!(s1.get("online").is_none());
    assert!(s1.get("display_alias").is_none());
    let s2 = streams
        .iter()
        .find(|s| s["reader_ip"] == "192.168.1.200")
        .unwrap();
    assert_eq!(s2["local_port"], 9900);
}
#[tokio::test]
async fn get_status_disconnected_initially() {
    let (status, val) = get_json(setup(), "/api/v1/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["connection_state"], "disconnected");
    assert_eq!(val["local_ok"], true);
}
#[tokio::test]
async fn get_status_shows_streams_count() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    put_json(app.clone(),"/api/v1/subscriptions",json!({"subscriptions":[{"forwarder_id":"f","reader_ip":"10.0.0.1","local_port_override":null}]})).await;
    let (_, val) = get_json(app, "/api/v1/status").await;
    assert_eq!(val["streams_count"], 1);
}
#[tokio::test]
async fn post_connect_returns_202_when_disconnected() {
    assert_eq!(
        post_empty(setup(), "/api/v1/connect").await,
        StatusCode::ACCEPTED
    );
}
#[tokio::test]
async fn post_connect_returns_200_when_connected() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    *state.connection_state.write().await = receiver::control_api::ConnectionState::Connected;
    assert_eq!(
        post_empty(build_router(state), "/api/v1/connect").await,
        StatusCode::OK
    );
}
#[tokio::test]
async fn post_disconnect_returns_202_when_connected() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    *state.connection_state.write().await = receiver::control_api::ConnectionState::Connected;
    assert_eq!(
        post_empty(build_router(state), "/api/v1/disconnect").await,
        StatusCode::ACCEPTED
    );
}
#[tokio::test]
async fn post_disconnect_returns_200_when_disconnected() {
    assert_eq!(
        post_empty(setup(), "/api/v1/disconnect").await,
        StatusCode::OK
    );
}
#[tokio::test]
async fn get_logs_empty_initially() {
    let (status, val) = get_json(setup(), "/api/v1/logs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["entries"].as_array().unwrap().len(), 0);
}
#[tokio::test]
async fn put_subscriptions_replaces_all() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    put_json(app.clone(),"/api/v1/subscriptions",json!({"subscriptions":[{"forwarder_id":"f","reader_ip":"10.0.0.1","local_port_override":null}]})).await;
    put_json(app.clone(),"/api/v1/subscriptions",json!({"subscriptions":[{"forwarder_id":"f2","reader_ip":"10.0.0.2","local_port_override":null}]})).await;
    let (_, val) = get_json(app, "/api/v1/streams").await;
    let streams = val["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["forwarder_id"], "f2");
}

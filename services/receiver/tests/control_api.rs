use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use receiver::Db;
use receiver::control_api::{AppState, ConnectionState, build_router};
use serde_json::{Value, json};
use std::sync::Arc;
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

async fn post_json(app: axum::Router, path: &str, body: Value) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
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
async fn profile_round_trip() {
    let app = setup();
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok"})
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
async fn mode_endpoints_round_trip() {
    let app = setup();
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, _) = get_json(app.clone(), "/api/v1/mode").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/mode",
            json!({
                "mode":"live",
                "streams":[{"forwarder_id":"f1","reader_ip":"10.0.0.1:10000"}],
                "earliest_epochs":[]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/mode").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["mode"], "live");
    assert_eq!(val["streams"][0]["forwarder_id"], "f1");
}

#[tokio::test]
async fn put_mode_requires_profile() {
    let app = setup();
    assert_eq!(
        put_json(
            app,
            "/api/v1/mode",
            json!({"mode":"live","streams":[],"earliest_epochs":[]})
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn mode_switch_pauses_streams() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/subscriptions",
            json!({
                "subscriptions":[{"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","local_port_override":null}]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        post_empty(app.clone(), "/api/v1/streams/resume-all").await,
        StatusCode::NO_CONTENT
    );

    let (_, before) = get_json(app.clone(), "/api/v1/streams").await;
    assert_eq!(before["streams"][0]["paused"], false);

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/mode",
            json!({"mode":"race","race_id":"race-1"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (_, after) = get_json(app, "/api/v1/streams").await;
    assert_eq!(after["streams"][0]["paused"], true);
}

#[tokio::test]
async fn pause_and_resume_stream_endpoints_update_stream_state() {
    let app = setup();
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/subscriptions",
            json!({
                "subscriptions":[{"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","local_port_override":null}]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        post_empty(app.clone(), "/api/v1/streams/resume-all").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_json(
            app.clone(),
            "/api/v1/streams/pause",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (_, paused) = get_json(app.clone(), "/api/v1/streams").await;
    assert_eq!(paused["streams"][0]["paused"], true);

    assert_eq!(
        post_json(
            app.clone(),
            "/api/v1/streams/resume",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (_, resumed) = get_json(app, "/api/v1/streams").await;
    assert_eq!(resumed["streams"][0]["paused"], false);
}

#[tokio::test]
async fn resume_stream_requests_reconnect_when_connected() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1:10000", None).unwrap();
    }
    state.set_connection_state(ConnectionState::Connected).await;
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        post_json(
            app.clone(),
            "/api/v1/streams/resume",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (_, status) = get_json(app, "/api/v1/status").await;
    assert_eq!(status["connection_state"], "connecting");
}

#[tokio::test]
async fn resume_stream_after_pause_all_unpauses_only_target_stream() {
    let app = setup();
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/subscriptions",
            json!({
                "subscriptions":[
                    {"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","local_port_override":null},
                    {"forwarder_id":"f2","reader_ip":"10.0.0.2:10000","local_port_override":null}
                ]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        post_empty(app.clone(), "/api/v1/streams/pause-all").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_json(
            app.clone(),
            "/api/v1/streams/resume",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (_, streams) = get_json(app, "/api/v1/streams").await;
    let entries = streams["streams"].as_array().unwrap();
    let mut paused_by_key = std::collections::HashMap::new();
    for entry in entries {
        let key = format!(
            "{}/{}",
            entry["forwarder_id"].as_str().unwrap(),
            entry["reader_ip"].as_str().unwrap()
        );
        paused_by_key.insert(key, entry["paused"].as_bool().unwrap());
    }

    assert_eq!(paused_by_key.get("f1/10.0.0.1:10000").copied(), Some(false));
    assert_eq!(paused_by_key.get("f2/10.0.0.2:10000").copied(), Some(true));
}

#[tokio::test]
async fn pause_all_and_resume_all_endpoints_update_stream_state() {
    let app = setup();
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/subscriptions",
            json!({
                "subscriptions":[{"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","local_port_override":null}]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        post_empty(app.clone(), "/api/v1/streams/resume-all").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_empty(app.clone(), "/api/v1/streams/pause-all").await,
        StatusCode::NO_CONTENT
    );
    let (_, paused) = get_json(app.clone(), "/api/v1/streams").await;
    assert_eq!(paused["streams"][0]["paused"], true);

    assert_eq!(
        post_empty(app.clone(), "/api/v1/streams/resume-all").await,
        StatusCode::NO_CONTENT
    );
    let (_, resumed) = get_json(app, "/api/v1/streams").await;
    assert_eq!(resumed["streams"][0]["paused"], false);
}

#[tokio::test]
async fn put_earliest_epoch_persists_to_db() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        put_json(
            app,
            "/api/v1/streams/earliest-epoch",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","earliest_epoch":7})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let rows = state.db.lock().await.load_earliest_epochs().unwrap();
    assert_eq!(
        rows,
        vec![("f1".to_owned(), "10.0.0.1:10000".to_owned(), 7)]
    );
}

#[tokio::test]
async fn put_earliest_epoch_rejects_negative_values() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        put_json(
            app,
            "/api/v1/streams/earliest-epoch",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","earliest_epoch":-1})
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    let rows = state.db.lock().await.load_earliest_epochs().unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn put_mode_emits_mode_changed_event() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let mut ui_rx = state.ui_tx.subscribe();
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        put_json(
            app,
            "/api/v1/mode",
            json!({"mode":"race","race_id":"race-1"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    loop {
        let event = ui_rx.recv().await.unwrap();
        if let receiver::ui_events::ReceiverUiEvent::ModeChanged { mode } = event {
            assert_eq!(
                mode,
                rt_protocol::ReceiverMode::Race {
                    race_id: "race-1".to_owned()
                }
            );
            break;
        }
    }
}

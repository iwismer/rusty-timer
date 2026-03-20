use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use receiver::Db;
use receiver::control_api::{AppState, ConnectionState, build_router};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const TEST_RACE_ID: &str = "11111111-1111-1111-1111-111111111111";

fn setup() -> axum::Router {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
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

fn setup_with_state() -> (axum::Router, Arc<AppState>) {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
    let router = build_router(Arc::clone(&state));
    (router, state)
}

async fn post_empty_with_intent(
    app: axum::Router,
    path: &str,
    intent: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("x-rt-receiver-admin-intent", intent)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
}

async fn post_json_with_intent(
    app: axum::Router,
    path: &str,
    body: Value,
    intent: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-rt-receiver-admin-intent", intent)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
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
    assert_eq!(val["receiver_id"], "test-receiver");
    assert!(val.get("update_mode").is_none());
}

#[tokio::test]
async fn update_routes_are_not_registered() {
    let app = setup();

    let (status, _) = get_json(app.clone(), "/api/v1/update/status").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/update/check")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/update/download")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/update/apply")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_profile_with_receiver_id_updates_state() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok", "receiver_id":"recv-new"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (_, val) = get_json(app.clone(), "/api/v1/profile").await;
    assert_eq!(val["receiver_id"], "recv-new");

    let (_, status) = get_json(app, "/api/v1/status").await;
    assert_eq!(status["receiver_id"], "recv-new");
}

#[tokio::test]
async fn put_profile_with_whitespace_receiver_id_keeps_original() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok", "receiver_id":"  "})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (_, val) = get_json(app.clone(), "/api/v1/profile").await;
    assert_eq!(val["receiver_id"], "test-receiver");
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
async fn put_mode_rejects_invalid_race_id_format() {
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

    assert_eq!(
        put_json(
            app,
            "/api/v1/mode",
            json!({"mode":"race","race_id":"not-a-uuid"})
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn put_earliest_epoch_persists_to_db() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
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
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
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
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
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
            json!({"mode":"race","race_id":TEST_RACE_ID})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, ui_rx.recv())
            .await
            .expect("timed out waiting for ModeChanged event")
            .unwrap();
        if let receiver::ui_events::ReceiverUiEvent::ModeChanged { mode } = event {
            assert_eq!(
                mode,
                rt_protocol::ReceiverMode::Race {
                    race_id: TEST_RACE_ID.to_owned()
                }
            );
            break;
        }
    }
}

#[tokio::test]
async fn put_profile_without_receiver_id_preserves_db_value() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db, "recv-original".to_owned());
    let app = build_router(Arc::clone(&state));

    // First save: set a receiver_id
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok", "receiver_id":"recv-original"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    // Second save: omit receiver_id entirely
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s2.com", "token":"tok2"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    // DB should still have the original receiver_id
    let db = state.db.lock().await;
    let profile = db.load_profile().unwrap().unwrap();
    assert_eq!(profile.receiver_id, Some("recv-original".to_owned()));
}

#[tokio::test]
async fn put_profile_rejects_too_long_receiver_id() {
    let app = setup();
    let long_id = "a".repeat(65);
    assert_eq!(
        put_json(
            app,
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok", "receiver_id": long_id})
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn put_profile_rejects_receiver_id_with_special_chars() {
    let app = setup();
    assert_eq!(
        put_json(
            app,
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok", "receiver_id": "recv/bad@id"})
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn put_profile_accepts_valid_receiver_id() {
    let app = setup();
    assert_eq!(
        put_json(
            app,
            "/api/v1/profile",
            json!({"server_url":"wss://s.com", "token":"tok", "receiver_id": "my-recv-01"})
        )
        .await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn admin_reset_all_cursors_requires_intent_header() {
    let app = setup();
    let status = post_empty(app, "/api/v1/admin/cursors/reset-all").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_reset_all_cursors_deletes_all() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_cursor("f1", "10.0.0.1:10000", 1, 10).unwrap();
        db.save_cursor("f2", "10.0.0.2:10000", 2, 20).unwrap();
    }
    let (status, body) =
        post_empty_with_intent(app, "/api/v1/admin/cursors/reset-all", "reset-all-cursors").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], 2);
}

#[tokio::test]
async fn admin_reset_all_earliest_epochs_deletes_all() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
    }
    let (status, body) = post_empty_with_intent(
        app,
        "/api/v1/admin/earliest-epochs/reset-all",
        "reset-all-earliest-epochs",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], 1);
}

#[tokio::test]
async fn admin_reset_earliest_epoch_per_stream() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
        db.save_earliest_epoch("f2", "10.0.0.2", 3).unwrap();
    }
    let (status, _) = post_json_with_intent(
        app,
        "/api/v1/admin/earliest-epochs/reset",
        json!({"forwarder_id": "f1", "reader_ip": "10.0.0.1"}),
        "reset-earliest-epoch",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let remaining = state.db.lock().await.load_earliest_epochs().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].0, "f2");
}

#[tokio::test]
async fn admin_purge_subscriptions_deletes_all() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
    }
    let (status, body) = post_empty_with_intent(
        app,
        "/api/v1/admin/subscriptions/purge",
        "purge-subscriptions",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], 1);
}

#[tokio::test]
async fn admin_purge_subscriptions_requests_reconnect_when_connected() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
    }
    state.set_connection_state(ConnectionState::Connected).await;

    let (status, _) = post_empty_with_intent(
        app.clone(),
        "/api/v1/admin/subscriptions/purge",
        "purge-subscriptions",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, runtime_status) = get_json(app, "/api/v1/status").await;
    assert_eq!(runtime_status["connection_state"], "connecting");
}

#[tokio::test]
async fn admin_reset_profile_clears_credentials() {
    let (app, state) = setup_with_state();
    {
        let mut db = state.db.lock().await;
        db.save_profile("wss://s.com", "tok", "check-only", Some("recv-1"))
            .unwrap();
    }
    let (status, _) =
        post_empty_with_intent(app.clone(), "/api/v1/admin/profile/reset", "reset-profile").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, profile) = get_json(app, "/api/v1/profile").await;
    assert_eq!(profile["server_url"], "");
    assert_eq!(profile["token"], "");
}

#[tokio::test]
async fn admin_reset_profile_disconnects_when_connected() {
    let (app, state) = setup_with_state();
    {
        let mut db = state.db.lock().await;
        db.save_profile("wss://s.com", "tok", "check-only", Some("recv-1"))
            .unwrap();
    }
    state.set_connection_state(ConnectionState::Connected).await;

    let (status, _) =
        post_empty_with_intent(app.clone(), "/api/v1/admin/profile/reset", "reset-profile").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, runtime_status) = get_json(app, "/api/v1/status").await;
    assert_eq!(runtime_status["connection_state"], "disconnecting");
}

#[tokio::test]
async fn admin_factory_reset_clears_everything() {
    let (app, state) = setup_with_state();
    {
        let mut db = state.db.lock().await;
        db.save_profile("wss://s.com", "tok", "check-only", Some("recv-1"))
            .unwrap();
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 1, 10).unwrap();
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
    }
    let (status, _) =
        post_empty_with_intent(app.clone(), "/api/v1/admin/factory-reset", "factory-reset").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, profile) = get_json(app, "/api/v1/profile").await;
    assert_eq!(profile["server_url"], "");
    assert_eq!(profile["token"], "");
}

#[tokio::test]
async fn admin_update_port_sets_override() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
    }
    let (status, _) = post_json_with_intent(
        app,
        "/api/v1/admin/subscriptions/port",
        json!({"forwarder_id": "f1", "reader_ip": "10.0.0.1", "local_port_override": 9000}),
        "update-local-port",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let subs = state.db.lock().await.load_subscriptions().unwrap();
    assert_eq!(subs[0].local_port_override, Some(9000));
}

#[tokio::test]
async fn admin_update_port_returns_404_for_missing_subscription() {
    let app = setup();
    let (status, _) = post_json_with_intent(
        app,
        "/api/v1/admin/subscriptions/port",
        json!({"forwarder_id": "f1", "reader_ip": "10.0.0.1", "local_port_override": 9000}),
        "update-local-port",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_update_port_clears_override() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", Some(9000)).unwrap();
    }
    let (status, _) = post_json_with_intent(
        app,
        "/api/v1/admin/subscriptions/port",
        json!({"forwarder_id": "f1", "reader_ip": "10.0.0.1", "local_port_override": null}),
        "update-local-port",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let subs = state.db.lock().await.load_subscriptions().unwrap();
    assert_eq!(subs[0].local_port_override, None);
}

#[tokio::test]
async fn streams_response_includes_cursor_data() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
        db.save_subscription("f2", "10.0.0.2", None).unwrap();
        db.save_cursor("f1", "10.0.0.1", 5, 42).unwrap();
        // f2 has no cursor — fields should be absent
    }
    let (status, body) = get_json(app, "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);

    // Find f1 — should have cursor data
    let f1 = streams.iter().find(|s| s["forwarder_id"] == "f1").unwrap();
    assert_eq!(f1["cursor_epoch"], 5);
    assert_eq!(f1["cursor_seq"], 42);

    // Find f2 — should have no cursor fields (skip_serializing_if)
    let f2 = streams.iter().find(|s| s["forwarder_id"] == "f2").unwrap();
    assert!(f2.get("cursor_epoch").is_none() || f2["cursor_epoch"].is_null());
    assert!(f2.get("cursor_seq").is_none() || f2["cursor_seq"].is_null());
}

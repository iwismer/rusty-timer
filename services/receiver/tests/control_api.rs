use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use receiver::control_api::{build_router, AppState, ConnectionState};
use receiver::Db;
use rt_updater::UpdateMode;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
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
async fn post_json(app: axum::Router, path: &str, body: Value) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

async fn post_json_with_reset_guard(app: axum::Router, path: &str, body: Value) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-rt-receiver-admin-intent", "reset-stream-cursor")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

async fn spawn_upstream_streams_server(
    status: StatusCode,
    payload: Value,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/api/v1/streams",
        axum::routing::get(move || {
            let status = status;
            let payload = payload.clone();
            async move { (status, axum::Json(payload)) }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("ws://{addr}/ws/v1/receivers"), handle)
}

async fn spawn_upstream_races_server(
    status: StatusCode,
    payload: Value,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/api/v1/races",
        axum::routing::get(move || {
            let status = status;
            let payload = payload.clone();
            async move { (status, axum::Json(payload)) }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("ws://{addr}/ws/v1.1/receivers"), handle)
}

async fn spawn_upstream_replay_epochs_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new()
        .route(
            "/api/v1/streams",
            axum::routing::get(|| async {
                (
                    StatusCode::OK,
                    axum::Json(json!({
                        "streams": [{
                            "stream_id":"11111111-1111-1111-1111-111111111111",
                            "forwarder_id":"f1",
                            "reader_ip":"10.0.0.1:10000",
                            "display_alias":"Finish",
                            "stream_epoch":9,
                            "online":true
                        }]
                    })),
                )
            }),
        )
        .route(
            "/api/v1/streams/{stream_id}/epochs",
            axum::routing::get(
                |axum::extract::Path(stream_id): axum::extract::Path<String>| async move {
                    let body = if stream_id == "11111111-1111-1111-1111-111111111111" {
                        json!([
                            {
                                "epoch": 9,
                                "name": "Lap 9",
                                "first_event_at": "2026-01-03T00:00:00Z"
                            }
                        ])
                    } else {
                        json!([])
                    };
                    (StatusCode::OK, axum::Json(body))
                },
            ),
        )
        .route(
            "/api/v1/races",
            axum::routing::get(|| async {
                (
                    StatusCode::OK,
                    axum::Json(json!({
                        "races": [{
                            "race_id":"00000000-0000-0000-0000-000000000001",
                            "name":"Spring 5K",
                            "created_at":"2026-01-01T00:00:00Z"
                        }]
                    })),
                )
            }),
        )
        .route(
            "/api/v1/races/{race_id}/stream-epochs",
            axum::routing::get(
                |axum::extract::Path(race_id): axum::extract::Path<String>| async move {
                    let body = if race_id == "00000000-0000-0000-0000-000000000001" {
                        json!({
                            "mappings": [{
                                "stream_id":"11111111-1111-1111-1111-111111111111",
                                "forwarder_id":"f1",
                                "reader_ip":"10.0.0.1:10000",
                                "stream_epoch":9,
                                "race_id":"00000000-0000-0000-0000-000000000001"
                            }]
                        })
                    } else {
                        json!({ "mappings": [] })
                    };
                    (StatusCode::OK, axum::Json(body))
                },
            ),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("ws://{addr}/ws/v1.1/receivers"), handle)
}

async fn spawn_upstream_replay_epochs_server_with_race_mapping_delay(
    race_count: usize,
    delay: Duration,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let race_count_u64 = u64::try_from(race_count).unwrap_or(0);

    let app = axum::Router::new()
        .route(
            "/api/v1/streams",
            axum::routing::get(|| async {
                (
                    StatusCode::OK,
                    axum::Json(json!({
                        "streams": [{
                            "stream_id":"11111111-1111-1111-1111-111111111111",
                            "forwarder_id":"f1",
                            "reader_ip":"10.0.0.1:10000",
                            "display_alias":"Finish",
                            "stream_epoch":9,
                            "online":true
                        }]
                    })),
                )
            }),
        )
        .route(
            "/api/v1/streams/{stream_id}/epochs",
            axum::routing::get(|_path: axum::extract::Path<String>| async move {
                (
                    StatusCode::OK,
                    axum::Json(json!([
                        {
                            "epoch": 9,
                            "name": "Lap 9",
                            "first_event_at": "2026-01-03T00:00:00Z"
                        }
                    ])),
                )
            }),
        )
        .route(
            "/api/v1/races",
            axum::routing::get(move || async move {
                let races = (0..race_count_u64)
                    .map(|i| {
                        json!({
                            "race_id": format!("00000000-0000-0000-0000-{i:012}"),
                            "name": format!("Race {i}"),
                            "created_at":"2026-01-01T00:00:00Z"
                        })
                    })
                    .collect::<Vec<_>>();
                (StatusCode::OK, axum::Json(json!({ "races": races })))
            }),
        )
        .route(
            "/api/v1/races/{race_id}/stream-epochs",
            axum::routing::get({
                let in_flight = Arc::clone(&in_flight);
                let max_in_flight = Arc::clone(&max_in_flight);
                move |_path: axum::extract::Path<String>| {
                    let in_flight = Arc::clone(&in_flight);
                    let max_in_flight = Arc::clone(&max_in_flight);
                    async move {
                        let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        loop {
                            let max_seen = max_in_flight.load(Ordering::SeqCst);
                            if current <= max_seen {
                                break;
                            }
                            if max_in_flight
                                .compare_exchange(
                                    max_seen,
                                    current,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_ok()
                            {
                                break;
                            }
                        }
                        tokio::time::sleep(delay).await;
                        in_flight.fetch_sub(1, Ordering::SeqCst);

                        (
                            StatusCode::OK,
                            axum::Json(json!({
                                "mappings": [{
                                    "stream_id":"11111111-1111-1111-1111-111111111111",
                                    "forwarder_id":"f1",
                                    "reader_ip":"10.0.0.1:10000",
                                    "stream_epoch":9,
                                    "race_id":"00000000-0000-0000-0000-000000000001"
                                }]
                            })),
                        )
                    }
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (
        format!("ws://{addr}/ws/v1.1/receivers"),
        handle,
        in_flight,
        max_in_flight,
    )
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
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let (status, val) = get_json(app, "/api/v1/profile").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["server_url"], "wss://s.com");
    assert_eq!(val["token"], "tok");
    assert_eq!(val["update_mode"], "check-and-download");
}

#[tokio::test]
async fn get_selection_returns_manual_resume_by_default() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/selection").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["selection"]["mode"], "manual");
    assert_eq!(val["selection"]["streams"], json!([]));
    assert_eq!(val["replay_policy"], "resume");
    assert!(val["replay_targets"].is_null());
}

#[tokio::test]
async fn put_selection_persists_and_get_selection_returns_it() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/selection",
            json!({
                "selection": {"mode":"race","race_id":"race-1","epoch_scope":"current"},
                "replay_policy":"live_only"
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/selection").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["selection"]["mode"], "race");
    assert_eq!(val["selection"]["race_id"], "race-1");
    assert_eq!(val["selection"]["epoch_scope"], "current");
    assert_eq!(val["replay_policy"], "live_only");
}

#[tokio::test]
async fn put_selection_connected_transitions_to_connecting() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    *state.connection_state.write().await = ConnectionState::Connected;
    assert_eq!(
        put_json(
            app,
            "/api/v1/selection",
            json!({
                "selection":{"mode":"manual","streams":[]},
                "replay_policy":"resume"
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        *state.connection_state.read().await,
        ConnectionState::Connecting
    );
}

#[tokio::test]
async fn put_selection_connecting_reissues_connect_attempt() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    state.request_connect().await;
    let before_attempt = state.current_connect_attempt();

    assert_eq!(
        put_json(
            app,
            "/api/v1/selection",
            json!({
                "selection":{"mode":"manual","streams":[]},
                "replay_policy":"resume"
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        *state.connection_state.read().await,
        ConnectionState::Connecting
    );
    assert!(
        state.current_connect_attempt() > before_attempt,
        "selection update while connecting should request a fresh connect attempt"
    );
}

#[tokio::test]
async fn put_selection_rejects_targeted_without_targets() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/selection",
            json!({
                "selection":{"mode":"manual","streams":[]},
                "replay_policy":"targeted"
            })
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/selection",
            json!({
                "selection":{"mode":"manual","streams":[]},
                "replay_policy":"targeted",
                "replay_targets":[]
            })
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn get_races_proxies_to_upstream() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let (ws_url, upstream_handle) = spawn_upstream_races_server(
        StatusCode::OK,
        json!({
            "races": [
                {"race_id":"r1","name":"Spring 5K"},
                {"race_id":"r2","name":"Summer 10K"}
            ]
        }),
    )
    .await;
    let app = build_router(Arc::clone(&state));
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":ws_url,"token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/races").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["races"].as_array().unwrap().len(), 2);
    assert_eq!(val["races"][0]["race_id"], "r1");

    upstream_handle.abort();
}

#[tokio::test]
async fn get_replay_target_epochs_proxies_upstream_epochs_with_race_names() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let (ws_url, upstream_handle) = spawn_upstream_replay_epochs_server().await;
    let app = build_router(Arc::clone(&state));
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":ws_url,"token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(
        app,
        "/api/v1/replay-targets/epochs?forwarder_id=f1&reader_ip=10.0.0.1:10000",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["epochs"].as_array().unwrap().len(), 1);
    assert_eq!(val["epochs"][0]["stream_epoch"], 9);
    assert_eq!(val["epochs"][0]["name"], "Lap 9");
    assert_eq!(val["epochs"][0]["first_seen_at"], "2026-01-03T00:00:00Z");
    assert_eq!(val["epochs"][0]["race_names"], json!(["Spring 5K"]));

    upstream_handle.abort();
}

#[tokio::test]
async fn get_replay_target_epochs_fetches_race_mappings_concurrently() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let (ws_url, upstream_handle, _in_flight, max_in_flight) =
        spawn_upstream_replay_epochs_server_with_race_mapping_delay(3, Duration::from_millis(75))
            .await;
    let app = build_router(Arc::clone(&state));
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":ws_url,"token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, _val) = get_json(
        app,
        "/api/v1/replay-targets/epochs?forwarder_id=f1&reader_ip=10.0.0.1:10000",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        max_in_flight.load(Ordering::SeqCst) >= 2,
        "expected at least two concurrent race mapping fetches"
    );

    upstream_handle.abort();
}
#[tokio::test]
async fn put_profile_updates_existing() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    put_json(
        app.clone(),
        "/api/v1/profile",
        json!({"server_url":"wss://old","token":"t1","update_mode":"disabled"}),
    )
    .await;
    put_json(
        app.clone(),
        "/api/v1/profile",
        json!({"server_url":"wss://new","token":"t2","update_mode":"check-only"}),
    )
    .await;
    let (_, val) = get_json(app, "/api/v1/profile").await;
    assert_eq!(val["server_url"], "wss://new");
    assert_eq!(val["update_mode"], "check-only");
}

#[tokio::test]
async fn put_profile_omitted_update_mode_preserves_existing() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    put_json(
        app.clone(),
        "/api/v1/profile",
        json!({"server_url":"wss://old","token":"t1","update_mode":"check-only"}),
    )
    .await;
    put_json(
        app.clone(),
        "/api/v1/profile",
        json!({"server_url":"wss://new","token":"t2"}),
    )
    .await;
    let (_, val) = get_json(app, "/api/v1/profile").await;
    assert_eq!(val["server_url"], "wss://new");
    assert_eq!(val["update_mode"], "check-only");
}

#[tokio::test]
async fn put_profile_rejects_invalid_update_mode() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    let status = put_json(
        app,
        "/api/v1/profile",
        json!({"server_url":"wss://s.com","token":"tok","update_mode":"bogus"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_profile_updates_in_memory_update_mode() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    let status = put_json(
        app,
        "/api/v1/profile",
        json!({"server_url":"wss://s.com","token":"tok","update_mode":"check-only"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(*state.update_mode.read().await, UpdateMode::CheckOnly);
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
        .save_profile("wss://s.com", "tok", "check-and-download")
        .unwrap();
    *state.upstream_url.write().await = Some("wss://s.com".to_owned());
    let (_, val) = get_json(build_router(state), "/api/v1/streams").await;
    assert_eq!(val["degraded"], true);
    assert!(val["upstream_error"].is_string());
}

#[tokio::test]
async fn post_admin_cursor_reset_clears_only_target_stream_cursor() {
    let db = Db::open_in_memory().unwrap();
    db.save_cursor("f1", "10.0.0.1:10000", 7, 42).unwrap();
    db.save_cursor("f2", "10.0.0.2:10000", 3, 9).unwrap();

    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    assert_eq!(
        post_json_with_reset_guard(
            app.clone(),
            "/api/v1/admin/cursors/reset",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000"}),
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let cursors = state.db.lock().await.load_cursors().unwrap();
    assert_eq!(cursors.len(), 1);
    assert_eq!(cursors[0].forwarder_id, "f2");
    assert_eq!(cursors[0].reader_ip, "10.0.0.2:10000");
}

#[tokio::test]
async fn post_admin_cursor_reset_is_idempotent_for_missing_cursor() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);

    assert_eq!(
        post_json_with_reset_guard(
            app,
            "/api/v1/admin/cursors/reset",
            json!({"forwarder_id":"f-missing","reader_ip":"10.9.0.1:10000"}),
        )
        .await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn post_admin_cursor_reset_rejects_missing_guard_header() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);

    assert_eq!(
        post_json(
            app,
            "/api/v1/admin/cursors/reset",
            json!({"forwarder_id":"f1","reader_ip":"10.0.0.1:10000"}),
        )
        .await,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn put_subscriptions_and_get_streams() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    let body = json!({"subscriptions":[{"forwarder_id":"f","reader_ip":"192.168.1.100:10000","local_port_override":null},{"forwarder_id":"f","reader_ip":"192.168.1.200:10000","local_port_override":9900}]});
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
        .find(|s| s["reader_ip"] == "192.168.1.100:10000")
        .unwrap();
    assert_eq!(s1["local_port"], 10100);
    assert_eq!(s1["subscribed"], true);
    // In degraded mode (no server connection), online/display_alias are absent.
    assert!(s1.get("online").is_none());
    assert!(s1.get("display_alias").is_none());
    let s2 = streams
        .iter()
        .find(|s| s["reader_ip"] == "192.168.1.200:10000")
        .unwrap();
    assert_eq!(s2["local_port"], 9900);
}

#[tokio::test]
async fn get_subscriptions_returns_empty_initially() {
    let (status, val) = get_json(setup(), "/api/v1/subscriptions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val, json!({ "subscriptions": [] }));
}

#[tokio::test]
async fn get_subscriptions_returns_saved_subscriptions() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);
    let body = json!({
        "subscriptions": [
            {"forwarder_id":"f2","reader_ip":"10.0.0.2:10000","local_port_override":9988},
            {"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","local_port_override":null}
        ]
    });
    assert_eq!(
        put_json(app.clone(), "/api/v1/subscriptions", body).await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/subscriptions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        val,
        json!({
            "subscriptions": [
                {"forwarder_id":"f1","reader_ip":"10.0.0.1:10000","local_port_override":null},
                {"forwarder_id":"f2","reader_ip":"10.0.0.2:10000","local_port_override":9988}
            ]
        })
    );
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
    put_json(app.clone(),"/api/v1/subscriptions",json!({"subscriptions":[{"forwarder_id":"f","reader_ip":"10.0.0.1:10000","local_port_override":null}]})).await;
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
    put_json(app.clone(),"/api/v1/subscriptions",json!({"subscriptions":[{"forwarder_id":"f","reader_ip":"10.0.0.1:10000","local_port_override":null}]})).await;
    put_json(app.clone(),"/api/v1/subscriptions",json!({"subscriptions":[{"forwarder_id":"f2","reader_ip":"10.0.0.2:10000","local_port_override":null}]})).await;
    let (_, val) = get_json(app, "/api/v1/streams").await;
    let streams = val["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["forwarder_id"], "f2");
}

#[tokio::test]
async fn put_subscriptions_rejects_duplicate_entries_and_preserves_existing() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/subscriptions",
            json!({"subscriptions":[{"forwarder_id":"f1","reader_ip":"10.0.0.1","local_port_override":null}]}),
        )
        .await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/subscriptions",
            json!({"subscriptions":[
                {"forwarder_id":"f2","reader_ip":"10.0.0.2","local_port_override":null},
                {"forwarder_id":"f2","reader_ip":"10.0.0.2","local_port_override":9950}
            ]}),
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    let (_, val) = get_json(app, "/api/v1/streams").await;
    let streams = val["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["forwarder_id"], "f1");
    assert_eq!(streams[0]["reader_ip"], "10.0.0.1");
}

#[tokio::test]
async fn get_streams_connected_merges_server_and_local_streams() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);

    let (ws_url, upstream_handle) = spawn_upstream_streams_server(
        StatusCode::OK,
        json!({
            "streams": [
                {
                    "stream_id":"11111111-1111-1111-1111-111111111111",
                    "forwarder_id":"f1",
                    "reader_ip":"192.168.1.100:10000",
                    "display_alias":"Finish",
                    "stream_epoch":1,
                    "current_epoch_name":"Heat 1",
                    "online":true
                },
                {
                    "stream_id":"22222222-2222-2222-2222-222222222222",
                    "forwarder_id":"f2",
                    "reader_ip":"192.168.1.200:10000",
                    "display_alias":null,
                    "stream_epoch":1,
                    "current_epoch_name":null,
                    "online":false
                }
            ]
        }),
    )
    .await;

    // Insert subscriptions directly via the DB to avoid triggering a reconnect
    // (put_subscriptions now calls request_connect() when connected).
    {
        let mut db = state.db.lock().await;
        db.replace_subscriptions(&[
            receiver::Subscription {
                forwarder_id: "f1".to_owned(),
                reader_ip: "192.168.1.100:10000".to_owned(),
                local_port_override: None,
            },
            receiver::Subscription {
                forwarder_id: "f3".to_owned(),
                reader_ip: "192.168.1.250:10000".to_owned(),
                local_port_override: Some(9950),
            },
        ])
        .unwrap();
    }

    *state.upstream_url.write().await = Some(ws_url);
    *state.connection_state.write().await = receiver::control_api::ConnectionState::Connected;

    let app = build_router(state);

    let (status, val) = get_json(app, "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["degraded"], false);
    assert!(val["upstream_error"].is_null());

    let streams = val["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 3);

    let matched = streams
        .iter()
        .find(|s| s["forwarder_id"] == "f1" && s["reader_ip"] == "192.168.1.100:10000")
        .unwrap();
    assert_eq!(matched["subscribed"], true);
    assert_eq!(matched["online"], true);
    assert_eq!(matched["display_alias"], "Finish");
    assert_eq!(matched["current_epoch_name"], "Heat 1");
    assert_eq!(matched["local_port"], 10100);

    let server_only = streams
        .iter()
        .find(|s| s["forwarder_id"] == "f2" && s["reader_ip"] == "192.168.1.200:10000")
        .unwrap();
    assert_eq!(server_only["subscribed"], false);
    assert_eq!(server_only["online"], false);
    assert!(server_only["display_alias"].is_null());
    assert!(server_only["current_epoch_name"].is_null());
    assert!(server_only["local_port"].is_null());

    let local_only = streams
        .iter()
        .find(|s| s["forwarder_id"] == "f3" && s["reader_ip"] == "192.168.1.250:10000")
        .unwrap();
    assert_eq!(local_only["subscribed"], true);
    assert_eq!(local_only["local_port"], 9950);
    assert!(local_only.get("online").is_none());
    assert!(local_only.get("display_alias").is_none());
    assert!(local_only.get("current_epoch_name").is_none());

    upstream_handle.abort();
}

#[tokio::test]
async fn get_streams_connected_degrades_on_upstream_http_error() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);

    let (ws_url, upstream_handle) =
        spawn_upstream_streams_server(StatusCode::INTERNAL_SERVER_ERROR, json!({"error":"boom"}))
            .await;

    *state.upstream_url.write().await = Some(ws_url);
    *state.connection_state.write().await = receiver::control_api::ConnectionState::Connected;

    let app = build_router(state);
    let body = json!({
        "subscriptions":[
            {"forwarder_id":"f3","reader_ip":"192.168.1.250:10000","local_port_override":9950}
        ]
    });
    assert_eq!(
        put_json(app.clone(), "/api/v1/subscriptions", body).await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["degraded"], true);
    assert!(val["upstream_error"].is_string());
    assert!(!val["upstream_error"].as_str().unwrap().is_empty());

    let streams = val["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["forwarder_id"], "f3");
    assert_eq!(streams[0]["reader_ip"], "192.168.1.250:10000");
    assert_eq!(streams[0]["subscribed"], true);
    assert_eq!(streams[0]["local_port"], 9950);
    assert!(streams[0].get("online").is_none());
    assert!(streams[0].get("display_alias").is_none());

    upstream_handle.abort();
}

#[tokio::test]
async fn get_streams_connected_degrades_on_invalid_upstream_json() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);

    let (ws_url, upstream_handle) =
        spawn_upstream_streams_server(StatusCode::OK, json!({"invalid":"shape"})).await;

    *state.upstream_url.write().await = Some(ws_url);
    *state.connection_state.write().await = receiver::control_api::ConnectionState::Connected;

    let app = build_router(state);
    let body = json!({
        "subscriptions":[
            {"forwarder_id":"f4","reader_ip":"192.168.1.251:10000","local_port_override":null}
        ]
    });
    assert_eq!(
        put_json(app.clone(), "/api/v1/subscriptions", body).await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["degraded"], true);
    assert!(val["upstream_error"].is_string());
    assert!(!val["upstream_error"].as_str().unwrap().is_empty());

    let streams = val["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["forwarder_id"], "f4");
    assert_eq!(streams[0]["reader_ip"], "192.168.1.251:10000");
    assert_eq!(streams[0]["subscribed"], true);
    assert_eq!(streams[0]["local_port"], 10251);
    assert!(streams[0].get("online").is_none());
    assert!(streams[0].get("display_alias").is_none());

    upstream_handle.abort();
}

#[tokio::test]
async fn emit_log_stores_entry_and_broadcasts() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let mut ui_rx = state.ui_tx.subscribe();

    state.logger.log("test message");

    let entries = state.logger.entries();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].contains("test message"));

    let event = ui_rx.try_recv().unwrap();
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "log_entry");
    assert!(json["entry"].as_str().unwrap().contains("test message"));
}

#[tokio::test]
async fn set_connection_state_updates_and_broadcasts() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let mut ui_rx = state.ui_tx.subscribe();

    state.set_connection_state(ConnectionState::Connected).await;

    let cs = state.connection_state.read().await.clone();
    assert_eq!(cs, ConnectionState::Connected);

    // First event: StatusChanged
    let event = ui_rx.try_recv().unwrap();
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "status_changed");
    assert_eq!(json["connection_state"], "connected");

    // Second event: LogEntry from emit_log inside set_connection_state
    let event2 = ui_rx.try_recv().unwrap();
    let json2 = serde_json::to_value(&event2).unwrap();
    assert_eq!(json2["type"], "log_entry");
    assert!(json2["entry"].as_str().unwrap().contains("Connected"));
}

#[tokio::test]
async fn emit_log_caps_at_max_entries() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    for i in 0..510 {
        state.logger.log(format!("msg {i}"));
    }
    let entries = state.logger.entries();
    assert_eq!(entries.len(), 500);
    // Oldest entries should have been drained
    assert!(entries[0].contains("msg 10"));
}

#[tokio::test]
async fn sse_events_endpoint_returns_status_changed() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    // Spawn the SSE request
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/events")
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Publish an event
    state
        .set_connection_state(receiver::control_api::ConnectionState::Connecting)
        .await;

    // Read the SSE body — collect frames until we see "status_changed" or timeout
    use http_body_util::BodyExt;
    let mut body = resp.into_body();
    let mut collected = String::new();
    let timeout_result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(Ok(frame)) = body.frame().await {
            if let Some(data) = frame.data_ref() {
                collected.push_str(&String::from_utf8_lossy(data));
                if collected.contains("status_changed") {
                    break;
                }
            }
        }
    })
    .await;
    let _ = timeout_result;

    assert!(
        collected.contains("event: status_changed"),
        "Expected status_changed event in SSE stream, got: {collected}"
    );
    assert!(collected.contains("\"connecting\""));
}

#[tokio::test]
async fn sse_events_endpoint_emits_initial_connected_event() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/events")
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    use http_body_util::BodyExt;
    let mut body = resp.into_body();
    let first_chunk = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while let Some(Ok(frame)) = body.frame().await {
            if let Some(data) = frame.data_ref() {
                return String::from_utf8_lossy(data).to_string();
            }
        }
        String::new()
    })
    .await
    .expect("expected initial SSE frame within 1s");

    assert!(
        first_chunk.contains("event: connected"),
        "Expected initial connected event in SSE stream, got: {first_chunk}"
    );
}

#[tokio::test]
async fn put_subscriptions_emits_status_changed_with_count() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let mut rx = state.ui_tx.subscribe();
    let app = build_router(Arc::clone(&state));

    let body = json!({
        "subscriptions": [
            {"forwarder_id": "f1", "reader_ip": "10.0.0.1:10000", "local_port_override": null},
            {"forwarder_id": "f2", "reader_ip": "10.0.0.2:10000", "local_port_override": null}
        ]
    });
    assert_eq!(
        put_json(app, "/api/v1/subscriptions", body).await,
        StatusCode::NO_CONTENT
    );

    // Expect a StatusChanged event with the updated count
    let mut found_status = false;
    while let Ok(event) = rx.try_recv() {
        let json = serde_json::to_value(&event).unwrap();
        if json["type"] == "status_changed" {
            assert_eq!(json["streams_count"], 2);
            found_status = true;
            break;
        }
    }
    assert!(
        found_status,
        "Expected StatusChanged event after put_subscriptions"
    );
}

#[tokio::test]
async fn put_subscriptions_concurrent_writes_emit_current_count() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let mut rx = state.ui_tx.subscribe();
    let app = build_router(Arc::clone(&state));

    // Hold connection_state so both requests can persist before either emits status.
    let conn_guard = state.connection_state.write().await;

    let first_body = json!({
        "subscriptions": [
            {"forwarder_id": "f1", "reader_ip": "10.0.0.1:10000", "local_port_override": null}
        ]
    });
    let second_body = json!({
        "subscriptions": [
            {"forwarder_id": "f1", "reader_ip": "10.0.0.1:10000", "local_port_override": null},
            {"forwarder_id": "f2", "reader_ip": "10.0.0.2:10000", "local_port_override": null}
        ]
    });

    let app_first = app.clone();
    let first =
        tokio::spawn(async move { put_json(app_first, "/api/v1/subscriptions", first_body).await });
    let second =
        tokio::spawn(async move { put_json(app, "/api/v1/subscriptions", second_body).await });

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let count = {
                let db = state.db.lock().await;
                db.load_subscriptions().unwrap().len()
            };
            if count == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timed out waiting for persisted subscriptions");

    drop(conn_guard);

    assert_eq!(first.await.unwrap(), StatusCode::NO_CONTENT);
    assert_eq!(second.await.unwrap(), StatusCode::NO_CONTENT);

    let mut saw_status = false;
    while let Ok(event) = rx.try_recv() {
        let json = serde_json::to_value(&event).unwrap();
        if json["type"] == "status_changed" {
            saw_status = true;
            assert_eq!(json["streams_count"], 2);
        }
    }
    assert!(saw_status, "Expected at least one status_changed event");
}

#[tokio::test]
async fn put_subscriptions_connected_transitions_to_connecting() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    *state.connection_state.write().await = ConnectionState::Connected;
    assert_eq!(
        put_json(
            app,
            "/api/v1/subscriptions",
            json!({
                "subscriptions": [
                    {"forwarder_id": "f1", "reader_ip": "10.0.0.1:10000", "local_port_override": null}
                ]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        *state.connection_state.read().await,
        ConnectionState::Connecting
    );
}

#[tokio::test]
async fn put_subscriptions_disconnected_triggers_reconnect() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));
    let before_attempt = state.current_connect_attempt();

    assert_eq!(
        put_json(
            app,
            "/api/v1/subscriptions",
            json!({
                "subscriptions": [
                    {"forwarder_id": "f1", "reader_ip": "10.0.0.1:10000", "local_port_override": null}
                ]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        *state.connection_state.read().await,
        ConnectionState::Connecting
    );
    assert!(
        state.current_connect_attempt() > before_attempt,
        "subscription update while disconnected should request a connect attempt"
    );
}

#[tokio::test]
async fn put_subscriptions_connecting_reissues_connect_attempt() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));

    state.request_connect().await;
    let before_attempt = state.current_connect_attempt();

    assert_eq!(
        put_json(
            app,
            "/api/v1/subscriptions",
            json!({
                "subscriptions": [
                    {"forwarder_id": "f1", "reader_ip": "10.0.0.1:10000", "local_port_override": null}
                ]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        *state.connection_state.read().await,
        ConnectionState::Connecting
    );
    assert!(
        state.current_connect_attempt() > before_attempt,
        "subscription update while connecting should request a fresh connect attempt"
    );
}

#[tokio::test]
async fn put_subscriptions_disconnect_in_progress_does_not_reconnect() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(Arc::clone(&state));
    *state.connection_state.write().await = ConnectionState::Disconnecting;
    let before_attempt = state.current_connect_attempt();

    assert_eq!(
        put_json(
            app,
            "/api/v1/subscriptions",
            json!({
                "subscriptions": [
                    {"forwarder_id": "f1", "reader_ip": "10.0.0.1:10000", "local_port_override": null}
                ]
            })
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        *state.connection_state.read().await,
        ConnectionState::Disconnecting
    );
    assert_eq!(
        state.current_connect_attempt(),
        before_attempt,
        "subscription update should not reconnect while disconnecting"
    );
}

// ---------------------------------------------------------------------------
// Selection: default behavior and validation
// ---------------------------------------------------------------------------

/// Without any profile set, GET /selection must return 200 with the default
/// Manual selection (empty stream list, resume policy).
#[tokio::test]
async fn selection_defaults_to_manual_empty_without_profile() {
    let app = setup();
    let (status, val) = get_json(app, "/api/v1/selection").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["selection"]["mode"], "manual");
    assert_eq!(val["selection"]["streams"], json!([]));
    assert_eq!(val["replay_policy"], "resume");
    assert!(val["replay_targets"].is_null());
}

/// With a profile set and Manual{[]} selection, GET /streams reports no
/// streams — the receiver has no ports to proxy until subscriptions are added.
#[tokio::test]
async fn manual_empty_selection_results_in_no_streams_to_proxy() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/selection",
            json!({"selection":{"mode":"manual","streams":[]},"replay_policy":"resume"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let (status, val) = get_json(app, "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["streams"], json!([]));
}

/// PUT /api/v1/selection with mode=race and an empty race_id must be rejected
/// with 400 to prevent a hard-to-diagnose connection failure at the protocol level.
#[tokio::test]
async fn put_selection_race_mode_rejects_empty_race_id() {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db);
    let app = build_router(state);

    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/profile",
            json!({"server_url":"wss://s.com","token":"tok"})
        )
        .await,
        StatusCode::NO_CONTENT
    );

    // Empty string race_id.
    assert_eq!(
        put_json(
            app.clone(),
            "/api/v1/selection",
            json!({
                "selection":{"mode":"race","race_id":"","epoch_scope":"current"},
                "replay_policy":"resume"
            })
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    // Whitespace-only race_id.
    assert_eq!(
        put_json(
            app,
            "/api/v1/selection",
            json!({
                "selection":{"mode":"race","race_id":"   ","epoch_scope":"current"},
                "replay_policy":"resume"
            })
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

//! Auth matrix: every protected HTTP route rejects missing, wrong-kind, or
//! pending credentials and accepts only its documented credential state.

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use server::http::{AppState, router};
use tower::ServiceExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeviceKind {
    Forwarder,
    Receiver,
}

impl DeviceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Forwarder => "forwarder",
            Self::Receiver => "receiver",
        }
    }
}

#[derive(Debug)]
struct DeviceTokens {
    active_forwarder: String,
    other_active_forwarder: String,
    pending_forwarder: String,
    active_receiver: String,
    pending_receiver: String,
}

#[derive(Clone, Copy, Debug)]
enum Bearer {
    None,
    ActiveForwarder,
    OtherActiveForwarder,
    PendingForwarder,
    ActiveReceiver,
    PendingReceiver,
}

impl Bearer {
    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ActiveForwarder => "active-forwarder",
            Self::OtherActiveForwarder => "other-active-forwarder",
            Self::PendingForwarder => "pending-forwarder",
            Self::ActiveReceiver => "active-receiver",
            Self::PendingReceiver => "pending-receiver",
        }
    }

    fn token(self, tokens: &DeviceTokens) -> Option<&str> {
        match self {
            Self::None => None,
            Self::ActiveForwarder => Some(&tokens.active_forwarder),
            Self::OtherActiveForwarder => Some(&tokens.other_active_forwarder),
            Self::PendingForwarder => Some(&tokens.pending_forwarder),
            Self::ActiveReceiver => Some(&tokens.active_receiver),
            Self::PendingReceiver => Some(&tokens.pending_receiver),
        }
    }
}

struct TestContext {
    state: AppState,
    tokens: DeviceTokens,
}

impl TestContext {
    async fn new() -> Self {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        server::db::migrate(&conn).unwrap();
        server::registry::migrate(&conn).unwrap();
        let state = AppState::new(conn, true);

        let active_forwarder =
            register_device(&state, "auth-fwd-active", DeviceKind::Forwarder, true).await;
        let other_active_forwarder =
            register_device(&state, "auth-fwd-other", DeviceKind::Forwarder, true).await;
        let pending_forwarder =
            register_device(&state, "auth-fwd-pending", DeviceKind::Forwarder, false).await;
        let active_receiver =
            register_device(&state, "auth-rx-active", DeviceKind::Receiver, true).await;
        let pending_receiver =
            register_device(&state, "auth-rx-pending", DeviceKind::Receiver, false).await;

        Self {
            state,
            tokens: DeviceTokens {
                active_forwarder,
                other_active_forwarder,
                pending_forwarder,
                active_receiver,
                pending_receiver,
            },
        }
    }
}

async fn register_device(
    state: &AppState,
    endpoint_id: &str,
    device_kind: DeviceKind,
    approve: bool,
) -> String {
    let app = router(state.clone());
    let voucher_resp = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/admin/enrollment-tokens",
            Some("admin"),
            None,
            &json!({
                "device_kind": device_kind.as_str(),
                "display_name": format!("{endpoint_id} name")
            }),
        ))
        .await
        .unwrap();
    assert_eq!(voucher_resp.status(), StatusCode::OK);
    let voucher_body = response_json(voucher_resp).await;
    let voucher = voucher_body["token"].as_str().unwrap();

    let register_resp = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/register",
            None,
            Some(voucher),
            &json!({
                "endpoint_id": endpoint_id,
                "device_kind": device_kind.as_str(),
                "display_name": format!("{endpoint_id} device")
            }),
        ))
        .await
        .unwrap();
    assert_eq!(register_resp.status(), StatusCode::OK);
    let register_body = response_json(register_resp).await;
    let device_token = register_body["device_token"].as_str().unwrap().to_owned();

    if approve {
        let approve_resp = app
            .oneshot(json_request(
                Method::POST,
                "/admin/devices/approve",
                Some("admin"),
                None,
                &json!({ "endpoint_id": endpoint_id }),
            ))
            .await
            .unwrap();
        assert_eq!(approve_resp.status(), StatusCode::OK);
    }

    device_token
}

fn json_request(
    method: Method,
    uri: &str,
    admin_user: Option<&str>,
    bearer: Option<&str>,
    body: &Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json");
    if let Some(user) = admin_user {
        builder = builder.header(server::http::status::ADMIN_HEADER, user);
    }
    if let Some(token) = bearer {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    builder
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn empty_request(method: Method, uri: &str, bearer: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

async fn response_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn device_route_auth_matrix() {
    struct Case {
        name: &'static str,
        method: Method,
        uri: &'static str,
        body: Option<Value>,
        expected: Vec<(Bearer, StatusCode)>,
    }

    let cases = vec![
        Case {
            name: "GET /allowlist/receivers",
            method: Method::GET,
            uri: "/allowlist/receivers",
            body: None,
            expected: vec![
                (Bearer::ActiveForwarder, StatusCode::OK),
                (Bearer::ActiveReceiver, StatusCode::UNAUTHORIZED),
                (Bearer::PendingForwarder, StatusCode::UNAUTHORIZED),
                (Bearer::None, StatusCode::UNAUTHORIZED),
            ],
        },
        Case {
            name: "GET /forwarders",
            method: Method::GET,
            uri: "/forwarders",
            body: None,
            expected: vec![
                (Bearer::ActiveReceiver, StatusCode::OK),
                (Bearer::ActiveForwarder, StatusCode::UNAUTHORIZED),
                (Bearer::PendingReceiver, StatusCode::UNAUTHORIZED),
                (Bearer::None, StatusCode::UNAUTHORIZED),
            ],
        },
        Case {
            name: "POST /announcer/rows",
            method: Method::POST,
            uri: "/announcer/rows",
            body: Some(json!({
                "announcer_source_generation": 0,
                "forwarder_endpoint_id": "fwd-auth",
                "stream_id": "stream-auth",
                "rows": [{
                    "seq": 1,
                    "chip_id": "chip-auth",
                    "bib": 101,
                    "display_name": "Runner Auth",
                    "reader_timestamp": "10:00:00",
                    "received_unix_ms": 1_000,
                    "division": "5k"
                }],
                "max_list_size": 25
            })),
            expected: vec![
                (Bearer::ActiveReceiver, StatusCode::OK),
                (Bearer::ActiveForwarder, StatusCode::UNAUTHORIZED),
                (Bearer::PendingReceiver, StatusCode::UNAUTHORIZED),
                (Bearer::None, StatusCode::UNAUTHORIZED),
            ],
        },
        Case {
            name: "POST /announcer/takeover",
            method: Method::POST,
            uri: "/announcer/takeover",
            body: Some(json!({})),
            expected: vec![
                (Bearer::ActiveReceiver, StatusCode::OK),
                (Bearer::ActiveForwarder, StatusCode::UNAUTHORIZED),
                (Bearer::PendingReceiver, StatusCode::UNAUTHORIZED),
                (Bearer::None, StatusCode::UNAUTHORIZED),
            ],
        },
        Case {
            name: "POST /forwarder/catalog",
            method: Method::POST,
            uri: "/forwarder/catalog",
            body: Some(json!({
                "endpoint_id": "auth-fwd-active",
                "display_name": null,
                "direct_addrs": [],
                "streams": []
            })),
            expected: vec![
                (Bearer::ActiveForwarder, StatusCode::OK),
                (Bearer::PendingForwarder, StatusCode::UNAUTHORIZED),
                (Bearer::OtherActiveForwarder, StatusCode::UNAUTHORIZED),
                (Bearer::ActiveReceiver, StatusCode::UNAUTHORIZED),
                (Bearer::None, StatusCode::UNAUTHORIZED),
            ],
        },
        Case {
            name: "POST /forwarder/catalog pending own token",
            method: Method::POST,
            uri: "/forwarder/catalog",
            body: Some(json!({
                "endpoint_id": "auth-fwd-pending",
                "display_name": null,
                "direct_addrs": [],
                "streams": []
            })),
            expected: vec![(Bearer::PendingForwarder, StatusCode::OK)],
        },
    ];

    for case in cases {
        let ctx = TestContext::new().await;
        let app = router(ctx.state);
        for (bearer, expected_status) in case.expected {
            let request = match &case.body {
                Some(body) => json_request(
                    case.method.clone(),
                    case.uri,
                    None,
                    bearer.token(&ctx.tokens),
                    body,
                ),
                None => empty_request(case.method.clone(), case.uri, bearer.token(&ctx.tokens)),
            };
            let resp = app.clone().oneshot(request).await.unwrap();
            assert_eq!(
                resp.status(),
                expected_status,
                "{} with bearer {}",
                case.name,
                bearer.label()
            );
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn admin_route_auth_matrix() {
    assert_admin_status(
        "POST /admin/devices/approve trusted header",
        true,
        Method::POST,
        "/admin/devices/approve",
        Some("admin"),
        &json!({ "endpoint_id": "missing-device" }),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_admin_status(
        "POST /admin/devices/approve no header",
        true,
        Method::POST,
        "/admin/devices/approve",
        None,
        &json!({ "endpoint_id": "missing-device" }),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_admin_status(
        "POST /admin/devices/approve untrusted proxy",
        false,
        Method::POST,
        "/admin/devices/approve",
        Some("admin"),
        &json!({ "endpoint_id": "missing-device" }),
        StatusCode::UNAUTHORIZED,
    )
    .await;

    assert_admin_status(
        "GET /admin/enrollment-tokens trusted header",
        true,
        Method::GET,
        "/admin/enrollment-tokens",
        Some("admin"),
        &json!({}),
        StatusCode::OK,
    )
    .await;
    assert_admin_status(
        "GET /admin/enrollment-tokens no header",
        true,
        Method::GET,
        "/admin/enrollment-tokens",
        None,
        &json!({}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_admin_status(
        "GET /admin/enrollment-tokens untrusted proxy",
        false,
        Method::GET,
        "/admin/enrollment-tokens",
        Some("admin"),
        &json!({}),
        StatusCode::UNAUTHORIZED,
    )
    .await;

    assert_admin_status(
        "POST /admin/enrollment-tokens trusted header",
        true,
        Method::POST,
        "/admin/enrollment-tokens",
        Some("admin"),
        &json!({ "device_kind": "receiver", "display_name": "rx voucher" }),
        StatusCode::OK,
    )
    .await;
    assert_admin_status(
        "POST /admin/enrollment-tokens no header",
        true,
        Method::POST,
        "/admin/enrollment-tokens",
        None,
        &json!({ "device_kind": "receiver", "display_name": "rx voucher" }),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_admin_status(
        "POST /admin/enrollment-tokens untrusted proxy",
        false,
        Method::POST,
        "/admin/enrollment-tokens",
        Some("admin"),
        &json!({ "device_kind": "receiver", "display_name": "rx voucher" }),
        StatusCode::UNAUTHORIZED,
    )
    .await;

    assert_revoke_status(
        "POST /admin/enrollment-tokens/{token_id}/revoke trusted header",
        true,
        Some("admin"),
        StatusCode::OK,
    )
    .await;
    assert_revoke_status(
        "POST /admin/enrollment-tokens/{token_id}/revoke no header",
        true,
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_revoke_status(
        "POST /admin/enrollment-tokens/{token_id}/revoke untrusted proxy",
        false,
        Some("admin"),
        StatusCode::UNAUTHORIZED,
    )
    .await;
}

async fn assert_admin_status(
    label: &str,
    admin_proxy_trusted: bool,
    method: Method,
    uri: &str,
    admin_user: Option<&str>,
    body: &Value,
    expected: StatusCode,
) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    server::db::migrate(&conn).unwrap();
    server::registry::migrate(&conn).unwrap();
    let state = AppState::new(conn, admin_proxy_trusted);
    let resp = router(state)
        .oneshot(json_request(method, uri, admin_user, None, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), expected, "{label}");
}

async fn assert_revoke_status(
    label: &str,
    admin_proxy_trusted: bool,
    admin_user: Option<&str>,
    expected: StatusCode,
) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    server::db::migrate(&conn).unwrap();
    server::registry::migrate(&conn).unwrap();
    let state = AppState::new(conn, admin_proxy_trusted);
    let app = router(state);

    let uri = if expected == StatusCode::OK {
        let create_resp = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/admin/enrollment-tokens",
                Some("admin"),
                None,
                &json!({ "device_kind": "receiver", "display_name": "rx voucher" }),
            ))
            .await
            .unwrap();
        assert_eq!(create_resp.status(), StatusCode::OK);
        let create_body = response_json(create_resp).await;
        let token_id = create_body["token_id"].as_str().unwrap();
        format!("/admin/enrollment-tokens/{token_id}/revoke")
    } else {
        "/admin/enrollment-tokens/unused-token/revoke".to_owned()
    };

    let resp = app
        .oneshot(json_request(
            Method::POST,
            &uri,
            admin_user,
            None,
            &json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), expected, "{label}");
}

//! HTTP surface for the thin node.

pub mod allowlist;
pub mod announcer;
pub mod catalog;
pub mod forwarders;
pub mod register;
pub mod status;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    routing::{get, post},
};
use rusqlite::Connection;

use crate::announcer::AnnouncerRuntime;

/// Shared application state for HTTP handlers.
///
/// Holds the `SQLite` connection (guarded by a mutex, since rusqlite connections
/// are single-threaded) and the hashed provisioning bearer token used to
/// authorize `POST /register`, `POST /forwarder/catalog`, `POST
/// /announcer/rows`, and `POST /announcer/takeover`.
///
/// `admin_proxy_trusted` is the fail-closed guard for `/admin/*` routes: those
/// routes trust the upstream-injected [`status::ADMIN_HEADER`] only when this is
/// `true`, which an operator opts into at startup to assert that a header-
/// stripping reverse proxy (Caddy/Authelia) sits in front of the node. When
/// `false`, admin routes are denied regardless of any client-supplied header.
#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    pub provisioning_token_hash: Arc<Vec<u8>>,
    pub announcer_runtime: Arc<Mutex<AnnouncerRuntime>>,
    pub admin_proxy_trusted: bool,
}

impl AppState {
    #[must_use]
    pub fn new(conn: Connection, provisioning_token: &str, admin_proxy_trusted: bool) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            provisioning_token_hash: Arc::new(crate::registry::hash_token(provisioning_token)),
            announcer_runtime: Arc::new(Mutex::new(AnnouncerRuntime::new())),
            admin_proxy_trusted,
        }
    }
}

/// Build the thin-node HTTP router.
///
/// Routes are grouped by the auth posture documented in [`status`]:
///
/// - Public (unauthenticated): `GET /status`.
/// - Admin (upstream [`status::ADMIN_HEADER`] required): `POST
///   /admin/devices/approve`.
/// - M2M/device (in-process provisioning bearer auth): `POST /register`, `POST
///   /forwarder/catalog`, `POST /announcer/rows`, `POST /announcer/takeover`.
pub fn router(state: AppState) -> Router {
    Router::new()
        // Public, unauthenticated read endpoints.
        .route("/status", get(status::status))
        // Admin endpoints — must be protected by Caddy/Authelia.
        .route("/admin/devices/approve", post(status::approve_device))
        // M2M/device endpoints — in-process provisioning bearer auth.
        .route("/register", post(register::register))
        .route("/forwarder/catalog", post(catalog::push_catalog))
        .route("/announcer/rows", post(announcer::push_row))
        .route("/announcer/takeover", post(announcer::takeover))
        .route("/allowlist/receivers", get(allowlist::receiver_allowlist))
        .route("/forwarders", get(forwarders::list_forwarders))
        .fallback(crate::ui_server::serve_ui)
        .with_state(state)
}

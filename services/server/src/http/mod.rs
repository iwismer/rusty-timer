//! HTTP surface for the server.

pub mod allowlist;
pub mod announcer;
pub mod catalog;
pub mod enrollment_tokens;
pub mod forwarders;
pub mod register;
pub mod status;
mod validate;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use rusqlite::Connection;
use tokio::sync::watch;

use crate::announcer::AnnouncerRuntime;
use crate::registry::{self, ApprovalState, DeviceKind};

/// Shared application state for HTTP handlers.
///
/// Holds the `SQLite` connection (guarded by a mutex, since rusqlite connections
/// are single-threaded). Devices authenticate with their server-minted
/// per-device token (`authenticate_device`); there is no shared secret.
///
/// `admin_proxy_trusted` is the fail-closed guard for `/admin/*` routes: those
/// routes trust the upstream-injected [`status::ADMIN_HEADER`] only when this is
/// `true`, which an operator opts into at startup to assert that a header-
/// stripping reverse proxy (Caddy/Authelia) sits in front of the node. When
/// `false`, admin routes are denied regardless of any client-supplied header.
#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    pub announcer_runtime: Arc<Mutex<AnnouncerRuntime>>,
    pub admin_proxy_trusted: bool,
    /// Monotonic receiver allow-list version. Bumped whenever the set of active
    /// receivers could change (currently: device approval), so forwarders
    /// long-polling `GET /allowlist/receivers` are released immediately instead
    /// of waiting for their periodic poll backstop. In-memory only: a server
    /// restart resets it to 0, which forwarders self-heal from because a
    /// mismatched `since` cursor returns the current snapshot immediately.
    pub allowlist_version: watch::Sender<u64>,
}

impl AppState {
    #[must_use]
    pub fn new(conn: Connection, admin_proxy_trusted: bool) -> Self {
        let announcer_runtime = Arc::new(Mutex::new(AnnouncerRuntime::new()));
        // Rebuild the live announcer board from persisted rows so a server
        // restart does not blank /status until the next push. The receiver's
        // per-push max_list_size is unknown at startup; use the default cap.
        match registry::list_announcer_rows_ordered(&conn) {
            Ok(rows) => {
                announcer::rebuild_runtime(
                    &announcer_runtime,
                    rows,
                    announcer::DEFAULT_MAX_ANNOUNCER_ROWS,
                );
            }
            Err(err) => {
                tracing::error!(error = ?err, "failed to rebuild announcer runtime from db");
            }
        }
        Self {
            conn: Arc::new(Mutex::new(conn)),
            announcer_runtime,
            admin_proxy_trusted,
            allowlist_version: watch::channel(0).0,
        }
    }

    /// Bump the receiver allow-list version, releasing any forwarders currently
    /// long-polling for a change. Safe to call on every approval; an extra bump
    /// only causes one harmless idempotent re-fetch.
    pub fn bump_allowlist_version(&self) {
        self.allowlist_version.send_modify(|version| {
            *version = version.wrapping_add(1);
        });
    }
}

/// Authorize an M2M request that any **active** device of `kind` may make
/// (receiver allow-list distribution, forwarder discovery, announcer push).
///
/// Authenticates the bearer as a minted per-device token of the right kind that
/// is `active`; anything else (unknown, wrong kind, pending) is rejected.
/// `Err(status)` carries the
/// reject (`401`) or internal-error (`500`) code.
pub(crate) fn authorize_active_device_kind(
    state: &AppState,
    headers: &HeaderMap,
    kind: DeviceKind,
) -> Result<(), StatusCode> {
    let Some(raw) = register::bearer_token(headers) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let conn = state.conn.lock().expect("registry mutex poisoned");
    match registry::authenticate_device(&conn, raw) {
        Ok(Some(record))
            if record.device_kind == kind && record.approval_state == ApprovalState::Active =>
        {
            Ok(())
        }
        Ok(_) => Err(StatusCode::UNAUTHORIZED),
        Err(err) => {
            tracing::error!(error = %err, "device authentication failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Authorize a forwarder catalog push for `endpoint_id`, regardless of approval
/// state (a pending forwarder must publish its catalog so an admin can approve
/// it). Requires the forwarder's own minted device token, bound to `endpoint_id`.
pub(crate) fn authorize_forwarder_catalog(
    state: &AppState,
    headers: &HeaderMap,
    endpoint_id: &str,
) -> Result<(), StatusCode> {
    let Some(raw) = register::bearer_token(headers) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let conn = state.conn.lock().expect("registry mutex poisoned");
    match registry::authenticate_device(&conn, raw) {
        Ok(Some(record))
            if record.device_kind == DeviceKind::Forwarder && record.endpoint_id == endpoint_id =>
        {
            Ok(())
        }
        Ok(_) => Err(StatusCode::UNAUTHORIZED),
        Err(err) => {
            tracing::error!(error = %err, "device authentication failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Authorize a self-scoped forwarder request, resolving the caller's endpoint
/// id from its own minted device token (any approval state, matching the
/// catalog-push posture). Receiver tokens and unknown bearers are rejected.
pub(crate) fn authorize_forwarder_self(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, StatusCode> {
    let Some(raw) = register::bearer_token(headers) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let conn = state.conn.lock().expect("registry mutex poisoned");
    match registry::authenticate_device(&conn, raw) {
        Ok(Some(record)) if record.device_kind == DeviceKind::Forwarder => Ok(record.endpoint_id),
        Ok(_) => Err(StatusCode::UNAUTHORIZED),
        Err(err) => {
            tracing::error!(error = %err, "device authentication failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Build the server HTTP router.
///
/// Routes are grouped by the auth posture documented in [`status`]:
///
/// - Public (unauthenticated): `GET /status`.
/// - Admin (upstream [`status::ADMIN_HEADER`] required): `POST
///   /admin/devices/approve` and enrollment token management under
///   `/admin/enrollment-tokens`.
/// - M2M/device: every device route authenticates a server-minted per-device
///   token (`authenticate_device`) and asserts kind/approval per endpoint
///   (active forwarder for `/allowlist/receivers`; active receiver for
///   `/forwarders` and `/announcer/*`; the matching forwarder for
///   `/forwarder/catalog`). `/register` accepts an enrollment voucher or the
///   device's own token.
pub fn router(state: AppState) -> Router {
    Router::new()
        // Public, unauthenticated read endpoints.
        .route("/status", get(status::status))
        // Admin endpoints — must be protected by Caddy/Authelia.
        .route("/admin/devices/approve", post(status::approve_device))
        .route(
            "/admin/enrollment-tokens",
            get(enrollment_tokens::list_tokens).post(enrollment_tokens::create_token),
        )
        .route(
            "/admin/enrollment-tokens/{token_id}/revoke",
            post(enrollment_tokens::revoke_token),
        )
        // M2M/device endpoints — per-device minted-token auth.
        .route("/register", post(register::register))
        .route(
            "/forwarder/catalog",
            post(catalog::push_catalog).get(catalog::get_own_catalog),
        )
        .route("/announcer/rows", post(announcer::push_row))
        .route("/announcer/takeover", post(announcer::takeover))
        .route("/allowlist/receivers", get(allowlist::receiver_allowlist))
        .route("/forwarders", get(forwarders::list_forwarders))
        .fallback(crate::ui_server::serve_ui)
        .with_state(state)
}

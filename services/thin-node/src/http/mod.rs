//! HTTP surface for the thin node.

pub mod register;

use std::sync::{Arc, Mutex};

use axum::{Router, routing::post};
use rusqlite::Connection;

/// Shared application state for HTTP handlers.
///
/// Holds the `SQLite` connection (guarded by a mutex, since rusqlite connections
/// are single-threaded) and the hashed provisioning bearer token used to
/// authorize `POST /register`.
#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    pub provisioning_token_hash: Arc<Vec<u8>>,
}

impl AppState {
    #[must_use]
    pub fn new(conn: Connection, provisioning_token: &str) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            provisioning_token_hash: Arc::new(crate::registry::hash_token(provisioning_token)),
        }
    }
}

/// Build the registration router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/register", post(register::register))
        .with_state(state)
}

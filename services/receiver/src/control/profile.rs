//! Profile, server-config, and update-mode control handlers.

use crate::control_api::{AppState, ConnectionState};
use crate::db::DEFAULT_UPDATE_MODE;
use crate::error::ReceiverError;
use rt_domain::ReceiverMode;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::imports::reload_chip_lookup;

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileRequest {
    pub server_url: String,
    pub token: String,
    #[serde(default)]
    pub receiver_id: Option<String>,
}

fn is_valid_receiver_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_uuid_format(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }

    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    /// Effective server URL (resolved: env override > profile).
    pub server_url: String,
    /// Effective server token (resolved: env override > profile).
    pub token: String,
    pub receiver_id: String,
    /// Where the effective server config comes from: `"env"` (RT_P2P_SERVER_*
    /// override active), `"profile"` (stored profile), or `"none"`. The UI
    /// renders the URL/token read-only when this is `"env"`.
    pub server_source: String,
    /// Global announcer publish toggle state.
    pub announcer_enabled: bool,
    /// Receiver-configured cap on visible rows in the server announcer feed.
    pub announcer_max_list_size: u32,
}

/// Enable or disable the global announcer publish toggle. The P2P reconcile
/// loop picks up the change on its next pass (within one reconcile interval).
pub async fn set_announcer_enabled(state: &AppState, enabled: bool) -> Result<(), ReceiverError> {
    {
        let db = state.storage.db.lock().await;
        db.set_announcer_enabled(enabled)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    // Other UI/bridge clients hold this in profile state; nudge them to refetch.
    state.emit_resync();
    Ok(())
}

/// Set the receiver-configured cap on visible rows in the server announcer
/// feed. The value is clamped to `1..=500` and rides the next announcer push to
/// the server. The P2P reconcile loop picks up the change on its next pass.
pub async fn set_announcer_max_list_size(
    state: &AppState,
    max_list_size: u32,
) -> Result<(), ReceiverError> {
    let clamped = max_list_size.clamp(1, 500);
    {
        let db = state.storage.db.lock().await;
        db.set_announcer_max_list_size(clamped)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    // Other UI/bridge clients hold this in profile state; nudge them to refetch.
    state.emit_resync();
    Ok(())
}

/// Reset the server announcer feed between races.
///
/// Calls `POST /announcer/reset` on the configured server with the receiver's
/// device token, then marks all existing received events as announcer-pushed
/// so older reads never push into the fresh race.
pub async fn reset_announcer(state: &AppState) -> Result<(), ReceiverError> {
    let (server_url, mut token) = {
        let db = state.storage.db.lock().await;
        let profile = db
            .load_profile()
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
        let override_ = state.server_override().await;
        let resolved = crate::runtime::resolve_server_config(profile.as_ref(), override_);
        let device_token = db
            .load_device_token()
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;

        match resolved {
            Some(s) if !s.url.trim().is_empty() => {
                let token = device_token.unwrap_or(s.token);
                if token.trim().is_empty() {
                    return Err(ReceiverError::BadRequest(
                        "server URL and token must be configured to reset the announcer feed"
                            .into(),
                    ));
                }
                (s.url, token)
            }
            _ => {
                return Err(ReceiverError::BadRequest(
                    "server URL and token must be configured to reset the announcer feed".into(),
                ));
            }
        }
    };

    if state
        .storage
        .db
        .lock()
        .await
        .load_device_token()
        .ok()
        .flatten()
        .is_none()
        && let Some(endpoint_id) = state.forwarders.p2p_endpoint_id.read().await.clone()
    {
        let url = server_url.clone();
        let voucher = token.clone();
        let receiver_id = state.receiver_id.read().await.clone();
        let minted = tokio::task::spawn_blocking(move || {
            crate::announcer_push::register_receiver_with_server(
                &url,
                &voucher,
                &endpoint_id,
                &receiver_id,
            )
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten();

        if let Some(minted_token) = minted {
            let _ = state
                .storage
                .db
                .lock()
                .await
                .set_device_token(&minted_token);
            token = minted_token;
        }
    }

    crate::announcer_push::reset_announcer_feed(&state.http_client, &server_url, &token)
        .await
        .map_err(|e| ReceiverError::Internal(e.to_string()))?;

    {
        let db = state.storage.db.lock().await;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let _ = db.mark_all_announcer_events_pushed(now_ms);
        let _ = db.reset_announcer_fences();
    }

    state.emit_resync();
    Ok(())
}

/// Choose the server URL+token to persist on a profile save.
///
/// When a server env override is active the UI locks the URL/token inputs and
/// `get_profile` returns the effective (env) values, which the client echoes
/// back on save. Persisting those would copy the env token into the profile
/// DB, so the stored values are preserved instead. Otherwise the request body
/// (already trimmed for the URL) is persisted.
pub(crate) fn server_fields_to_persist(
    env_active: bool,
    body_url: String,
    body_token: String,
    existing: Option<&crate::db::Profile>,
) -> (String, String) {
    if env_active {
        existing.map_or((String::new(), String::new()), |p| {
            (p.server_url.clone(), p.token.clone())
        })
    } else {
        (body_url, body_token)
    }
}

/// Whether a server URL+token override is active (both set and non-empty).
/// When active, the stored profile server fields are read-only in the UI and
/// must not be overwritten by a profile save.
fn server_override_active(override_: &(Option<String>, Option<String>)) -> bool {
    let non_empty = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
    non_empty(&override_.0) && non_empty(&override_.1)
}

pub async fn get_profile(state: &AppState) -> Result<ProfileResponse, ReceiverError> {
    let receiver_id = state.receiver_id.read().await.clone();
    let profile = {
        let db = state.storage.db.lock().await;
        db.load_profile()
            .map_err(|e| ReceiverError::Internal(e.to_string()))?
    };

    // Report the effective server config and its source so the UI can show the
    // real values (and lock the fields) when an override is active.
    let override_ = state.server_override().await;
    let env_active = server_override_active(&override_);
    let resolved = crate::runtime::resolve_server_config(profile.as_ref(), override_);
    let server_source = if env_active {
        "env"
    } else if resolved.is_some() {
        "profile"
    } else {
        "none"
    }
    .to_owned();
    let (server_url, token) =
        resolved.map_or_else(|| (String::new(), String::new()), |s| (s.url, s.token));
    let (announcer_enabled, announcer_max_list_size) = {
        let db = state.storage.db.lock().await;
        (
            db.load_announcer_enabled().unwrap_or(false),
            db.load_announcer_max_list_size().unwrap_or(25),
        )
    };
    Ok(ProfileResponse {
        server_url,
        token,
        receiver_id,
        server_source,
        announcer_enabled,
        announcer_max_list_size,
    })
}

pub async fn get_mode(state: &AppState) -> Result<ReceiverMode, ReceiverError> {
    let db = state.storage.db.lock().await;
    match db.load_receiver_mode() {
        Ok(Some(mode)) => Ok(mode),
        Ok(None) => Err(ReceiverError::NotFound("no mode configured".to_owned())),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn put_profile(state: &AppState, body: ProfileRequest) -> Result<(), ReceiverError> {
    let url = body.server_url.trim().trim_end_matches('/').to_owned();

    let new_receiver_id = body
        .receiver_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);

    if let Some(ref id) = new_receiver_id
        && !is_valid_receiver_id(id)
    {
        return Err(ReceiverError::BadRequest(
            "receiver_id must be 1-64 characters, alphanumeric/hyphens/underscores only".to_owned(),
        ));
    }

    let mut db = state.storage.db.lock().await;
    let existing = db.load_profile().ok().flatten();
    let persist_receiver_id = new_receiver_id
        .clone()
        .or_else(|| existing.as_ref().and_then(|p| p.receiver_id.clone()));

    let (persist_url, persist_token) = server_fields_to_persist(
        server_override_active(&state.server_override().await),
        url,
        body.token.clone(),
        existing.as_ref(),
    );

    match db.save_profile(
        &persist_url,
        &persist_token,
        DEFAULT_UPDATE_MODE,
        persist_receiver_id.as_deref(),
    ) {
        Ok(()) => {
            drop(db);
            if let Some(id) = new_receiver_id {
                *state.receiver_id.write().await = id;
            }
            // The server URL+token may have changed; signal the P2P reconcile
            // loop to rebind its server-bound tasks (register/takeover,
            // discovery, announcer).
            state.notify_server_config_changed();
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn put_mode(state: &AppState, mode: ReceiverMode) -> Result<(), ReceiverError> {
    if let ReceiverMode::Race { race_id } = &mode {
        if race_id.trim().is_empty() {
            return Err(ReceiverError::BadRequest(
                "race_id must not be empty when mode is race".to_owned(),
            ));
        }
        if !is_uuid_format(race_id) {
            return Err(ReceiverError::BadRequest(
                "race_id must be a valid UUID when mode is race".to_owned(),
            ));
        }
    }

    let db = state.storage.db.lock().await;
    match db.save_receiver_mode(&mode) {
        Ok(()) => {
            drop(db);
            let _ = state
                .ui
                .ui_tx
                .send(crate::ui_events::ReceiverUiEvent::ModeChanged { mode: mode.clone() });
            state.emit_streams_snapshot().await;
            state.request_connect().await;
            Ok(())
        }
        Err(crate::db::DbError::ProfileMissing) => {
            Err(ReceiverError::NotFound("no profile".to_owned()))
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_profile(state: &AppState) -> Result<(), ReceiverError> {
    let current = state.signals.connection_state.borrow().clone();
    if current != ConnectionState::Disconnected {
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        state.request_disconnect_shutdown();
    }
    let db = state.storage.db.lock().await;
    match db.reset_profile() {
        Ok(()) => {
            drop(db);
            *state.receiver_id.write().await = String::new();
            // The server URL+token were cleared; rebind the always-on P2P
            // runtime so it drops its old server-bound tasks immediately
            // instead of waiting for a later profile save.
            state.notify_server_config_changed();
            state.emit_streams_snapshot().await;
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_factory_reset(state: &AppState) -> Result<(), ReceiverError> {
    let current = state.signals.connection_state.borrow().clone();
    if current != ConnectionState::Disconnected {
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        state.request_disconnect_shutdown();
    }
    let mut db = state.storage.db.lock().await;
    match db.factory_reset() {
        Ok(()) => {
            drop(db);
            state.ui.stream_counts.clear();
            state.clear_stream_metrics_cache().await;
            state.clear_all_stream_delta_buffer();
            state.clear_forwarder_intent_cache();
            *state.receiver_id.write().await = String::new();
            // Drop the now-empty participant/chip lookup from memory so a
            // factory reset does not leave prior identities resolvable.
            if let Err(e) = reload_chip_lookup(state).await {
                warn!(error = %e, "failed to reload chip lookup after factory reset");
            }
            // The server config was wiped; rebind the always-on P2P runtime.
            state.notify_server_config_changed();
            state.emit_streams_snapshot().await;
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn reset_announcer_uses_minted_device_token_over_voucher() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let app = axum::Router::new().route(
            "/announcer/reset",
            axum::routing::post(|headers: axum::http::HeaderMap| async move {
                let auth = headers.get("authorization").and_then(|v| v.to_str().ok());
                if auth == Some("Bearer minted-device-token-123") {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "announcer_source_generation": 42,
                            "finisher_count": 0,
                        })),
                    )
                        .into_response()
                } else {
                    axum::http::StatusCode::UNAUTHORIZED.into_response()
                }
            }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut db = Db::open_in_memory().unwrap();
        // Persist profile with voucher token
        db.save_profile(
            &format!("http://127.0.0.1:{port}"),
            "voucher-token-456",
            DEFAULT_UPDATE_MODE,
            Some("recv-test"),
        )
        .unwrap();
        // Persist minted device token in DB
        db.set_device_token("minted-device-token-123").unwrap();

        let (state, _rx) = AppState::new(db, "recv-test".to_owned());
        let result = reset_announcer(&state).await;
        assert!(
            result.is_ok(),
            "reset_announcer should succeed with minted device token: {result:?}"
        );
    }

    #[tokio::test]
    async fn reset_announcer_reports_helpful_error_on_401() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let app = axum::Router::new().route(
            "/announcer/reset",
            axum::routing::post(|| async { axum::http::StatusCode::UNAUTHORIZED.into_response() }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut db = Db::open_in_memory().unwrap();
        db.save_profile(
            &format!("http://127.0.0.1:{port}"),
            "voucher-token-456",
            DEFAULT_UPDATE_MODE,
            Some("recv-test"),
        )
        .unwrap();

        let (state, _rx) = AppState::new(db, "recv-test".to_owned());
        let result = reset_announcer(&state).await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("401 Unauthorized"),
            "Error should mention 401 Unauthorized: {err}"
        );
    }
}

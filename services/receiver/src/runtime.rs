use crate::control_api::{AppState, ConnectionState, ShutdownSignal};
use crate::db::Db;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn};

pub fn generate_receiver_id() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("failed to generate random bytes");
    format!("recv-{:08x}", u32::from_be_bytes(bytes))
}

pub fn resolve_receiver_id(cli_id: Option<String>, db: &Db) -> Result<String, String> {
    if let Some(id) = cli_id
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        if id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(
                "receiver_id must be 1-64 characters, alphanumeric/hyphens/underscores only"
                    .to_owned(),
            );
        }
        if let Err(e) = db.save_receiver_id(&id) {
            warn!(error = %e, "failed to persist CLI receiver_id to DB");
        }
        return Ok(id);
    }

    match db.load_profile() {
        Ok(Some(p)) => {
            if let Some(id) = p.receiver_id.filter(|id| !id.is_empty()) {
                return Ok(id);
            }
        }
        Ok(None) => {}
        Err(e) => {
            let id = generate_receiver_id();
            warn!(error = %e, receiver_id = %id, "failed to load profile; using ephemeral receiver ID");
            return Ok(id);
        }
    }

    let id = generate_receiver_id();
    if let Err(e) = db.save_receiver_id(&id) {
        warn!(error = %e, "failed to persist auto-generated receiver ID; ID will not survive restart");
    }
    info!(receiver_id = %id, "auto-generated receiver ID");
    Ok(id)
}

pub fn profile_has_connect_credentials(profile: Option<&crate::db::Profile>) -> bool {
    profile.is_some_and(|profile| {
        !profile.server_url.trim().is_empty() && !profile.token.trim().is_empty()
    })
}

/// Resolve the effective server URL+token for the P2P runtime.
///
/// The stored `profile` is the source of truth; an `override_` pair (env vars
/// for the desktop app, CLI flags for headless) takes precedence when both its
/// URL and token are present. Each source contributes only when *both* of its
/// values are non-empty (after trimming); a partial source is ignored. Returns
/// `None` when neither source is fully configured.
pub fn resolve_server_config(
    profile: Option<&crate::db::Profile>,
    override_: (Option<String>, Option<String>),
) -> Option<crate::p2p_runtime::ServerClientConfig> {
    let non_empty = |s: String| {
        let t = s.trim().to_owned();
        if t.is_empty() { None } else { Some(t) }
    };
    // Override (env/CLI) wins when fully specified.
    if let (Some(url), Some(token)) = (
        override_.0.and_then(non_empty),
        override_.1.and_then(non_empty),
    ) {
        return Some(crate::p2p_runtime::ServerClientConfig { url, token });
    }
    // Otherwise fall back to a fully-configured profile.
    profile.and_then(|p| {
        let url = non_empty(p.server_url.clone())?;
        let token = non_empty(p.token.clone())?;
        Some(crate::p2p_runtime::ServerClientConfig { url, token })
    })
}

/// The default receiver data directory (OS app-local data dir, namespaced).
/// Shared by [`init`] and callers that need to derive sibling paths (e.g. the
/// persistent P2P secret-key file) from the same location.
pub fn default_data_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rusty-timer")
        .join("receiver")
}

pub async fn init(
    receiver_id: Option<String>,
) -> Result<(Arc<AppState>, watch::Receiver<ShutdownSignal>), String> {
    init_with_data_dir(receiver_id, default_data_dir()).await
}

pub async fn init_with_data_dir(
    receiver_id: Option<String>,
    data_dir: impl AsRef<Path>,
) -> Result<(Arc<AppState>, watch::Receiver<ShutdownSignal>), String> {
    let data_dir = data_dir.as_ref();
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("could not create data directory: {e}"))?;

    let db_path = data_dir.join("receiver.sqlite3");
    let db = Db::open(&db_path).map_err(|e| format!("failed to open DB: {e}"))?;
    let db_integrity_ok = db
        .integrity_check()
        .map(|()| true)
        .map_err(|e| format!("integrity_check failed: {e}"))?;

    let receiver_id = resolve_receiver_id(receiver_id, &db)?;
    info!(receiver_id = %receiver_id, "resolved receiver ID");

    // Dedicated writer thread for hot-path persistence (group commit, one
    // fsync per commit window). It owns its own connection to the same DB
    // file; `state.db` remains the cold control-plane connection. The thread
    // exits when the last WriterHandle drops (process shutdown).
    let (writer, _writer_thread) =
        crate::writer::spawn_writer(&db_path, crate::writer::WriterConfig::from_env())
            .map_err(|e| format!("failed to start sqlite writer: {e}"))?;

    let (state, shutdown_rx) = AppState::with_integrity(db, receiver_id, db_integrity_ok, writer);
    state.logger.log("Receiver started");

    // Populate the chip->participant lookup from any previously imported
    // participant/chip data so the announcer can resolve bib/name immediately.
    if let Err(e) = crate::control_api::reload_chip_lookup(&state).await {
        warn!(error = %e, "failed to load chip lookup at startup");
    }

    Ok((state, shutdown_rx))
}

/// Run local receiver housekeeping until shutdown.
///
/// The central relay session was removed at P2P cutover. Durable
/// event delivery, local TCP proxy replay, DBF writes, and server announcer
/// pushes are driven by `p2p_runtime` when a P2P config is present.
pub async fn run(state: Arc<AppState>, mut shutdown_rx: watch::Receiver<ShutdownSignal>) {
    // Race Director DBF import poller (spec §5 step B). Config-gated at runtime
    // (it no-ops while disabled) and observes the same shutdown signal.
    let rd_poller = tokio::spawn(crate::rd_poll::run(Arc::clone(&state), shutdown_rx.clone()));

    loop {
        if !matches!(*shutdown_rx.borrow(), ShutdownSignal::None) {
            break;
        }
        if shutdown_rx.changed().await.is_err() {
            break;
        }
    }

    rd_poller.abort();
    state.clear_stream_metrics_cache().await;
    state
        .set_connection_state(ConnectionState::Disconnected)
        .await;
    state.logger.log("Receiver stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Profile;

    fn profile(url: &str, token: &str) -> Profile {
        Profile {
            server_url: url.to_owned(),
            token: token.to_owned(),
            update_mode: String::new(),
            receiver_id: None,
        }
    }

    #[test]
    fn resolve_server_config_env_overrides_profile() {
        let p = profile("http://profile", "pt");
        // Override (env/CLI) both set -> override wins.
        let cfg = resolve_server_config(
            Some(&p),
            (Some("http://env".to_owned()), Some("et".to_owned())),
        )
        .expect("resolved");
        assert_eq!(cfg.url, "http://env");
        assert_eq!(cfg.token, "et");

        // Override absent -> profile is used.
        let cfg = resolve_server_config(Some(&p), (None, None)).expect("resolved");
        assert_eq!(cfg.url, "http://profile");
        assert_eq!(cfg.token, "pt");

        // Neither source -> None.
        assert!(resolve_server_config(None, (None, None)).is_none());

        // Partial override (url only), no profile -> None.
        assert!(resolve_server_config(None, (Some("http://x".to_owned()), None)).is_none());

        // Partial override falls back to a full profile.
        let cfg =
            resolve_server_config(Some(&p), (Some("http://x".to_owned()), None)).expect("resolved");
        assert_eq!(cfg.url, "http://profile");

        // Whitespace-only values are treated as absent.
        let blank = profile("  ", "  ");
        assert!(resolve_server_config(Some(&blank), (None, None)).is_none());
    }
}

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

pub async fn init(
    receiver_id: Option<String>,
) -> Result<(Arc<AppState>, watch::Receiver<ShutdownSignal>), String> {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rusty-timer")
        .join("receiver");
    init_with_data_dir(receiver_id, data_dir).await
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

    let (state, shutdown_rx) = AppState::with_integrity(db, receiver_id, db_integrity_ok);
    state.logger.log("Receiver started");

    Ok((state, shutdown_rx))
}

/// Run local receiver housekeeping until shutdown.
///
/// The central relay session was removed at P2P cutover. Durable
/// event delivery, local TCP proxy replay, DBF writes, and thin-node announcer
/// pushes are driven by `p2p_runtime` when a P2P config is present.
pub async fn run(state: Arc<AppState>, mut shutdown_rx: watch::Receiver<ShutdownSignal>) {
    {
        let db = state.db.lock().await;
        if let Ok(Some(profile)) = db.load_profile() {
            *state.upstream_url.write().await = Some(profile.server_url);
        }
    }

    loop {
        if !matches!(*shutdown_rx.borrow(), ShutdownSignal::None) {
            break;
        }
        if shutdown_rx.changed().await.is_err() {
            break;
        }
    }

    state.clear_stream_metrics_cache().await;
    state
        .set_connection_state(ConnectionState::Disconnected)
        .await;
    state.logger.log("Receiver stopped");
}

use crate::control_api::{self, AppState};
use crate::p2p_runtime::{P2pReceiverConfig, P2pReceiverRuntime, start_receiver_p2p};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct HeadlessConfig {
    pub data_dir: PathBuf,
    pub bind_addr: SocketAddr,
    pub receiver_id: Option<String>,
    /// Optional P2P receiver runtime. When present, the headless host runs the
    /// real loopback P2P lane. When `None`, only the local control API starts.
    pub p2p: Option<P2pReceiverConfig>,
}

pub struct HeadlessHost {
    data_dir: PathBuf,
    local_addr: SocketAddr,
    state: Arc<AppState>,
    server_shutdown_tx: Option<oneshot::Sender<()>>,
    server_task: JoinHandle<Result<(), String>>,
    runtime_task: JoinHandle<()>,
    p2p_runtime: Option<P2pReceiverRuntime>,
}

impl HeadlessHost {
    pub async fn start(config: HeadlessConfig) -> Result<Self, String> {
        if !config.bind_addr.ip().is_loopback() {
            return Err(format!(
                "bind_addr must be a loopback address, got {}",
                config.bind_addr
            ));
        }
        let (state, shutdown_rx) =
            crate::runtime::init_with_data_dir(config.receiver_id, &config.data_dir).await?;
        let listener = TcpListener::bind(config.bind_addr)
            .await
            .map_err(|e| format!("failed to bind control API: {e}"))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| format!("failed to read control API address: {e}"))?;
        let router = control_router(Arc::clone(&state));
        let (server_shutdown_tx, server_shutdown_rx) = oneshot::channel();

        // Start the fallible P2P lane *before* spawning the long-lived control
        // server and legacy runtime tasks. If P2P startup fails, returning here
        // drops the (still-unmoved) listener and channels without leaving a
        // bound control server or orphaned tasks running; the bind address is
        // released promptly so a caller can retry.
        let p2p_runtime = match config.p2p {
            Some(p2p_config) => Some(start_receiver_p2p(Arc::clone(&state), p2p_config).await?),
            None => None,
        };

        let server_task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = server_shutdown_rx.await;
                })
                .await
                .map_err(|e| format!("control API server failed: {e}"))
        });
        let runtime_task = tokio::spawn(crate::runtime::run(Arc::clone(&state), shutdown_rx));

        Ok(Self {
            data_dir: config.data_dir,
            local_addr,
            state,
            server_shutdown_tx: Some(server_shutdown_tx),
            server_task,
            runtime_task,
            p2p_runtime,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub async fn shutdown(mut self) -> Result<(), String> {
        self.state.request_process_shutdown();
        if let Some(p2p_runtime) = self.p2p_runtime.take() {
            p2p_runtime.shutdown().await;
        }
        if let Some(tx) = self.server_shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Await both tasks so neither JoinHandle is dropped without being
        // driven to completion, then surface any error. The runtime task is
        // awaited even if the server task failed, preserving clean shutdown.
        let server_result = self
            .server_task
            .await
            .map_err(|e| format!("control API server task failed: {e}"))
            .and_then(|inner| inner);
        let runtime_result = self
            .runtime_task
            .await
            .map_err(|e| format!("receiver runtime task failed: {e}"));
        server_result?;
        runtime_result?;
        Ok(())
    }
}

pub(crate) fn control_router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        .route("/api/v1/status", get(get_status))
        .with_state(Arc::clone(&state));
    install_test_bridge_routes(router, state)
}

async fn get_status(State(state): State<Arc<AppState>>) -> Json<control_api::StatusResponse> {
    Json(control_api::get_status(state.as_ref()).await)
}

/// Mount the headless test bridge (`/bridge/*`) when the `test-bridge` feature
/// is enabled. The bridge is a loopback-only agent surface compiled out of
/// release/default builds entirely.
#[cfg(feature = "test-bridge")]
fn install_test_bridge_routes(router: Router, state: Arc<AppState>) -> Router {
    router.merge(crate::control_bridge::router(state))
}

#[cfg(not(feature = "test-bridge"))]
fn install_test_bridge_routes(router: Router, _state: Arc<AppState>) -> Router {
    router
}

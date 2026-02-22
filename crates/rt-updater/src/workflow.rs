use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::{UpdateChecker, UpdateMode, UpdateStatus};

/// Async checker abstraction used by update workflow state transitions.
pub trait Checker: Send + Sync {
    fn check<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<UpdateStatus, String>> + Send + 'a>>;

    fn download<'a>(
        &'a self,
        version: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send + 'a>>;
}

/// Adapter for using the real `UpdateChecker` in the shared workflow.
pub struct RealChecker {
    inner: UpdateChecker,
}

impl RealChecker {
    pub fn new(inner: UpdateChecker) -> Self {
        Self { inner }
    }
}

impl Checker for RealChecker {
    fn check<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<UpdateStatus, String>> + Send + 'a>> {
        Box::pin(async move { self.inner.check().await.map_err(|e| e.to_string()) })
    }

    fn download<'a>(
        &'a self,
        version: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send + 'a>> {
        Box::pin(async move {
            self.inner
                .download(version)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

/// Service-specific adapter for status/path persistence and UI emission.
pub trait WorkflowState: Send + Sync {
    fn current_status<'a>(&'a self) -> Pin<Box<dyn Future<Output = UpdateStatus> + Send + 'a>>;

    fn set_status<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    fn set_downloaded<'a>(
        &'a self,
        status: UpdateStatus,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    fn emit_status_changed<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Run an update check and apply mode-specific transitions.
pub async fn run_check(
    state: &dyn WorkflowState,
    checker: &dyn Checker,
    update_mode: UpdateMode,
) -> UpdateStatus {
    match checker.check().await {
        Ok(UpdateStatus::Available { version }) => {
            state
                .set_status(UpdateStatus::Available {
                    version: version.clone(),
                })
                .await;

            if update_mode == UpdateMode::CheckAndDownload {
                match checker.download(&version).await {
                    Ok(path) => {
                        let status = UpdateStatus::Downloaded {
                            version: version.clone(),
                        };
                        state.set_downloaded(status.clone(), path).await;
                        state.emit_status_changed(status.clone()).await;
                        status
                    }
                    Err(error) => {
                        let status = UpdateStatus::Failed { error };
                        state.set_status(status.clone()).await;
                        state.emit_status_changed(status.clone()).await;
                        status
                    }
                }
            } else {
                let status = UpdateStatus::Available { version };
                state.emit_status_changed(status.clone()).await;
                status
            }
        }
        Ok(status) => {
            state.set_status(status.clone()).await;
            state.emit_status_changed(status.clone()).await;
            status
        }
        Err(error) => {
            let status = UpdateStatus::Failed { error };
            state.set_status(status.clone()).await;
            state.emit_status_changed(status.clone()).await;
            status
        }
    }
}

/// Run a manual update download based on current status.
pub async fn run_download(
    state: &dyn WorkflowState,
    checker: &dyn Checker,
) -> Result<UpdateStatus, UpdateStatus> {
    let current = state.current_status().await;
    match current {
        UpdateStatus::Available { ref version } => match checker.download(version).await {
            Ok(path) => {
                let status = UpdateStatus::Downloaded {
                    version: version.clone(),
                };
                state.set_downloaded(status.clone(), path).await;
                state.emit_status_changed(status.clone()).await;
                Ok(status)
            }
            Err(error) => {
                let status = UpdateStatus::Failed { error };
                state.set_status(status.clone()).await;
                state.emit_status_changed(status.clone()).await;
                Err(status)
            }
        },
        s @ UpdateStatus::Downloaded { .. } => Ok(s),
        other => Err(other),
    }
}

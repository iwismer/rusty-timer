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
    stage_root_override: Option<PathBuf>,
}

impl RealChecker {
    pub fn new(inner: UpdateChecker) -> Self {
        Self {
            inner,
            stage_root_override: None,
        }
    }

    pub fn with_stage_root(inner: UpdateChecker, stage_root: PathBuf) -> Self {
        Self {
            inner,
            stage_root_override: Some(stage_root),
        }
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
            if let Some(stage_root) = self.stage_root_override.as_deref() {
                self.inner
                    .download_with_stage_root(version, stage_root)
                    .await
                    .map_err(|e| e.to_string())
            } else {
                self.inner
                    .download(version)
                    .await
                    .map_err(|e| e.to_string())
            }
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

async fn set_and_emit(state: &dyn WorkflowState, status: UpdateStatus) {
    state.set_status(status.clone()).await;
    state.emit_status_changed(status).await;
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
                        state.emit_status_changed(status).await;
                        UpdateStatus::Downloaded { version }
                    }
                    Err(error) => {
                        let status = UpdateStatus::Failed { error };
                        set_and_emit(state, status.clone()).await;
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
            set_and_emit(state, status.clone()).await;
            status
        }
        Err(error) => {
            let status = UpdateStatus::Failed { error };
            set_and_emit(state, status.clone()).await;
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
                state.emit_status_changed(status).await;
                Ok(UpdateStatus::Downloaded {
                    version: version.clone(),
                })
            }
            Err(error) => {
                let status = UpdateStatus::Failed { error };
                set_and_emit(state, status.clone()).await;
                Err(status)
            }
        },
        s @ UpdateStatus::Downloaded { .. } => Ok(s),
        other => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockChecker {
        check_result: Result<UpdateStatus, String>,
        download_result: Result<PathBuf, String>,
    }

    impl Checker for MockChecker {
        fn check<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<UpdateStatus, String>> + Send + 'a>> {
            Box::pin(async { self.check_result.clone() })
        }

        fn download<'a>(
            &'a self,
            _version: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send + 'a>> {
            Box::pin(async { self.download_result.clone() })
        }
    }

    struct MockState {
        status: Mutex<UpdateStatus>,
        downloaded_path: Mutex<Option<PathBuf>>,
        emitted: Mutex<Vec<UpdateStatus>>,
    }

    impl MockState {
        fn new() -> Self {
            Self {
                status: Mutex::new(UpdateStatus::UpToDate),
                downloaded_path: Mutex::new(None),
                emitted: Mutex::new(Vec::new()),
            }
        }
    }

    impl WorkflowState for MockState {
        fn current_status<'a>(&'a self) -> Pin<Box<dyn Future<Output = UpdateStatus> + Send + 'a>> {
            Box::pin(async { self.status.lock().unwrap().clone() })
        }

        fn set_status<'a>(
            &'a self,
            status: UpdateStatus,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                *self.status.lock().unwrap() = status;
            })
        }

        fn set_downloaded<'a>(
            &'a self,
            status: UpdateStatus,
            path: PathBuf,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                *self.status.lock().unwrap() = status;
                *self.downloaded_path.lock().unwrap() = Some(path);
            })
        }

        fn emit_status_changed<'a>(
            &'a self,
            status: UpdateStatus,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                self.emitted.lock().unwrap().push(status);
            })
        }
    }

    #[tokio::test]
    async fn check_only_available() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.0".into(),
            }),
            download_result: Ok(PathBuf::from("/tmp/staged")),
        };
        let state = MockState::new();
        let result = run_check(&state, &checker, UpdateMode::CheckOnly).await;
        assert_eq!(
            result,
            UpdateStatus::Available {
                version: "1.2.0".into()
            }
        );
        assert!(state.downloaded_path.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn check_and_download_available() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.0".into(),
            }),
            download_result: Ok(PathBuf::from("/tmp/staged")),
        };
        let state = MockState::new();
        let result = run_check(&state, &checker, UpdateMode::CheckAndDownload).await;
        assert_eq!(
            result,
            UpdateStatus::Downloaded {
                version: "1.2.0".into()
            }
        );
        assert_eq!(
            *state.downloaded_path.lock().unwrap(),
            Some(PathBuf::from("/tmp/staged"))
        );
    }

    #[tokio::test]
    async fn check_up_to_date() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(PathBuf::from("/tmp/staged")),
        };
        let state = MockState::new();
        let result = run_check(&state, &checker, UpdateMode::CheckAndDownload).await;
        assert_eq!(result, UpdateStatus::UpToDate);
    }

    #[tokio::test]
    async fn check_failure() {
        let checker = MockChecker {
            check_result: Err("network error".into()),
            download_result: Ok(PathBuf::from("/tmp/staged")),
        };
        let state = MockState::new();
        let result = run_check(&state, &checker, UpdateMode::CheckAndDownload).await;
        assert_eq!(
            result,
            UpdateStatus::Failed {
                error: "network error".into()
            }
        );
    }

    #[tokio::test]
    async fn download_failure() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.0".into(),
            }),
            download_result: Err("checksum mismatch".into()),
        };
        let state = MockState::new();
        let result = run_check(&state, &checker, UpdateMode::CheckAndDownload).await;
        assert_eq!(
            result,
            UpdateStatus::Failed {
                error: "checksum mismatch".into()
            }
        );
    }

    #[tokio::test]
    async fn run_download_when_available() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(PathBuf::from("/tmp/staged")),
        };
        let state = MockState::new();
        *state.status.lock().unwrap() = UpdateStatus::Available {
            version: "2.0.0".into(),
        };
        let result = run_download(&state, &checker).await;
        assert_eq!(
            result,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".into()
            })
        );
    }

    #[tokio::test]
    async fn run_download_when_already_downloaded() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(PathBuf::from("/tmp/staged")),
        };
        let state = MockState::new();
        *state.status.lock().unwrap() = UpdateStatus::Downloaded {
            version: "2.0.0".into(),
        };
        let result = run_download(&state, &checker).await;
        assert_eq!(
            result,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".into()
            })
        );
    }

    #[tokio::test]
    async fn run_download_when_up_to_date_returns_err() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(PathBuf::from("/tmp/staged")),
        };
        let state = MockState::new();
        let result = run_download(&state, &checker).await;
        assert_eq!(result, Err(UpdateStatus::UpToDate));
    }

    #[tokio::test]
    async fn run_download_failure() {
        let checker = MockChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Err("disk full".into()),
        };
        let state = MockState::new();
        *state.status.lock().unwrap() = UpdateStatus::Available {
            version: "2.0.0".into(),
        };
        let result = run_download(&state, &checker).await;
        assert_eq!(
            result,
            Err(UpdateStatus::Failed {
                error: "disk full".into()
            })
        );
    }
}

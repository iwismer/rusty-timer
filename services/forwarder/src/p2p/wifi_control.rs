//! P2P adapter for forwarder Wi-Fi control operations.

use std::sync::Arc;

use rt_iroh::EndpointId;
use rt_p2p_protocol::{WifiControlRequest, WifiControlResponse};

use crate::wifi::WifiService;

use super::control::{WIFI_CONTROL_DISABLED, WifiControlFuture, WifiControlHandler};

#[derive(Clone)]
pub struct ForwarderWifiControlHandler {
    allow_wifi_control: bool,
    service: WifiService,
}

impl std::fmt::Debug for ForwarderWifiControlHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwarderWifiControlHandler")
            .field("allow_wifi_control", &self.allow_wifi_control)
            .finish_non_exhaustive()
    }
}

impl ForwarderWifiControlHandler {
    #[must_use]
    pub fn new(allow_wifi_control: bool, service: WifiService) -> Self {
        Self {
            allow_wifi_control,
            service,
        }
    }
}

impl WifiControlHandler for ForwarderWifiControlHandler {
    fn allow_wifi_control(&self) -> bool {
        self.allow_wifi_control
    }

    fn handle(&self, request: WifiControlRequest, peer: EndpointId) -> WifiControlFuture<'_> {
        Box::pin(async move {
            // When `WifiService` is backed by a live `ConfigState`, it checks
            // `read_allow_wifi_control` dynamically on every call so runtime
            // config updates take effect immediately. Fall back to the static
            // flag only when no `ConfigState` is attached.
            if !self.service.has_config_state() && !self.allow_wifi_control {
                return error_response(request.request_id, WIFI_CONTROL_DISABLED.to_owned());
            }

            let actor = format!("receiver {peer}");
            match request.command.as_str() {
                "status" => match self.service.status().await {
                    Ok(status) => match serde_json::to_string(&status) {
                        Ok(payload_json) => WifiControlResponse {
                            request_id: request.request_id,
                            ok: true,
                            message: "ok".to_owned(),
                            payload_json: Some(payload_json),
                        },
                        Err(e) => error_response(
                            request.request_id,
                            format!("failed to serialize wifi status: {e}"),
                        ),
                    },
                    Err((_, body)) => service_error_response(request.request_id, &body),
                },
                "scan" => {
                    let rescan = request.rescan.unwrap_or(true);
                    match self.service.scan(rescan, &actor).await {
                        Ok(scan_result) => match serde_json::to_string(&scan_result) {
                            Ok(payload_json) => WifiControlResponse {
                                request_id: request.request_id,
                                ok: true,
                                message: "ok".to_owned(),
                                payload_json: Some(payload_json),
                            },
                            Err(e) => error_response(
                                request.request_id,
                                format!("failed to serialize wifi scan result: {e}"),
                            ),
                        },
                        Err((_, body)) => service_error_response(request.request_id, &body),
                    }
                }
                "connect" => {
                    let Some(ssid) = request.ssid.as_deref() else {
                        return error_response(
                            request.request_id,
                            "connect requires ssid".to_owned(),
                        );
                    };
                    let password = request.password.as_deref();
                    let hidden = request.hidden.unwrap_or(false);
                    match self.service.connect(ssid, password, hidden, &actor).await {
                        Ok(action) => action_response(request.request_id, action),
                        Err((_, body)) => service_error_response(request.request_id, &body),
                    }
                }
                "disconnect" => match self.service.disconnect(&actor).await {
                    Ok(action) => action_response(request.request_id, action),
                    Err((_, body)) => service_error_response(request.request_id, &body),
                },
                "forget" => {
                    let Some(ssid) = request.ssid.as_deref() else {
                        return error_response(
                            request.request_id,
                            "forget requires ssid".to_owned(),
                        );
                    };
                    match self.service.forget(ssid, &actor).await {
                        Ok(action) => action_response(request.request_id, action),
                        Err((_, body)) => service_error_response(request.request_id, &body),
                    }
                }
                other => error_response(
                    request.request_id,
                    format!("unsupported wifi control command: {other}"),
                ),
            }
        })
    }
}

fn action_response(request_id: String, action: rt_domain::WifiActionResult) -> WifiControlResponse {
    let payload_json = action
        .status
        .as_ref()
        .and_then(|status| serde_json::to_string(status).ok());
    WifiControlResponse {
        request_id,
        ok: action.ok,
        message: action.message,
        payload_json,
    }
}

fn service_error_response(request_id: String, json_body: &str) -> WifiControlResponse {
    let message = serde_json::from_str::<serde_json::Value>(json_body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .or_else(|| v.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| json_body.to_owned());
    error_response(request_id, message)
}

fn error_response(request_id: String, message: String) -> WifiControlResponse {
    WifiControlResponse {
        request_id,
        ok: false,
        message,
        payload_json: None,
    }
}

impl<W: WifiControlHandler + ?Sized> WifiControlHandler for Arc<W> {
    fn allow_wifi_control(&self) -> bool {
        (**self).allow_wifi_control()
    }

    fn handle(&self, request: WifiControlRequest, peer: EndpointId) -> WifiControlFuture<'_> {
        (**self).handle(request, peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_events::ForwarderUiEvent;
    use crate::wifi::{NmcliWifiBackend, WifiBackend, WifiFuture, WifiService};
    use rt_domain::{SavedWifiNetwork, WifiActionResult, WifiNetwork, WifiScanResult, WifiStatus};
    use std::path::PathBuf;
    use tokio::sync::broadcast;

    #[derive(Debug)]
    struct FakeBackend;

    impl WifiBackend for FakeBackend {
        fn status(&self) -> WifiFuture<'_, WifiStatus> {
            Box::pin(async {
                Ok(WifiStatus {
                    available: true,
                    enabled: true,
                    interface: Some("wlan0".to_owned()),
                    state: "connected".to_owned(),
                    ssid: Some("RaceTiming".to_owned()),
                    ip_address: Some("192.168.1.42".to_owned()),
                    gateway: Some("192.168.1.1".to_owned()),
                    signal: Some(82),
                    security: Some("WPA2".to_owned()),
                    band: Some("5 GHz".to_owned()),
                    saved_networks: vec![SavedWifiNetwork {
                        ssid: "RaceTiming".to_owned(),
                        uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
                        autoconnect: true,
                    }],
                })
            })
        }

        fn scan(&self, _rescan: bool) -> WifiFuture<'_, WifiScanResult> {
            Box::pin(async {
                let status = self.status().await?;
                Ok(WifiScanResult {
                    status,
                    networks: vec![WifiNetwork {
                        ssid: "RaceTiming".to_owned(),
                        in_use: true,
                        signal: 82,
                        security: "WPA2".to_owned(),
                        bands: vec!["5 GHz".to_owned()],
                        saved: true,
                    }],
                })
            })
        }

        fn connect<'a>(
            &'a self,
            ssid: &'a str,
            _password: Option<&'a str>,
            _hidden: bool,
        ) -> WifiFuture<'a, WifiActionResult> {
            Box::pin(async move {
                let status = self.status().await.ok();
                Ok(WifiActionResult {
                    ok: true,
                    message: format!("Connected to {ssid}"),
                    status,
                })
            })
        }

        fn disconnect(&self) -> WifiFuture<'_, WifiActionResult> {
            Box::pin(async {
                let status = self.status().await.ok();
                Ok(WifiActionResult {
                    ok: true,
                    message: "Disconnected from wlan0".to_owned(),
                    status,
                })
            })
        }

        fn forget<'a>(&'a self, ssid: &'a str) -> WifiFuture<'a, WifiActionResult> {
            Box::pin(async move {
                let status = self.status().await.ok();
                Ok(WifiActionResult {
                    ok: true,
                    message: format!("Forgot network {ssid}"),
                    status,
                })
            })
        }
    }

    fn test_peer() -> EndpointId {
        rt_iroh::SecretKey::from_bytes(&[7u8; 32]).public()
    }

    fn make_handler(
        allow: bool,
    ) -> (
        ForwarderWifiControlHandler,
        Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let token_path = dir.path().join("token");
        std::fs::write(&token_path, "test-token\n").expect("write token");
        let config_path: PathBuf = dir.path().join("forwarder.toml");
        let toml = format!(
            "schema_version = 1\n\n[auth]\ntoken_file = '{}'\n\n[[readers]]\ntarget = \"192.168.1.100\"\n\n[control]\nallow_wifi_control = {allow}\n",
            token_path.display()
        );
        std::fs::write(&config_path, toml).expect("write config");
        let config_state = Arc::new(crate::config_service::ConfigState::new(config_path));

        let (ui_tx, _rx) = broadcast::channel(16);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            ui_tx,
            |entry| ForwarderUiEvent::LogEntry { entry },
            32,
        ));
        let service = WifiService::new(
            Arc::new(FakeBackend),
            Some(config_state),
            Arc::clone(&logger),
        );
        (
            ForwarderWifiControlHandler::new(allow, service),
            logger,
            dir,
        )
    }

    #[tokio::test]
    async fn status_returns_serialized_wifi_status() {
        let (handler, _logger, _dir) = make_handler(true);
        let response = handler
            .handle(
                WifiControlRequest {
                    request_id: "req-status".to_owned(),
                    command: "status".to_owned(),
                    rescan: None,
                    ssid: None,
                    password: None,
                    hidden: None,
                },
                test_peer(),
            )
            .await;

        assert!(response.ok);
        let status: WifiStatus =
            serde_json::from_str(response.payload_json.as_deref().expect("payload")).unwrap();
        assert_eq!(status.ssid.as_deref(), Some("RaceTiming"));
    }

    #[tokio::test]
    async fn connect_attributes_action_to_peer_in_ui_log() {
        let (handler, logger, _dir) = make_handler(true);
        let peer = test_peer();
        let response = handler
            .handle(
                WifiControlRequest {
                    request_id: "req-connect".to_owned(),
                    command: "connect".to_owned(),
                    rescan: None,
                    ssid: Some("RaceTiming".to_owned()),
                    password: Some("secret123".to_owned()),
                    hidden: Some(false),
                },
                peer,
            )
            .await;

        assert!(response.ok);
        let entries = logger.entries();
        let peer_str = peer.to_string();
        assert!(
            entries.iter().any(|e| e.contains(&peer_str)),
            "expected UI log to mention peer {peer_str}, got {entries:?}"
        );
    }

    #[tokio::test]
    async fn disabled_handler_rejects_requests() {
        let (handler, _logger, _dir) = make_handler(false);
        let response = handler
            .handle(
                WifiControlRequest {
                    request_id: "req-disabled".to_owned(),
                    command: "status".to_owned(),
                    rescan: None,
                    ssid: None,
                    password: None,
                    hidden: None,
                },
                test_peer(),
            )
            .await;

        assert!(!response.ok);
        assert_eq!(response.message, WIFI_CONTROL_DISABLED);
        let _ = NmcliWifiBackend::default();
    }
}

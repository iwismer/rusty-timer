//! Wi-Fi management service backed by NetworkManager (`nmcli`).
//!
//! Provides scan, status, connect, disconnect, and forget operations scoped
//! strictly to `wifi` (`802-11-wireless`) interfaces and connection profiles.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use rt_domain::{SavedWifiNetwork, WifiActionResult, WifiNetwork, WifiScanResult, WifiStatus};
use tokio::sync::Mutex;

use crate::config_service::{ConfigState, read_allow_wifi_control};
use crate::ui_events::ForwarderUiEvent;

/// Output of a single `nmcli` command invocation.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub not_found: bool,
}

pub type CmdFuture<'a> = Pin<Box<dyn Future<Output = Result<CommandOutput, String>> + Send + 'a>>;

/// Low-level runner for `nmcli` invocations, mockable in unit tests.
pub trait NmcliRunner: std::fmt::Debug + Send + Sync + 'static {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> CmdFuture<'a>;
}

/// Production `nmcli` process runner.
#[derive(Debug, Default)]
pub struct SystemNmcliRunner;

impl NmcliRunner for SystemNmcliRunner {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> CmdFuture<'a> {
        Box::pin(async move {
            let mut cmd = tokio::process::Command::new("nmcli");
            cmd.args(args).env("LC_ALL", "C").kill_on_drop(true);
            match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
                Ok(Ok(output)) => Ok(CommandOutput {
                    success: output.status.success(),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    not_found: false,
                }),
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(CommandOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: "nmcli binary not found".to_owned(),
                    not_found: true,
                }),
                Ok(Err(e)) => Err(format!("failed to execute nmcli: {e}")),
                Err(_) => Err("nmcli command timed out after 30s".to_owned()),
            }
        })
    }
}

pub fn validate_ssid(ssid: &str) -> Result<&str, String> {
    let trimmed = ssid.trim();
    if trimmed.is_empty() {
        return Err("SSID is required".to_owned());
    }
    if trimmed.len() > 32 {
        return Err("SSID must be 32 bytes or fewer".to_owned());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("SSID must not contain control characters".to_owned());
    }
    if trimmed.starts_with('-') {
        return Err("SSID must not start with '-'".to_owned());
    }
    Ok(trimmed)
}

pub type WifiFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Transport-independent Wi-Fi backend abstraction.
pub trait WifiBackend: std::fmt::Debug + Send + Sync + 'static {
    fn status(&self) -> WifiFuture<'_, WifiStatus>;
    fn scan(&self, rescan: bool) -> WifiFuture<'_, WifiScanResult>;
    fn connect<'a>(
        &'a self,
        ssid: &'a str,
        password: Option<&'a str>,
        hidden: bool,
    ) -> WifiFuture<'a, WifiActionResult>;
    fn disconnect(&self) -> WifiFuture<'_, WifiActionResult>;
    fn forget<'a>(&'a self, ssid: &'a str) -> WifiFuture<'a, WifiActionResult>;
}

/// `nmcli`-backed [`WifiBackend`] implementation.
#[derive(Debug)]
pub struct NmcliWifiBackend<R: NmcliRunner = SystemNmcliRunner> {
    runner: R,
}

impl NmcliWifiBackend<SystemNmcliRunner> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runner: SystemNmcliRunner,
        }
    }
}

impl Default for NmcliWifiBackend<SystemNmcliRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: NmcliRunner> NmcliWifiBackend<R> {
    #[must_use]
    pub fn with_runner(runner: R) -> Self {
        Self { runner }
    }

    async fn discover_wifi_device(
        &self,
    ) -> Result<Option<(String, String, Option<String>)>, String> {
        let out = self
            .runner
            .run(&[
                "-t",
                "-f",
                "DEVICE,TYPE,STATE,CONNECTION",
                "device",
                "status",
            ])
            .await?;
        if out.not_found || !out.success {
            return Ok(None);
        }
        Ok(parse_wifi_device_status(&out.stdout))
    }

    async fn read_radio_enabled(&self) -> Result<bool, String> {
        let out = self.runner.run(&["-t", "-f", "WIFI", "general"]).await?;
        if out.not_found || !out.success {
            return Ok(false);
        }
        Ok(out.stdout.trim().eq_ignore_ascii_case("enabled"))
    }

    async fn list_saved_wifi_networks(&self) -> Result<Vec<SavedWifiNetwork>, String> {
        let out = self
            .runner
            .run(&[
                "-t",
                "-f",
                "NAME,UUID,TYPE,AUTOCONNECT",
                "connection",
                "show",
            ])
            .await?;
        if out.not_found || !out.success {
            return Ok(Vec::new());
        }
        Ok(parse_saved_wifi_connections(&out.stdout))
    }

    async fn read_device_ip_and_gateway(
        &self,
        iface: &str,
    ) -> Result<(Option<String>, Option<String>), String> {
        let out = self
            .runner
            .run(&[
                "-t",
                "-f",
                "IP4.ADDRESS,IP4.GATEWAY",
                "device",
                "show",
                iface,
            ])
            .await?;
        if out.not_found || !out.success {
            return Ok((None, None));
        }
        Ok(parse_device_ip_and_gateway(&out.stdout))
    }

    async fn fetch_raw_wifi_list(&self, iface: &str, rescan_arg: &str) -> Result<String, String> {
        let out = self
            .runner
            .run(&[
                "-t",
                "-f",
                "IN-USE,SSID,SIGNAL,FREQ,SECURITY",
                "device",
                "wifi",
                "list",
                "ifname",
                iface,
                "--rescan",
                rescan_arg,
            ])
            .await?;
        if out.success {
            return Ok(out.stdout);
        }
        // NetworkManager/wpa_supplicant frequently rate-limits consecutive
        // explicit rescans within a few seconds; fall back to cached results
        // rather than surfacing a hard error.
        if rescan_arg == "yes" {
            let fallback = self
                .runner
                .run(&[
                    "-t",
                    "-f",
                    "IN-USE,SSID,SIGNAL,FREQ,SECURITY",
                    "device",
                    "wifi",
                    "list",
                    "ifname",
                    iface,
                    "--rescan",
                    "no",
                ])
                .await?;
            if fallback.success {
                return Ok(fallback.stdout);
            }
        }
        Err(command_error_detail(&out))
    }

    async fn build_status_and_networks(
        &self,
        rescan_arg: &str,
    ) -> Result<(WifiStatus, Vec<WifiNetwork>), String> {
        let Some((iface, raw_state, active_conn)) = self.discover_wifi_device().await? else {
            return Ok((WifiStatus::unavailable(), Vec::new()));
        };

        let enabled = self.read_radio_enabled().await.unwrap_or(true);
        let saved_networks = self.list_saved_wifi_networks().await.unwrap_or_default();
        let saved_ssids: HashSet<String> = saved_networks.iter().map(|s| s.ssid.clone()).collect();

        let state = normalize_device_state(&raw_state);
        let (ip_address, gateway) = if state == "connected" || state == "connecting" {
            self.read_device_ip_and_gateway(&iface)
                .await
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let raw_list = if enabled {
            self.fetch_raw_wifi_list(&iface, rescan_arg)
                .await
                .unwrap_or_default()
        } else {
            String::new()
        };

        let active_info = parse_active_bssid_info(&raw_list);
        let networks = parse_wifi_scan_list(&raw_list, &saved_ssids);

        let ssid = if state == "connected" || state == "connecting" {
            active_info.as_ref().map(|a| a.ssid.clone()).or(active_conn)
        } else {
            None
        };

        let status = WifiStatus {
            available: true,
            enabled,
            interface: Some(iface),
            state,
            ssid,
            ip_address,
            gateway,
            signal: active_info.as_ref().map(|a| a.signal),
            security: active_info.as_ref().map(|a| a.security.clone()),
            band: active_info.and_then(|a| a.band),
            saved_networks,
        };

        Ok((status, networks))
    }
}

impl<R: NmcliRunner> WifiBackend for NmcliWifiBackend<R> {
    fn status(&self) -> WifiFuture<'_, WifiStatus> {
        Box::pin(async move {
            let (status, _) = self.build_status_and_networks("no").await?;
            Ok(status)
        })
    }

    fn scan(&self, rescan: bool) -> WifiFuture<'_, WifiScanResult> {
        Box::pin(async move {
            let rescan_arg = if rescan { "yes" } else { "auto" };
            let (status, networks) = self.build_status_and_networks(rescan_arg).await?;
            Ok(WifiScanResult { status, networks })
        })
    }

    fn connect<'a>(
        &'a self,
        ssid: &'a str,
        password: Option<&'a str>,
        hidden: bool,
    ) -> WifiFuture<'a, WifiActionResult> {
        Box::pin(async move {
            let trimmed_ssid = validate_ssid(ssid)?;
            let password_non_empty = password.map(str::trim).filter(|p| !p.is_empty());
            if let Some(pw) = password_non_empty
                && pw.chars().any(char::is_control)
            {
                return Err("Password must not contain control characters".to_owned());
            }

            let Some((iface, _, _)) = self.discover_wifi_device().await? else {
                return Err("No Wi-Fi interface available on this device".to_owned());
            };

            // Ensure Wi-Fi radio is enabled before attempting to connect.
            if !self.read_radio_enabled().await.unwrap_or(true) {
                let _ = self.runner.run(&["radio", "wifi", "on"]).await;
            }

            let saved_before = self.list_saved_wifi_networks().await.unwrap_or_default();
            let matching_saved: Vec<&SavedWifiNetwork> = saved_before
                .iter()
                .filter(|n| n.ssid == trimmed_ssid)
                .collect();

            if let Some(pw) = password_non_empty {
                let existing_uuids: HashSet<String> =
                    saved_before.iter().map(|n| n.uuid.clone()).collect();

                let mut args = vec![
                    "--wait",
                    "20",
                    "device",
                    "wifi",
                    "connect",
                    trimmed_ssid,
                    "ifname",
                    &iface,
                    "password",
                    pw,
                ];
                if hidden {
                    args.push("hidden");
                    args.push("yes");
                }

                let out = self.runner.run(&args).await?;
                let saved_after = self.list_saved_wifi_networks().await.unwrap_or_default();
                if !out.success {
                    // Clean up any broken profile created by the failed attempt
                    // so NetworkManager does not loop retrying a bad PSK, while
                    // preserving any pre-existing working profile in existing_uuids.
                    for net in saved_after {
                        if net.ssid == trimmed_ssid && !existing_uuids.contains(&net.uuid) {
                            let _ = self
                                .runner
                                .run(&["connection", "delete", "uuid", &net.uuid])
                                .await;
                        }
                    }
                    return Err(format!(
                        "Failed to connect to '{}': {}",
                        trimmed_ssid,
                        command_error_detail(&out)
                    ));
                }

                // If connecting with a new password created a new profile UUID
                // for this SSID, remove superseded duplicate UUIDs only after
                // the new connection succeeded.
                let new_uuid_created = saved_after
                    .iter()
                    .any(|n| n.ssid == trimmed_ssid && !existing_uuids.contains(&n.uuid));
                if new_uuid_created {
                    for existing in &matching_saved {
                        let _ = self
                            .runner
                            .run(&["connection", "delete", "uuid", &existing.uuid])
                            .await;
                    }
                }
            } else if let Some(saved) = matching_saved.first()
                && !hidden
            {
                let out = self
                    .runner
                    .run(&[
                        "--wait",
                        "20",
                        "connection",
                        "up",
                        "uuid",
                        &saved.uuid,
                        "ifname",
                        &iface,
                    ])
                    .await?;
                if !out.success {
                    return Err(format!(
                        "Failed to activate saved network '{}': {}",
                        trimmed_ssid,
                        command_error_detail(&out)
                    ));
                }
            } else {
                let mut args = vec![
                    "--wait",
                    "20",
                    "device",
                    "wifi",
                    "connect",
                    trimmed_ssid,
                    "ifname",
                    &iface,
                ];
                if hidden {
                    args.push("hidden");
                    args.push("yes");
                }
                let out = self.runner.run(&args).await?;
                if !out.success {
                    return Err(format!(
                        "Failed to connect to '{}': {}",
                        trimmed_ssid,
                        command_error_detail(&out)
                    ));
                }
            }

            let status = self.status().await.ok();
            Ok(WifiActionResult {
                ok: true,
                message: format!("Connected to '{trimmed_ssid}'"),
                status,
            })
        })
    }

    fn disconnect(&self) -> WifiFuture<'_, WifiActionResult> {
        Box::pin(async move {
            let Some((iface, _, active_conn)) = self.discover_wifi_device().await? else {
                return Err("No Wi-Fi interface available on this device".to_owned());
            };

            let out = self
                .runner
                .run(&["--wait", "10", "device", "disconnect", &iface])
                .await?;
            if !out.success {
                return Err(format!(
                    "Failed to disconnect Wi-Fi ({}): {}",
                    iface,
                    command_error_detail(&out)
                ));
            }

            let status = self.status().await.ok();
            let label = active_conn.unwrap_or(iface);
            Ok(WifiActionResult {
                ok: true,
                message: format!("Disconnected from '{label}'"),
                status,
            })
        })
    }

    fn forget<'a>(&'a self, ssid: &'a str) -> WifiFuture<'a, WifiActionResult> {
        Box::pin(async move {
            let trimmed_ssid = validate_ssid(ssid)?;

            let saved = self.list_saved_wifi_networks().await?;
            let matching: Vec<&SavedWifiNetwork> =
                saved.iter().filter(|n| n.ssid == trimmed_ssid).collect();
            if matching.is_empty() {
                return Err(format!("No saved Wi-Fi network found for '{trimmed_ssid}'"));
            }

            for profile in matching {
                let out = self
                    .runner
                    .run(&["connection", "delete", "uuid", &profile.uuid])
                    .await?;
                if !out.success {
                    return Err(format!(
                        "Failed to forget '{}': {}",
                        trimmed_ssid,
                        command_error_detail(&out)
                    ));
                }
            }

            let status = self.status().await.ok();
            Ok(WifiActionResult {
                ok: true,
                message: format!("Forgot saved network '{trimmed_ssid}'"),
                status,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// High-level serialized WifiService (shared by HTTP and P2P control)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct WifiService {
    backend: Arc<dyn WifiBackend>,
    op_lock: Arc<Mutex<()>>,
    config_state: Option<Arc<ConfigState>>,
    logger: Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
}

impl std::fmt::Debug for WifiService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WifiService")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

pub const WIFI_CONTROL_DISABLED: &str = "wifi control disabled";

impl WifiService {
    #[must_use]
    pub fn new(
        backend: Arc<dyn WifiBackend>,
        config_state: Option<Arc<ConfigState>>,
        logger: Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
    ) -> Self {
        Self {
            backend,
            op_lock: Arc::new(Mutex::new(())),
            config_state,
            logger,
        }
    }

    #[must_use]
    pub fn has_config_state(&self) -> bool {
        self.config_state.is_some()
    }

    pub async fn is_allowed(&self) -> Result<bool, String> {
        match &self.config_state {
            Some(cs) => read_allow_wifi_control(cs).await.map_err(|(_, body)| body),
            None => Ok(true),
        }
    }

    async fn ensure_allowed(&self) -> Result<(), (u16, String)> {
        match &self.config_state {
            Some(cs) => {
                let allowed = read_allow_wifi_control(cs).await?;
                if !allowed {
                    return Err((
                        403u16,
                        serde_json::json!({"ok": false, "error": WIFI_CONTROL_DISABLED})
                            .to_string(),
                    ));
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub async fn status(&self) -> Result<WifiStatus, (u16, String)> {
        self.ensure_allowed().await?;
        let _guard = self.op_lock.lock().await;
        self.backend.status().await.map_err(|e| {
            (
                500u16,
                serde_json::json!({"ok": false, "error": e}).to_string(),
            )
        })
    }

    pub async fn scan(&self, rescan: bool, actor: &str) -> Result<WifiScanResult, (u16, String)> {
        self.ensure_allowed().await?;
        let _guard = self.op_lock.lock().await;
        match self.backend.scan(rescan).await {
            Ok(res) => {
                if rescan {
                    self.logger.log_info(
                        "FWD:WIFI",
                        format!(
                            "Wi-Fi scan completed ({} networks found, requested by {actor})",
                            res.networks.len()
                        ),
                    );
                }
                Ok(res)
            }
            Err(e) => {
                self.logger.log_warn(
                    "FWD:WIFI",
                    format!("Wi-Fi scan failed (requested by {actor}): {e}"),
                );
                Err((
                    500u16,
                    serde_json::json!({"ok": false, "error": e}).to_string(),
                ))
            }
        }
    }

    pub async fn connect(
        &self,
        ssid: &str,
        password: Option<&str>,
        hidden: bool,
        actor: &str,
    ) -> Result<WifiActionResult, (u16, String)> {
        self.ensure_allowed().await?;
        if let Err(e) = validate_ssid(ssid) {
            return Err((
                400u16,
                serde_json::json!({"ok": false, "error": e, "message": e}).to_string(),
            ));
        }
        let _guard = self.op_lock.lock().await;
        match self.backend.connect(ssid, password, hidden).await {
            Ok(res) => {
                self.logger.log_info(
                    "FWD:WIFI",
                    format!(
                        "Connected to Wi-Fi '{}' (requested by {actor})",
                        ssid.trim()
                    ),
                );
                Ok(res)
            }
            Err(e) => {
                self.logger.log_error(
                    "FWD:WIFI",
                    format!(
                        "Failed to connect to Wi-Fi '{}' (requested by {actor}): {e}",
                        ssid.trim()
                    ),
                );
                Err((
                    500u16,
                    serde_json::json!({"ok": false, "error": e, "message": e}).to_string(),
                ))
            }
        }
    }

    pub async fn disconnect(&self, actor: &str) -> Result<WifiActionResult, (u16, String)> {
        self.ensure_allowed().await?;
        let _guard = self.op_lock.lock().await;
        match self.backend.disconnect().await {
            Ok(res) => {
                self.logger.log_info(
                    "FWD:WIFI",
                    format!("{} (requested by {actor})", res.message),
                );
                Ok(res)
            }
            Err(e) => {
                self.logger.log_error(
                    "FWD:WIFI",
                    format!("Wi-Fi disconnect failed (requested by {actor}): {e}"),
                );
                Err((
                    500u16,
                    serde_json::json!({"ok": false, "error": e, "message": e}).to_string(),
                ))
            }
        }
    }

    pub async fn forget(&self, ssid: &str, actor: &str) -> Result<WifiActionResult, (u16, String)> {
        self.ensure_allowed().await?;
        if let Err(e) = validate_ssid(ssid) {
            return Err((
                400u16,
                serde_json::json!({"ok": false, "error": e, "message": e}).to_string(),
            ));
        }
        let _guard = self.op_lock.lock().await;
        match self.backend.forget(ssid).await {
            Ok(res) => {
                self.logger.log_info(
                    "FWD:WIFI",
                    format!(
                        "Forgot saved Wi-Fi network '{}' (requested by {actor})",
                        ssid.trim()
                    ),
                );
                Ok(res)
            }
            Err(e) => {
                self.logger.log_error(
                    "FWD:WIFI",
                    format!(
                        "Failed to forget Wi-Fi network '{}' (requested by {actor}): {e}",
                        ssid.trim()
                    ),
                );
                Err((
                    500u16,
                    serde_json::json!({"ok": false, "error": e, "message": e}).to_string(),
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers for `nmcli -t` output
// ---------------------------------------------------------------------------

/// Split a single `nmcli --terse` line on unescaped `:` delimiters, unescaping
/// `\:` and `\\`.
pub fn split_nmcli_terse_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ':' {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    if escaped {
        current.push('\\');
    }
    fields.push(current);
    fields
}

fn command_error_detail(out: &CommandOutput) -> String {
    let stderr = out.stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let stdout = out.stdout.trim();
    if !stdout.is_empty() {
        return stdout.to_owned();
    }
    "nmcli exited with an unknown error".to_owned()
}

fn normalize_device_state(raw_state: &str) -> String {
    let lower = raw_state.trim().to_ascii_lowercase();
    if lower.starts_with("connected") {
        "connected".to_owned()
    } else if lower.starts_with("connecting") {
        "connecting".to_owned()
    } else if lower.starts_with("disconnected") {
        "disconnected".to_owned()
    } else if lower.starts_with("unavailable") || lower.starts_with("unmanaged") {
        "unavailable".to_owned()
    } else {
        lower
    }
}

/// Parse `nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device status` and return
/// `(device_name, state, active_connection)` for the first `wifi` interface.
pub fn parse_wifi_device_status(stdout: &str) -> Option<(String, String, Option<String>)> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields = split_nmcli_terse_line(trimmed);
        if fields.len() < 4 {
            continue;
        }
        if fields[1] == "wifi" {
            let conn = fields[3].trim();
            let active_conn = (!conn.is_empty() && conn != "--").then(|| conn.to_owned());
            return Some((fields[0].clone(), fields[2].clone(), active_conn));
        }
    }
    None
}

/// Parse `nmcli -t -f NAME,UUID,TYPE,AUTOCONNECT connection show` and return
/// saved `802-11-wireless` profiles.
pub fn parse_saved_wifi_connections(stdout: &str) -> Vec<SavedWifiNetwork> {
    let mut results = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields = split_nmcli_terse_line(trimmed);
        if fields.len() < 4 {
            continue;
        }
        if fields[2] == "802-11-wireless" || fields[2] == "wifi" {
            results.push(SavedWifiNetwork {
                ssid: fields[0].clone(),
                uuid: fields[1].clone(),
                autoconnect: fields[3].eq_ignore_ascii_case("yes"),
            });
        }
    }
    results
}

/// Parse `nmcli -t -f IP4.ADDRESS,IP4.GATEWAY device show <iface>`.
pub fn parse_device_ip_and_gateway(stdout: &str) -> (Option<String>, Option<String>) {
    let mut ip_address = None;
    let mut gateway = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some((_, val)) = trimmed.split_once(':') {
            let val = val.trim();
            if val.is_empty() || val == "--" {
                continue;
            }
            if trimmed.starts_with("IP4.ADDRESS") && ip_address.is_none() {
                let ip_only = val.split('/').next().unwrap_or(val).trim();
                if !ip_only.is_empty() {
                    ip_address = Some(ip_only.to_owned());
                }
            } else if trimmed.starts_with("IP4.GATEWAY") && gateway.is_none() {
                gateway = Some(val.to_owned());
            }
        }
    }

    (ip_address, gateway)
}

#[derive(Debug, Clone)]
struct ActiveBssidInfo {
    ssid: String,
    signal: u8,
    security: String,
    band: Option<String>,
}

fn parse_freq_band(freq_raw: &str) -> Option<String> {
    let digits: String = freq_raw
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let mhz: u32 = digits.parse().ok()?;
    if (2400..2500).contains(&mhz) {
        Some("2.4 GHz".to_owned())
    } else if (4900..5925).contains(&mhz) {
        Some("5 GHz".to_owned())
    } else if mhz >= 5925 {
        Some("6 GHz".to_owned())
    } else {
        None
    }
}

fn normalize_security_label(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "--" {
        return "Open".to_owned();
    }
    if trimmed.contains("802.1X") {
        return "Enterprise".to_owned();
    }
    if trimmed.contains("WPA3") && trimmed.contains("WPA2") {
        return "WPA2/WPA3".to_owned();
    }
    if trimmed.contains("WPA3") {
        return "WPA3".to_owned();
    }
    if trimmed.contains("WPA2") {
        return "WPA2".to_owned();
    }
    if trimmed.contains("WPA1") || trimmed.contains("WPA") {
        return "WPA".to_owned();
    }
    if trimmed.contains("WEP") {
        return "WEP".to_owned();
    }
    trimmed.to_owned()
}

fn parse_active_bssid_info(stdout: &str) -> Option<ActiveBssidInfo> {
    for line in stdout.lines() {
        let fields = split_nmcli_terse_line(line.trim());
        if fields.len() < 5 {
            continue;
        }
        if fields[0].trim() == "*" {
            let ssid = fields[1].trim().to_owned();
            if ssid.is_empty() {
                continue;
            }
            let signal = fields[2].trim().parse::<u8>().unwrap_or(0).min(100);
            let band = parse_freq_band(&fields[3]);
            let security = normalize_security_label(&fields[4]);
            return Some(ActiveBssidInfo {
                ssid,
                signal,
                security,
                band,
            });
        }
    }
    None
}

/// Parse `nmcli -t -f IN-USE,SSID,SIGNAL,FREQ,SECURITY device wifi list`,
/// deduplicating BSSIDs by SSID and sorting:
/// `in_use` first -> `saved` second -> `signal` descending -> `ssid` ascending.
pub fn parse_wifi_scan_list(stdout: &str, saved_ssids: &HashSet<String>) -> Vec<WifiNetwork> {
    struct AggregatedNetwork {
        in_use: bool,
        signal: u8,
        security: String,
        bands: BTreeSet<String>,
        saved: bool,
    }

    let mut by_ssid: BTreeMap<String, AggregatedNetwork> = BTreeMap::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields = split_nmcli_terse_line(trimmed);
        if fields.len() < 5 {
            continue;
        }
        let in_use = fields[0].trim() == "*";
        let ssid = fields[1].trim();
        if ssid.is_empty() || ssid == "--" {
            continue;
        }
        let signal = fields[2].trim().parse::<u8>().unwrap_or(0).min(100);
        let band = parse_freq_band(&fields[3]);
        let security = normalize_security_label(&fields[4]);
        let saved = saved_ssids.contains(ssid);

        let entry = by_ssid
            .entry(ssid.to_owned())
            .or_insert_with(|| AggregatedNetwork {
                in_use,
                signal,
                security: security.clone(),
                bands: BTreeSet::new(),
                saved,
            });
        if in_use {
            entry.in_use = true;
        }
        if signal > entry.signal {
            entry.signal = signal;
        }
        if entry.security == "Open" && security != "Open" {
            entry.security = security;
        }
        if let Some(b) = band {
            entry.bands.insert(b);
        }
    }

    let mut networks: Vec<WifiNetwork> = by_ssid
        .into_iter()
        .map(|(ssid, agg)| WifiNetwork {
            ssid,
            in_use: agg.in_use,
            signal: agg.signal,
            security: agg.security,
            bands: agg.bands.into_iter().collect(),
            saved: agg.saved,
        })
        .collect();

    networks.sort_by(|a, b| {
        b.in_use
            .cmp(&a.in_use)
            .then_with(|| b.saved.cmp(&a.saved))
            .then_with(|| b.signal.cmp(&a.signal))
            .then_with(|| a.ssid.cmp(&b.ssid))
    });

    networks
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    #[derive(Debug)]
    struct MockNmcliRunner {
        calls: StdMutex<Vec<Vec<String>>>,
        responses: StdMutex<VecDeque<CommandOutput>>,
    }

    impl MockNmcliRunner {
        fn new(responses: Vec<CommandOutput>) -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                responses: StdMutex::new(responses.into()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl NmcliRunner for Arc<MockNmcliRunner> {
        fn run<'a>(&'a self, args: &'a [&'a str]) -> CmdFuture<'a> {
            let recorded: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
            self.calls.lock().unwrap().push(recorded);
            let next = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(CommandOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                    not_found: false,
                });
            Box::pin(async move { Ok(next) })
        }
    }

    fn ok_out(stdout: &str) -> CommandOutput {
        CommandOutput {
            success: true,
            stdout: stdout.to_owned(),
            stderr: String::new(),
            not_found: false,
        }
    }

    fn err_out(stderr: &str) -> CommandOutput {
        CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_owned(),
            not_found: false,
        }
    }

    #[test]
    fn split_nmcli_terse_unescapes_colons_and_backslashes() {
        let line = r"*:Cafe\: Guest\\WiFi:84:5180 MHz:WPA2";
        let fields = split_nmcli_terse_line(line);
        assert_eq!(
            fields,
            vec!["*", r"Cafe: Guest\WiFi", "84", "5180 MHz", "WPA2"]
        );
    }

    #[test]
    fn parse_wifi_device_status_ignores_eth_and_usb() {
        let raw = "\
eth0:ethernet:connected:Wired connection 1\n\
usb0:ethernet:connected:usb-tether\n\
wlan0:wifi:connected:Pixel\\:Hotspot\n\
lo:loopback:connected (externally):lo\n";
        let parsed = parse_wifi_device_status(raw).expect("should find wlan0");
        assert_eq!(parsed.0, "wlan0");
        assert_eq!(parsed.1, "connected");
        assert_eq!(parsed.2.as_deref(), Some("Pixel:Hotspot"));
    }

    #[test]
    fn parse_wifi_scan_list_deduplicates_bssids_and_sorts() {
        let raw = "\
 :Venue-Guest:60:2412 MHz:--\n\
 :Race-HQ:55:2437 MHz:WPA2\n\
 :Race-HQ:82:5180 MHz:WPA2 WPA3\n\
*:Pixel-Hotspot:70:5200 MHz:WPA2\n\
 :Saved-Van:40:2462 MHz:WPA2\n\
 ::90:2412 MHz:WPA2\n";
        let mut saved = HashSet::new();
        saved.insert("Saved-Van".to_owned());
        saved.insert("Pixel-Hotspot".to_owned());

        let networks = parse_wifi_scan_list(raw, &saved);
        assert_eq!(networks.len(), 4);

        // 1. In-use first
        assert_eq!(networks[0].ssid, "Pixel-Hotspot");
        assert!(networks[0].in_use);
        assert!(networks[0].saved);
        assert_eq!(networks[0].bands, vec!["5 GHz"]);

        // 2. Saved network second even with lower signal
        assert_eq!(networks[1].ssid, "Saved-Van");
        assert!(!networks[1].in_use);
        assert!(networks[1].saved);

        // 3. Unsaved sorted by max signal descending, with merged bands
        assert_eq!(networks[2].ssid, "Race-HQ");
        assert_eq!(networks[2].signal, 82);
        assert_eq!(networks[2].bands, vec!["2.4 GHz", "5 GHz"]);

        // 4. Open network
        assert_eq!(networks[3].ssid, "Venue-Guest");
        assert_eq!(networks[3].security, "Open");
    }

    #[tokio::test]
    async fn connect_with_bad_password_cleans_up_newly_created_profile() {
        let runner = Arc::new(MockNmcliRunner::new(vec![
            // 1. discover_wifi_device
            ok_out("wlan0:wifi:disconnected:--\n"),
            // 2. read_radio_enabled
            ok_out("enabled\n"),
            // 3. list_saved_wifi_networks (before)
            ok_out("ExistingWiFi:uuid-existing:802-11-wireless:yes\n"),
            // 4. device wifi connect -> fails (bad password)
            err_out(
                "Error: Connection activation failed: Secrets were required, but not provided.",
            ),
            // 5. list_saved_wifi_networks (after failed connect)
            ok_out(
                "ExistingWiFi:uuid-existing:802-11-wireless:yes\n\
                 BadTarget:uuid-broken-new:802-11-wireless:yes\n",
            ),
            // 6. connection delete uuid uuid-broken-new
            ok_out("Connection 'BadTarget' (uuid-broken-new) successfully deleted.\n"),
        ]));

        let backend = NmcliWifiBackend::with_runner(Arc::clone(&runner));
        let err = backend
            .connect("BadTarget", Some("wrongpass"), false)
            .await
            .unwrap_err();
        assert!(err.contains("Failed to connect to 'BadTarget'"));

        let calls = runner.calls();
        assert!(
            calls.iter().any(|c| c
                == &vec![
                    "connection".to_string(),
                    "delete".to_string(),
                    "uuid".to_string(),
                    "uuid-broken-new".to_string()
                ]),
            "must delete newly created broken profile uuid, calls: {calls:?}"
        );
    }

    #[tokio::test]
    async fn forget_deletes_only_matching_wireless_uuid() {
        let runner = Arc::new(MockNmcliRunner::new(vec![
            // 1. list_saved_wifi_networks
            ok_out(
                "eth0:uuid-eth:802-3-ethernet:yes\n\
                 MyHotspot:uuid-wifi-1:802-11-wireless:yes\n",
            ),
            // 2. connection delete uuid uuid-wifi-1
            ok_out("Deleted\n"),
            // 3. status() refresh
            ok_out("wlan0:wifi:disconnected:--\n"),
            ok_out("enabled\n"),
            ok_out(""),
            ok_out(""),
        ]));

        let backend = NmcliWifiBackend::with_runner(Arc::clone(&runner));
        let res = backend.forget("MyHotspot").await.expect("forget succeeds");
        assert!(res.ok);

        let calls = runner.calls();
        assert!(
            calls.iter().any(|c| c
                == &vec![
                    "connection".to_string(),
                    "delete".to_string(),
                    "uuid".to_string(),
                    "uuid-wifi-1".to_string()
                ]),
            "must delete by wireless UUID, calls: {calls:?}"
        );
    }

    #[test]
    fn validate_ssid_rejects_invalid_inputs() {
        assert!(validate_ssid("").is_err());
        assert!(validate_ssid("   ").is_err());
        assert!(validate_ssid("-flag").is_err());
        assert!(validate_ssid("--help").is_err());
        assert!(validate_ssid("bad\nssid").is_err());
        assert!(validate_ssid(&"a".repeat(33)).is_err());
        assert_eq!(validate_ssid("  Race-WiFi  ").unwrap(), "Race-WiFi");
    }

    #[test]
    fn parse_freq_band_handles_leading_whitespace() {
        assert_eq!(parse_freq_band(" 2412 MHz").as_deref(), Some("2.4 GHz"));
        assert_eq!(parse_freq_band("  5180 MHz").as_deref(), Some("5 GHz"));
    }

    #[tokio::test]
    async fn connect_failure_does_not_delete_existing_saved_profile() {
        let runner = Arc::new(MockNmcliRunner::new(vec![
            // 1. discover_wifi_device
            ok_out("wlan0:wifi:connected:RaceWiFi\n"),
            // 2. read_radio_enabled
            ok_out("enabled\n"),
            // 3. list_saved_wifi_networks (before)
            ok_out("RaceWiFi:uuid-working:802-11-wireless:yes\n"),
            // 4. device wifi connect -> fails
            err_out("Error: Connection activation failed"),
            // 5. list_saved_wifi_networks (after failed connect)
            ok_out("RaceWiFi:uuid-working:802-11-wireless:yes\n"),
        ]));

        let backend = NmcliWifiBackend::with_runner(Arc::clone(&runner));
        let err = backend
            .connect("RaceWiFi", Some("typo-password"), false)
            .await
            .unwrap_err();
        assert!(err.contains("Failed to connect to 'RaceWiFi'"));

        let calls = runner.calls();
        assert!(
            !calls
                .iter()
                .any(|c| c.contains(&"delete".to_string())
                    && c.contains(&"uuid-working".to_string())),
            "existing working profile must not be deleted on failed connect: {calls:?}"
        );
    }
}

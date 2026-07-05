#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use receiver::control_api::{self, AppState, ShutdownSignal};
use receiver::ui_events::ReceiverUiEvent;
use tauri::async_runtime::JoinHandle;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{Emitter, Manager, RunEvent, State};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::warn;

struct ZoomLevel(Mutex<f64>);

fn open_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();

    if let Err(e) = result {
        warn!(path = %path.display(), error = %e, "failed to open file manager");
    }
}

// ---------------------------------------------------------------------------
// Result alias for Tauri commands
// ---------------------------------------------------------------------------

type CmdResult<T> = Result<T, String>;

const APP_IDENTIFIER: &str = "com.rusty-timer.receiver";
const CRASH_LOG_FILENAME: &str = "crash.log";
const DEV_RECEIVER_ID_ENV: &str = "RT_RECEIVER_ID";
const DEV_DATA_DIR_ENV: &str = "RT_RECEIVER_DATA_DIR";

enum BridgeAction {
    EmitEvent {
        name: &'static str,
        event: ReceiverUiEvent,
    },
    EmitResync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Undo, Redo, Separator only constructed on macOS
enum EditMenuItem {
    Undo,
    Redo,
    Separator,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[cfg(target_os = "macos")]
const EDIT_MENU_ITEMS: &[EditMenuItem] = &[
    EditMenuItem::Undo,
    EditMenuItem::Redo,
    EditMenuItem::Separator,
    EditMenuItem::Cut,
    EditMenuItem::Copy,
    EditMenuItem::Paste,
    EditMenuItem::SelectAll,
];

#[cfg(not(target_os = "macos"))]
const EDIT_MENU_ITEMS: &[EditMenuItem] = &[
    EditMenuItem::Cut,
    EditMenuItem::Copy,
    EditMenuItem::Paste,
    EditMenuItem::SelectAll,
];

/// Canonical event name for a UI event, delegating to the receiver library's
/// single source of truth so the Tauri bridge and the headless / test bridge
/// always agree (see `control_api::event_name` and `EVENT_NAMES`).
fn ui_event_name(event: &ReceiverUiEvent) -> &'static str {
    control_api::event_name(event)
}

// ---------------------------------------------------------------------------
// Canonical Tauri command list
//
// The command surface is enumerated once, in the receiver library's
// `receiver_command_list!` macro (alongside `COMMAND_REGISTRY`). Both adapters
// below expand that single list: `tauri_generate_handler` into
// `tauri::generate_handler!` and `tauri_command_names` into the test-only
// `TAURI_COMMAND_NAMES`. This keeps the IPC handler set, the exported name
// list, and the canonical registry from ever drifting; the
// `tauri_and_bridge_command_sets_match` test asserts that parity.
// ---------------------------------------------------------------------------

/// Adapter: expands one `receiver_command_list!` entry per command into the
/// `tauri::generate_handler!` invocation, keeping only the identifiers.
macro_rules! tauri_generate_handler {
    ($($name:ident ( $($arg:ident : $argty:literal),* $(,)? ) -> $ret:literal),* $(,)?) => {
        tauri::generate_handler![$($name),*]
    };
}

/// Adapter: expands the same list into a `&[&str]` of command names for the
/// test-only `TAURI_COMMAND_NAMES`.
#[cfg(test)]
macro_rules! tauri_command_names {
    ($($name:ident ( $($arg:ident : $argty:literal),* $(,)? ) -> $ret:literal),* $(,)?) => {
        &[$(stringify!($name)),*]
    };
}

/// Names of every Tauri command, derived from the same receiver-provided list
/// that drives `generate_handler!`. Compared against
/// `control_api::bridge_command_names()` by the
/// `tauri_and_bridge_command_sets_match` parity test.
#[cfg(test)]
const TAURI_COMMAND_NAMES: &[&str] = receiver::receiver_command_list!(tauri_command_names);

fn bridge_action_from_item(
    item: Result<ReceiverUiEvent, BroadcastStreamRecvError>,
) -> BridgeAction {
    match item {
        // Library-emitted Resync goes through the same path as lag-induced resync
        // so the frontend always receives an identical empty-payload "resync" event.
        Ok(ReceiverUiEvent::Resync) => BridgeAction::EmitResync,
        Ok(event) => BridgeAction::EmitEvent {
            name: ui_event_name(&event),
            event,
        },
        Err(BroadcastStreamRecvError::Lagged(skipped)) => {
            warn!(
                skipped,
                "receiver UI event bridge lagged; requesting resync"
            );
            BridgeAction::EmitResync
        }
    }
}

fn parsed_receiver_id_override(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn receiver_id_override_from_env() -> Option<String> {
    parsed_receiver_id_override(std::env::var(DEV_RECEIVER_ID_ENV).ok())
}

/// Optional data-directory override for local development / the manual dev
/// stack. When `RT_RECEIVER_DATA_DIR` is set, the receiver stores its SQLite
/// state there instead of the OS app-local data dir, so a dev run can use an
/// isolated, disposable directory.
fn data_dir_override_from_env() -> Option<PathBuf> {
    std::env::var(DEV_DATA_DIR_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn fallback_app_local_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(APP_IDENTIFIER))
}

fn write_crash_log(log_dir: &Path, message: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(log_dir)?;
    let path = log_dir.join(CRASH_LOG_FILENAME);
    std::fs::write(&path, message)?;
    Ok(path)
}

fn write_crash_log_best_effort(log_dir: Option<&Path>, message: &str) {
    if let Some(log_dir) = log_dir {
        let _ = write_crash_log(log_dir, message);
    } else if let Some(log_dir) = fallback_app_local_data_dir() {
        let _ = write_crash_log(&log_dir, message);
    }
}

fn record_startup_failure(message: &str) {
    eprintln!("{message}");
    write_crash_log_best_effort(None, message);
}

fn record_app_failure(app: &tauri::AppHandle, message: &str) {
    eprintln!("{message}");
    let log_dir = app.path().app_local_data_dir().ok();
    write_crash_log_best_effort(log_dir.as_deref(), message);
}

fn edit_menu_items() -> &'static [EditMenuItem] {
    EDIT_MENU_ITEMS
}

fn build_edit_menu<R: tauri::Runtime, M: Manager<R>>(app: &M) -> tauri::Result<Submenu<R>> {
    let mut builder = SubmenuBuilder::new(app, "Edit");

    for item in edit_menu_items() {
        builder = match item {
            EditMenuItem::Undo => builder.item(&PredefinedMenuItem::undo(app, None)?),
            EditMenuItem::Redo => builder.item(&PredefinedMenuItem::redo(app, None)?),
            EditMenuItem::Separator => builder.separator(),
            EditMenuItem::Cut => builder.item(&PredefinedMenuItem::cut(app, None)?),
            EditMenuItem::Copy => builder.item(&PredefinedMenuItem::copy(app, None)?),
            EditMenuItem::Paste => builder.item(&PredefinedMenuItem::paste(app, None)?),
            EditMenuItem::SelectAll => builder.item(&PredefinedMenuItem::select_all(app, None)?),
        };
    }

    builder.build()
}

// ---------------------------------------------------------------------------
// Tauri commands — thin wrappers around receiver library functions
// ---------------------------------------------------------------------------

#[tauri::command]
async fn get_profile(state: State<'_, Arc<AppState>>) -> CmdResult<control_api::ProfileResponse> {
    control_api::get_profile(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn put_profile(
    state: State<'_, Arc<AppState>>,
    body: control_api::ProfileRequest,
) -> CmdResult<()> {
    control_api::put_profile(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_participants(
    state: State<'_, Arc<AppState>>,
    contents: String,
) -> CmdResult<control_api::ImportSummary> {
    control_api::import_participants(&state, contents)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_chips(
    state: State<'_, Arc<AppState>>,
    contents: String,
) -> CmdResult<control_api::ImportSummary> {
    control_api::import_chips(&state, contents)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_participants_file(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> CmdResult<control_api::ImportSummary> {
    control_api::import_participants_file(&state, path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_chips_file(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> CmdResult<control_api::ImportSummary> {
    control_api::import_chips_file(&state, path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_participants_from_rd(
    state: State<'_, Arc<AppState>>,
    dir: String,
) -> CmdResult<control_api::ImportSummary> {
    control_api::import_participants_from_rd(&state, dir)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_rd_import_config(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<receiver::db::RdImportConfig> {
    control_api::get_rd_import_config(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn put_rd_import_config(
    state: State<'_, Arc<AppState>>,
    body: receiver::db::RdImportConfig,
) -> CmdResult<()> {
    control_api::put_rd_import_config(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_data_stats(state: State<'_, Arc<AppState>>) -> CmdResult<receiver::db::DataStats> {
    control_api::get_data_stats(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_announcer_enabled(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    control_api::set_announcer_enabled(&state, enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_announcer_max_list_size(
    state: State<'_, Arc<AppState>>,
    max_list_size: u32,
) -> CmdResult<()> {
    control_api::set_announcer_max_list_size(&state, max_list_size)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_stream_announcer_publish(
    state: State<'_, Arc<AppState>>,
    forwarder_endpoint_id: String,
    stream_id: String,
    publish: bool,
) -> CmdResult<()> {
    control_api::set_stream_announcer_publish(&state, &forwarder_endpoint_id, &stream_id, publish)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_mode(state: State<'_, Arc<AppState>>) -> CmdResult<rt_domain::ReceiverMode> {
    control_api::get_mode(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn put_mode(state: State<'_, Arc<AppState>>, mode: rt_domain::ReceiverMode) -> CmdResult<()> {
    control_api::put_mode(&state, mode)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_streams(state: State<'_, Arc<AppState>>) -> CmdResult<control_api::StreamsResponse> {
    Ok(control_api::get_streams(&state).await)
}

#[tauri::command]
async fn get_stream_metrics(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<receiver::ui_events::StreamMetricsPayload>> {
    Ok(control_api::get_stream_metrics(&state).await)
}

#[tauri::command]
async fn put_earliest_epoch(
    state: State<'_, Arc<AppState>>,
    body: control_api::EarliestEpochRequest,
) -> CmdResult<()> {
    control_api::put_earliest_epoch(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_replay_target_epochs(
    state: State<'_, Arc<AppState>>,
    forwarder_endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReplayTargetEpochsResponse> {
    control_api::get_replay_target_epochs(&state, forwarder_endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_subscriptions(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<control_api::SubscriptionsBody> {
    control_api::get_subscriptions(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn put_subscriptions(
    state: State<'_, Arc<AppState>>,
    body: control_api::SubscriptionsBody,
) -> CmdResult<()> {
    control_api::put_subscriptions(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_status(state: State<'_, Arc<AppState>>) -> CmdResult<control_api::StatusResponse> {
    Ok(control_api::get_status(&state).await)
}

#[tauri::command]
async fn get_connections(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<control_api::ConnectionsResponse> {
    Ok(control_api::get_connections(&state).await)
}

#[tauri::command]
async fn reconnect_server(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    control_api::reconnect_server(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn connect_forwarder(state: State<'_, Arc<AppState>>, endpoint_id: String) -> CmdResult<()> {
    control_api::connect_forwarder(&state, endpoint_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn disconnect_forwarder(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
) -> CmdResult<()> {
    control_api::disconnect_forwarder(&state, endpoint_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reconnect_forwarder(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
) -> CmdResult<()> {
    control_api::reconnect_forwarder(&state, endpoint_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_forwarder_config(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
) -> CmdResult<control_api::ForwarderConfigResponse> {
    control_api::get_forwarder_config(&state, endpoint_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_forwarder_config(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    config_json: String,
) -> CmdResult<control_api::ForwarderConfigSetResult> {
    control_api::set_forwarder_config(&state, endpoint_id, config_json)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn restart_forwarder(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
) -> CmdResult<control_api::ForwarderRestartResult> {
    control_api::restart_forwarder(&state, endpoint_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_get_info(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_get_info(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_sync_clock(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_sync_clock(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_set_epoch_name(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
    name: Option<String>,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_set_epoch_name(&state, endpoint_id, stream_id, name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_advance_epoch(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_advance_epoch(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_set_read_mode(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
    mode: rt_domain::ReadMode,
    timeout: u8,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_set_read_mode(&state, endpoint_id, stream_id, mode, timeout)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_set_tto(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
    enabled: bool,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_set_tto(&state, endpoint_id, stream_id, enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_set_recording(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
    enabled: bool,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_set_recording(&state, endpoint_id, stream_id, enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_clear_records(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_clear_records(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_start_download(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_start_download(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_stop_download(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_stop_download(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_refresh(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_refresh(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_reconnect(
    state: State<'_, Arc<AppState>>,
    endpoint_id: String,
    stream_id: String,
) -> CmdResult<control_api::ReaderControlResult> {
    control_api::reader_reconnect(&state, endpoint_id, stream_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_version() -> String {
    control_api::get_version()
}

#[tauri::command]
async fn get_logs(state: State<'_, Arc<AppState>>) -> CmdResult<control_api::LogsResponse> {
    Ok(control_api::get_logs(&state).await)
}

#[tauri::command]
async fn admin_reset_cursor(
    state: State<'_, Arc<AppState>>,
    body: control_api::StreamRef,
) -> CmdResult<()> {
    control_api::admin_reset_cursor(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_reset_all_cursors(state: State<'_, Arc<AppState>>) -> CmdResult<serde_json::Value> {
    control_api::admin_reset_all_cursors(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_reset_earliest_epoch(
    state: State<'_, Arc<AppState>>,
    body: control_api::StreamRef,
) -> CmdResult<()> {
    control_api::admin_reset_earliest_epoch(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_reset_all_earliest_epochs(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<serde_json::Value> {
    control_api::admin_reset_all_earliest_epochs(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_purge_subscriptions(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<serde_json::Value> {
    control_api::admin_purge_subscriptions(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_update_port(
    state: State<'_, Arc<AppState>>,
    body: control_api::UpdatePortRequest,
) -> CmdResult<()> {
    control_api::admin_update_port(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_reset_profile(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    control_api::admin_reset_profile(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_clear_data(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    control_api::admin_clear_data(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn admin_factory_reset(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    control_api::admin_factory_reset(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_dbf_config(
    state: tauri::State<'_, Arc<AppState>>,
) -> CmdResult<receiver::db::DbfConfig> {
    receiver::control_api::get_dbf_config(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn put_dbf_config(
    state: tauri::State<'_, Arc<AppState>>,
    body: receiver::db::DbfConfig,
) -> CmdResult<()> {
    receiver::control_api::put_dbf_config(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn clear_dbf(state: tauri::State<'_, Arc<AppState>>) -> CmdResult<()> {
    receiver::control_api::clear_dbf(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn update_subscription_event_type(
    state: tauri::State<'_, Arc<AppState>>,
    forwarder_endpoint_id: String,
    stream_id: String,
    body: receiver::control_api::EventTypeRequest,
) -> CmdResult<()> {
    receiver::control_api::update_subscription_event_type(
        &state,
        &forwarder_endpoint_id,
        &stream_id,
        body,
    )
    .await
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Event bridge: forward ReceiverUiEvent -> Tauri frontend events
// ---------------------------------------------------------------------------

fn spawn_event_bridge(app_handle: tauri::AppHandle, state: &Arc<AppState>) {
    let rx = state.ui.ui_tx.subscribe();
    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let mut stream = BroadcastStream::new(rx);
        while let Some(item) = stream.next().await {
            match bridge_action_from_item(item) {
                BridgeAction::EmitEvent { name, event } => {
                    if let Err(e) = handle.emit(name, &event) {
                        warn!(event_name = name, error = %e, "failed to emit UI event to webview");
                    }
                }
                BridgeAction::EmitResync => {
                    let name = ui_event_name(&ReceiverUiEvent::Resync);
                    if let Err(e) = handle.emit(name, ()) {
                        warn!(error = %e, "failed to emit resync event to webview");
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls CryptoProvider");

    tracing_subscriber::fmt::init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();

            // Build native menu bar

            // --- File menu ---
            let check_update =
                MenuItemBuilder::with_id("check-update", "Check for Updates...").build(app)?;
            let open_data_dir =
                MenuItemBuilder::with_id("open-data-dir", "Open Data Directory").build(app)?;
            let quit = PredefinedMenuItem::quit(app, Some("Quit"))?;
            let file_menu = SubmenuBuilder::new(app, "File")
                .item(&check_update)
                .item(&open_data_dir)
                .separator()
                .item(&quit)
                .build()?;

            // --- Edit menu ---
            let edit_menu = build_edit_menu(app)?;

            // --- View menu ---
            let refresh = MenuItemBuilder::with_id("refresh", "Refresh")
                .accelerator("CmdOrCtrl+R")
                .build(app)?;
            let toggle_theme =
                MenuItemBuilder::with_id("toggle-theme", "Toggle Theme").build(app)?;
            let zoom_in = MenuItemBuilder::with_id("zoom-in", "Zoom In")
                .accelerator("CmdOrCtrl+=")
                .build(app)?;
            let zoom_out = MenuItemBuilder::with_id("zoom-out", "Zoom Out")
                .accelerator("CmdOrCtrl+-")
                .build(app)?;
            let zoom_reset = MenuItemBuilder::with_id("zoom-reset", "Reset Zoom")
                .accelerator("CmdOrCtrl+0")
                .build(app)?;

            #[allow(unused_mut)]
            // mutated only under cfg(target_os = "macos") and cfg(debug_assertions)
            let mut view_builder = SubmenuBuilder::new(app, "View")
                .item(&refresh)
                .item(&toggle_theme)
                .separator()
                .item(&zoom_in)
                .item(&zoom_out)
                .item(&zoom_reset);

            #[cfg(target_os = "macos")]
            {
                view_builder = view_builder
                    .separator()
                    .item(&PredefinedMenuItem::fullscreen(app, None)?);
            }

            #[cfg(debug_assertions)]
            {
                let toggle_devtools =
                    MenuItemBuilder::with_id("toggle-devtools", "Toggle Developer Tools")
                        .accelerator("CmdOrCtrl+Shift+I")
                        .build(app)?;
                view_builder = view_builder.separator().item(&toggle_devtools);
            }

            let view_menu = view_builder.build()?;

            // --- Help menu ---
            let about = PredefinedMenuItem::about(app, Some("About Rusty Timer Receiver"), None)?;
            let open_help = MenuItemBuilder::with_id("open-help", "Help...").build(app)?;
            let open_logs_dir =
                MenuItemBuilder::with_id("open-logs-dir", "Open Logs Directory").build(app)?;
            let help_menu = SubmenuBuilder::new(app, "Help")
                .item(&about)
                .item(&open_help)
                .item(&open_logs_dir)
                .build()?;

            let menu = MenuBuilder::new(app)
                .item(&file_menu)
                .item(&edit_menu)
                .item(&view_menu)
                .item(&help_menu)
                .build()?;

            app.set_menu(menu)?;

            // Zoom level state for View > Zoom In/Out/Reset
            app.manage(ZoomLevel(Mutex::new(1.0)));

            // Handle menu events
            app.on_menu_event(|app_handle, event| match event.id().as_ref() {
                "check-update" => {
                    if let Err(e) = app_handle.emit("menu-check-update", ()) {
                        warn!(error = %e, "failed to emit menu-check-update event");
                    }
                }
                "refresh" => {
                    if let Some(window) = app_handle.get_webview_window("main")
                        && let Err(e) = window.reload()
                    {
                        warn!(error = %e, "failed to reload webview");
                    }
                }
                "toggle-theme" => {
                    if let Err(e) = app_handle.emit("menu-toggle-theme", ()) {
                        warn!(error = %e, "failed to emit menu-toggle-theme event");
                    }
                }
                "zoom-in" => {
                    if let Some(level) = app_handle.try_state::<ZoomLevel>() {
                        let mut zoom = level.0.lock().unwrap();
                        *zoom = (*zoom + 0.1).min(3.0);
                        if let Some(window) = app_handle.get_webview_window("main")
                            && let Err(e) = window.set_zoom(*zoom)
                        {
                            warn!(error = %e, "failed to set zoom level");
                        }
                    }
                }
                "zoom-out" => {
                    if let Some(level) = app_handle.try_state::<ZoomLevel>() {
                        let mut zoom = level.0.lock().unwrap();
                        *zoom = (*zoom - 0.1).max(0.5);
                        if let Some(window) = app_handle.get_webview_window("main")
                            && let Err(e) = window.set_zoom(*zoom)
                        {
                            warn!(error = %e, "failed to set zoom level");
                        }
                    }
                }
                "zoom-reset" => {
                    if let Some(level) = app_handle.try_state::<ZoomLevel>() {
                        let mut zoom = level.0.lock().unwrap();
                        *zoom = 1.0;
                        if let Some(window) = app_handle.get_webview_window("main")
                            && let Err(e) = window.set_zoom(1.0)
                        {
                            warn!(error = %e, "failed to reset zoom level");
                        }
                    }
                }
                #[cfg(debug_assertions)]
                "toggle-devtools" => {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        if window.is_devtools_open() {
                            window.close_devtools();
                        } else {
                            window.open_devtools();
                        }
                    }
                }
                "open-data-dir" => {
                    if let Ok(dir) = app_handle.path().app_local_data_dir() {
                        open_in_file_manager(&dir);
                    }
                }
                "open-logs-dir" => {
                    if let Ok(dir) = app_handle.path().app_log_dir() {
                        open_in_file_manager(&dir);
                    }
                }
                "open-help" => {
                    if let Err(e) = app_handle.emit("menu-open-help", ()) {
                        warn!(error = %e, "failed to emit menu-open-help event");
                    }
                }
                _ => {}
            });

            // Initialize receiver runtime.
            // block_on is safe here because setup() runs before the Tauri event
            // loop starts, so we won't deadlock the async runtime.
            let receiver_id_override = receiver_id_override_from_env();
            // Resolve the data dir once so the DB and the persistent P2P
            // secret-key file live in the same place.
            let data_dir =
                data_dir_override_from_env().unwrap_or_else(receiver::runtime::default_data_dir);
            let (state, shutdown_rx) = tauri::async_runtime::block_on(async {
                receiver::runtime::init_with_data_dir(receiver_id_override, data_dir.clone()).await
            })
            .map_err(|e| -> Box<dyn std::error::Error> {
                let msg = format!("Fatal: failed to initialize receiver runtime: {e}");
                record_app_failure(&handle, &msg);
                Box::new(std::io::Error::other(msg))
            })?;

            // Register state for commands
            app.manage(state.clone());

            // Start event bridge
            spawn_event_bridge(handle, &state);

            // P2P lane: always start a runtime so a fresh install has a live
            // endpoint that a later profile save can reconfigure. The RT_P2P_*
            // env vars (dev/loopback overrides) take precedence; otherwise a
            // bare production config is used. The stored profile is the source
            // of truth for the server URL+token, with the env vars as override.
            let p2p_handle = app.handle().clone();
            let p2p_key_path = data_dir.join("p2p_secret.key");
            let mut p2p_config =
                match receiver::p2p_runtime::p2p_config_from_env(p2p_key_path.clone()) {
                    Ok(Some(cfg)) => cfg,
                    Ok(None) => {
                        receiver::p2p_runtime::P2pReceiverConfig::production_default(p2p_key_path)
                    }
                    Err(e) => {
                        let msg = format!("Fatal: invalid P2P env configuration: {e}");
                        record_app_failure(&p2p_handle, &msg);
                        return Err(Box::new(std::io::Error::other(msg)));
                    }
                };
            let profile = tauri::async_runtime::block_on(async {
                state.storage.db.lock().await.load_profile().ok().flatten()
            });
            // `p2p_config_from_lookup` (via `p2p_config_from_env`) is the
            // single writer of `server_override`; read it here and apply the
            // `env override > profile` precedence without re-deriving it.
            let server_override = p2p_config.server_override.clone();
            // Record the override on shared state so control handlers
            // (profile/status) resolve and gate consistently with the runtime.
            tauri::async_runtime::block_on(async {
                state.set_server_override(server_override.clone()).await;
            });
            p2p_config.server =
                receiver::runtime::resolve_server_config(profile.as_ref(), server_override);
            let p2p_state = state.clone();
            let p2p_runtime = tauri::async_runtime::block_on(async {
                receiver::p2p_runtime::start_receiver_p2p(p2p_state, p2p_config).await
            })
            .map_err(|e| -> Box<dyn std::error::Error> {
                let msg = format!("Fatal: failed to start P2P receiver runtime: {e}");
                record_app_failure(&p2p_handle, &msg);
                Box::new(std::io::Error::other(msg))
            })?;
            app.manage(Mutex::new(Some(p2p_runtime)));

            // Spawn receiver runtime, keeping the handle so we can await
            // graceful shutdown (cancel session, stop proxies) before exit.
            let runtime_handle: JoinHandle<()> =
                tauri::async_runtime::spawn(receiver::runtime::run(state, shutdown_rx));
            app.manage(Mutex::new(Some(runtime_handle)));

            Ok(())
        })
        .invoke_handler(receiver::receiver_command_list!(tauri_generate_handler))
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(e) => {
            let msg = format!("Fatal: failed to build tauri application: {e}");
            record_startup_failure(&msg);
            std::process::exit(1);
        }
    };

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<Arc<AppState>>() {
                let _ = state.signals.shutdown_tx.send(ShutdownSignal::Terminate);
            }
            // Shut down the optional P2P runtime first (cancel sessions,
            // proxies, and workers) before draining the housekeeping task.
            if let Some(guard) =
                app_handle.try_state::<Mutex<Option<receiver::p2p_runtime::P2pReceiverRuntime>>>()
                && let Some(p2p_runtime) = guard.lock().ok().and_then(|mut g| g.take())
            {
                tauri::async_runtime::block_on(async {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        p2p_runtime.shutdown(),
                    )
                    .await;
                });
            }
            // Wait for receiver runtime to finish graceful cleanup
            // (cancel P2P sessions, stop local proxies) before the process exits.
            if let Some(guard) = app_handle.try_state::<Mutex<Option<JoinHandle<()>>>>()
                && let Some(handle) = guard.lock().ok().and_then(|mut g| g.take())
            {
                tauri::async_runtime::block_on(async {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use receiver::control_api::ConnectionState;
    use std::fs;

    #[test]
    fn lagged_broadcast_requests_resync_instead_of_stopping_bridge() {
        let action = bridge_action_from_item(Err(BroadcastStreamRecvError::Lagged(3)));
        assert!(matches!(action, BridgeAction::EmitResync));
    }

    #[test]
    fn status_changed_maps_to_expected_event_name() {
        let action = bridge_action_from_item(Ok(ReceiverUiEvent::StatusChanged {
            connection_state: ConnectionState::Connected,
            streams_count: 2,
            receiver_id: "recv-1".to_owned(),
        }));
        assert!(matches!(
            action,
            BridgeAction::EmitEvent {
                name: "status_changed",
                ..
            }
        ));
    }

    #[test]
    fn tauri_and_bridge_command_sets_match() {
        use std::collections::BTreeSet;
        // Tauri side: derived from the same macro that drives `generate_handler!`.
        let tauri: BTreeSet<&str> = TAURI_COMMAND_NAMES.iter().copied().collect();
        // Bridge side: derived from the canonical receiver command registry.
        let bridge: BTreeSet<&str> = control_api::bridge_command_names()
            .iter()
            .copied()
            .collect();
        assert_eq!(
            tauri,
            bridge,
            "Tauri generate_handler! command set and bridge command registry diverged.\n\
             Tauri-only: {:?}\nBridge-only: {:?}",
            tauri.difference(&bridge).collect::<Vec<_>>(),
            bridge.difference(&tauri).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn event_names_match() {
        use receiver::ui_events::{ForwarderReaderCounts, StreamDelta, StreamMetricsPayload};
        use std::collections::BTreeSet;

        // One sample of every ReceiverUiEvent variant. `ui_event_name`
        // delegates to `control_api::event_name`, whose exhaustive parity
        // test forces every new variant to be classified; including each
        // variant here keeps the canonical EVENT_NAMES list honest.
        let samples = vec![
            ReceiverUiEvent::Resync,
            ReceiverUiEvent::StatusChanged {
                connection_state: ConnectionState::Connected,
                streams_count: 0,
                receiver_id: "recv".to_owned(),
            },
            ReceiverUiEvent::ConnectionsChanged,
            ReceiverUiEvent::StreamsSnapshot {
                streams: vec![],
                degraded: false,
                upstream_error: None,
            },
            ReceiverUiEvent::LogEntry {
                entry: "x".to_owned(),
            },
            ReceiverUiEvent::ForwarderReaderCountsUpdated(ForwarderReaderCounts {
                forwarder_id: "f".to_owned(),
                stream_id: "ip".to_owned(),
                reads_session: 0,
                reads_total: 0,
                reads_epoch: None,
                last_read_unix_ms: None,
                last_seen_secs: None,
            }),
            ReceiverUiEvent::ModeChanged {
                mode: rt_domain::ReceiverMode::Race {
                    race_id: "r".to_owned(),
                },
            },
            ReceiverUiEvent::StreamDeltas {
                updates: vec![StreamDelta {
                    forwarder_endpoint_id: "e".to_owned(),
                    stream_id: "s".to_owned(),
                    forwarder_id: "f".to_owned(),
                    reader_ip: "ip".to_owned(),
                    reads_total: 0,
                    reads_epoch: 0,
                    metrics: StreamMetricsPayload {
                        forwarder_endpoint_id: "e".to_owned(),
                        stream_id: "s".to_owned(),
                        forwarder_id: "f".to_owned(),
                        reader_ip: "ip".to_owned(),
                        raw_count: 0,
                        dedup_count: 0,
                        retransmit_count: 0,
                        lag_ms: None,
                        epoch_raw_count: 0,
                        epoch_dedup_count: 0,
                        epoch_retransmit_count: 0,
                        unique_chips: 0,
                        epoch_last_received_at: None,
                        epoch_lag_ms: None,
                    },
                    last_read: None,
                }],
            },
            ReceiverUiEvent::ForwarderUpsUpdated {
                forwarder_id: "f".to_owned(),
                available: false,
                status: None,
            },
        ];

        // Names the Tauri bridge actually emits, via ui_event_name.
        let emitted: BTreeSet<&str> = samples.iter().map(ui_event_name).collect();
        // Canonical names the receiver library publishes (consumed by the
        // headless/test bridge in T5.1).
        let canonical: BTreeSet<&str> = control_api::EVENT_NAMES.iter().copied().collect();
        assert_eq!(
            emitted,
            canonical,
            "Tauri-emitted event names and canonical EVENT_NAMES diverged.\n\
             Emitted-only: {:?}\nCanonical-only: {:?}",
            emitted.difference(&canonical).collect::<Vec<_>>(),
            canonical.difference(&emitted).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn parsed_receiver_id_override_trims_and_filters_empty_values() {
        assert_eq!(
            parsed_receiver_id_override(Some(" recv-dev ".to_owned())),
            Some("recv-dev".to_owned())
        );
        assert_eq!(parsed_receiver_id_override(Some("   ".to_owned())), None);
        assert_eq!(parsed_receiver_id_override(None), None);
    }

    #[test]
    fn write_crash_log_creates_expected_file() {
        let unique = format!(
            "receiver-tauri-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let path = write_crash_log(&dir, "fatal startup error").expect("write crash log");

        assert_eq!(path, dir.join("crash.log"));
        assert_eq!(
            fs::read_to_string(&path).expect("read crash log"),
            "fatal startup error"
        );

        fs::remove_file(&path).expect("remove crash log");
        fs::remove_dir(&dir).expect("remove crash dir");
    }

    #[test]
    fn edit_menu_items_match_platform_support() {
        let items = edit_menu_items();

        #[cfg(target_os = "macos")]
        assert_eq!(
            items,
            &[
                EditMenuItem::Undo,
                EditMenuItem::Redo,
                EditMenuItem::Separator,
                EditMenuItem::Cut,
                EditMenuItem::Copy,
                EditMenuItem::Paste,
                EditMenuItem::SelectAll
            ]
        );

        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            items,
            &[
                EditMenuItem::Cut,
                EditMenuItem::Copy,
                EditMenuItem::Paste,
                EditMenuItem::SelectAll
            ]
        );
    }
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use receiver::control_api::{self, AppState, ShutdownSignal};
use receiver::ui_events::ReceiverUiEvent;
use tauri::async_runtime::JoinHandle;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager, RunEvent, State};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::warn;

// ---------------------------------------------------------------------------
// Result alias for Tauri commands
// ---------------------------------------------------------------------------

type CmdResult<T> = Result<T, String>;

const APP_IDENTIFIER: &str = "com.rusty-timer.receiver";
const CRASH_LOG_FILENAME: &str = "crash.log";
const DEV_RECEIVER_ID_ENV: &str = "RT_RECEIVER_ID";

enum BridgeAction {
    EmitEvent {
        name: &'static str,
        event: ReceiverUiEvent,
    },
    EmitResync,
}

fn ui_event_name(event: &ReceiverUiEvent) -> &'static str {
    match event {
        ReceiverUiEvent::Resync => "resync",
        ReceiverUiEvent::StatusChanged { .. } => "status_changed",
        ReceiverUiEvent::StreamsSnapshot { .. } => "streams_snapshot",
        ReceiverUiEvent::LogEntry { .. } => "log_entry",
        ReceiverUiEvent::StreamCountsUpdated { .. } => "stream_counts_updated",
        ReceiverUiEvent::ForwarderMetricsUpdated(_) => "forwarder_metrics_updated",
        ReceiverUiEvent::ModeChanged { .. } => "mode_changed",
        ReceiverUiEvent::LastRead(_) => "last_read",
        ReceiverUiEvent::StreamMetricsUpdated(_) => "stream_metrics_updated",
    }
}

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
async fn get_mode(state: State<'_, Arc<AppState>>) -> CmdResult<rt_protocol::ReceiverMode> {
    control_api::get_mode(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn put_mode(
    state: State<'_, Arc<AppState>>,
    mode: rt_protocol::ReceiverMode,
) -> CmdResult<()> {
    control_api::put_mode(&state, mode)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_streams(state: State<'_, Arc<AppState>>) -> CmdResult<control_api::StreamsResponse> {
    Ok(control_api::get_streams(&state).await)
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
async fn get_races(state: State<'_, Arc<AppState>>) -> CmdResult<serde_json::Value> {
    control_api::get_races(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_forwarders(state: State<'_, Arc<AppState>>) -> CmdResult<serde_json::Value> {
    control_api::get_forwarders(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_forwarder_config(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
) -> CmdResult<serde_json::Value> {
    control_api::get_forwarder_config(&state, forwarder_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_forwarder_config(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    section: String,
    data: serde_json::Value,
) -> CmdResult<serde_json::Value> {
    control_api::set_forwarder_config(&state, forwarder_id, section, data)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn restart_forwarder_service(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
) -> CmdResult<serde_json::Value> {
    control_api::restart_forwarder_service(&state, forwarder_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn restart_forwarder_device(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
) -> CmdResult<serde_json::Value> {
    control_api::restart_forwarder_device(&state, forwarder_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn shutdown_forwarder_device(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
) -> CmdResult<serde_json::Value> {
    control_api::shutdown_forwarder_device(&state, forwarder_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_replay_target_epochs(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<control_api::ReplayTargetEpochsResponse> {
    control_api::get_replay_target_epochs(&state, forwarder_id, reader_ip)
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
fn get_version() -> String {
    control_api::get_version()
}

#[tauri::command]
async fn get_logs(state: State<'_, Arc<AppState>>) -> CmdResult<control_api::LogsResponse> {
    Ok(control_api::get_logs(&state).await)
}

#[tauri::command]
async fn connect(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    control_api::connect(&state).await;
    Ok(())
}

#[tauri::command]
async fn disconnect(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    control_api::disconnect(&state).await;
    Ok(())
}

#[tauri::command]
async fn admin_reset_cursor(
    state: State<'_, Arc<AppState>>,
    body: control_api::CursorResetRequest,
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
    body: control_api::CursorResetRequest,
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
    forwarder_id: String,
    reader_ip: String,
    body: receiver::control_api::EventTypeRequest,
) -> CmdResult<()> {
    receiver::control_api::update_subscription_event_type(&state, &forwarder_id, &reader_ip, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_server_streams(state: State<'_, Arc<AppState>>) -> CmdResult<serde_json::Value> {
    control_api::get_server_streams(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_announcer_config(state: State<'_, Arc<AppState>>) -> CmdResult<serde_json::Value> {
    control_api::get_announcer_config(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn put_announcer_config(
    state: State<'_, Arc<AppState>>,
    body: serde_json::Value,
) -> CmdResult<serde_json::Value> {
    control_api::put_announcer_config(&state, body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reset_announcer(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    control_api::reset_announcer(&state)
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Event bridge: forward ReceiverUiEvent -> Tauri frontend events
// ---------------------------------------------------------------------------

fn spawn_event_bridge(app_handle: tauri::AppHandle, state: &Arc<AppState>) {
    let rx = state.ui_tx.subscribe();
    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let mut stream = BroadcastStream::new(rx);
        while let Some(item) = stream.next().await {
            match bridge_action_from_item(item) {
                BridgeAction::EmitEvent { name, event } => {
                    let _ = handle.emit(name, &event);
                }
                BridgeAction::EmitResync => {
                    let _ = handle.emit("resync", ());
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();

            // Build native menu bar
            let check_update =
                MenuItemBuilder::with_id("check-update", "Check for Updates...").build(app)?;
            let quit = PredefinedMenuItem::quit(app, Some("Quit"))?;
            let file_menu = SubmenuBuilder::new(app, "File")
                .item(&check_update)
                .separator()
                .item(&quit)
                .build()?;

            let toggle_theme =
                MenuItemBuilder::with_id("toggle-theme", "Toggle Theme").build(app)?;
            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&toggle_theme)
                .build()?;

            let open_help = MenuItemBuilder::with_id("open-help", "Help...").build(app)?;
            let help_menu = SubmenuBuilder::new(app, "Help").item(&open_help).build()?;

            let menu = MenuBuilder::new(app)
                .item(&file_menu)
                .item(&view_menu)
                .item(&help_menu)
                .build()?;

            app.set_menu(menu)?;

            // Handle menu events
            app.on_menu_event(|app_handle, event| match event.id().as_ref() {
                "check-update" => {
                    let _ = app_handle.emit("menu-check-update", ());
                }
                "toggle-theme" => {
                    let _ = app_handle.emit("menu-toggle-theme", ());
                }
                "open-help" => {
                    let _ = app_handle.emit("menu-open-help", ());
                }
                _ => {}
            });

            // Initialize receiver runtime.
            // block_on is safe here because setup() runs before the Tauri event
            // loop starts, so we won't deadlock the async runtime.
            let receiver_id_override = receiver_id_override_from_env();
            let (state, shutdown_rx) = tauri::async_runtime::block_on(async {
                receiver::runtime::init(receiver_id_override).await
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

            // Spawn receiver runtime, keeping the handle so we can await
            // graceful shutdown (cancel session, stop proxies) before exit.
            let runtime_handle: JoinHandle<()> =
                tauri::async_runtime::spawn(receiver::runtime::run(state, shutdown_rx));
            app.manage(Mutex::new(Some(runtime_handle)));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_profile,
            put_profile,
            get_mode,
            put_mode,
            get_streams,
            put_earliest_epoch,
            get_races,
            get_forwarders,
            get_forwarder_config,
            set_forwarder_config,
            restart_forwarder_service,
            restart_forwarder_device,
            shutdown_forwarder_device,
            get_replay_target_epochs,
            get_subscriptions,
            put_subscriptions,
            get_status,
            get_version,
            get_logs,
            connect,
            disconnect,
            admin_reset_cursor,
            admin_reset_all_cursors,
            admin_reset_earliest_epoch,
            admin_reset_all_earliest_epochs,
            admin_purge_subscriptions,
            admin_update_port,
            admin_reset_profile,
            admin_factory_reset,
            get_dbf_config,
            put_dbf_config,
            clear_dbf,
            update_subscription_event_type,
            get_server_streams,
            get_announcer_config,
            put_announcer_config,
            reset_announcer,
        ])
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
                let _ = state.shutdown_tx.send(ShutdownSignal::Terminate);
            }
            // Wait for receiver runtime to finish graceful cleanup
            // (cancel WS session, stop local proxies) before the process exits.
            if let Some(guard) = app_handle.try_state::<Mutex<Option<JoinHandle<()>>>>() {
                if let Some(handle) = guard.lock().ok().and_then(|mut g| g.take()) {
                    tauri::async_runtime::block_on(async {
                        let _ =
                            tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
                    });
                }
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
}

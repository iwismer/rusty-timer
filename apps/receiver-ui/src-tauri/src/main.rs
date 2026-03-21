#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use receiver::control_api::{self, AppState};
use receiver::ui_events::ReceiverUiEvent;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager, State};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

// ---------------------------------------------------------------------------
// Result alias for Tauri commands
// ---------------------------------------------------------------------------

type CmdResult<T> = Result<T, String>;

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

// ---------------------------------------------------------------------------
// Event bridge: forward ReceiverUiEvent -> Tauri frontend events
// ---------------------------------------------------------------------------

fn spawn_event_bridge(app_handle: tauri::AppHandle, state: &Arc<AppState>) {
    let rx = state.ui_tx.subscribe();
    let handle = app_handle.clone();
    tokio::spawn(async move {
        let mut stream = BroadcastStream::new(rx);
        while let Some(Ok(event)) = stream.next().await {
            let event_name = match &event {
                ReceiverUiEvent::Resync => "resync",
                ReceiverUiEvent::StatusChanged { .. } => "status_changed",
                ReceiverUiEvent::StreamsSnapshot { .. } => "streams_snapshot",
                ReceiverUiEvent::LogEntry { .. } => "log_entry",
                ReceiverUiEvent::StreamCountsUpdated { .. } => "stream_counts_updated",
                ReceiverUiEvent::ModeChanged { .. } => "mode_changed",
                ReceiverUiEvent::LastRead(_) => "last_read",
            };
            let _ = handle.emit(event_name, &event);
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


    tauri::Builder::default()
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

            // Initialize receiver runtime
            let (state, shutdown_rx) =
                tauri::async_runtime::block_on(async { receiver::runtime::init(None).await })
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

            // Register state for commands
            app.manage(state.clone());

            // Start event bridge
            spawn_event_bridge(handle, &state);

            // Spawn receiver runtime
            tokio::spawn(receiver::runtime::run(state, shutdown_rx));

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

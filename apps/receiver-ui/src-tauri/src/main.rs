#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;

const RECEIVER_URL: &str = "http://127.0.0.1:9090";
const DEV_URL: &str = "http://127.0.0.1:5173";
const HEALTH_URL: &str = "http://127.0.0.1:9090/api/v1/version";
const HEALTH_POLL_INTERVAL_MS: u64 = 200;
const HEALTH_TIMEOUT_MS: u64 = 10_000;
const MAX_RESTART_ATTEMPTS: u32 = 3;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
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

            // Spawn sidecar lifecycle
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = run_sidecar_lifecycle(&handle).await {
                    // Log to both stderr and a file for packaged Windows builds
                    // where the console may not be visible.
                    let msg = format!("Fatal: failed to start receiver: {e}");
                    eprintln!("{msg}");
                    // Write to a crash log next to the app data directory.
                    if let Ok(dir) = handle.path().app_local_data_dir() {
                        let log_path = dir.join("crash.log");
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::write(&log_path, &msg);
                    }
                    handle.exit(1);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

async fn run_sidecar_lifecycle(handle: &AppHandle) -> Result<(), String> {
    let mut attempts = 0;

    loop {
        attempts += 1;
        if attempts > MAX_RESTART_ATTEMPTS {
            return Err(format!(
                "Receiver failed to start after {MAX_RESTART_ATTEMPTS} attempts"
            ));
        }

        if attempts > 1 {
            eprintln!("Restarting receiver (attempt {attempts}/{MAX_RESTART_ATTEMPTS})...");
        }

        // Spawn the sidecar
        // Basename only: `externalBin` is `binaries/receiver`, but the bundler installs
        // the sidecar as `receiver.exe` next to the main exe (no `binaries/` prefix).
        let (mut rx, child) = handle
            .shell()
            .sidecar("receiver")
            .map_err(|e| format!("Failed to create sidecar command: {e}"))?
            .args(["--no-open-browser"])
            .spawn()
            .map_err(|e| format!("Failed to spawn receiver: {e}"))?;

        // Monitor sidecar stdout/stderr in background
        tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    tauri_plugin_shell::process::CommandEvent::Stdout(line) => {
                        print!("[receiver] {}", String::from_utf8_lossy(&line));
                    }
                    tauri_plugin_shell::process::CommandEvent::Stderr(line) => {
                        eprint!("[receiver] {}", String::from_utf8_lossy(&line));
                    }
                    _ => {}
                }
            }
        });

        // Wait for the receiver to be healthy
        match wait_for_healthy().await {
            Ok(()) => {}
            Err(e) => {
                let _ = child.kill();
                eprintln!("Health check failed: {e}");
                continue;
            }
        }

        // Create the main window
        // In dev mode, point to Vite dev server for SvelteKit hot-reload.
        // In release mode, point to the receiver's embedded SPA.
        let url = if cfg!(debug_assertions) {
            DEV_URL
        } else {
            RECEIVER_URL
        };
        let window = tauri::WebviewWindowBuilder::new(
            handle,
            "main",
            tauri::WebviewUrl::External(url.parse().unwrap()),
        )
        .title("Rusty Timer Receiver")
        .inner_size(1200.0, 800.0)
        .build()
        .map_err(|e| format!("Failed to create window: {e}"))?;

        // Wait for the window to be closed
        let (tx, rx_close) = tokio::sync::oneshot::channel::<()>();
        let tx = std::sync::Mutex::new(Some(tx));
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::Destroyed = event
                && let Ok(mut guard) = tx.lock()
                && let Some(sender) = guard.take()
            {
                let _ = sender.send(());
            }
        });

        let _ = rx_close.await;

        // Kill the sidecar on window close
        let _ = child.kill();
        handle.exit(0);
        return Ok(());
    }
}

async fn wait_for_healthy() -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(HEALTH_TIMEOUT_MS);

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("Receiver did not become healthy within 10 seconds. \
                 Port 9090 may be in use by another process."
                .to_string());
        }

        match client.get(HEALTH_URL).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(HEALTH_POLL_INTERVAL_MS)).await;
            }
        }
    }
}

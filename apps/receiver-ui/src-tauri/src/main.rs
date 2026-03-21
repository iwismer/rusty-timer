#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::CommandEvent;

const RECEIVER_URL: &str = "http://127.0.0.1:9090";
const DEV_URL: &str = "http://127.0.0.1:5173";
/// Prefer `version`, then `status` — both return 200 when the control API is up.
const HEALTH_URLS: &[&str] = &[
    "http://127.0.0.1:9090/api/v1/version",
    "http://127.0.0.1:9090/api/v1/status",
];
const HEALTH_POLL_INTERVAL_MS: u64 = 200;
const HEALTH_TIMEOUT_MS: u64 = 30_000;
const MAX_STDERR_CAPTURE: usize = 8192;
const MAX_RESTART_ATTEMPTS: u32 = 3;

fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls CryptoProvider");

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
    let mut last_failure = String::new();

    loop {
        attempts += 1;
        if attempts > MAX_RESTART_ATTEMPTS {
            return Err(format!(
                "Receiver failed to start after {MAX_RESTART_ATTEMPTS} attempts. Last error: {last_failure}"
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

        let stderr_capture: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

        // Monitor sidecar stdout/stderr in background
        let cap = Arc::clone(&stderr_capture);
        tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    CommandEvent::Stdout(line) => {
                        print!("[receiver] {}", String::from_utf8_lossy(&line));
                    }
                    CommandEvent::Stderr(line) => {
                        eprint!("[receiver] {}", String::from_utf8_lossy(&line));
                        if let Ok(mut g) = cap.lock() {
                            let room = MAX_STDERR_CAPTURE.saturating_sub(g.len());
                            if room > 0 {
                                g.extend_from_slice(&line[..line.len().min(room)]);
                            }
                        }
                    }
                    CommandEvent::Terminated(p) => {
                        eprintln!("[receiver] process exited with code {:?}", p.code);
                    }
                    CommandEvent::Error(e) => {
                        eprintln!("[receiver] command error: {e}");
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
                let tail = stderr_capture
                    .lock()
                    .map(|g| String::from_utf8_lossy(&g).into_owned())
                    .unwrap_or_default();
                let detail = if tail.trim().is_empty() {
                    e
                } else {
                    format!("{e}\nReceiver output (stderr tail):\n{}", tail.trim())
                };
                last_failure = detail.clone();
                eprintln!("Health check failed: {detail}");
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
    // Disable system proxy: on Windows, HTTP_PROXY / corporate proxy can break
    // requests to 127.0.0.1 and cause spurious health-check failures.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .no_proxy()
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(HEALTH_TIMEOUT_MS);

    let mut last_detail = String::new();

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Receiver did not become healthy within {}s. Last poll: {}. \
                 If another app uses port 9090, quit it or stop the standalone receiver.exe.",
                HEALTH_TIMEOUT_MS / 1000,
                if last_detail.is_empty() {
                    "(no successful HTTP response)".to_string()
                } else {
                    last_detail
                }
            ));
        }

        last_detail.clear();
        for url in HEALTH_URLS {
            match client.get(*url).send().await {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                Ok(resp) => {
                    last_detail.push_str(&format!("{} -> HTTP {}; ", url, resp.status().as_u16()));
                }
                Err(e) => {
                    last_detail.push_str(&format!("{} -> {e}; ", url));
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(HEALTH_POLL_INTERVAL_MS)).await;
    }
}

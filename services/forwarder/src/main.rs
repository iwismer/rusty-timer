// forwarder: Reads from IPICO timing hardware and serves events over P2P iroh.
//
// Runtime event loop: wires together journal, local fanout, IPICO TCP readers,
// the P2P endpoint, and the status HTTP server.

use forwarder::discovery::expand_target;
use forwarder::local_fanout::FanoutServer;
use forwarder::status_http::{
    ConfigState, ForwarderStatusEvent, ReaderConnectionState, StatusConfig, StatusServer,
    SubsystemStatus,
};
use forwarder::storage::journal::Journal;
use rt_ui_log::UiLogLevel;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, watch};
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

// run_reader is used by main() in production; NOT cfg(test)-gated.
use forwarder::reader_task::run_reader;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive the forwarder_id from the raw token bytes.
///
/// SHA-256 hex of token bytes, first 16 hex chars, prefixed with "fwd-".
fn derive_forwarder_id(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    // sha2 0.11 digests no longer implement `LowerHex`; hex-encode manually.
    let hex: String = result.iter().map(|b| format!("{b:02x}")).collect();
    format!("fwd-{}", &hex[..16])
}

/// Detect the local IP used to reach a given target IP.
///
/// Uses a UDP socket connect (no traffic sent) to let the OS choose the
/// outgoing interface, then reads back the local address.
fn detect_local_ip(target_ip: &str) -> Option<String> {
    let dest = format!("{}:10000", target_ip);
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(&dest).ok()?;
    let local_addr = socket.local_addr().ok()?;
    Some(local_addr.ip().to_string())
}

/// Read CPU temperature from the Linux thermal zone.
///
/// Returns `None` on non-Linux platforms or if the file cannot be read.
#[cfg(any(feature = "eink", feature = "lcd"))]
fn read_cpu_temp() -> Option<f32> {
    let content = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?;
    let millidegrees: f32 = content.trim().parse().ok()?;
    Some(millidegrees / 1000.0)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls CryptoProvider");

    // Initialize tracing subscriber for structured logging to stdout.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "forwarder starting");

    // Parse optional --config <path> argument.
    // Defaults to /etc/rusty-timer/forwarder.toml when not supplied.
    let args: Vec<String> = std::env::args().collect();
    let config_path = match args.iter().position(|a| a == "--config") {
        Some(i) => match args.get(i + 1) {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                eprintln!("FATAL: --config requires a path argument");
                std::process::exit(1);
            }
        },
        None => std::path::PathBuf::from("/etc/rusty-timer/forwarder.toml"),
    };

    let cfg = match forwarder::config::load_config_from_path(&config_path) {
        Ok(cfg) => {
            info!(readers = cfg.readers.len(), "config loaded");
            cfg
        }
        Err(e) => {
            eprintln!("FATAL: failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Derive forwarder_id from token
    let forwarder_id = derive_forwarder_id(&cfg.token);
    info!(forwarder_id = %forwarder_id, "forwarder identity derived");

    // Open journal
    let journal_path = Path::new(&cfg.journal.sqlite_path);
    let journal = match Journal::open(journal_path) {
        Ok(j) => {
            info!(path = %cfg.journal.sqlite_path, "journal opened");
            Arc::new(Mutex::new(j))
        }
        Err(e) => {
            eprintln!("FATAL: failed to open journal: {}", e);
            std::process::exit(1);
        }
    };

    // Set up shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Start status HTTP server (not-ready initially)
    let status_cfg = StatusConfig {
        bind: cfg.status_http.bind.clone(),
        forwarder_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let subsystem = SubsystemStatus::not_ready("starting".to_owned());
    let config_state = Arc::new(ConfigState::new(config_path.clone()));
    let restart_signal = Arc::new(Notify::new());
    #[allow(unused_mut)]
    let mut status_server = match StatusServer::start_with_config(
        status_cfg,
        subsystem,
        journal.clone(),
        config_state.clone(),
        restart_signal.clone(),
    )
    .await
    {
        Ok(s) => {
            info!(addr = %s.local_addr(), "status HTTP server started");
            s
        }
        Err(e) => {
            eprintln!("FATAL: failed to start status HTTP server: {}", e);
            std::process::exit(1);
        }
    };
    status_server.set_update_mode(cfg.update.mode).await;
    let logger = status_server.logger();

    // Collect enabled reader endpoints
    let mut all_readers: Vec<(String, u16)> = Vec::new(); // (addr, local_port)
    let mut fanout_addrs: Vec<(String, u16, SocketAddr)> = Vec::new(); // (ip, port, fanout_addr)

    for reader_cfg in &cfg.readers {
        if !reader_cfg.enabled {
            info!(target = %reader_cfg.target, "reader disabled, skipping");
            continue;
        }

        let endpoints = match expand_target(&reader_cfg.target) {
            Ok(eps) => eps,
            Err(e) => {
                eprintln!(
                    "FATAL: invalid reader target '{}': {}",
                    reader_cfg.target, e
                );
                std::process::exit(1);
            }
        };

        for ep in endpoints {
            let local_port = reader_cfg
                .local_fallback_port
                .unwrap_or_else(|| ep.default_local_fallback_port());

            let bind_addr = format!("0.0.0.0:{}", local_port);
            let fanout = match FanoutServer::bind(&bind_addr).await {
                Ok(f) => f,
                Err(e) => {
                    eprintln!(
                        "FATAL: failed to bind fanout for {} on port {}: {}",
                        ep.ip, local_port, e
                    );
                    std::process::exit(1);
                }
            };

            let fanout_addr = fanout.local_addr();
            info!(
                reader_ip = %ep.ip,
                local_port = local_port,
                "local fanout listener started"
            );

            // Spawn the fanout accept loop
            tokio::spawn(async move {
                fanout.run().await;
            });

            all_readers.push((ep.addr(), local_port));
            fanout_addrs.push((ep.ip, ep.port, fanout_addr));
        }
    }

    // Initialize reader status tracking
    status_server.init_readers(&all_readers).await;

    if all_readers.is_empty() {
        eprintln!("FATAL: no enabled readers configured");
        std::process::exit(1);
    }

    // Restore stream identity for any journal-missing stream keys BEFORE
    // `start_forwarder_p2p` (which idempotently seeds advertised streams at
    // seq 1 and would pre-empt a restore) and before reader tasks spawn.
    // Receivers dedup on (stream_id, seq); if the journal was lost, restarting
    // an existing stream key at seq 1 would make them silently discard reads.
    // See `forwarder::storage::restore` for the high-water + slack rationale.
    {
        use forwarder::storage::restore::{
            RegistryFetch, RegistryStreamRecord, fetch_registry_snapshot_with_retries,
            restore_streams_at_startup,
        };

        let stream_keys: Vec<String> = all_readers.iter().map(|(addr, _)| addr.clone()).collect();
        let any_missing = {
            let j = journal.lock().await;
            stream_keys
                .iter()
                .any(|key| !j.stream_exists(key).unwrap_or(false))
        };
        if any_missing {
            let fetch = if cfg.p2p.enabled && cfg.p2p.server_url.is_some() {
                // Reuse the persisted minted device token (it lives at a
                // different path than the journal, so it typically survives a
                // journal-loss reboot). Never bootstrap here: minting needs the
                // P2P endpoint id, which does not exist yet. A missing token is
                // treated as "registry unavailable" (loud fallback); on a true
                // first boot the journal is expected to be empty anyway.
                let token_path = forwarder::p2p::device_token_path(&cfg.p2p);
                match forwarder::p2p::read_device_token(&token_path) {
                    Ok(Some(device_token)) => {
                        let client = forwarder::p2p::ServerCatalogClient::with_timeout(
                            cfg.p2p.server_url.clone().unwrap_or_default(),
                            device_token,
                            Duration::from_secs(cfg.p2p.allowlist_request_timeout_secs),
                        );
                        fetch_registry_snapshot_with_retries(
                            || {
                                let client = client.clone();
                                async move {
                                    client.fetch_own_catalog().await.map(|streams| {
                                        streams
                                            .into_iter()
                                            .map(|s| RegistryStreamRecord {
                                                stream_id: s.stream_id,
                                                epoch: s.epoch,
                                                next_seq: s.next_seq,
                                            })
                                            .collect()
                                    })
                                }
                            },
                            3,
                            Duration::from_secs(5),
                        )
                        .await
                    }
                    Ok(None) => {
                        warn!(
                            path = %token_path.display(),
                            "no persisted device token; cannot fetch registry high-water for stream restore"
                        );
                        RegistryFetch::Unavailable
                    }
                    Err(e) => {
                        warn!(
                            path = %token_path.display(),
                            error = %e,
                            "failed to read device token for stream restore"
                        );
                        RegistryFetch::Unavailable
                    }
                }
            } else {
                RegistryFetch::NotConfigured
            };

            let restore_result = {
                let mut j = journal.lock().await;
                restore_streams_at_startup(&mut j, &stream_keys, &fetch, Some(logger.as_ref()))
            };
            if let Err(e) = restore_result {
                // Reader tasks retry `ensure_stream_state` as a safety net, so
                // local capture still proceeds; but a failed restore means a
                // previously forwarded stream key may restart at seq 1.
                error!(error = %e, "startup stream identity restore failed");
                logger.log_at(
                    UiLogLevel::Error,
                    format!("stream identity restore failed: {e}"),
                );
            }
        }
    }

    // Seed historical totals once at startup to avoid per-request DB counting.
    for (reader_addr, _) in &all_readers {
        let total = {
            let j = journal.lock().await;
            match j.event_count(reader_addr) {
                Ok(count) => count,
                Err(e) => {
                    warn!(reader_ip = %reader_addr, error = %e, "failed to load reader total");
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} historical total unavailable: {}", reader_addr, e),
                    );
                    0
                }
            }
        };
        status_server.set_reader_total(reader_addr, total).await;
    }

    // Set forwarder identity on status page
    status_server.set_forwarder_id(&forwarder_id).await;

    // Detect local IP from first reader
    let detect_target = all_readers.first().map(|(addr, _)| {
        addr.rsplit_once(':')
            .map(|(ip, _)| ip)
            .unwrap_or(addr)
            .to_owned()
    });
    let local_ip = detect_target.as_deref().and_then(detect_local_ip);
    if let Some(ref ip) = local_ip {
        info!(local_ip = %ip, "detected local IP");
    }
    status_server.set_local_ip(local_ip.clone()).await;

    // Re-detect the local IP whenever a reader connects or disconnects. The
    // startup detection above is a one-shot snapshot: if the interface facing
    // the reader (e.g. a direct Ethernet cable) has no carrier yet, the kernel
    // routes via the default interface (often WiFi) and the wrong IP would
    // otherwise stay on the status screen forever. Reader state transitions are
    // the natural signal that routing may have changed: a direct link coming up
    // is immediately followed by a successful connect.
    if let Some(target) = detect_target {
        let status = status_server.clone();
        let mut shutdown = shutdown_rx.clone();
        let mut last_ip = local_ip;
        tokio::spawn(async move {
            let (mut status_rx, _snapshot) = status.status_feed().subscribe_and_snapshot().await;
            loop {
                tokio::select! {
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            return;
                        }
                    }
                    event = status_rx.recv() => match event {
                        Ok(ForwarderStatusEvent::ReaderStatus { status: reader, .. })
                            if reader.state != ReaderConnectionState::Connecting =>
                        {
                            let detected = detect_local_ip(&target);
                            if detected != last_ip {
                                info!(
                                    local_ip = detected.as_deref().unwrap_or("none"),
                                    "local IP changed, updating status"
                                );
                                last_ip = detected.clone();
                                status.set_local_ip(detected).await;
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        });
    }

    let remote_config_handler = Arc::new(forwarder::p2p::ForwarderRemoteConfigHandler::new(
        cfg.control.allow_remote_config,
        config_state.clone(),
        status_server.subsystem_arc(),
        status_server.ui_sender(),
        status_server.logger(),
        restart_signal.clone(),
    ));
    let reader_control_handler = Arc::new(forwarder::p2p::ForwarderReaderControlHandler::new(
        cfg.control.allow_reader_control,
        status_server.reader_control_service(),
        Arc::clone(&journal),
    ));
    let p2p_runtime = match forwarder::p2p::start_forwarder_p2p(
        &cfg.p2p,
        Arc::clone(&journal),
        &all_readers
            .iter()
            .map(|(addr, _)| addr.clone())
            .collect::<Vec<_>>(),
        cfg.display_name.clone(),
        status_server.status_feed(),
        remote_config_handler,
        reader_control_handler,
    )
    .await
    {
        Ok(Some(runtime)) => {
            status_server
                .set_p2p_endpoint_id(runtime.endpoint_id().to_string())
                .await;
            status_server.set_p2p_connected(true).await;
            let endpoint_addr = runtime.endpoint_addr().await;
            info!(
                p2p_endpoint_id = %runtime.endpoint_id(),
                p2p_endpoint_addr = ?endpoint_addr,
                "p2p iroh server started"
            );
            Some(runtime)
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("FATAL: failed to start P2P endpoint: {e}");
            std::process::exit(1);
        }
    };

    // --- Screen display (optional, compile-time gated) ---
    // Backend-neutral: the effective `cfg.screen` selects e-ink or LCD at
    // runtime; the compiled backend features gate which hardware drivers exist.
    #[cfg(any(feature = "eink", feature = "lcd"))]
    {
        if let Some(ref screen) = cfg.screen {
            if screen.enabled {
                info!(
                    backend = ?screen.backend,
                    source = "screen config",
                    "screen display enabled, initializing"
                );
                let (display_tx, display_rx) =
                    tokio::sync::watch::channel(rt_screen::state::DisplayState::initial());

                status_server.set_display_sender(display_tx);
                status_server
                    .set_display_name(cfg.display_name.clone())
                    .await;

                // Spawn CPU temperature polling task. The polling interval comes
                // from the active backend's telemetry cadence.
                let temp_interval_secs = match screen.backend {
                    rt_screen::state::ScreenBackend::Lcd => screen.lcd.telemetry_interval_secs,
                    rt_screen::state::ScreenBackend::Eink => screen.eink.telemetry_interval_secs,
                };
                let ss_temp = status_server.clone();
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_secs(temp_interval_secs));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        let temp = read_cpu_temp();
                        ss_temp.set_cpu_temp_cached(temp).await;
                    }
                });

                match screen.backend {
                    rt_screen::state::ScreenBackend::Eink => {
                        #[cfg(all(feature = "eink", target_os = "linux"))]
                        {
                            let eink_cfg = screen.eink.clone();
                            let eink_shutdown_rx = shutdown_rx.clone();
                            tokio::spawn(async move {
                                match rt_screen::eink::driver::EinkDriver::new() {
                                    Ok(mut driver) => {
                                        tracing::info!(
                                            "e-ink driver initialized, starting display task"
                                        );
                                        let mut consecutive_errors: u32 = 0;
                                        rt_screen::task::run_screen_task(
                                            display_rx,
                                            eink_shutdown_rx,
                                            rt_screen::task::ScreenTaskConfig::from(eink_cfg),
                                            |state, full| {
                                                use embedded_graphics::pixelcolor::BinaryColor;
                                                use embedded_graphics::prelude::*;
                                                let refresh_type = if full { "full" } else { "partial" };
                                                let mut display = driver.display_mut().color_converted();
                                                display.clear(BinaryColor::Off).ok();
                                                if let Err(e) =
                                                    rt_screen::eink::render::render_display(&mut display, state)
                                                {
                                                    consecutive_errors += 1;
                                                    tracing::warn!(
                                                        error = %e,
                                                        refresh_type,
                                                        consecutive_errors,
                                                        total_reads = state.total_reads,
                                                        "eink: render failed, skipping refresh"
                                                    );
                                                    return;
                                                }
                                                let result = if full {
                                                    driver.full_refresh()
                                                } else {
                                                    driver.partial_refresh()
                                                };
                                                match result {
                                                    Ok(()) => {
                                                        if consecutive_errors > 0 {
                                                            tracing::info!(
                                                                previous_errors = consecutive_errors,
                                                                "eink: refresh succeeded after previous errors"
                                                            );
                                                        }
                                                        consecutive_errors = 0;
                                                    }
                                                    Err(e) => {
                                                        consecutive_errors += 1;
                                                        tracing::warn!(
                                                            error = %e,
                                                            refresh_type,
                                                            consecutive_errors,
                                                            "eink: refresh failed"
                                                        );
                                                    }
                                                }
                                            },
                                        )
                                        .await;
                                        // Show "Powered Off" on the display before sleeping.
                                        {
                                            use embedded_graphics::pixelcolor::BinaryColor;
                                            use embedded_graphics::prelude::*;
                                            let mut display =
                                                driver.display_mut().color_converted();
                                            display.clear(BinaryColor::Off).ok();
                                            if let Err(e) = rt_screen::eink::render::render_shutdown(
                                                &mut display,
                                            ) {
                                                tracing::warn!(error = %e, "eink: failed to render shutdown screen");
                                            } else if let Err(e) = driver.full_refresh() {
                                                tracing::warn!(error = %e, "eink: failed to refresh shutdown screen");
                                            }
                                        }
                                        if let Err(e) = driver.sleep() {
                                            tracing::warn!(error = %e, "eink: failed to sleep display on shutdown");
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            error = %e,
                                            "e-ink display init failed (continuing without display) — \
                                             check SPI enabled, HAT seated, and pin wiring"
                                        );
                                    }
                                }
                            });
                            info!("e-ink display task spawned");
                        }

                        #[cfg(all(feature = "eink", not(target_os = "linux")))]
                        {
                            let eink_cfg = screen.eink.clone();
                            let eink_shutdown_rx = shutdown_rx.clone();
                            tokio::spawn(async move {
                                rt_screen::task::run_screen_task(
                                    display_rx,
                                    eink_shutdown_rx,
                                    rt_screen::task::ScreenTaskConfig::from(eink_cfg),
                                    |_state, _full| {},
                                )
                                .await;
                            });
                            warn!(
                                "e-ink hardware updates are only supported on Linux; using no-op renderer"
                            );
                        }

                        #[cfg(not(feature = "eink"))]
                        {
                            let _ = display_rx;
                            warn!(
                                "screen.backend = \"eink\" but this build was not compiled with the `eink` feature; continuing without a display"
                            );
                        }
                    }
                    rt_screen::state::ScreenBackend::Lcd => {
                        #[cfg(all(feature = "lcd", target_os = "linux"))]
                        {
                            let lcd_cfg = screen.lcd.clone();
                            let shutdown = shutdown_rx.clone();
                            tokio::spawn(async move {
                                match rt_screen::lcd::driver::LcdDriver::new(&lcd_cfg) {
                                    Ok(mut driver) => {
                                        tracing::info!(
                                            "lcd driver initialized, starting display task"
                                        );
                                        let mut consecutive_errors: u32 = 0;
                                        let mut backlight_on = false;
                                        rt_screen::task::run_screen_task(
                                            display_rx,
                                            shutdown,
                                            rt_screen::task::ScreenTaskConfig::from(lcd_cfg.clone()),
                                            |state, _full| {
                                                let start = std::time::Instant::now();
                                                // Compose the whole frame in RAM (infallible), then
                                                // blit it to the panel in one pass via `flush` so the
                                                // operator never sees the clear-to-black + redraw.
                                                let _ = rt_screen::lcd::render::render_display(
                                                    driver.framebuffer_mut(),
                                                    state,
                                                );
                                                if let Err(e) = driver.flush() {
                                                    consecutive_errors += 1;
                                                    tracing::warn!(error = ?e, consecutive_errors, "lcd: flush failed");
                                                    return;
                                                }
                                                if !backlight_on {
                                                    driver.set_backlight(true);
                                                    backlight_on = true;
                                                }
                                                if consecutive_errors > 0 {
                                                    tracing::info!(previous_errors = consecutive_errors, "lcd: refresh recovered");
                                                }
                                                consecutive_errors = 0;
                                                tracing::debug!(
                                                    refresh_latency_ms = start.elapsed().as_millis() as u64,
                                                    total_reads = state.total_reads,
                                                    "lcd: rendered"
                                                );
                                            },
                                        )
                                        .await;
                                        // On shutdown, turn the backlight off and sleep the panel.
                                        // (Unlike e-ink, an LCD shows nothing with the backlight
                                        // off, so there is no "Powered Off" screen to render.)
                                        driver.set_backlight(false);
                                        if let Err(e) = driver.sleep() {
                                            tracing::warn!(error = ?e, "lcd: failed to sleep display on shutdown");
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "lcd display init failed (continuing without display)");
                                    }
                                }
                            });
                            info!("lcd display task spawned");
                        }

                        #[cfg(all(feature = "lcd", not(target_os = "linux")))]
                        {
                            let lcd_cfg = screen.lcd.clone();
                            let shutdown = shutdown_rx.clone();
                            tokio::spawn(async move {
                                rt_screen::task::run_screen_task(
                                    display_rx,
                                    shutdown,
                                    rt_screen::task::ScreenTaskConfig::from(lcd_cfg),
                                    |_state, _full| {},
                                )
                                .await;
                            });
                            warn!("lcd hardware only on Linux; using no-op renderer");
                        }

                        #[cfg(not(feature = "lcd"))]
                        {
                            let _ = display_rx;
                            warn!(
                                "screen.backend = \"lcd\" but this build was not compiled with the `lcd` feature; continuing without a display"
                            );
                        }
                    }
                }
            } else {
                info!("screen display configured but disabled");
            }
        }
    }

    // Spawn reader tasks
    for (reader_ip, reader_port, fanout_addr) in fanout_addrs {
        let j = journal.clone();
        let rx = shutdown_rx.clone();
        let ss = status_server.clone();
        let lg = logger.clone();
        tokio::spawn(async move {
            run_reader(reader_ip, reader_port, fanout_addr, j, rx, ss, lg).await;
        });
    }

    // Spawn UPS monitoring task (if enabled)
    let ups_handle = if cfg.ups.enabled {
        let ss = status_server.clone();
        let rx = shutdown_rx.clone();
        let fwd_id = forwarder_id.clone();
        Some(forwarder::ups_task::spawn_ups_task(
            cfg.ups.clone(),
            fwd_id,
            ss,
            rx,
        ))
    } else {
        None
    };
    let _ups_status_rx = ups_handle.map(|h| h.ups_status_rx);

    // All worker tasks started — mark subsystem ready
    status_server.set_ready().await;

    let updater_stage_root = forwarder::updater_stage_root_dir();
    info!(
        stage_dir = %updater_stage_root.display(),
        "configured updater stage directory"
    );

    // Spawn background update check
    {
        let ss = status_server.clone();
        let update_mode = cfg.update.mode;
        let lg = logger.clone();
        let updater_stage_root = updater_stage_root.clone();
        tokio::spawn(async move {
            if update_mode == rt_updater::UpdateMode::Disabled {
                lg.log("auto-update disabled by configuration");
                return;
            }

            let checker = match rt_updater::UpdateChecker::new(
                "iwismer",
                "rusty-timer",
                "forwarder",
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(c) => c,
                Err(e) => {
                    lg.log_at(
                        UiLogLevel::Warn,
                        format!("failed to create update checker: {e}"),
                    );
                    return;
                }
            };

            let status = checker.check().await;
            match status {
                Ok(rt_updater::UpdateStatus::Available { ref version }) => {
                    lg.log(format!("Update v{version} available"));
                    ss.set_update_status(rt_updater::UpdateStatus::Available {
                        version: version.clone(),
                    })
                    .await;

                    if update_mode == rt_updater::UpdateMode::CheckAndDownload {
                        match checker
                            .download_with_stage_root(version, updater_stage_root.as_path())
                            .await
                        {
                            Ok(path) => {
                                lg.log(format!("Update v{version} downloaded and staged"));
                                ss.set_update_status(rt_updater::UpdateStatus::Downloaded {
                                    version: version.clone(),
                                })
                                .await;
                                ss.set_staged_update_path(path).await;
                            }
                            Err(e) => {
                                lg.log_at(UiLogLevel::Warn, format!("update download failed: {e}"));
                                ss.set_update_status(rt_updater::UpdateStatus::Failed {
                                    error: e.to_string(),
                                })
                                .await;
                            }
                        }
                    }
                }
                Ok(_) => {
                    lg.log("forwarder is up to date");
                }
                Err(e) => {
                    lg.log_at(UiLogLevel::Warn, format!("update check failed: {e}"));
                    ss.set_update_status(rt_updater::UpdateStatus::Failed {
                        error: e.to_string(),
                    })
                    .await;
                }
            }
        });
    }

    logger.log(format!(
        "forwarder v{} initialized — all workers running",
        env!("CARGO_PKG_VERSION")
    ));

    // Wait for Ctrl-C, SIGTERM, or restart request
    let restart_requested;
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                error!("failed to install SIGTERM handler: {}", e);
                tokio::signal::ctrl_c().await.ok();
                shutdown_tx.send(true).ok();
                return;
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                logger.log("shutdown: SIGINT received");
                restart_requested = false;
            }
            _ = sigterm.recv() => {
                logger.log("shutdown: SIGTERM received");
                restart_requested = false;
            }
            _ = restart_signal.notified() => {
                logger.log("restart requested via API");
                restart_requested = true;
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                logger.log("shutdown: Ctrl-C received");
            }
            _ = restart_signal.notified() => {
                logger.log("restart requested via API");
            }
        }
        restart_requested = false; // exec not available on non-unix
    }

    // Signal all tasks to stop
    shutdown_tx.send(true).ok();

    if let Some(runtime) = p2p_runtime {
        status_server.set_p2p_connected(false).await;
        runtime.shutdown().await;
    }

    // Brief delay to allow tasks to observe shutdown and flush
    sleep(Duration::from_millis(200)).await;

    info!("forwarder shutdown complete");

    // Self-exec to restart if requested
    #[cfg(unix)]
    if restart_requested {
        use std::os::unix::process::CommandExt;
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                error!("could not determine executable path: {}", e);
                std::process::exit(1);
            }
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        info!(exe = %exe.display(), "exec-ing self to restart");
        let err = std::process::Command::new(&exe).args(&args).exec();
        error!("exec failed: {}", err);
        std::process::exit(1);
    }
}

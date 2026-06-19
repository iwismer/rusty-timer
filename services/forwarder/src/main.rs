// forwarder: Reads from IPICO timing hardware and serves events over P2P iroh.
//
// Runtime event loop: wires together journal, local fanout, IPICO TCP readers,
// the P2P endpoint, and the status HTTP server.

mod reader_task;

use forwarder::discovery::expand_target;
use forwarder::local_fanout::FanoutServer;
use forwarder::status_http::{ConfigState, StatusConfig, StatusServer, SubsystemStatus};
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
use reader_task::run_reader;

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
    let hex = format!("{:x}", result);
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
#[cfg(feature = "eink")]
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
    let local_ip = all_readers.first().and_then(|(addr, _)| {
        let ip = addr.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(addr);
        detect_local_ip(ip)
    });
    if let Some(ref ip) = local_ip {
        info!(local_ip = %ip, "detected local IP");
    }
    status_server.set_local_ip(local_ip).await;

    let p2p_runtime = match forwarder::p2p::start_forwarder_p2p(
        &cfg.p2p,
        Arc::clone(&journal),
        &all_readers
            .iter()
            .map(|(addr, _)| addr.clone())
            .collect::<Vec<_>>(),
        cfg.display_name.clone(),
        status_server.status_feed(),
    )
    .await
    {
        Ok(Some(runtime)) => {
            status_server
                .set_p2p_endpoint_id(runtime.node_id().to_string())
                .await;
            status_server.set_p2p_connected(true).await;
            let node_addr = runtime.node_addr().await;
            info!(
                p2p_node_id = %runtime.node_id(),
                p2p_node_addr = ?node_addr,
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

    // --- E-ink display (optional, compile-time gated) ---
    #[cfg(feature = "eink")]
    {
        if let Some(ref eink_config) = cfg.eink {
            if eink_config.enabled {
                info!(
                    refresh_mode = ?eink_config.refresh_mode,
                    full_refresh_interval = eink_config.full_refresh_interval,
                    min_refresh_interval_ms = eink_config.min_refresh_interval_ms,
                    telemetry_interval_secs = eink_config.telemetry_interval_secs,
                    "e-ink display enabled, initializing"
                );
                let (display_tx, display_rx) =
                    tokio::sync::watch::channel(rt_eink::state::DisplayState::initial());

                status_server.set_display_sender(display_tx);
                status_server
                    .set_display_name(cfg.display_name.clone())
                    .await;

                // Spawn CPU temperature polling task.
                let temp_interval_secs = eink_config.telemetry_interval_secs;
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

                // Spawn the e-ink display task.
                let eink_cfg = eink_config.clone();
                let eink_shutdown_rx = shutdown_rx.clone();
                #[cfg(target_os = "linux")]
                {
                    tokio::spawn(async move {
                        match rt_eink::driver::EinkDriver::new() {
                            Ok(mut driver) => {
                                tracing::info!("e-ink driver initialized, starting display task");
                                let mut consecutive_errors: u32 = 0;
                                rt_eink::task::run_eink_task(
                                    display_rx,
                                    eink_shutdown_rx,
                                    eink_cfg,
                                    |state, full| {
                                        use embedded_graphics::pixelcolor::BinaryColor;
                                        use embedded_graphics::prelude::*;
                                        let refresh_type = if full { "full" } else { "partial" };
                                        let mut display = driver.display_mut().color_converted();
                                        display.clear(BinaryColor::Off).ok();
                                        if let Err(e) =
                                            rt_eink::render::render_display(&mut display, state)
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
                                    let mut display = driver.display_mut().color_converted();
                                    display.clear(BinaryColor::Off).ok();
                                    if let Err(e) = rt_eink::render::render_shutdown(&mut display) {
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
                }

                #[cfg(not(target_os = "linux"))]
                {
                    tokio::spawn(async move {
                        rt_eink::task::run_eink_task(
                            display_rx,
                            eink_shutdown_rx,
                            eink_cfg,
                            |_state, _full| {},
                        )
                        .await;
                    });
                    warn!(
                        "e-ink hardware updates are only supported on Linux; using no-op renderer"
                    );
                }

                info!("e-ink display task spawned");
            } else {
                info!("e-ink display configured but disabled");
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

// forwarder: Reads from IPICO timing hardware and forwards events to the server.
//
// Runtime event loop: wires together journal, local fanout, IPICO TCP readers,
// WebSocket uplink/replay, and the status HTTP server.

use forwarder::config::ForwarderConfig;
use forwarder::discovery::expand_target;
use forwarder::local_fanout::FanoutServer;
use forwarder::replay::ReplayEngine;
use forwarder::status_http::{StatusConfig, StatusServer, SubsystemStatus};
use forwarder::storage::journal::Journal;
use forwarder::uplink::{UplinkConfig, UplinkSession};
use ipico_core::read::ChipRead;
use rt_protocol::{ReadEvent, ResumeCursor};
use sha2::{Digest, Sha256};
use std::convert::TryFrom;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{watch, Mutex};
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

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

// ---------------------------------------------------------------------------
// Reader task: TCP connect → parse IPICO frames → journal + fanout
// ---------------------------------------------------------------------------

async fn run_reader(
    reader_ip: String,
    reader_port: u16,
    read_type_str: String,
    fanout_addr: SocketAddr,
    journal: Arc<Mutex<Journal>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let target_addr = format!("{}:{}", reader_ip, reader_port);
    let stream_key = reader_ip.clone();
    let mut backoff_secs: u64 = 1;

    loop {
        // Check for shutdown before attempting connect
        if *shutdown_rx.borrow() {
            info!(reader_ip = %reader_ip, "reader task stopping (shutdown)");
            return;
        }

        info!(reader_ip = %reader_ip, target = %target_addr, "connecting to reader");

        let stream = match TcpStream::connect(&target_addr).await {
            Ok(s) => {
                info!(reader_ip = %reader_ip, "reader TCP connected");
                backoff_secs = 1; // reset backoff on successful connect
                s
            }
            Err(e) => {
                warn!(
                    reader_ip = %reader_ip,
                    error = %e,
                    backoff_secs = backoff_secs,
                    "reader TCP connect failed, retrying"
                );
                let delay = Duration::from_secs(backoff_secs);
                tokio::select! {
                    _ = sleep(delay) => {}
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            return;
                        }
                    }
                }
                backoff_secs = (backoff_secs * 2).min(30);
                continue;
            }
        };

        // Initialize stream state in journal on first connect
        {
            let mut j = journal.lock().await;
            // Current Unix timestamp seconds as initial epoch
            let epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if let Err(e) = j.ensure_stream_state(&stream_key, epoch) {
                warn!(reader_ip = %reader_ip, error = %e, "ensure_stream_state failed");
            }
        }

        let mut reader = BufReader::new(stream);
        let mut line_buf = String::new();

        loop {
            line_buf.clear();

            // Wait for a line or shutdown
            let read_result = tokio::select! {
                result = reader.read_line(&mut line_buf) => result,
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!(reader_ip = %reader_ip, "reader task stopping (shutdown)");
                        return;
                    }
                    continue;
                }
            };

            match read_result {
                Err(e) => {
                    warn!(reader_ip = %reader_ip, error = %e, "TCP read error, reconnecting");
                    break;
                }
                Ok(0) => {
                    warn!(reader_ip = %reader_ip, "TCP connection closed by reader, reconnecting");
                    break;
                }
                Ok(_) => {}
            }

            let raw_line = line_buf.trim_end_matches(['\r', '\n']).to_owned();
            if raw_line.is_empty() {
                continue;
            }

            // Parse IPICO chip read to validate and extract metadata
            let parsed_read_type;
            let reader_timestamp;
            match ChipRead::try_from(raw_line.as_str()) {
                Ok(chip) => {
                    reader_timestamp = Some(chip.timestamp.to_string());
                    parsed_read_type = read_type_str.clone();
                }
                Err(_) => {
                    // Line is not a valid IPICO read — log and skip
                    warn!(reader_ip = %reader_ip, line = %raw_line, "skipping unparseable line");
                    continue;
                }
            }

            // Write to journal
            let (epoch, seq) = {
                let mut j = journal.lock().await;
                let (epoch, _) = match j.current_epoch_and_next_seq(&stream_key) {
                    Ok(v) => v,
                    Err(e) => {
                        error!(reader_ip = %reader_ip, error = %e, "failed to get epoch");
                        break;
                    }
                };
                let seq = match j.next_seq(&stream_key) {
                    Ok(s) => s,
                    Err(e) => {
                        error!(reader_ip = %reader_ip, error = %e, "failed to get next_seq");
                        break;
                    }
                };
                if let Err(e) = j.insert_event(
                    &stream_key,
                    epoch,
                    seq,
                    reader_timestamp.as_deref(),
                    &raw_line,
                    &parsed_read_type,
                ) {
                    error!(reader_ip = %reader_ip, error = %e, "journal insert failed");
                    break;
                }
                (epoch, seq)
            };

            info!(
                reader_ip = %reader_ip,
                epoch = epoch,
                seq = seq,
                "event journaled"
            );

            // Fan out raw bytes to local TCP consumers
            let raw_bytes = format!("{}\n", raw_line).into_bytes();
            if let Err(e) = FanoutServer::push_to_addr(fanout_addr, raw_bytes).await {
                warn!(reader_ip = %reader_ip, error = %e, "local fanout push failed");
                // Non-fatal: local fanout failure doesn't break uplink path
            }
        }

        // Reconnect with backoff
        let delay = Duration::from_secs(backoff_secs);
        info!(
            reader_ip = %reader_ip,
            backoff_secs = backoff_secs,
            "waiting before reconnect"
        );
        tokio::select! {
            _ = sleep(delay) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return;
                }
            }
        }
        backoff_secs = (backoff_secs * 2).min(30);
    }
}

// ---------------------------------------------------------------------------
// Uplink task: WebSocket connect → replay → send batches → receive acks
// ---------------------------------------------------------------------------

async fn run_uplink(
    cfg: ForwarderConfig,
    forwarder_id: String,
    reader_ips: Vec<String>,
    journal: Arc<Mutex<Journal>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let server_url = format!(
        "{}{}",
        cfg.server.base_url.trim_end_matches('/'),
        cfg.server.forwarders_ws_path
    );
    // Convert http(s) scheme to ws(s) scheme
    let ws_url = if server_url.starts_with("https://") {
        server_url.replacen("https://", "wss://", 1)
    } else if server_url.starts_with("http://") {
        server_url.replacen("http://", "ws://", 1)
    } else {
        server_url
    };

    let uplink_cfg = UplinkConfig {
        server_url: ws_url.clone(),
        token: cfg.token.clone(),
        forwarder_id: forwarder_id.clone(),
        batch_mode: cfg.uplink.batch_mode.clone(),
        batch_flush_ms: cfg.uplink.batch_flush_ms,
        batch_max_events: cfg.uplink.batch_max_events,
    };

    let mut backoff_secs: u64 = 1;

    loop {
        if *shutdown_rx.borrow() {
            info!("uplink task stopping (shutdown)");
            return;
        }

        // Build resume cursors from journal state
        let resume_cursors: Vec<ResumeCursor> = {
            let j = journal.lock().await;
            reader_ips
                .iter()
                .filter_map(|ip| match j.ack_cursor(ip) {
                    Ok((acked_epoch, acked_seq)) if acked_epoch > 0 => Some(ResumeCursor {
                        forwarder_id: forwarder_id.clone(),
                        reader_ip: ip.clone(),
                        stream_epoch: acked_epoch as u64,
                        last_seq: acked_seq as u64,
                    }),
                    _ => None,
                })
                .collect()
        };

        info!(
            url = %ws_url,
            cursors = resume_cursors.len(),
            "connecting uplink WebSocket"
        );

        let mut session = match UplinkSession::connect_with_resume(
            uplink_cfg.clone(),
            reader_ips.clone(),
            resume_cursors,
        )
        .await
        {
            Ok(s) => {
                info!(
                    session_id = %s.session_id(),
                    device_id = %s.device_id(),
                    "uplink connected"
                );
                backoff_secs = 1;
                s
            }
            Err(e) => {
                warn!(
                    error = %e,
                    backoff_secs = backoff_secs,
                    "uplink connect failed, retrying"
                );
                let delay = Duration::from_secs(backoff_secs);
                tokio::select! {
                    _ = sleep(delay) => {}
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            return;
                        }
                    }
                }
                backoff_secs = (backoff_secs * 2).min(30);
                continue;
            }
        };

        // Replay unacked events from journal
        let replay_results = {
            let j = journal.lock().await;
            let engine = ReplayEngine::new();
            let mut all: Vec<(String, i64, Vec<forwarder::storage::journal::JournalEvent>)> =
                Vec::new();
            for ip in &reader_ips {
                match engine.pending_events(&j, ip) {
                    Ok(results) => {
                        for rr in results {
                            all.push((rr.stream_key, rr.stream_epoch, rr.events));
                        }
                    }
                    Err(e) => {
                        warn!(reader_ip = %ip, error = %e, "replay engine error");
                    }
                }
            }
            all
        };

        // Send replayed events
        for (stream_key, stream_epoch, events) in replay_results {
            if events.is_empty() {
                continue;
            }
            let read_events: Vec<ReadEvent> = events
                .iter()
                .map(|ev| ReadEvent {
                    forwarder_id: forwarder_id.clone(),
                    reader_ip: ev.stream_key.clone(),
                    stream_epoch: ev.stream_epoch as u64,
                    seq: ev.seq as u64,
                    reader_timestamp: ev.reader_timestamp.clone().unwrap_or_default(),
                    raw_read_line: ev.raw_read_line.clone(),
                    read_type: ev.read_type.clone(),
                })
                .collect();

            info!(
                stream_key = %stream_key,
                epoch = stream_epoch,
                count = read_events.len(),
                "replaying unacked events"
            );

            match session.send_batch(read_events).await {
                Ok(ack) => {
                    let mut j = journal.lock().await;
                    for entry in &ack.entries {
                        if let Err(e) = j.update_ack_cursor(
                            &entry.reader_ip,
                            entry.stream_epoch as i64,
                            entry.last_seq as i64,
                        ) {
                            warn!(error = %e, "failed to update ack cursor");
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "send_batch (replay) failed, reconnecting");
                    break;
                }
            }
        }

        // Main uplink loop: periodically send new events and wait for acks
        let flush_interval = Duration::from_millis(cfg.uplink.batch_flush_ms);

        'uplink: loop {
            if *shutdown_rx.borrow() {
                info!("uplink task stopping (shutdown)");
                return;
            }

            // Wait for flush interval or shutdown
            tokio::select! {
                _ = sleep(flush_interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        return;
                    }
                }
            }

            // Gather new events since last ack cursor
            let pending: Vec<ReadEvent> = {
                let j = journal.lock().await;
                let engine = ReplayEngine::new();
                let mut batch = Vec::new();
                for ip in &reader_ips {
                    match engine.pending_events(&j, ip) {
                        Ok(results) => {
                            for rr in results {
                                for ev in rr.events {
                                    batch.push(ReadEvent {
                                        forwarder_id: forwarder_id.clone(),
                                        reader_ip: ev.stream_key.clone(),
                                        stream_epoch: ev.stream_epoch as u64,
                                        seq: ev.seq as u64,
                                        reader_timestamp: ev
                                            .reader_timestamp
                                            .clone()
                                            .unwrap_or_default(),
                                        raw_read_line: ev.raw_read_line.clone(),
                                        read_type: ev.read_type.clone(),
                                    });
                                    if batch.len() >= cfg.uplink.batch_max_events as usize {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(reader_ip = %ip, error = %e, "pending_events error in uplink loop");
                        }
                    }
                }
                batch
            };

            if pending.is_empty() {
                continue;
            }

            info!(count = pending.len(), "sending event batch");

            match session.send_batch(pending).await {
                Ok(ack) => {
                    let mut j = journal.lock().await;
                    for entry in &ack.entries {
                        if let Err(e) = j.update_ack_cursor(
                            &entry.reader_ip,
                            entry.stream_epoch as i64,
                            entry.last_seq as i64,
                        ) {
                            warn!(error = %e, "failed to update ack cursor");
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "send_batch failed, reconnecting uplink");
                    break 'uplink;
                }
            }
        }

        // Reconnect with backoff
        let delay = Duration::from_secs(backoff_secs);
        warn!(
            backoff_secs = backoff_secs,
            "uplink disconnected, reconnecting"
        );
        tokio::select! {
            _ = sleep(delay) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return;
                }
            }
        }
        backoff_secs = (backoff_secs * 2).min(30);
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
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
            info!(
                base_url = %cfg.server.base_url,
                readers = cfg.readers.len(),
                "config loaded"
            );
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
    let status_server =
        match StatusServer::start_with_journal(status_cfg, subsystem, journal.clone()).await {
            Ok(s) => {
                info!(addr = %s.local_addr(), "status HTTP server started");
                s
            }
            Err(e) => {
                eprintln!("FATAL: failed to start status HTTP server: {}", e);
                std::process::exit(1);
            }
        };
    let _ = status_server; // keep handle alive

    // Collect enabled reader endpoints
    let mut all_reader_ips: Vec<String> = Vec::new();
    let mut fanout_addrs: Vec<(String, u16, String, SocketAddr)> = Vec::new(); // (ip, port, read_type, fanout_addr)

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

            all_reader_ips.push(ep.ip.clone());
            fanout_addrs.push((ep.ip, ep.port, reader_cfg.read_type.clone(), fanout_addr));
        }
    }

    if all_reader_ips.is_empty() {
        eprintln!("FATAL: no enabled readers configured");
        std::process::exit(1);
    }

    // Spawn reader tasks
    for (reader_ip, reader_port, read_type, fanout_addr) in fanout_addrs {
        let j = journal.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move {
            run_reader(reader_ip, reader_port, read_type, fanout_addr, j, rx).await;
        });
    }

    // Spawn uplink task
    {
        let j = journal.clone();
        let rx = shutdown_rx.clone();
        let fwd_cfg = cfg.clone();
        let fwd_id = forwarder_id.clone();
        let ips = all_reader_ips.clone();
        tokio::spawn(async move {
            run_uplink(fwd_cfg, fwd_id, ips, j, rx).await;
        });
    }

    info!(
        readers = all_reader_ips.len(),
        forwarder_id = %forwarder_id,
        "forwarder initialized — all worker tasks started"
    );

    // Wait for Ctrl-C or SIGTERM
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
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
                info!("SIGINT received, shutting down");
            }
            _ = sigterm.recv() => {
                info!("SIGTERM received, shutting down");
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
        info!("Ctrl-C received, shutting down");
    }

    // Signal all tasks to stop
    shutdown_tx.send(true).ok();

    // Brief delay to allow tasks to observe shutdown and flush
    sleep(Duration::from_millis(200)).await;

    info!("forwarder shutdown complete");
}

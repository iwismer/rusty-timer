// reader_task: IPICO reader TCP connect/read loop, journal append, local fanout.

use crate::local_fanout::FanoutServer;
use crate::status_store::{ReaderConnectionState, StatusStore};
use crate::storage::journal::Journal;
use crate::ui_events::ForwarderUiEvent;
use ipico_core::read::ChipRead;
use rt_ui_log::UiLogLevel;
use std::convert::TryFrom;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Notify, watch};
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) async fn mark_reader_disconnected(status: &StatusStore, reader_ip: &str) {
    status
        .update_reader_state(reader_ip, ReaderConnectionState::Disconnected)
        .await;
}

async fn disconnect_and_notify(status: &StatusStore, stream_key: &str) {
    mark_reader_disconnected(status, stream_key).await;
}

#[derive(Debug)]
pub(crate) enum JournalAppendError {
    Append(String),
}

pub(crate) fn append_read_to_journal(
    journal: &mut Journal,
    stream_key: &str,
    reader_timestamp: Option<&str>,
    raw_frame: &[u8],
    read_type: &str,
) -> Result<(i64, i64), JournalAppendError> {
    journal
        .append_read(stream_key, reader_timestamp, raw_frame, read_type)
        .map_err(|e| JournalAppendError::Append(e.to_string()))
}

fn append_retry_backoff(failed_attempts: u64) -> Duration {
    match failed_attempts {
        1 => Duration::from_millis(100),
        2 => Duration::from_millis(500),
        3 => Duration::from_secs(1),
        4 => Duration::from_secs(2),
        _ => Duration::from_secs(5),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn append_with_retry(
    journal: &Arc<Mutex<Journal>>,
    stream_key: &str,
    reader_timestamp: Option<&str>,
    raw_frame: &[u8],
    read_type: &str,
    shutdown_rx: &mut watch::Receiver<bool>,
    logger: &Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
    reader_ip: &str,
) -> Option<(i64, i64)> {
    let mut failed_attempts = 0_u64;
    let mut last_retry_log = tokio::time::Instant::now();

    loop {
        if *shutdown_rx.borrow() {
            return None;
        }

        let append_result = {
            let mut journal = journal.lock().await;
            append_read_to_journal(
                &mut journal,
                stream_key,
                reader_timestamp,
                raw_frame,
                read_type,
            )
        };

        match append_result {
            Ok(value) => {
                if failed_attempts > 0 {
                    logger.log(format!(
                        "reader {reader_ip} journal append recovered after {failed_attempts} failed attempts"
                    ));
                }
                return Some(value);
            }
            Err(JournalAppendError::Append(error)) => {
                failed_attempts += 1;
                if failed_attempts == 1 {
                    logger.log_at(
                        UiLogLevel::Error,
                        format!("reader {reader_ip} journal append failed: {error}; retrying"),
                    );
                } else if last_retry_log.elapsed() >= Duration::from_secs(60) {
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!(
                            "reader {reader_ip} still retrying journal append after {failed_attempts} failed attempts; last error: {error}"
                        ),
                    );
                    last_retry_log = tokio::time::Instant::now();
                }
            }
        }

        let delay = append_retry_backoff(failed_attempts);
        tokio::select! {
            _ = sleep(delay) => {}
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    return None;
                }
            }
        }
    }
}

pub(crate) fn download_progress_advanced_or_started(
    was_downloading: &mut bool,
    last_download_progress: &mut u32,
    last_download_reads: &mut u32,
    last_progress_time: &mut tokio::time::Instant,
    current_progress: u32,
    current_reads: u32,
) -> bool {
    if !*was_downloading {
        *was_downloading = true;
        *last_download_progress = current_progress;
        *last_download_reads = current_reads;
        *last_progress_time = tokio::time::Instant::now();
        return true;
    }

    if current_progress != *last_download_progress || current_reads != *last_download_reads {
        *last_download_progress = current_progress;
        *last_download_reads = current_reads;
        *last_progress_time = tokio::time::Instant::now();
        return true;
    }

    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownloadStallOutcome {
    CompleteEmpty,
    FailStalled,
}

pub(crate) fn stall_outcome_for_download(
    reported_extent: u32,
    saw_download_activity: bool,
) -> DownloadStallOutcome {
    if reported_extent == 0 && !saw_download_activity {
        DownloadStallOutcome::CompleteEmpty
    } else {
        DownloadStallOutcome::FailStalled
    }
}

pub(crate) async fn fail_active_download(
    tracker: &tokio::sync::Mutex<crate::reader_control::DownloadTracker>,
    message: String,
) {
    let mut dt = tracker.lock().await;
    if dt.is_active() {
        dt.fail(message);
    }
}

// ---------------------------------------------------------------------------
// Reader task: TCP connect → parse IPICO frames → journal + fanout
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn run_reader(
    reader_ip: String,
    reader_port: u16,
    fanout_addr: SocketAddr,
    journal: Arc<Mutex<Journal>>,
    mut shutdown_rx: watch::Receiver<bool>,
    status: StatusStore,
    logger: Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
) {
    let target_addr = format!("{}:{}", reader_ip, reader_port);
    let mut backoff_secs: u64 = 1;

    let reconnect_notify = Arc::new(Notify::new());
    status.register_reconnect_notify(&target_addr, reconnect_notify.clone());

    loop {
        // Check for shutdown before attempting connect
        if *shutdown_rx.borrow() {
            info!(reader_ip = %reader_ip, "reader task stopping (shutdown)");
            status.deregister_reconnect_notify(&target_addr);
            return;
        }

        info!(reader_ip = %reader_ip, target = %target_addr, "connecting to reader");

        status
            .update_reader_state(&target_addr, ReaderConnectionState::Connecting)
            .await;

        let stream = match tokio::time::timeout(
            Duration::from_secs(5),
            TcpStream::connect(&target_addr),
        )
        .await
        {
            Ok(Ok(s)) => s,
            other => {
                let e = match other {
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => "connect timeout (5s)".to_string(),
                    _ => unreachable!(),
                };
                logger.log_at(
                    UiLogLevel::Warn,
                    format!(
                        "reader {} connect failed: {}; retrying in {}s",
                        reader_ip, e, backoff_secs
                    ),
                );
                disconnect_and_notify(&status, &target_addr).await;
                let delay = Duration::from_secs(backoff_secs);
                tokio::select! {
                    _ = sleep(delay) => {}
                    _ = reconnect_notify.notified() => {
                        info!(reader_ip = %reader_ip, "reconnect requested during connect backoff");
                        backoff_secs = 1;
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            status.deregister_reconnect_notify(&target_addr);
                            return;
                        }
                    }
                }
                backoff_secs = (backoff_secs * 2).min(30);
                continue;
            }
        };
        logger.log(format!("reader {} connected", reader_ip));
        backoff_secs = 1;
        status
            .update_reader_state(&target_addr, ReaderConnectionState::Connected)
            .await;

        // Ensure journal has stream state for this reader (idempotent).
        // Startup stream-identity restore (main.rs, before reader tasks spawn)
        // already seeded every configured stream key — possibly from the server
        // registry high-water — so this is only a cheap safety net for stream
        // keys startup did not seed. Do not remove it.
        {
            let mut j = journal.lock().await;
            let epoch = 1_i64;
            if let Err(e) = j.ensure_stream_state(&target_addr, epoch) {
                logger.log_at(
                    UiLogLevel::Error,
                    format!(
                        "reader {} journal init failed: {}; will retry in {}s",
                        reader_ip, e, backoff_secs
                    ),
                );
                disconnect_and_notify(&status, &target_addr).await;
                let delay = Duration::from_secs(backoff_secs);
                tokio::select! {
                    _ = sleep(delay) => {}
                    _ = reconnect_notify.notified() => {
                        backoff_secs = 1;
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            status.deregister_reconnect_notify(&target_addr);
                            return;
                        }
                    }
                }
                backoff_secs = (backoff_secs * 2).min(30);
                continue; // back to outer reconnect loop
            }
        }

        let (read_half, write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);

        // Set up control channels
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        let control_client = Arc::new(control_client);
        status.register_control_client(&target_addr, control_client.clone());

        let download_tracker = Arc::new(tokio::sync::Mutex::new(
            crate::reader_control::DownloadTracker::new(),
        ));
        status.register_download_tracker(&target_addr, download_tracker.clone());

        // Writer task: drains command channel to TCP socket
        let mut writer = write_half;
        let writer_reader_ip = reader_ip.clone();
        let writer_handle = tokio::spawn(async move {
            while let Some(frame) = cmd_rx.recv().await {
                if let Err(e) = writer.write_all(&frame).await {
                    warn!(reader_ip = %writer_reader_ip, "control write failed: {e}");
                    drop(cmd_rx); // Close channel immediately so cmd_tx.send() fails
                    return;
                }
            }
        });

        // Spawn connect sequence + 10s status polling task.
        // This runs concurrently with the read loop so that control
        // responses arriving on the socket can be demuxed to the sink.
        let poll_client = control_client.clone();
        let mut poll_shutdown = shutdown_rx.clone();
        let poll_logger = logger.clone();
        let poll_reader_ip = reader_ip.clone();
        let poll_status = status.clone();
        let poll_target_addr = target_addr.clone();
        let poll_download_tracker = download_tracker.clone();
        let poll_handle = tokio::spawn(async move {
            // Run initial connection sequence
            let reader_info = crate::reader_control::run_connect_sequence(&poll_client).await;
            if reader_info.connect_failures == 6 {
                poll_logger.log_at(
                    rt_ui_log::UiLogLevel::Error,
                    format!(
                        "Reader {}: control protocol non-functional — all 6 connect queries failed",
                        poll_reader_ip,
                    ),
                );
            }
            poll_logger.log(format!(
                "reader {} identified: fw={}, stored_reads={}",
                poll_reader_ip,
                reader_info
                    .hardware
                    .as_ref()
                    .map(|h| h.fw_version.as_str())
                    .unwrap_or("?"),
                reader_info.estimated_stored_reads.unwrap_or(0),
            ));
            poll_status
                .update_reader_info_unless_disconnected(&poll_target_addr, reader_info.clone())
                .await;

            // Transition to 10s polling
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            interval.tick().await; // skip first immediate tick
            let mut info = reader_info;
            let mut last_download_progress = 0u32;
            let mut last_download_reads = 0u32;
            let mut last_progress_time = tokio::time::Instant::now();
            let mut was_downloading = false;
            let mut saw_download_activity = false;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        crate::reader_control::run_status_poll(&poll_client, &mut info).await;
                        poll_status
                            .update_reader_info_unless_disconnected(&poll_target_addr, info.clone())
                            .await;

                        // Check download progress
                        let is_downloading = {
                            let dt = poll_download_tracker.lock().await;
                            dt.is_downloading()
                        };

                        if is_downloading {
                            match poll_client.get_extended_status().await {
                                Ok(ext) => {
                                    let progress = ext.download_progress;
                                    let extent = ext.stored_data_extent;

                                    let mut dt = poll_download_tracker.lock().await;
                                    if !dt.is_downloading() {
                                        was_downloading = false;
                                        continue;
                                    }

                                    dt.update_progress(progress, extent);

                                    // Safety timeout: no progress for 30 seconds.
                                    // If we have never observed any activity, treat this as an
                                    // empty download and complete cleanly.
                                    let current_progress = progress;
                                    let current_reads = dt.reads_received();
                                    if current_progress > 0 || current_reads > 0 {
                                        saw_download_activity = true;
                                    }
                                    if !download_progress_advanced_or_started(
                                        &mut was_downloading,
                                        &mut last_download_progress,
                                        &mut last_download_reads,
                                        &mut last_progress_time,
                                        current_progress,
                                        current_reads,
                                    ) && last_progress_time.elapsed() > Duration::from_secs(30)
                                    {
                                        drop(dt);
                                        let stop_result = poll_client.stop_download().await;
                                        let mut dt = poll_download_tracker.lock().await;
                                        match stop_result {
                                            Err(e) => {
                                                dt.fail(format!("stop_download failed: {}", e))
                                            }
                                            Ok(()) => match stall_outcome_for_download(
                                                extent,
                                                saw_download_activity,
                                            ) {
                                                DownloadStallOutcome::CompleteEmpty => dt.complete(),
                                                DownloadStallOutcome::FailStalled => {
                                                    dt.fail(
                                                        "download stalled: no progress for 30 seconds"
                                                            .to_owned(),
                                                    )
                                                }
                                            },
                                        }
                                        was_downloading = false;
                                        saw_download_activity = false;
                                        continue;
                                    }

                                    if extent > 0 && progress >= extent {
                                        drop(dt); // release lock before sending commands
                                        if let Err(e) = poll_client.stop_download().await {
                                            let mut dt = poll_download_tracker.lock().await;
                                            dt.fail(format!("stop_download failed: {}", e));
                                        } else {
                                            let mut dt = poll_download_tracker.lock().await;
                                            dt.complete();
                                        }
                                        was_downloading = false;
                                        saw_download_activity = false;
                                    }
                                }
                                Err(error) => {
                                    fail_active_download(
                                        &poll_download_tracker,
                                        format!("download status polling failed: {}", error),
                                    )
                                    .await;
                                    was_downloading = false;
                                    saw_download_activity = false;
                                }
                            }
                        } else {
                            was_downloading = false;
                            saw_download_activity = false;
                        }
                    }
                    _ = poll_shutdown.changed() => break,
                }
            }
        });

        let mut frame_buf = Vec::new();

        loop {
            frame_buf.clear();

            // Wait for a line or shutdown
            let read_result = tokio::select! {
                result = reader.read_until(b'\n', &mut frame_buf) => result,
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!(reader_ip = %reader_ip, "reader task stopping (shutdown)");
                        status.deregister_control_client(&target_addr);
                        status.deregister_download_tracker(&target_addr);
                        status.deregister_reconnect_notify(&target_addr);
                        return;
                    }
                    continue;
                }
            };

            match read_result {
                Err(e) => {
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} read error: {}; reconnecting", reader_ip, e),
                    );
                    fail_active_download(
                        &download_tracker,
                        format!("reader {} read error during download: {}", reader_ip, e),
                    )
                    .await;
                    disconnect_and_notify(&status, &target_addr).await;
                    break;
                }
                Ok(0) => {
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} connection closed; reconnecting", reader_ip),
                    );
                    fail_active_download(
                        &download_tracker,
                        format!("reader {} connection closed during download", reader_ip),
                    )
                    .await;
                    disconnect_and_notify(&status, &target_addr).await;
                    break;
                }
                Ok(_) => {}
            }

            let raw_payload = frame_buf
                .strip_suffix(b"\r\n")
                .or_else(|| frame_buf.strip_suffix(b"\n"))
                .unwrap_or(&frame_buf);
            if raw_payload.is_empty() {
                continue;
            }

            let raw_line = match std::str::from_utf8(raw_payload) {
                Ok(s) => s.to_owned(),
                Err(_) => {
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} skipped non-utf8 frame", reader_ip),
                    );
                    continue;
                }
            };
            if raw_line.is_empty() {
                continue;
            }

            // Demux: control responses vs tag reads
            if raw_line.starts_with("ab") {
                control_sink.feed(raw_line.as_bytes()).await;
                continue;
            }
            if !raw_line.starts_with("aa") {
                // Non-framed: banner text or other reader output
                control_sink.feed_banner_line(raw_line.as_bytes()).await;
                continue;
            }

            // Parse IPICO chip read to validate and extract metadata
            let parsed_read_type;
            let reader_timestamp;
            match ChipRead::try_from(raw_line.as_str()) {
                Ok(chip) => {
                    reader_timestamp = Some(chip.timestamp.to_string());
                    parsed_read_type = chip.read_type.as_str().to_owned();
                }
                Err(_) => {
                    // Line is not a valid IPICO read — log and skip
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} skipped unparseable line", reader_ip),
                    );
                    continue;
                }
            }

            // Write to journal. Keep status updates out of the DB lock scope.
            let Some((epoch, seq)) = append_with_retry(
                &journal,
                &target_addr,
                reader_timestamp.as_deref(),
                &frame_buf,
                &parsed_read_type,
                &mut shutdown_rx,
                &logger,
                &reader_ip,
            )
            .await
            else {
                info!(reader_ip = %reader_ip, "reader task stopping (shutdown)");
                status.deregister_control_client(&target_addr);
                status.deregister_download_tracker(&target_addr);
                status.deregister_reconnect_notify(&target_addr);
                return;
            };

            debug!(
                reader_ip = %reader_ip,
                epoch = epoch,
                seq = seq,
                "event journaled"
            );

            {
                let mut dt = download_tracker.lock().await;
                dt.record_read();
            }

            // Fan out the exact bytes read from the reader, preserving
            // upstream line framing (for example CRLF).
            let raw_bytes = frame_buf.clone();
            if let Err(e) = FanoutServer::push_to_addr(fanout_addr, raw_bytes).await {
                warn!(reader_ip = %reader_ip, error = %e, "local fanout push failed");
                // Non-fatal: local fanout failure doesn't break P2P path
            }

            status.record_read(&target_addr).await;
        }

        writer_handle.abort();
        poll_handle.abort();
        status.deregister_control_client(&target_addr);
        status.deregister_download_tracker(&target_addr);

        // Reconnect with backoff
        let delay = Duration::from_secs(backoff_secs);
        info!(
            reader_ip = %reader_ip,
            backoff_secs = backoff_secs,
            "waiting before reconnect"
        );
        tokio::select! {
            _ = sleep(delay) => {}
            _ = reconnect_notify.notified() => {
                info!(reader_ip = %reader_ip, "reconnect requested during backoff");
                backoff_secs = 1;
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    status.deregister_reconnect_notify(&target_addr);
                    return;
                }
            }
        }
        backoff_secs = (backoff_secs * 2).min(30);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::broadcast;
    use tokio::time::{Duration, advance};

    const STREAM_KEY: &str = "192.0.2.10:10000";
    const RAW_FRAME: &[u8] = b"aa000000000000000000000000000000000000000000\r\n";

    fn test_logger() -> Arc<rt_ui_log::UiLogger<ForwarderUiEvent>> {
        let (tx, _) = broadcast::channel(16);
        Arc::new(rt_ui_log::UiLogger::with_buffer(
            tx,
            |entry| ForwarderUiEvent::LogEntry { entry },
            64,
        ))
    }

    fn test_journal() -> (TempDir, Arc<Mutex<Journal>>) {
        let tempdir = tempfile::tempdir().expect("create tempdir");
        let mut journal =
            Journal::open(&tempdir.path().join("journal.sqlite")).expect("open journal");
        journal
            .ensure_stream_state(STREAM_KEY, 1)
            .expect("ensure stream");
        (tempdir, Arc::new(Mutex::new(journal)))
    }

    async fn set_query_only(journal: &Arc<Mutex<Journal>>, on: bool) {
        let journal = journal.lock().await;
        journal.set_query_only(on).expect("set query_only");
    }

    fn spawn_append_with_retry(
        journal: Arc<Mutex<Journal>>,
        mut shutdown_rx: watch::Receiver<bool>,
        logger: Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
    ) -> tokio::task::JoinHandle<Option<(i64, i64)>> {
        tokio::spawn(async move {
            append_with_retry(
                &journal,
                STREAM_KEY,
                Some("123456"),
                RAW_FRAME,
                "tag",
                &mut shutdown_rx,
                &logger,
                "192.0.2.10",
            )
            .await
        })
    }

    #[tokio::test(start_paused = true)]
    async fn append_with_retry_keeps_retrying_until_shutdown() {
        let (_tempdir, journal) = test_journal();
        set_query_only(&journal, true).await;
        let logger = test_logger();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let task = spawn_append_with_retry(journal, shutdown_rx, logger.clone());
        tokio::task::yield_now().await;

        advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;
        advance(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;

        assert!(!task.is_finished(), "append should still be retrying");
        let entries = logger.entries();
        assert!(
            entries.iter().any(|entry| entry.contains("[ERROR]")
                && entry.contains("journal append failed")
                && entry.contains("retrying")),
            "first append failure must be logged at error level: {entries:?}"
        );
        assert!(
            entries.iter().any(|entry| entry.contains("[WARN]")
                && entry.contains("still retrying journal append")
                && entry.contains("attempt")),
            "long-running retry must re-log a warn: {entries:?}"
        );

        shutdown_tx.send(true).expect("send shutdown");
        assert_eq!(task.await.expect("task join"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn append_with_retry_logs_recovery_after_failures() {
        let (_tempdir, journal) = test_journal();
        set_query_only(&journal, true).await;
        let logger = test_logger();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let task = spawn_append_with_retry(journal.clone(), shutdown_rx, logger.clone());
        tokio::task::yield_now().await;
        advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;

        set_query_only(&journal, false).await;
        advance(Duration::from_secs(1)).await;
        let result = task.await.expect("task join");
        assert_eq!(result, Some((1, 1)));
        drop(shutdown_tx);

        let entries = logger.entries();
        assert!(
            entries.iter().any(|entry| entry.contains("[ERROR]")
                && entry.contains("journal append failed")
                && entry.contains("retrying")),
            "first append failure must be logged at error level: {entries:?}"
        );
        assert!(
            entries.iter().any(|entry| entry.contains("[INFO]")
                && entry.contains("journal append recovered")
                && entry.contains("attempt")),
            "recovery must be logged: {entries:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn append_with_retry_returns_immediately_for_healthy_journal() {
        let (_tempdir, journal) = test_journal();
        let logger = test_logger();
        let (_shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let result = append_with_retry(
            &journal,
            STREAM_KEY,
            Some("123456"),
            RAW_FRAME,
            "tag",
            &mut shutdown_rx,
            &logger,
            "192.0.2.10",
        )
        .await;

        assert_eq!(result, Some((1, 1)));
        let entries = logger.entries();
        assert!(
            entries
                .iter()
                .all(|entry| !entry.contains("journal append failed")
                    && !entry.contains("journal append recovered")),
            "healthy append should not log retry/recovery: {entries:?}"
        );
    }
}

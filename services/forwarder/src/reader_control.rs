//! IPICO reader control client for the forwarder.

use ipico_core::control::{self, Command, ControlError, ControlFrame};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tracing::{info, warn};

/// Error from the control client, wrapping protocol parse errors and adding
/// transport-level errors (timeout, channel closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlClientError {
    /// Protocol-level error from ipico-core.
    Protocol(ControlError),
    /// Reader did not respond within the timeout.
    Timeout,
    /// Control channel closed (connection lost).
    ChannelClosed,
}

impl fmt::Display for ControlClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(e) => write!(f, "{e}"),
            Self::Timeout => write!(f, "reader response timeout"),
            Self::ChannelClosed => write!(f, "control channel closed (connection lost)"),
        }
    }
}

impl std::error::Error for ControlClientError {}

impl From<ControlError> for ControlClientError {
    fn from(e: ControlError) -> Self {
        Self::Protocol(e)
    }
}

struct PendingRequest {
    kind: PendingRequestKind,
    reply_tx: oneshot::Sender<Result<ControlFrame, ControlError>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRequestKind {
    AnyInstruction(u8),
    ExtendedStatusWrite,
    DownloadStopWrite,
    ExtendedStatusQuery,
    Config3Write,
    Config3Query,
    TagMessageFormatWrite,
    TagMessageFormatQuery,
}

impl PendingRequestKind {
    fn for_command(cmd: &Command) -> Self {
        match cmd {
            Command::SetRecordingState { .. }
            | Command::SetAccessMode { .. }
            | Command::InitDownload
            | Command::ConfigureDownload
            | Command::CleanupDownload
            | Command::TriggerErase => Self::ExtendedStatusWrite,
            Command::GetExtendedStatus => Self::ExtendedStatusQuery,
            Command::SetConfig3 { .. } => Self::Config3Write,
            Command::GetConfig3 => Self::Config3Query,
            Command::SetTagMessageFormat { .. } => Self::TagMessageFormatWrite,
            Command::GetTagMessageFormat => Self::TagMessageFormatQuery,
            _ => Self::AnyInstruction(cmd.instruction()),
        }
    }

    fn matches(self, frame: &ControlFrame) -> bool {
        match self {
            Self::AnyInstruction(instruction) => frame.instruction() == instruction,
            Self::ExtendedStatusWrite => {
                frame.instruction() == control::INSTR_EXT_STATUS
                    && (frame.data().is_empty() || frame.data().len() >= 12)
            }
            Self::DownloadStopWrite => {
                frame.instruction() == control::INSTR_EXT_STATUS
                    && (frame.data().is_empty()
                        || (frame.data().len() >= 12 && frame.data()[0] != 0x03))
            }
            Self::ExtendedStatusQuery => {
                frame.instruction() == control::INSTR_EXT_STATUS && frame.data().len() >= 12
            }
            Self::Config3Write => {
                frame.instruction() == control::INSTR_CONFIG3 && frame.data().is_empty()
            }
            Self::Config3Query => {
                frame.instruction() == control::INSTR_CONFIG3 && frame.data().len() >= 2
            }
            Self::TagMessageFormatWrite => {
                frame.instruction() == control::INSTR_TAG_MESSAGE_FORMAT && frame.data().is_empty()
            }
            Self::TagMessageFormatQuery => {
                frame.instruction() == control::INSTR_TAG_MESSAGE_FORMAT && frame.data().len() >= 8
            }
        }
    }
}

/// Sends commands to a single IPICO reader over TCP and awaits responses.
///
/// Commands are serialized via `in_flight` to prevent interleaving. Multi-step
/// sequences (clear_records, start_download, stop_download) hold the lock
/// across all sub-steps.
pub struct ControlClient {
    cmd_tx: mpsc::Sender<Vec<u8>>,
    pending: Arc<Mutex<Option<PendingRequest>>>,
    banner_buf: Arc<Mutex<Vec<String>>>,
    in_flight: Arc<Mutex<()>>,
    timeout: Duration,
}

pub struct ControlResponseSink {
    pending: Arc<Mutex<Option<PendingRequest>>>,
    banner_buf: Arc<Mutex<Vec<String>>>,
}

impl ControlClient {
    pub fn new(cmd_tx: mpsc::Sender<Vec<u8>>) -> (Self, ControlResponseSink) {
        let pending: Arc<Mutex<Option<PendingRequest>>> = Arc::new(Mutex::new(None));
        let banner_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let in_flight: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
        let sink = ControlResponseSink {
            pending: pending.clone(),
            banner_buf: banner_buf.clone(),
        };
        let client = ControlClient {
            cmd_tx,
            pending,
            banner_buf,
            in_flight,
            timeout: Duration::from_secs(2),
        };
        (client, sink)
    }

    /// Send a command without acquiring the in-flight lock.
    ///
    /// The `_guard` parameter is a proof token that the caller holds the
    /// `in_flight` lock. Used within multi-step sequences (clear_records,
    /// start_download, stop_download) that hold the lock themselves.
    async fn send_inner(
        &self,
        cmd: &Command,
        _guard: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<ControlFrame, ControlClientError> {
        self.send_inner_with_kind(cmd, PendingRequestKind::for_command(cmd), _guard)
            .await
    }

    async fn send_inner_with_kind(
        &self,
        cmd: &Command,
        kind: PendingRequestKind,
        _guard: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<ControlFrame, ControlClientError> {
        let frame = control::encode_command(cmd, 0x00)?;

        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            *pending = Some(PendingRequest { kind, reply_tx });
        }

        self.cmd_tx
            .send(frame)
            .await
            .map_err(|_| ControlClientError::ChannelClosed)?;

        match tokio::time::timeout(self.timeout, reply_rx).await {
            Ok(Ok(result)) => result.map_err(ControlClientError::Protocol),
            Ok(Err(_)) => {
                // oneshot sender dropped — control response sink is gone (connection lost)
                let mut pending = self.pending.lock().await;
                *pending = None;
                Err(ControlClientError::ChannelClosed)
            }
            Err(_) => {
                // tokio timeout elapsed — reader did not respond in time
                let mut pending = self.pending.lock().await;
                *pending = None;
                Err(ControlClientError::Timeout)
            }
        }
    }

    /// Send a single command, serializing with other callers via the in-flight lock.
    async fn send(&self, cmd: &Command) -> Result<ControlFrame, ControlClientError> {
        let guard = self.in_flight.lock().await;
        self.send_inner(cmd, &guard).await
    }

    pub async fn get_date_time(&self) -> Result<control::ReaderDateTime, ControlClientError> {
        let frame = self.send(&Command::GetDateTime).await?;
        Ok(control::decode_date_time(&frame)?)
    }

    pub async fn get_statistics(&self) -> Result<control::ReaderStatistics, ControlClientError> {
        let frame = self.send(&Command::GetStatistics).await?;
        Ok(control::decode_statistics(&frame)?)
    }

    pub async fn get_extended_status(&self) -> Result<control::ExtendedStatus, ControlClientError> {
        let frame = self.send(&Command::GetExtendedStatus).await?;
        Ok(control::decode_extended_status(&frame)?)
    }

    pub async fn get_config3(&self) -> Result<(control::ReadMode, u8), ControlClientError> {
        let frame = self.send(&Command::GetConfig3).await?;
        Ok(control::decode_config3(&frame)?)
    }

    pub async fn get_tag_message_format(
        &self,
    ) -> Result<control::TagMessageFormat, ControlClientError> {
        let frame = self.send(&Command::GetTagMessageFormat).await?;
        Ok(control::decode_tag_message_format(&frame)?)
    }

    pub async fn set_config3(
        &self,
        mode: control::ReadMode,
        timeout: u8,
    ) -> Result<(), ControlClientError> {
        let _frame = self.send(&Command::SetConfig3 { mode, timeout }).await?;
        Ok(())
    }

    pub async fn set_tag_message_format(
        &self,
        format: control::TagMessageFormat,
    ) -> Result<(), ControlClientError> {
        let _frame = self.send(&Command::SetTagMessageFormat { format }).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_date_time(
        &self,
        year: u8,
        month: u8,
        day: u8,
        dow: u8,
        hour: u8,
        min: u8,
        sec: u8,
    ) -> Result<(), ControlClientError> {
        let _frame = self
            .send(&Command::SetDateTime {
                year,
                month,
                day,
                day_of_week: dow,
                hour,
                minute: min,
                second: sec,
            })
            .await?;
        Ok(())
    }

    pub async fn print_banner(&self) -> Result<String, ControlClientError> {
        {
            self.banner_buf.lock().await.clear();
        }
        let _frame = self.send(&Command::PrintBanner).await?;
        let buf = self.banner_buf.lock().await;
        Ok(buf.join("\n").trim().to_owned())
    }

    /// Execute the full clear-records workflow: 3 erase sub-commands via 0x4b,
    /// 10s wait, counter reset, then CONFIG3 cycling (Event -> Raw).
    pub async fn clear_records(&self) -> Result<(), ControlClientError> {
        let _in_flight = self.in_flight.lock().await;

        // Step 1: set recording off (sub-cmd 0x00, state=0x00)
        let _ = self
            .send_inner(&Command::SetRecordingState { on: false }, &_in_flight)
            .await?;

        // Step 2: set download/access off (sub-cmd 0x01, state=0x00)
        let _ = self
            .send_inner(&Command::SetAccessMode { on: false }, &_in_flight)
            .await?;

        // Step 3: trigger erase
        let _ = self.send_inner(&Command::TriggerErase, &_in_flight).await?;

        // Wait for erase to complete
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Re-send recording off to reset counter
        let _ = self
            .send_inner(&Command::SetRecordingState { on: false }, &_in_flight)
            .await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Cycle CONFIG3: Event -> Raw
        let _ = self
            .send_inner(
                &Command::SetConfig3 {
                    mode: control::ReadMode::Event,
                    timeout: 5,
                },
                &_in_flight,
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = self
            .send_inner(
                &Command::SetConfig3 {
                    mode: control::ReadMode::Raw,
                    timeout: 5,
                },
                &_in_flight,
            )
            .await?;

        Ok(())
    }

    /// Toggle the reader's internal recording state.
    pub async fn set_recording(
        &self,
        on: bool,
    ) -> Result<control::ExtendedStatus, ControlClientError> {
        let frame = self.send(&Command::SetRecordingState { on }).await?;
        Ok(control::decode_extended_status(&frame)?)
    }

    /// Send the 3-step download-start sequence (init, configure, start).
    /// Returns the initial ExtendedStatus after the start command.
    pub async fn start_download(&self) -> Result<control::ExtendedStatus, ControlClientError> {
        let _in_flight = self.in_flight.lock().await;
        let _ = self.send_inner(&Command::InitDownload, &_in_flight).await?;
        let _ = self
            .send_inner(&Command::ConfigureDownload, &_in_flight)
            .await?;
        let frame = self
            .send_inner(&Command::SetAccessMode { on: true }, &_in_flight)
            .await?;
        Ok(control::decode_extended_status(&frame)?)
    }

    /// Send the 2-step download-stop sequence (stop, cleanup).
    pub async fn stop_download(&self) -> Result<(), ControlClientError> {
        let _in_flight = self.in_flight.lock().await;
        let _ = self
            .send_inner_with_kind(
                &Command::SetAccessMode { on: false },
                PendingRequestKind::DownloadStopWrite,
                &_in_flight,
            )
            .await?;
        let _ = self
            .send_inner_with_kind(
                &Command::CleanupDownload,
                PendingRequestKind::DownloadStopWrite,
                &_in_flight,
            )
            .await?;
        Ok(())
    }
}

impl ControlResponseSink {
    /// Feed an `ab`-prefixed control response line (without \r\n).
    ///
    /// Returns `true` if the frame matched and was consumed by a pending request,
    /// or `false` if no request was pending or the instruction did not match.
    pub async fn feed(&self, line: &[u8]) -> bool {
        let result = control::parse_response(line);
        let mut pending = self.pending.lock().await;
        if let Some(req) = pending.take() {
            match &result {
                Ok(frame) if req.kind.matches(frame) => {
                    let _ = req.reply_tx.send(result);
                    return true;
                }
                Err(_) => {
                    let _ = req.reply_tx.send(result);
                    return true;
                }
                Ok(_) => {
                    // Wrong instruction (unsolicited) — put request back
                    *pending = Some(req);
                }
            }
        }
        false
    }

    /// Feed a non-framed line (banner text).
    pub async fn feed_banner_line(&self, line: &[u8]) {
        match std::str::from_utf8(line) {
            Ok(s) => {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    self.banner_buf.lock().await.push(trimmed.to_owned());
                }
            }
            Err(_) => {
                warn!("non-UTF-8 banner line ({} bytes), skipping", line.len());
            }
        }
    }
}

/// Hardware info from GET_STATISTICS (0x0a).
#[derive(Debug, Clone, serde::Serialize)]
pub struct HardwareInfo {
    pub fw_version: String,
    pub hw_code: u8,
    pub reader_id: u8,
    pub config3: u8,
}

/// Read mode configuration from CONFIG3 (0x09).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Config3Info {
    pub mode: String,
    pub timeout: u8,
}

/// Reader clock info from GET_DATE_TIME (0x02).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClockInfo {
    pub reader_clock: String,
    pub drift_ms: i64,
}

/// Reader info gathered on connect and refreshed by polling.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReaderInfo {
    pub banner: Option<String>,
    pub hardware: Option<HardwareInfo>,
    pub config: Option<Config3Info>,
    pub tto_enabled: Option<bool>,
    pub clock: Option<ClockInfo>,
    pub estimated_stored_reads: Option<u32>,
    pub recording: Option<bool>,
    /// Number of connect sequence failures (0 = all succeeded).
    pub connect_failures: u8,
}

/// Events emitted by [`DownloadTracker`] for SSE consumers.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DownloadEvent {
    /// Download is in progress. `progress` and `total` are raw 24-bit extent
    /// values from the 0x4b extended status register (not read counts).
    /// `reads_received` is the count of `aa`-frame chip reads received so far.
    Downloading {
        progress: u32,
        total: u32,
        reads_received: u32,
    },
    /// Download completed successfully.
    Complete { reads_received: u32 },
    /// Download failed with the given error message.
    Error { message: String },
    /// No download active.
    Idle,
}

/// The current phase of a stored-read download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadState {
    Idle,
    Starting,
    Downloading,
    Complete,
    Error(String),
}

/// Tracks progress of a stored-read download and broadcasts events to SSE
/// subscribers.
#[derive(Debug)]
pub struct DownloadTracker {
    state: DownloadState,
    reads_received: u32,
    stored_data_extent: u32,
    download_progress: u32,
    event_tx: broadcast::Sender<DownloadEvent>,
}

impl Default for DownloadTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadTracker {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            state: DownloadState::Idle,
            reads_received: 0,
            stored_data_extent: 0,
            download_progress: 0,
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.event_tx.subscribe()
    }

    pub fn record_read(&mut self) {
        if self.state != DownloadState::Downloading {
            return;
        }
        self.reads_received += 1;
        let _ = self.event_tx.send(DownloadEvent::Downloading {
            progress: self.download_progress,
            total: self.stored_data_extent,
            reads_received: self.reads_received,
        });
    }

    pub fn update_progress(&mut self, progress: u32, extent: u32) {
        if self.state != DownloadState::Downloading {
            return;
        }
        self.download_progress = progress;
        self.stored_data_extent = extent;
    }

    pub fn begin_startup(&mut self) {
        self.state = DownloadState::Starting;
        self.reads_received = 0;
        self.stored_data_extent = 0;
        self.download_progress = 0;
    }

    pub fn start(&mut self, stored_data_extent: u32) {
        debug_assert!(
            matches!(self.state, DownloadState::Starting),
            "start() called in {:?} state, expected Starting",
            self.state,
        );
        self.state = DownloadState::Downloading;
        self.reads_received = 0;
        self.download_progress = 0;
        self.stored_data_extent = stored_data_extent;
    }

    pub fn complete(&mut self) {
        debug_assert!(
            matches!(self.state, DownloadState::Downloading),
            "complete() called in {:?} state, expected Downloading",
            self.state,
        );
        self.state = DownloadState::Complete;
        let _ = self.event_tx.send(DownloadEvent::Complete {
            reads_received: self.reads_received,
        });
    }

    pub fn fail(&mut self, msg: String) {
        debug_assert!(
            matches!(
                self.state,
                DownloadState::Starting | DownloadState::Downloading
            ),
            "fail() called in {:?} state, expected Starting or Downloading",
            self.state,
        );
        self.state = DownloadState::Error(msg.clone());
        let _ = self.event_tx.send(DownloadEvent::Error { message: msg });
    }

    pub fn reset(&mut self) {
        self.state = DownloadState::Idle;
        self.reads_received = 0;
        self.stored_data_extent = 0;
        self.download_progress = 0;
    }

    pub fn state(&self) -> &DownloadState {
        &self.state
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            DownloadState::Starting | DownloadState::Downloading
        )
    }

    pub fn is_downloading(&self) -> bool {
        self.state == DownloadState::Downloading
    }

    pub fn reads_received(&self) -> u32 {
        self.reads_received
    }

    pub fn stored_data_extent(&self) -> u32 {
        self.stored_data_extent
    }

    pub fn download_progress(&self) -> u32 {
        self.download_progress
    }
}

/// Run the initial connection sequence: statistics, banner, ext status, config3, tag format, clock.
pub async fn run_connect_sequence(client: &ControlClient) -> ReaderInfo {
    let mut ri = ReaderInfo::default();
    let mut failures = 0u8;

    match client.get_statistics().await {
        Ok(stats) => {
            ri.hardware = Some(HardwareInfo {
                fw_version: stats.fw_version_string(),
                hw_code: stats.hw_code,
                reader_id: stats.reader_id,
                config3: stats.config3,
            });
            info!(fw = %stats.fw_version_string(), hw = stats.hw_code, "reader statistics");
        }
        Err(e) => {
            warn!("get_statistics failed: {}", e);
            failures += 1;
        }
    }

    match client.print_banner().await {
        Ok(banner) if !banner.is_empty() => {
            info!(banner = %banner, "reader banner");
            ri.banner = Some(banner);
        }
        Ok(_) => {}
        Err(e) => {
            warn!("print_banner failed: {}", e);
            failures += 1;
        }
    }

    match client.get_extended_status().await {
        Ok(ext) => {
            ri.estimated_stored_reads = Some(ext.estimated_stored_reads());
            ri.recording = Some(ext.recording_state.is_recording());
            info!(
                estimated_stored_reads = ext.estimated_stored_reads(),
                storage_state = ext.storage_state,
                "reader extended status"
            );
        }
        Err(e) => {
            warn!("get_extended_status failed: {}", e);
            failures += 1;
        }
    }

    match client.get_config3().await {
        Ok((mode, timeout)) => {
            ri.config = Some(Config3Info {
                mode: mode.as_str().to_owned(),
                timeout,
            });
        }
        Err(e) => {
            warn!("get_config3 failed: {}", e);
            failures += 1;
        }
    }

    if poll_tag_message_format(client, &mut ri).await.is_err() {
        failures += 1;
    }

    if poll_clock(client, &mut ri).await.is_err() {
        failures += 1;
    }

    ri.connect_failures = failures;

    if failures == 6 {
        tracing::error!(
            "connect sequence failed for all 6 queries — control protocol may be non-functional"
        );
    }

    ri
}

/// Poll extended status, CONFIG3 (read mode), tag format, and clock, updating info in place.
pub async fn run_status_poll(client: &ControlClient, info: &mut ReaderInfo) {
    match client.get_extended_status().await {
        Ok(ext) => {
            info.estimated_stored_reads = Some(ext.estimated_stored_reads());
            info.recording = Some(ext.recording_state.is_recording());
        }
        Err(e) => {
            warn!("status poll: get_extended_status failed: {e}");
            info.estimated_stored_reads = None;
            info.recording = None;
        }
    }

    match client.get_config3().await {
        Ok((mode, timeout)) => {
            info.config = Some(Config3Info {
                mode: mode.as_str().to_owned(),
                timeout,
            });
        }
        Err(e) => {
            warn!("status poll: get_config3 failed: {e}");
            info.config = None;
        }
    }

    let _ = poll_tag_message_format(client, info).await;
    let _ = poll_clock(client, info).await;
}

async fn poll_tag_message_format(client: &ControlClient, info: &mut ReaderInfo) -> Result<(), ()> {
    match client.get_tag_message_format().await {
        Ok(format) => {
            info.tto_enabled = Some(format.tto_enabled());
            Ok(())
        }
        Err(e) => {
            warn!("get_tag_message_format failed: {e}");
            info.tto_enabled = None;
            Err(())
        }
    }
}

async fn poll_clock(client: &ControlClient, info: &mut ReaderInfo) -> Result<(), ()> {
    match client.get_date_time().await {
        Ok(dt) => {
            let reader_iso = dt.to_iso_string();
            let now = chrono::Local::now();
            if let Ok(reader_naive) =
                chrono::NaiveDateTime::parse_from_str(&reader_iso, "%Y-%m-%dT%H:%M:%S%.3f")
            {
                let system_naive = now.naive_local();
                let drift = system_naive
                    .signed_duration_since(reader_naive)
                    .num_milliseconds();
                info.clock = Some(ClockInfo {
                    reader_clock: reader_iso,
                    drift_ms: drift,
                });
            } else {
                warn!("status poll: failed to parse reader clock: {reader_iso}");
                info.clock = None;
            }
            Ok(())
        }
        Err(e) => {
            warn!("status poll: get_date_time failed: {e}");
            info.clock = None;
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn concurrent_control_calls_are_serialized_and_both_complete() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (client, sink) = ControlClient::new(cmd_tx);
        let client = Arc::new(client);

        let c1 = client.clone();
        let first_call = tokio::spawn(async move { c1.get_extended_status().await });

        let first_cmd = cmd_rx.recv().await.expect("first command");
        assert_eq!(
            std::str::from_utf8(&first_cmd).expect("first command utf8"),
            "ab00ff4bc2\r\n"
        );

        let c2 = client.clone();
        let second_call = tokio::spawn(async move { c2.get_config3().await });

        let maybe_second = timeout(Duration::from_millis(50), cmd_rx.recv()).await;
        assert!(
            maybe_second.is_err(),
            "second command should not be sent before first response is consumed"
        );

        assert!(sink.feed(b"ab000d4b010b012f0000000059058f0c005a").await);

        let second_cmd = cmd_rx.recv().await.expect("second command");
        assert_eq!(
            std::str::from_utf8(&second_cmd).expect("second command utf8"),
            "ab00ff0995\r\n"
        );
        assert!(sink.feed(b"ab0002090305f3").await);

        let ext = first_call
            .await
            .expect("first call task")
            .expect("first call result");
        assert_eq!(ext.stored_data_extent, 0x0b012f);

        let (mode, timeout_secs) = second_call
            .await
            .expect("second call task")
            .expect("second call result");
        assert_eq!(mode, control::ReadMode::Event);
        assert_eq!(timeout_secs, 5);
    }

    #[tokio::test]
    async fn clear_records_uses_tracked_command_sequence() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(16);
        let (client, sink) = ControlClient::new(cmd_tx);

        let clear_task = tokio::spawn(async move { client.clear_records().await });

        let ack_for = |instruction: u8| -> String {
            let body = format!("0000{instruction:02x}");
            let lrc = control::lrc(body.as_bytes());
            format!("ab{body}{lrc:02x}")
        };

        let step1 = cmd_rx.recv().await.expect("clear step 1");
        assert_eq!(
            std::str::from_utf8(&step1).expect("clear step 1 utf8"),
            "ab00024b000018\r\n"
        );
        assert!(
            sink.feed(ack_for(control::INSTR_EXT_STATUS).as_bytes())
                .await
        );

        let step2 = cmd_rx.recv().await.expect("clear step 2");
        assert_eq!(
            std::str::from_utf8(&step2).expect("clear step 2 utf8"),
            "ab00024b010019\r\n"
        );
        assert!(
            sink.feed(ack_for(control::INSTR_EXT_STATUS).as_bytes())
                .await
        );

        let step3 = cmd_rx.recv().await.expect("clear step 3");
        assert_eq!(
            std::str::from_utf8(&step3).expect("clear step 3 utf8"),
            "ab00014bd0eb\r\n"
        );
        assert!(
            sink.feed(ack_for(control::INSTR_EXT_STATUS).as_bytes())
                .await
        );

        let step4 = cmd_rx.recv().await.expect("clear step 4");
        assert_eq!(
            std::str::from_utf8(&step4).expect("clear step 4 utf8"),
            "ab00024b000018\r\n"
        );
        assert!(
            sink.feed(ack_for(control::INSTR_EXT_STATUS).as_bytes())
                .await
        );

        let step5 = cmd_rx.recv().await.expect("clear step 5");
        assert_eq!(
            std::str::from_utf8(&step5).expect("clear step 5 utf8"),
            "ab0003090305075b\r\n"
        );
        assert!(sink.feed(ack_for(control::INSTR_CONFIG3).as_bytes()).await);

        let step6 = cmd_rx.recv().await.expect("clear step 6");
        assert_eq!(
            std::str::from_utf8(&step6).expect("clear step 6 utf8"),
            "ab00030900050758\r\n"
        );
        assert!(sink.feed(ack_for(control::INSTR_CONFIG3).as_bytes()).await);

        clear_task
            .await
            .expect("clear task join")
            .expect("clear task result");
    }

    #[tokio::test]
    async fn clear_records_progress_frame_does_not_satisfy_pending_request() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (client, sink) = ControlClient::new(cmd_tx);

        let request = tokio::spawn(async move {
            let guard = client.in_flight.lock().await;
            client.send_inner(&Command::TriggerErase, &guard).await
        });

        let cmd = cmd_rx.recv().await.expect("clear trigger command");
        assert_eq!(
            std::str::from_utf8(&cmd).expect("clear trigger utf8"),
            "ab00014bd0eb\r\n"
        );

        let consumed = sink.feed(b"ab00034bd05959c9").await;
        assert!(!consumed, "progress frame should be treated as unsolicited");

        let still_pending = timeout(Duration::from_millis(50), request).await;
        assert!(
            still_pending.is_err(),
            "request should still be pending after progress frame"
        );
    }

    #[tokio::test]
    async fn set_recording_on_sends_correct_frame() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (client, sink) = ControlClient::new(cmd_tx);

        let task = tokio::spawn(async move { client.set_recording(true).await });

        let cmd = cmd_rx.recv().await.expect("recording command");
        assert_eq!(
            std::str::from_utf8(&cmd).expect("utf8"),
            "ab00024b000119\r\n"
        );

        // Reply with a 12-byte status: byte 0 = 0x01 (recording on)
        assert!(sink.feed(b"ab000c4b010000000000000059058f015c").await);

        task.await
            .expect("task join")
            .expect("set_recording result");
    }

    #[tokio::test]
    async fn set_recording_off_sends_correct_frame() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (client, sink) = ControlClient::new(cmd_tx);

        let task = tokio::spawn(async move { client.set_recording(false).await });

        let cmd = cmd_rx.recv().await.expect("recording command");
        assert_eq!(
            std::str::from_utf8(&cmd).expect("utf8"),
            "ab00024b000018\r\n"
        );

        // Reply with a 12-byte status: byte 0 = 0x00 (recording off)
        assert!(sink.feed(b"ab000c4b000000000000000059058f015b").await);

        task.await
            .expect("task join")
            .expect("set_recording result");
    }

    #[tokio::test]
    async fn download_tracker_lifecycle() {
        let mut tracker = DownloadTracker::new();
        let mut rx = tracker.subscribe();

        tracker.begin_startup();
        tracker.start(100);
        assert_eq!(*tracker.state(), DownloadState::Downloading);
        assert_eq!(tracker.stored_data_extent(), 100);

        tracker.record_read();
        tracker.record_read();
        assert_eq!(tracker.reads_received(), 2);

        let ev = rx.try_recv().expect("first read event");
        assert!(matches!(
            ev,
            DownloadEvent::Downloading {
                reads_received: 1,
                ..
            }
        ));
        let ev = rx.try_recv().expect("second read event");
        assert!(matches!(
            ev,
            DownloadEvent::Downloading {
                reads_received: 2,
                ..
            }
        ));

        tracker.update_progress(50, 100);
        assert_eq!(tracker.download_progress(), 50);

        tracker.complete();
        assert_eq!(*tracker.state(), DownloadState::Complete);
        let ev = rx.try_recv().expect("complete event");
        assert!(matches!(ev, DownloadEvent::Complete { reads_received: 2 }));

        tracker.reset();
        assert_eq!(*tracker.state(), DownloadState::Idle);
        assert_eq!(tracker.reads_received(), 0);
        assert_eq!(tracker.download_progress(), 0);
        assert_eq!(tracker.stored_data_extent(), 0);
    }

    #[tokio::test]
    async fn download_tracker_record_read_ignored_when_idle() {
        let mut tracker = DownloadTracker::new();
        let mut rx = tracker.subscribe();

        tracker.record_read();
        assert_eq!(tracker.reads_received(), 0);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn download_tracker_error_sends_event() {
        let mut tracker = DownloadTracker::new();
        let mut rx = tracker.subscribe();

        tracker.begin_startup();
        tracker.start(50);
        tracker.fail("connection lost".to_string());
        assert_eq!(
            *tracker.state(),
            DownloadState::Error("connection lost".to_string())
        );

        let ev = rx.try_recv().expect("error event");
        assert!(matches!(ev, DownloadEvent::Error { message } if message == "connection lost"));
    }

    #[tokio::test]
    async fn start_download_sends_correct_3_step_sequence() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(16);
        let (client, sink) = ControlClient::new(cmd_tx);

        let task = tokio::spawn(async move { client.start_download().await });

        let status_reply = |byte0: u8| -> String {
            let data_hex = format!("{:02x}0204f60000000059058f0300", byte0);
            let body = format!("000d4b{}", data_hex);
            let lrc = control::lrc(body.as_bytes());
            format!("ab{}{:02x}", body, lrc)
        };

        // Step 1: init (sub-cmd 0x02)
        let step1 = cmd_rx.recv().await.expect("download step 1");
        assert_eq!(
            std::str::from_utf8(&step1).expect("step 1 utf8"),
            "ab00014b02b9\r\n"
        );
        assert!(sink.feed(status_reply(0x01).as_bytes()).await);

        // Step 2: configure (sub-cmd 0x07, params [0x01, 0x05])
        let step2 = cmd_rx.recv().await.expect("download step 2");
        assert_eq!(
            std::str::from_utf8(&step2).expect("step 2 utf8"),
            "ab00034b07010586\r\n"
        );
        assert!(sink.feed(status_reply(0x01).as_bytes()).await);

        // Step 3: start (sub-cmd 0x01, state=0x01)
        let step3 = cmd_rx.recv().await.expect("download step 3");
        assert_eq!(
            std::str::from_utf8(&step3).expect("step 3 utf8"),
            "ab00024b01011a\r\n"
        );
        assert!(sink.feed(status_reply(0x03).as_bytes()).await);

        let ext = task
            .await
            .expect("task join")
            .expect("start_download result");
        assert_eq!(ext.recording_state, control::RecordingState::Downloading);
    }

    #[tokio::test]
    async fn stop_download_sends_correct_2_step_sequence() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(16);
        let (client, sink) = ControlClient::new(cmd_tx);

        let task = tokio::spawn(async move { client.stop_download().await });

        let status_reply = |byte0: u8| -> String {
            let data_hex = format!("{:02x}0204f60204f60059058f0300", byte0);
            let body = format!("000d4b{}", data_hex);
            let lrc = control::lrc(body.as_bytes());
            format!("ab{}{:02x}", body, lrc)
        };

        // Step 1: stop (sub-cmd 0x01, state=0x00)
        let step1 = cmd_rx.recv().await.expect("stop step 1");
        assert_eq!(
            std::str::from_utf8(&step1).expect("step 1 utf8"),
            "ab00024b010019\r\n"
        );
        assert!(sink.feed(status_reply(0x00).as_bytes()).await);

        // Step 2: cleanup (sub-cmd 0x07, param 0x00)
        let step2 = cmd_rx.recv().await.expect("stop step 2");
        assert_eq!(
            std::str::from_utf8(&step2).expect("step 2 utf8"),
            "ab00024b07001f\r\n"
        );
        assert!(sink.feed(status_reply(0x00).as_bytes()).await);

        task.await
            .expect("task join")
            .expect("stop_download result");
    }

    #[tokio::test]
    async fn stop_download_ignores_unsolicited_downloading_status_before_stop_reply() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(16);
        let (client, sink) = ControlClient::new(cmd_tx);

        let task = tokio::spawn(async move { client.stop_download().await });

        let status_reply = |byte0: u8| -> String {
            let data_hex = format!("{:02x}0204f60204f60059058f0300", byte0);
            let body = format!("000d4b{}", data_hex);
            let lrc = control::lrc(body.as_bytes());
            format!("ab{}{:02x}", body, lrc)
        };

        let step1 = cmd_rx.recv().await.expect("stop step 1");
        assert_eq!(
            std::str::from_utf8(&step1).expect("step 1 utf8"),
            "ab00024b010019\r\n"
        );

        let consumed = sink.feed(status_reply(0x03).as_bytes()).await;
        assert!(
            !consumed,
            "still-downloading status should be treated as unsolicited"
        );

        let maybe_step2 = timeout(Duration::from_millis(50), cmd_rx.recv()).await;
        assert!(
            maybe_step2.is_err(),
            "cleanup step should not be sent before the stop response is received"
        );

        assert!(sink.feed(status_reply(0x00).as_bytes()).await);

        let step2 = cmd_rx.recv().await.expect("stop step 2");
        assert_eq!(
            std::str::from_utf8(&step2).expect("step 2 utf8"),
            "ab00024b07001f\r\n"
        );

        assert!(sink.feed(status_reply(0x00).as_bytes()).await);

        task.await
            .expect("task join")
            .expect("stop_download result");
    }

    #[tokio::test]
    async fn feed_with_no_pending_request_returns_false() {
        let (cmd_tx, _cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (_client, sink) = ControlClient::new(cmd_tx);

        // Feed a valid frame when nothing is pending
        let consumed = sink.feed(b"ab000902260306051855443727cf").await;
        assert!(
            !consumed,
            "feed should return false when no request is pending"
        );
    }

    #[tokio::test]
    async fn feed_with_mismatched_instruction_requeues_pending() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (client, sink) = ControlClient::new(cmd_tx);

        // Start a GetDateTime request
        let task = tokio::spawn(async move { client.get_date_time().await });
        let _cmd = cmd_rx.recv().await.expect("command sent");

        // Feed a frame with a different instruction (0x4c unsolicited status)
        let status_body = "00014c01";
        let lrc_val = control::lrc(status_body.as_bytes());
        let frame = format!("ab{status_body}{lrc_val:02x}");
        let consumed = sink.feed(frame.as_bytes()).await;
        assert!(
            !consumed,
            "mismatched instruction should not consume the pending request"
        );

        // The original request should still be pending — verify by feeding the correct response
        let consumed = sink.feed(b"ab000902260306051855443727cf").await;
        assert!(consumed, "correct response should now be consumed");

        let dt = task
            .await
            .expect("task join")
            .expect("get_date_time result");
        assert_eq!(dt.year, 26);
    }

    #[tokio::test]
    async fn send_inner_timeout_returns_timeout_and_clears_pending() {
        tokio::time::pause();

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (client, _sink) = ControlClient::new(cmd_tx);

        let task = tokio::spawn(async move {
            // Acquire in_flight as send_inner requires
            let guard = client.in_flight.lock().await;
            client.send_inner(&Command::GetDateTime, &guard).await
        });

        // Consume the command so the channel doesn't block
        let _cmd = cmd_rx.recv().await.expect("command sent");

        // Advance time past the 2s timeout without feeding a response
        tokio::time::advance(Duration::from_secs(3)).await;

        let result = task.await.expect("task join");
        assert!(
            matches!(result, Err(ControlClientError::Timeout)),
            "expected Timeout, got {result:?}"
        );
    }

    #[tokio::test]
    async fn send_returns_channel_closed_when_sink_dropped() {
        let (cmd_tx, _cmd_rx) = mpsc::channel::<Vec<u8>>(8);
        let (client, sink) = ControlClient::new(cmd_tx);

        // Drop the sink so the oneshot sender will be dropped when pending is cleared
        drop(sink);

        // Drop the cmd_rx so cmd_tx.send() fails immediately
        drop(_cmd_rx);

        let result = client.get_date_time().await;
        assert!(
            matches!(result, Err(ControlClientError::ChannelClosed)),
            "expected ChannelClosed, got {result:?}"
        );
    }

    #[test]
    fn pending_request_kind_ext_status_write_matches_empty_ack() {
        let frame = ControlFrame::new(0, control::INSTR_EXT_STATUS, vec![]);
        assert!(PendingRequestKind::ExtendedStatusWrite.matches(&frame));
        assert!(!PendingRequestKind::ExtendedStatusQuery.matches(&frame));
    }

    #[test]
    fn pending_request_kind_ext_status_write_matches_full_response() {
        let frame = ControlFrame::new(0, control::INSTR_EXT_STATUS, vec![0; 12]);
        assert!(PendingRequestKind::ExtendedStatusWrite.matches(&frame));
        assert!(PendingRequestKind::ExtendedStatusQuery.matches(&frame));
    }

    #[test]
    fn pending_request_kind_download_stop_write_rejects_downloading_status() {
        let frame = ControlFrame::new(
            0,
            control::INSTR_EXT_STATUS,
            vec![0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert!(!PendingRequestKind::DownloadStopWrite.matches(&frame));
    }

    #[test]
    fn pending_request_kind_download_stop_write_accepts_stopped_status() {
        let frame = ControlFrame::new(
            0,
            control::INSTR_EXT_STATUS,
            vec![0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert!(PendingRequestKind::DownloadStopWrite.matches(&frame));
    }

    #[test]
    fn pending_request_kind_ext_status_query_rejects_short() {
        let frame = ControlFrame::new(0, control::INSTR_EXT_STATUS, vec![0; 5]);
        assert!(!PendingRequestKind::ExtendedStatusQuery.matches(&frame));
        assert!(!PendingRequestKind::ExtendedStatusWrite.matches(&frame));
    }

    #[test]
    fn pending_request_kind_config3_write_matches_empty_ack() {
        let frame = ControlFrame::new(0, control::INSTR_CONFIG3, vec![]);
        assert!(PendingRequestKind::Config3Write.matches(&frame));
        assert!(!PendingRequestKind::Config3Query.matches(&frame));
    }

    #[test]
    fn pending_request_kind_config3_query_matches_data_response() {
        let frame = ControlFrame::new(0, control::INSTR_CONFIG3, vec![0x03, 0x05]);
        assert!(!PendingRequestKind::Config3Write.matches(&frame));
        assert!(PendingRequestKind::Config3Query.matches(&frame));
    }

    #[test]
    fn pending_request_kind_tag_message_format_write_matches_empty_ack() {
        let frame = ControlFrame::new(0, control::INSTR_TAG_MESSAGE_FORMAT, vec![]);
        assert!(PendingRequestKind::TagMessageFormatWrite.matches(&frame));
        assert!(!PendingRequestKind::TagMessageFormatQuery.matches(&frame));
    }

    #[test]
    fn pending_request_kind_tag_message_format_query_matches_data_response() {
        let frame = ControlFrame::new(
            0,
            control::INSTR_TAG_MESSAGE_FORMAT,
            vec![0x7f, 0xfc, 0x61, 0x61, 0xaa, 0x00, 0x0d, 0x0a],
        );
        assert!(!PendingRequestKind::TagMessageFormatWrite.matches(&frame));
        assert!(PendingRequestKind::TagMessageFormatQuery.matches(&frame));
    }

    #[test]
    fn pending_request_kind_tag_message_format_query_matches_10_byte_response() {
        let frame = ControlFrame::new(
            0,
            control::INSTR_TAG_MESSAGE_FORMAT,
            vec![0x7f, 0xfc, 0x61, 0x61, 0xaa, 0x00, 0x0d, 0x0a, 0x00, 0x00],
        );
        assert!(!PendingRequestKind::TagMessageFormatWrite.matches(&frame));
        assert!(PendingRequestKind::TagMessageFormatQuery.matches(&frame));
    }

    #[test]
    fn pending_request_kind_any_instruction_matches() {
        let frame = ControlFrame::new(0, control::INSTR_GET_DATE_TIME, vec![0; 9]);
        assert!(PendingRequestKind::AnyInstruction(control::INSTR_GET_DATE_TIME).matches(&frame));
        assert!(!PendingRequestKind::AnyInstruction(control::INSTR_GET_STATISTICS).matches(&frame));
    }

    #[tokio::test]
    async fn download_tracker_update_progress_ignored_when_idle() {
        let mut tracker = DownloadTracker::new();
        tracker.update_progress(50, 100);
        assert_eq!(tracker.download_progress(), 0);
        assert_eq!(tracker.stored_data_extent(), 0);
    }

    #[tokio::test]
    async fn download_tracker_update_progress_ignored_when_complete() {
        let mut tracker = DownloadTracker::new();
        tracker.begin_startup();
        tracker.start(100);
        tracker.complete();
        tracker.update_progress(50, 100);
        // download_progress was 0 from start(), complete() doesn't change it
        assert_eq!(tracker.download_progress(), 0);
    }

    #[tokio::test]
    async fn download_tracker_begin_startup_then_start() {
        let mut tracker = DownloadTracker::new();
        assert!(!tracker.is_active());

        tracker.begin_startup();
        assert!(tracker.is_active());
        assert!(!tracker.is_downloading());
        assert_eq!(*tracker.state(), DownloadState::Starting);

        tracker.start(200);
        assert!(tracker.is_active());
        assert!(tracker.is_downloading());
        assert_eq!(tracker.stored_data_extent(), 200);
        assert_eq!(tracker.reads_received(), 0);
    }

    #[tokio::test]
    async fn download_tracker_fail_during_starting() {
        let mut tracker = DownloadTracker::new();
        let mut rx = tracker.subscribe();

        tracker.begin_startup();
        tracker.fail("connection lost".to_string());
        assert_eq!(
            *tracker.state(),
            DownloadState::Error("connection lost".to_string())
        );

        let ev = rx.try_recv().expect("error event");
        assert!(matches!(ev, DownloadEvent::Error { message } if message == "connection lost"));
    }
}

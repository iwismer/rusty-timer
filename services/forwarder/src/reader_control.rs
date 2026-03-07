//! IPICO reader control client for the forwarder.

use ipico_core::control::{self, Command, ControlError, ControlFrame};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{info, warn};

struct PendingRequest {
    instruction: u8,
    reply_tx: oneshot::Sender<Result<ControlFrame, ControlError>>,
}

pub struct ControlClient {
    cmd_tx: mpsc::Sender<Vec<u8>>,
    pending: Arc<Mutex<Option<PendingRequest>>>,
    banner_buf: Arc<Mutex<Vec<String>>>,
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
        let sink = ControlResponseSink {
            pending: pending.clone(),
            banner_buf: banner_buf.clone(),
        };
        let client = ControlClient {
            cmd_tx,
            pending,
            banner_buf,
            timeout: Duration::from_secs(2),
        };
        (client, sink)
    }

    async fn send(&self, cmd: &Command) -> Result<ControlFrame, ControlError> {
        let frame = control::encode_command(cmd, 0x00);
        let instruction = cmd.instruction();

        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            *pending = Some(PendingRequest {
                instruction,
                reply_tx,
            });
        }

        self.cmd_tx
            .send(frame)
            .await
            .map_err(|_| ControlError::Timeout)?;

        match tokio::time::timeout(self.timeout, reply_rx).await {
            Ok(Ok(result)) => result,
            _ => {
                let mut pending = self.pending.lock().await;
                *pending = None;
                Err(ControlError::Timeout)
            }
        }
    }

    pub async fn get_date_time(&self) -> Result<control::ReaderDateTime, ControlError> {
        let frame = self.send(&Command::GetDateTime).await?;
        control::decode_date_time(&frame)
    }

    pub async fn get_statistics(&self) -> Result<control::ReaderStatistics, ControlError> {
        let frame = self.send(&Command::GetStatistics).await?;
        control::decode_statistics(&frame)
    }

    pub async fn get_extended_status(&self) -> Result<control::ExtendedStatus, ControlError> {
        let frame = self.send(&Command::GetExtendedStatus).await?;
        control::decode_extended_status(&frame)
    }

    pub async fn get_config3(&self) -> Result<(control::ReadMode, u8), ControlError> {
        let frame = self.send(&Command::GetConfig3).await?;
        control::decode_config3(&frame)
    }

    pub async fn set_config3(
        &self,
        mode: control::ReadMode,
        timeout: u8,
    ) -> Result<(), ControlError> {
        let _frame = self.send(&Command::SetConfig3 { mode, timeout }).await?;
        Ok(())
    }

    pub async fn set_date_time(
        &self,
        year: u8,
        month: u8,
        day: u8,
        dow: u8,
        hour: u8,
        min: u8,
        sec: u8,
    ) -> Result<(), ControlError> {
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

    pub async fn print_banner(&self) -> Result<String, ControlError> {
        {
            self.banner_buf.lock().await.clear();
        }
        let _frame = self.send(&Command::PrintBanner).await?;
        let buf = self.banner_buf.lock().await;
        Ok(buf.join("\n").trim().to_owned())
    }

    /// Send the 3-step clear records sequence + CONFIG3 cycling.
    pub async fn clear_records(&self) -> Result<(), ControlError> {
        // Step 1: reset address
        self.cmd_tx
            .send(control::encode_command(
                &Command::SetExtendedStatus {
                    data: vec![0x00, 0x00],
                },
                0x00,
            ))
            .await
            .map_err(|_| ControlError::Timeout)?;

        // Step 2: set mode
        self.cmd_tx
            .send(control::encode_command(
                &Command::SetExtendedStatus {
                    data: vec![0x01, 0x00],
                },
                0x00,
            ))
            .await
            .map_err(|_| ControlError::Timeout)?;

        // Step 3: trigger erase
        self.cmd_tx
            .send(control::encode_command(
                &Command::SetExtendedStatus { data: vec![0xd0] },
                0x00,
            ))
            .await
            .map_err(|_| ControlError::Timeout)?;

        // Wait for erase to complete
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Reset counter
        self.cmd_tx
            .send(control::encode_command(
                &Command::SetExtendedStatus {
                    data: vec![0x00, 0x00],
                },
                0x00,
            ))
            .await
            .map_err(|_| ControlError::Timeout)?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Cycle CONFIG3: Event -> Raw
        self.set_config3(control::ReadMode::Event, 5).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.set_config3(control::ReadMode::Raw, 5).await?;

        Ok(())
    }
}

impl ControlResponseSink {
    /// Feed an `ab`-prefixed control response line (without \r\n).
    pub async fn feed(&self, line: &[u8]) -> bool {
        let result = control::parse_response(line);
        let mut pending = self.pending.lock().await;
        if let Some(req) = pending.take() {
            match &result {
                Ok(frame) if frame.instruction == req.instruction => {
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
        if let Ok(s) = std::str::from_utf8(line) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                self.banner_buf.lock().await.push(trimmed.to_owned());
            }
        }
    }
}

/// Reader info gathered on connect and refreshed by polling.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReaderInfo {
    pub banner: Option<String>,
    pub fw_version: Option<String>,
    pub hw_code: Option<u8>,
    pub reader_id: Option<u8>,
    pub config3: Option<u8>,
    pub unique_tag_count: Option<u16>,
    pub stored_record_pages: Option<u8>,
    pub reader_clock: Option<String>,
    pub clock_drift_ms: Option<i64>,
    pub read_mode: Option<String>,
    pub read_mode_timeout: Option<u8>,
}

/// Run the initial connection sequence: statistics, banner, ext status, config3, clock.
pub async fn run_connect_sequence(client: &ControlClient) -> ReaderInfo {
    let mut ri = ReaderInfo::default();

    match client.get_statistics().await {
        Ok(stats) => {
            ri.fw_version = Some(stats.fw_version_string());
            ri.hw_code = Some(stats.hw_code);
            ri.reader_id = Some(stats.reader_id);
            ri.config3 = Some(stats.config3);
            info!(fw = %stats.fw_version_string(), hw = stats.hw_code, "reader statistics");
        }
        Err(e) => warn!("get_statistics failed: {}", e),
    }

    match client.print_banner().await {
        Ok(banner) if !banner.is_empty() => {
            info!(banner = %banner, "reader banner");
            ri.banner = Some(banner);
        }
        Ok(_) => {}
        Err(e) => warn!("print_banner failed: {}", e),
    }

    match client.get_extended_status().await {
        Ok(ext) => {
            ri.unique_tag_count = Some(ext.unique_tag_count);
            ri.stored_record_pages = Some(ext.stored_record_pages);
            info!(
                unique_tags = ext.unique_tag_count,
                stored_pages = ext.stored_record_pages,
                "reader extended status"
            );
        }
        Err(e) => warn!("get_extended_status failed: {}", e),
    }

    match client.get_config3().await {
        Ok((mode, timeout)) => {
            ri.read_mode = Some(mode.as_str().to_owned());
            ri.read_mode_timeout = Some(timeout);
        }
        Err(e) => warn!("get_config3 failed: {}", e),
    }

    poll_clock(client, &mut ri).await;

    ri
}

/// Poll clock and extended status, updating info in place.
pub async fn run_status_poll(client: &ControlClient, info: &mut ReaderInfo) {
    if let Ok(ext) = client.get_extended_status().await {
        info.unique_tag_count = Some(ext.unique_tag_count);
        info.stored_record_pages = Some(ext.stored_record_pages);
    }

    if let Ok((mode, timeout)) = client.get_config3().await {
        info.read_mode = Some(mode.as_str().to_owned());
        info.read_mode_timeout = Some(timeout);
    }

    poll_clock(client, info).await;
}

async fn poll_clock(client: &ControlClient, info: &mut ReaderInfo) {
    if let Ok(dt) = client.get_date_time().await {
        let reader_iso = dt.to_iso_string();
        info.reader_clock = Some(reader_iso.clone());

        let now = chrono::Local::now();
        if let Ok(reader_naive) =
            chrono::NaiveDateTime::parse_from_str(&reader_iso, "%Y-%m-%dT%H:%M:%S%.3f")
        {
            let system_naive = now.naive_local();
            let drift = system_naive
                .signed_duration_since(reader_naive)
                .num_milliseconds();
            info.clock_drift_ms = Some(drift);
        }
    }
}

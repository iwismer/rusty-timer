//! Shared reader-control operations used by HTTP and P2P control paths.

use crate::reader_control::{ControlClient, DownloadTracker};
use crate::status_store::{ForwarderStatusEvent, SubsystemStatus};
use ipico_core::control;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, broadcast};

/// Fixed delay (ms) from SET_DATE_TIME receipt to the new second taking effect.
///
/// The reader resets cs to ~52 and the rollover to second S.000 occurs ~480ms
/// later; the reader's 0x4c frame reports 500ms.
const SYNC_DELAY_MS: u64 = 500;

#[derive(Clone)]
pub struct ReaderControlService {
    subsystem: Arc<Mutex<SubsystemStatus>>,
    control_clients: Arc<std::sync::RwLock<HashMap<String, Arc<ControlClient>>>>,
    download_trackers: Arc<std::sync::RwLock<HashMap<String, Arc<Mutex<DownloadTracker>>>>>,
    reconnect_notifies: Arc<std::sync::RwLock<HashMap<String, Arc<Notify>>>>,
    ui_tx: broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
    logger: Arc<rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
}

impl ReaderControlService {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        subsystem: Arc<Mutex<SubsystemStatus>>,
        control_clients: Arc<std::sync::RwLock<HashMap<String, Arc<ControlClient>>>>,
        download_trackers: Arc<std::sync::RwLock<HashMap<String, Arc<Mutex<DownloadTracker>>>>>,
        reconnect_notifies: Arc<std::sync::RwLock<HashMap<String, Arc<Notify>>>>,
        ui_tx: broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
        status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
        logger: Arc<rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
    ) -> Self {
        Self {
            subsystem,
            control_clients,
            download_trackers,
            reconnect_notifies,
            ui_tx,
            status_event_tx,
            logger,
        }
    }

    pub async fn get_info(
        &self,
        reader_ip: &str,
    ) -> Result<crate::reader_control::ReaderInfo, String> {
        self.client(reader_ip)?;
        Ok(self.cached_reader_info(reader_ip).await.unwrap_or_default())
    }

    pub async fn refresh(
        &self,
        reader_ip: &str,
    ) -> Result<crate::reader_control::ReaderInfo, String> {
        let client = self.client(reader_ip)?;
        let mut info = self.cached_reader_info(reader_ip).await.unwrap_or_default();
        crate::reader_control::run_status_poll(&client, &mut info).await;
        self.update_cached_reader_info(reader_ip, info.clone())
            .await;
        Ok(info)
    }

    pub async fn set_epoch_name(
        &self,
        reader_ip: &str,
        name: Option<String>,
    ) -> Result<crate::reader_control::ReaderInfo, String> {
        let status = {
            let mut ss = self.subsystem.lock().await;
            let reader = ss
                .readers
                .get_mut(reader_ip)
                .ok_or_else(|| "reader not found".to_owned())?;
            reader.current_epoch_name = name;
            let status = reader.clone();
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: (&reader.state).into(),
                    reads_session: reader.reads_since_restart,
                    reads_total: reader.reads_total,
                    reads_epoch: reader.reads_epoch,
                    last_seen_secs: reader.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: reader.local_port,
                    current_epoch_name: reader.current_epoch_name.clone(),
                });
            let _ = self
                .status_event_tx
                .send(ForwarderStatusEvent::ReaderStatus {
                    stream_id: reader_ip.to_owned(),
                    status: status.clone(),
                });
            status
        };
        Ok(status.reader_info.unwrap_or_default())
    }

    pub async fn set_current_epoch_metadata(
        &self,
        reader_ip: &str,
        metadata: crate::storage::journal::CurrentEpochMetadata,
    ) {
        let mut ss = self.subsystem.lock().await;
        if let Some(status) = ss.readers.get_mut(reader_ip) {
            if status.current_epoch.is_some_and(|e| e != metadata.epoch) {
                // Epoch changed: the new epoch starts with zero reads.
                status.reads_epoch = 0;
            }
            status.current_epoch = Some(metadata.epoch);
            status.current_epoch_created_unix_ms = metadata.created_unix_ms;
            status.current_epoch_name = None;
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: (&status.state).into(),
                    reads_session: status.reads_since_restart,
                    reads_total: status.reads_total,
                    reads_epoch: status.reads_epoch,
                    last_seen_secs: status.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: status.local_port,
                    current_epoch_name: None,
                });
            let _ = self
                .status_event_tx
                .send(ForwarderStatusEvent::ReaderStatus {
                    stream_id: reader_ip.to_owned(),
                    status: status.clone(),
                });
        }
    }

    pub async fn emit_status_refresh(&self, reader_ip: &str) {
        let ss = self.subsystem.lock().await;
        if let Some(status) = ss.readers.get(reader_ip) {
            let _ = self
                .status_event_tx
                .send(ForwarderStatusEvent::ReaderStatus {
                    stream_id: reader_ip.to_owned(),
                    status: status.clone(),
                });
        }
    }

    pub async fn sync_clock(
        &self,
        reader_ip: &str,
    ) -> Result<crate::reader_control::ReaderInfo, String> {
        let client = self.client(reader_ip)?;

        let (one_way, _probes) = estimate_one_way_latency(&client).await?;
        let wall_now = chrono::Local::now();
        let (target_boundary, pre_set_wait) = compute_sync_timing(wall_now, one_way, SYNC_DELAY_MS);
        if !pre_set_wait.is_zero() {
            tokio::time::sleep(pre_set_wait).await;
        }

        use chrono::{Datelike, Timelike};
        let year = (target_boundary.year() % 100) as u8;
        let month = target_boundary.month() as u8;
        let day = target_boundary.day() as u8;
        let dow = target_boundary.weekday().num_days_from_sunday() as u8;
        let hour = target_boundary.hour() as u8;
        let minute = target_boundary.minute() as u8;
        let second = target_boundary.second() as u8;

        if let Err(e) = client
            .set_date_time(year, month, day, dow, hour, minute, second)
            .await
        {
            let mut info = self.cached_reader_info(reader_ip).await.unwrap_or_default();
            info.clock = None;
            self.update_cached_reader_info(reader_ip, info).await;
            return Err(e.to_string());
        }

        let verify_wait = std::time::Duration::from_millis(SYNC_DELAY_MS) + one_way;
        tokio::time::sleep(verify_wait).await;

        let dt = client
            .get_date_time()
            .await
            .map_err(|e| format!("set ok but verify failed: {e}"))?;
        let reader_iso = dt.to_iso_string();
        let verify_now = chrono::Local::now();
        let drift_ms = match chrono::NaiveDateTime::parse_from_str(
            &reader_iso,
            "%Y-%m-%dT%H:%M:%S%.3f",
        ) {
            Ok(reader_naive) => Some(
                verify_now
                    .naive_local()
                    .signed_duration_since(reader_naive)
                    .num_milliseconds(),
            ),
            Err(e) => {
                tracing::warn!(
                    reader_ip = %reader_ip,
                    reader_clock = %reader_iso,
                    error = %e,
                    "clock sync verification: failed to parse reader timestamp for drift calculation"
                );
                None
            }
        };

        let mut info = self.cached_reader_info(reader_ip).await.unwrap_or_default();
        info.clock = drift_ms.map(|d| crate::reader_control::ClockInfo {
            reader_clock: reader_iso.clone(),
            drift_ms: d,
        });
        self.update_cached_reader_info(reader_ip, info.clone())
            .await;

        self.logger.log(format!(
            "reader {} clock synced to {} (one-way latency: {:.1}ms, pre-set wait: {:.0}ms, sync delay: {}ms)",
            reader_ip,
            reader_iso,
            one_way.as_secs_f64() * 1000.0,
            pre_set_wait.as_secs_f64() * 1000.0,
            SYNC_DELAY_MS,
        ));
        Ok(info)
    }

    pub async fn get_read_mode(&self, reader_ip: &str) -> Result<(control::ReadMode, u8), String> {
        self.client(reader_ip)?
            .get_config3()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn set_read_mode(
        &self,
        reader_ip: &str,
        mode: control::ReadMode,
        timeout: u8,
    ) -> Result<crate::reader_control::ReaderInfo, String> {
        let client = self.client(reader_ip)?;
        client
            .set_config3(mode, timeout)
            .await
            .map_err(|e| e.to_string())?;
        self.logger
            .log(format!("reader {} read mode set to {}", reader_ip, mode));

        let mut info = self.cached_reader_info(reader_ip).await.unwrap_or_default();
        info.config = Some(crate::reader_control::Config3Info { mode, timeout });
        info.clock = None;
        crate::reader_control::run_status_poll_merge_successes(&client, &mut info).await;
        self.update_cached_reader_info(reader_ip, info.clone())
            .await;
        Ok(info)
    }

    pub async fn get_tto(&self, reader_ip: &str) -> Result<bool, String> {
        self.client(reader_ip)?
            .get_tag_message_format()
            .await
            .map(|format| format.tto_enabled())
            .map_err(|e| e.to_string())
    }

    pub async fn set_tto(
        &self,
        reader_ip: &str,
        enabled: bool,
    ) -> Result<crate::reader_control::ReaderInfo, String> {
        let client = self.client(reader_ip)?;
        let current = client
            .get_tag_message_format()
            .await
            .map_err(|e| e.to_string())?;
        let updated = current.with_tto_enabled(enabled);
        client
            .set_tag_message_format(updated)
            .await
            .map_err(|e| e.to_string())?;
        let enabled = client
            .get_tag_message_format()
            .await
            .map_err(|e| format!("set ok but verify failed: {e}"))?
            .tto_enabled();

        let mut info = self.cached_reader_info(reader_ip).await.unwrap_or_default();
        info.tto_enabled = Some(enabled);
        self.update_cached_reader_info(reader_ip, info.clone())
            .await;
        let label = if enabled { "enabled" } else { "disabled" };
        self.logger
            .log(format!("reader {} TTO reporting {}", reader_ip, label));
        Ok(info)
    }

    pub async fn set_recording(
        &self,
        reader_ip: &str,
        enabled: bool,
    ) -> Result<crate::reader_control::ReaderInfo, String> {
        let client = self.client(reader_ip)?;
        let label = if enabled { "on" } else { "off" };
        self.logger
            .log(format!("reader {} setting recording {}", reader_ip, label));
        let ext = client
            .set_recording(enabled)
            .await
            .map_err(|e| e.to_string())?;
        let mut info = self.cached_reader_info(reader_ip).await.unwrap_or_default();
        info.recording = Some(ext.recording_state.is_recording());
        info.estimated_stored_reads = Some(ext.estimated_stored_reads());
        crate::reader_control::run_status_poll(&client, &mut info).await;
        self.update_cached_reader_info(reader_ip, info.clone())
            .await;
        Ok(info)
    }

    pub async fn reconnect(&self, reader_ip: &str) -> Result<(), String> {
        let notify = self
            .reconnect_notifies
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(reader_ip)
            .cloned();
        match notify {
            Some(n) => {
                n.notify_one();
                Ok(())
            }
            None => Err("reader not found".to_owned()),
        }
    }

    pub async fn clear_records(&self, reader_ip: &str) -> Result<(), String> {
        let client = self.client(reader_ip)?;
        self.logger
            .log(format!("reader {} clearing onboard records...", reader_ip));
        client.clear_records().await.map_err(|e| e.to_string())?;
        self.logger
            .log(format!("reader {} records cleared", reader_ip));
        Ok(())
    }

    pub async fn start_download(&self, reader_ip: &str) -> Result<u32, String> {
        let client = self.client(reader_ip)?;
        let tracker = self.download_tracker(reader_ip)?;

        {
            let mut dt = tracker.lock().await;
            match dt.state() {
                crate::reader_control::DownloadState::Starting
                | crate::reader_control::DownloadState::Downloading => {
                    return Err("download already in progress".to_owned());
                }
                crate::reader_control::DownloadState::Complete
                | crate::reader_control::DownloadState::Error(_) => {
                    dt.reset();
                }
                crate::reader_control::DownloadState::Idle => {}
            }
            dt.begin_startup();
        }

        let estimated_reads = self
            .cached_reader_info(reader_ip)
            .await
            .and_then(|ri| ri.estimated_stored_reads)
            .unwrap_or(0);

        let bg_tracker = tracker.clone();
        let bg_ip = reader_ip.to_owned();
        tokio::spawn(
            async move {
                match client.start_download().await {
                    Ok(ext) => {
                        let mut dt = bg_tracker.lock().await;
                        dt.start(ext.stored_data_extent);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "download start failed");
                        let mut dt = bg_tracker.lock().await;
                        dt.fail(format!("{e}"));
                    }
                }
            }
            .instrument(tracing::info_span!("download_start", reader_ip = %bg_ip)),
        );

        Ok(estimated_reads)
    }

    pub async fn stop_download(&self, reader_ip: &str) -> Result<(), String> {
        self.client(reader_ip)?
            .stop_download()
            .await
            .map_err(|e| e.to_string())
    }

    fn client(&self, reader_ip: &str) -> Result<Arc<ControlClient>, String> {
        self.control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(reader_ip)
            .cloned()
            .ok_or_else(|| "reader not connected".to_owned())
    }

    fn download_tracker(&self, reader_ip: &str) -> Result<Arc<Mutex<DownloadTracker>>, String> {
        self.download_trackers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(reader_ip)
            .cloned()
            .ok_or_else(|| "reader not connected".to_owned())
    }

    async fn cached_reader_info(
        &self,
        reader_ip: &str,
    ) -> Option<crate::reader_control::ReaderInfo> {
        self.subsystem.lock().await.cached_reader_info(reader_ip)
    }

    async fn update_cached_reader_info(
        &self,
        reader_ip: &str,
        info: crate::reader_control::ReaderInfo,
    ) {
        {
            let mut ss = self.subsystem.lock().await;
            if !ss.update_cached_reader_info_unless_disconnected(reader_ip, info.clone()) {
                return;
            }
        }
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated {
                ip: reader_ip.to_owned(),
                info: info.clone(),
            });
        let _ = self.status_event_tx.send(ForwarderStatusEvent::ReaderInfo {
            stream_id: reader_ip.to_owned(),
            info,
        });
    }
}

/// Estimate one-way network latency to a reader by measuring RTT of GET_DATE_TIME probes.
/// Returns (median one-way latency, successful probe count) from 3 probes.
/// With an even number of successful probes, takes the upper-middle value (conservative estimate).
pub async fn estimate_one_way_latency(
    client: &ControlClient,
) -> Result<(std::time::Duration, usize), String> {
    const PROBES: usize = 3;
    let mut rtts = Vec::with_capacity(PROBES);
    for i in 0..PROBES {
        let start = std::time::Instant::now();
        match client.get_date_time().await {
            Ok(_) => rtts.push(start.elapsed()),
            Err(e) => tracing::warn!(probe = i + 1, error = %e, "RTT probe failed"),
        }
    }
    if rtts.is_empty() {
        return Err(
            "all RTT probes failed; cannot estimate network latency for clock sync".to_owned(),
        );
    }
    if rtts.len() < PROBES {
        tracing::warn!(
            successful = rtts.len(),
            total = PROBES,
            "clock sync latency estimate based on fewer probes than requested"
        );
    }
    rtts.sort();
    let median_rtt = rtts[rtts.len() / 2];
    Ok((median_rtt / 2, rtts.len()))
}

/// Compute the target second boundary and pre-SET wait duration for clock sync.
///
/// Given the current wall time, one-way latency estimate, and the fixed sync delay,
/// returns `(target_boundary, pre_set_wait)` where:
/// - `target_boundary` is the `DateTime<Local>` whole-second that the rollover should align with
/// - `pre_set_wait` is how long to sleep before sending SET_DATE_TIME
#[must_use]
pub fn compute_sync_timing(
    wall_now: chrono::DateTime<chrono::Local>,
    one_way: std::time::Duration,
    sync_delay_ms: u64,
) -> (chrono::DateTime<chrono::Local>, std::time::Duration) {
    use chrono::Timelike;

    let arrival_offset = chrono::Duration::from_std(one_way).unwrap_or_else(|_| {
        tracing::warn!(
            one_way_ms = one_way.as_millis(),
            "one-way latency exceeds chrono Duration range, falling back to zero"
        );
        chrono::Duration::zero()
    });
    let sync_delay = chrono::Duration::milliseconds(sync_delay_ms as i64);
    let wall_at_rollover_if_now = wall_now + arrival_offset + sync_delay;
    let rollover_frac = wall_at_rollover_if_now.nanosecond() as f64 / 1_000_000_000.0;

    let target = if rollover_frac >= 0.5 {
        wall_at_rollover_if_now + chrono::Duration::seconds(1)
    } else {
        wall_at_rollover_if_now
    };
    let target_boundary_initial = target
        .with_nanosecond(0)
        .expect("nanosecond 0 is always valid");

    let mut target_boundary = target_boundary_initial;
    let mut ideal_send = target_boundary - arrival_offset - sync_delay;
    if ideal_send < wall_now {
        target_boundary += chrono::Duration::seconds(1);
        ideal_send = target_boundary - arrival_offset - sync_delay;
    }
    let pre_set_wait = ideal_send
        .signed_duration_since(wall_now)
        .to_std()
        .unwrap_or(std::time::Duration::ZERO);

    (target_boundary, pre_set_wait)
}

#[must_use]
pub fn native_info_to_domain(info: &crate::reader_control::ReaderInfo) -> rt_domain::ReaderInfo {
    rt_domain::ReaderInfo {
        banner: info.banner.clone(),
        hardware: info
            .hardware
            .as_ref()
            .map(|hardware| rt_domain::HardwareInfo {
                fw_version: Some(hardware.fw_version.clone()),
                hw_code: Some(hardware.hw_code.to_string()),
                reader_id: Some(hardware.reader_id.to_string()),
            }),
        config: info.config.as_ref().map(|config| rt_domain::Config3Info {
            mode: native_read_mode_to_domain(config.mode),
            timeout: config.timeout,
        }),
        tto_enabled: info.tto_enabled,
        clock: info.clock.as_ref().map(|clock| rt_domain::ClockInfo {
            reader_clock: clock.reader_clock.clone(),
            drift_ms: clock.drift_ms,
        }),
        estimated_stored_reads: info.estimated_stored_reads,
        recording: info.recording,
        connect_failures: info.connect_failures,
    }
}

#[must_use]
pub fn native_read_mode_to_domain(mode: control::ReadMode) -> rt_domain::ReadMode {
    match mode {
        control::ReadMode::Raw => rt_domain::ReadMode::Raw,
        control::ReadMode::Event => rt_domain::ReadMode::Event,
        control::ReadMode::FirstLastSeen => rt_domain::ReadMode::FirstLastSeen,
    }
}

#[must_use]
pub fn domain_read_mode_to_native(mode: rt_domain::ReadMode) -> control::ReadMode {
    match mode {
        rt_domain::ReadMode::Raw => control::ReadMode::Raw,
        rt_domain::ReadMode::Event => control::ReadMode::Event,
        rt_domain::ReadMode::FirstLastSeen => control::ReadMode::FirstLastSeen,
    }
}

pub fn parse_native_read_mode(mode: &str) -> Result<control::ReadMode, String> {
    match mode {
        "raw" => Ok(control::ReadMode::Raw),
        "event" => Ok(control::ReadMode::Event),
        "fsls" => Ok(control::ReadMode::FirstLastSeen),
        _ => Err(format!("unknown mode: {mode}")),
    }
}

use tracing::Instrument;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    #[tokio::test]
    async fn set_current_epoch_metadata_updates_status_and_broadcasts_reader_status() {
        let subsystem = Arc::new(Mutex::new(crate::status_store::SubsystemStatus::ready()));
        {
            let mut ss = subsystem.lock().await;
            ss.readers.insert(
                "reader-a".to_owned(),
                crate::status_store::ReaderStatus {
                    state: crate::status_store::ReaderConnectionState::Connected,
                    last_seen: None,
                    reads_since_restart: 0,
                    reads_total: 0,
                    reads_epoch: 0,
                    local_port: 10_001,
                    current_epoch: None,
                    current_epoch_created_unix_ms: None,
                    current_epoch_name: None,
                    reader_info: None,
                },
            );
        }
        let control_clients = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let download_trackers = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let reconnect_notifies = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let (ui_tx, _) = tokio::sync::broadcast::channel(16);
        let (status_event_tx, mut status_event_rx) = tokio::sync::broadcast::channel(16);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            ui_tx.clone(),
            |entry| crate::ui_events::ForwarderUiEvent::LogEntry { entry },
            16,
        ));
        let service = ReaderControlService::new(
            subsystem,
            control_clients,
            download_trackers,
            reconnect_notifies,
            ui_tx,
            status_event_tx,
            logger,
        );

        service
            .set_current_epoch_metadata(
                "reader-a",
                crate::storage::journal::CurrentEpochMetadata {
                    epoch: 3,
                    created_unix_ms: Some(1_783_238_640_000),
                },
            )
            .await;

        let event = status_event_rx.recv().await.expect("reader status event");
        let crate::status_store::ForwarderStatusEvent::ReaderStatus { stream_id, status } = event
        else {
            panic!("expected reader status event");
        };
        assert_eq!(stream_id, "reader-a");
        assert_eq!(status.current_epoch, Some(3));
        assert_eq!(
            status.current_epoch_created_unix_ms,
            Some(1_783_238_640_000)
        );
    }

    #[test]
    fn compute_sync_timing_rounds_to_nearest_boundary_and_delays_send() {
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 6, 22, 12, 0, 0)
            .single()
            .expect("valid local time")
            + chrono::Duration::milliseconds(100);

        let (target, wait) = compute_sync_timing(
            wall_now,
            std::time::Duration::from_millis(25),
            SYNC_DELAY_MS,
        );

        assert_eq!(target.nanosecond(), 0);
        assert!(target > wall_now);
        assert_eq!(wait, std::time::Duration::from_millis(375));
    }

    #[test]
    fn sync_timing_rollover_frac_above_half_rounds_up() {
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(200_000_000)
            .unwrap();
        let one_way = std::time::Duration::from_millis(50);
        let (target, wait) = compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 1);
        assert_eq!(target.nanosecond(), 0);
        assert!(wait < std::time::Duration::from_secs(1));
    }

    #[test]
    fn sync_timing_rollover_frac_below_half_stays_same_second() {
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(800_000_000)
            .unwrap();
        let one_way = std::time::Duration::from_millis(50);
        let (target, _wait) = compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 2);
        assert_eq!(target.nanosecond(), 0);
    }

    #[test]
    fn sync_timing_ideal_send_past_bumps_target() {
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(500_000_000)
            .unwrap();
        let one_way = std::time::Duration::from_millis(1);
        let (target, wait) = compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 2);
        assert_eq!(target.nanosecond(), 0);
        assert!(wait > std::time::Duration::from_millis(900));
        assert!(wait < std::time::Duration::from_millis(1100));
    }

    #[test]
    fn sync_timing_zero_latency() {
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(300_000_000)
            .unwrap();
        let one_way = std::time::Duration::ZERO;
        let (target, _wait) = compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 1);
        assert_eq!(target.nanosecond(), 0);
    }
}

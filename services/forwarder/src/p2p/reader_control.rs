//! P2P adapter for forwarder reader-control operations.

use super::{ReaderControlFuture, ReaderControlHandler};
use crate::reader_control_service::{ReaderControlService, domain_read_mode_to_native};
use rt_p2p_protocol::{ReaderControlRequest, ReaderControlResponse};

#[derive(Clone)]
pub struct ForwarderReaderControlHandler {
    service: ReaderControlService,
}

impl std::fmt::Debug for ForwarderReaderControlHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwarderReaderControlHandler")
            .finish_non_exhaustive()
    }
}

impl ForwarderReaderControlHandler {
    #[must_use]
    pub fn new(service: ReaderControlService) -> Self {
        Self { service }
    }
}

impl ReaderControlHandler for ForwarderReaderControlHandler {
    fn supports_reader_control(&self) -> bool {
        true
    }

    fn handle(&self, request: ReaderControlRequest) -> ReaderControlFuture<'_> {
        Box::pin(async move {
            let stream_id = request.stream_id.clone();
            let request_id = request.request_id.clone();
            let reader_key = match std::str::from_utf8(&request.stream_id) {
                Ok(value) => value.to_owned(),
                Err(error) => {
                    return error_response(
                        stream_id,
                        request_id,
                        format!("stream id is not UTF-8: {error}"),
                    );
                }
            };
            let action = match request_to_action(&request) {
                Ok(action) => action,
                Err(error) => return error_response(stream_id, request_id, error),
            };

            match dispatch_action(&self.service, &reader_key, action).await {
                Ok(info) => success_response(stream_id, request_id, "ok", info.as_ref()),
                Err(error) => error_response(stream_id, request_id, error),
            }
        })
    }
}

async fn dispatch_action(
    service: &ReaderControlService,
    reader_key: &str,
    action: rt_domain::ReaderControlAction,
) -> Result<Option<rt_domain::ReaderInfo>, String> {
    match action {
        rt_domain::ReaderControlAction::GetInfo => service
            .get_info(reader_key)
            .await
            .map(|info| Some(crate::reader_control_service::native_info_to_domain(&info))),
        rt_domain::ReaderControlAction::SyncClock => service
            .sync_clock(reader_key)
            .await
            .map(|info| Some(crate::reader_control_service::native_info_to_domain(&info))),
        rt_domain::ReaderControlAction::SetReadMode { mode, timeout } => service
            .set_read_mode(reader_key, domain_read_mode_to_native(mode), timeout)
            .await
            .map(|info| Some(crate::reader_control_service::native_info_to_domain(&info))),
        rt_domain::ReaderControlAction::SetTto { enabled } => service
            .set_tto(reader_key, enabled)
            .await
            .map(|info| Some(crate::reader_control_service::native_info_to_domain(&info))),
        rt_domain::ReaderControlAction::SetRecording { enabled } => service
            .set_recording(reader_key, enabled)
            .await
            .map(|info| Some(crate::reader_control_service::native_info_to_domain(&info))),
        rt_domain::ReaderControlAction::ClearRecords => {
            service.clear_records(reader_key).await.map(|()| None)
        }
        rt_domain::ReaderControlAction::StartDownload => {
            service.start_download(reader_key).await.map(|_| None)
        }
        rt_domain::ReaderControlAction::StopDownload => {
            service.stop_download(reader_key).await.map(|()| None)
        }
        rt_domain::ReaderControlAction::Refresh => service
            .refresh(reader_key)
            .await
            .map(|info| Some(crate::reader_control_service::native_info_to_domain(&info))),
        rt_domain::ReaderControlAction::Reconnect => {
            service.reconnect(reader_key).await.map(|()| None)
        }
    }
}

pub(crate) fn request_to_action(
    request: &ReaderControlRequest,
) -> Result<rt_domain::ReaderControlAction, String> {
    match request.command.as_str() {
        "get_info" => Ok(rt_domain::ReaderControlAction::GetInfo),
        "sync_clock" => Ok(rt_domain::ReaderControlAction::SyncClock),
        "set_read_mode" => {
            let mode = request
                .mode
                .as_deref()
                .ok_or_else(|| "set_read_mode requires mode".to_owned())?;
            let timeout = request
                .timeout
                .ok_or_else(|| "set_read_mode requires timeout".to_owned())?;
            let timeout = u8::try_from(timeout)
                .map_err(|_| format!("set_read_mode timeout out of range: {timeout}"))?;
            Ok(rt_domain::ReaderControlAction::SetReadMode {
                mode: parse_domain_read_mode(mode)?,
                timeout,
            })
        }
        "set_tto" => Ok(rt_domain::ReaderControlAction::SetTto {
            enabled: request
                .enabled
                .ok_or_else(|| "set_tto requires enabled".to_owned())?,
        }),
        "set_recording" => Ok(rt_domain::ReaderControlAction::SetRecording {
            enabled: request
                .enabled
                .ok_or_else(|| "set_recording requires enabled".to_owned())?,
        }),
        "clear_records" => Ok(rt_domain::ReaderControlAction::ClearRecords),
        "start_download" => Ok(rt_domain::ReaderControlAction::StartDownload),
        "stop_download" => Ok(rt_domain::ReaderControlAction::StopDownload),
        "refresh" => Ok(rt_domain::ReaderControlAction::Refresh),
        "reconnect" => Ok(rt_domain::ReaderControlAction::Reconnect),
        other => Err(format!("unsupported reader control command: {other}")),
    }
}

fn parse_domain_read_mode(mode: &str) -> Result<rt_domain::ReadMode, String> {
    match mode {
        "raw" => Ok(rt_domain::ReadMode::Raw),
        "event" => Ok(rt_domain::ReadMode::Event),
        "fsls" => Ok(rt_domain::ReadMode::FirstLastSeen),
        _ => Err(format!("unknown mode: {mode}")),
    }
}

pub(crate) fn domain_info_json(info: &rt_domain::ReaderInfo) -> Result<String, String> {
    serde_json::to_string(info).map_err(|e| format!("serialize reader info: {e}"))
}

pub(crate) fn domain_info_to_p2p_event(
    stream_id: &[u8],
    info: rt_domain::ReaderInfo,
) -> Result<rt_p2p_protocol::ReaderInfo, String> {
    let hardware = info.hardware.as_ref();
    Ok(rt_p2p_protocol::ReaderInfo {
        stream_id: stream_id.to_vec(),
        hardware_reader_id: hardware
            .and_then(|hardware| hardware.reader_id.clone())
            .unwrap_or_default(),
        firmware_version: hardware
            .and_then(|hardware| hardware.fw_version.clone())
            .unwrap_or_default(),
        model: hardware
            .and_then(|hardware| hardware.hw_code.clone())
            .unwrap_or_default(),
        reader_info_json: Some(domain_info_json(&info)?),
    })
}

pub(crate) fn success_response(
    stream_id: Vec<u8>,
    request_id: String,
    message: &str,
    info: Option<&rt_domain::ReaderInfo>,
) -> ReaderControlResponse {
    ReaderControlResponse {
        stream_id,
        request_id,
        success: true,
        message: message.to_owned(),
        reader_info_json: info.and_then(|info| domain_info_json(info).ok()),
    }
}

fn error_response(
    stream_id: Vec<u8>,
    request_id: String,
    message: String,
) -> ReaderControlResponse {
    ReaderControlResponse {
        stream_id,
        request_id,
        success: false,
        message,
        reader_info_json: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request(command: &str) -> ReaderControlRequest {
        ReaderControlRequest {
            stream_id: b"10.0.0.5:10000".to_vec(),
            command: command.to_owned(),
            request_id: "req-1".to_owned(),
            mode: None,
            timeout: None,
            enabled: None,
        }
    }

    #[test]
    fn set_read_mode_requires_mode_and_timeout() {
        let request = base_request("set_read_mode");
        assert_eq!(
            request_to_action(&request).expect_err("missing mode"),
            "set_read_mode requires mode"
        );

        let request = ReaderControlRequest {
            mode: Some("event".to_owned()),
            ..base_request("set_read_mode")
        };
        assert_eq!(
            request_to_action(&request).expect_err("missing timeout"),
            "set_read_mode requires timeout"
        );
    }

    #[test]
    fn invalid_mode_returns_error() {
        let request = ReaderControlRequest {
            mode: Some("bad".to_owned()),
            timeout: Some(7),
            ..base_request("set_read_mode")
        };
        assert_eq!(
            request_to_action(&request).expect_err("invalid mode"),
            "unknown mode: bad"
        );
    }

    #[test]
    fn set_tto_requires_enabled() {
        let request = base_request("set_tto");
        assert_eq!(
            request_to_action(&request).expect_err("missing enabled"),
            "set_tto requires enabled"
        );
    }

    #[test]
    fn sync_clock_needs_no_params() {
        assert_eq!(
            request_to_action(&base_request("sync_clock")).expect("sync action"),
            rt_domain::ReaderControlAction::SyncClock
        );
    }

    #[test]
    fn domain_info_to_p2p_event_preserves_static_hardware_and_json() {
        let info = rt_domain::ReaderInfo {
            hardware: Some(rt_domain::HardwareInfo {
                fw_version: Some("1.2.3".to_owned()),
                hw_code: Some("9".to_owned()),
                reader_id: Some("42".to_owned()),
            }),
            tto_enabled: Some(true),
            ..empty_info()
        };

        let event = domain_info_to_p2p_event(b"reader", info).expect("event");

        assert_eq!(event.hardware_reader_id, "42");
        assert_eq!(event.firmware_version, "1.2.3");
        assert_eq!(event.model, "9");
        assert!(
            event
                .reader_info_json
                .expect("json")
                .contains("tto_enabled")
        );
    }

    #[test]
    fn success_response_populates_reader_info_json_when_info_present() {
        let info = rt_domain::ReaderInfo {
            clock: Some(rt_domain::ClockInfo {
                reader_clock: "2026-06-22T12:00:00.000".to_owned(),
                drift_ms: 5,
            }),
            ..empty_info()
        };

        let response = success_response(b"reader".to_vec(), "req-1".to_owned(), "ok", Some(&info));

        assert!(response.success);
        assert!(
            response
                .reader_info_json
                .expect("json")
                .contains("reader_clock")
        );
    }

    fn empty_info() -> rt_domain::ReaderInfo {
        rt_domain::ReaderInfo {
            banner: None,
            hardware: None,
            config: None,
            tto_enabled: None,
            clock: None,
            estimated_stored_reads: None,
            recording: None,
            connect_failures: 0,
        }
    }
}

//! Background task that polls the PiSugar UPS daemon and emits status updates.

use std::time::Duration;

use rt_protocol::{ForwarderUpsStatus, UpsStatus};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::config::UpsConfig;
use crate::status_http::{StatusServer, UpsStatusState};
use crate::ui_events::ForwarderUiEvent;

/// Handle returned by [`spawn_ups_task`] carrying the upstream status channel.
pub struct UpsTaskHandle {
    /// Receives [`ForwarderUpsStatus`] messages destined for the uplink.
    pub ups_status_rx: mpsc::UnboundedReceiver<ForwarderUpsStatus>,
}

/// Spawn a background task that periodically polls the PiSugar daemon.
///
/// The task runs until `shutdown_rx` signals shutdown.
pub fn spawn_ups_task(
    config: UpsConfig,
    forwarder_id: String,
    status: StatusServer,
    mut shutdown_rx: watch::Receiver<bool>,
) -> UpsTaskHandle {
    let (ups_tx, ups_rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        run_ups_loop(config, forwarder_id, status, &mut shutdown_rx, ups_tx).await;
    });

    UpsTaskHandle {
        ups_status_rx: ups_rx,
    }
}

async fn run_ups_loop(
    config: UpsConfig,
    forwarder_id: String,
    status: StatusServer,
    shutdown_rx: &mut watch::Receiver<bool>,
    ups_tx: mpsc::UnboundedSender<ForwarderUpsStatus>,
) {
    let poll_interval = Duration::from_secs(config.poll_interval_secs);
    let heartbeat_interval = Duration::from_secs(config.upstream_heartbeat_secs);
    let mut interval = tokio::time::interval(poll_interval);
    // First tick completes immediately.
    interval.tick().await;

    let ui_tx = status.ui_sender();

    // Tracking state
    let mut last_reported_available: Option<bool> = None;
    let mut last_status: Option<UpsStatus> = None;
    let mut last_send = tokio::time::Instant::now();
    let mut warned_power_unplugged = false;
    let mut warned_low_battery = false;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.changed() => {
                info!("UPS task shutting down");
                return;
            }
        }

        match crate::pisugar_client::poll_status(&config.daemon_addr).await {
            Ok(new_status) => {
                // Transition: unavailable -> available
                if last_reported_available != Some(true) {
                    info!(addr = %config.daemon_addr, "PiSugar daemon is now available");
                }

                // Warning: power unplugged
                if !new_status.power_plugged && !warned_power_unplugged {
                    warn!("UPS reports power unplugged — running on battery");
                    warned_power_unplugged = true;
                }
                if new_status.power_plugged && warned_power_unplugged {
                    info!("UPS reports power restored");
                    warned_power_unplugged = false;
                }

                // Warning: low battery (only while not plugged in)
                if !new_status.power_plugged
                    && new_status.battery_percent < 20
                    && !warned_low_battery
                {
                    warn!(
                        battery_percent = new_status.battery_percent,
                        "UPS battery below 20%"
                    );
                    warned_low_battery = true;
                }
                if (new_status.power_plugged || new_status.battery_percent >= 20)
                    && warned_low_battery
                {
                    warned_low_battery = false;
                }

                // Determine if readings changed
                let readings_changed = match &last_status {
                    Some(prev) => !prev.same_readings(&new_status),
                    None => true,
                };

                let heartbeat_due = last_send.elapsed() >= heartbeat_interval;

                // Update local subsystem status always
                status
                    .set_ups_status(UpsStatusState {
                        available: true,
                        status: Some(new_status.clone()),
                    })
                    .await;

                if last_reported_available != Some(true) || readings_changed || heartbeat_due {
                    let msg = ForwarderUpsStatus {
                        forwarder_id: forwarder_id.clone(),
                        available: true,
                        status: Some(new_status.clone()),
                    };

                    let _ = ui_tx.send(ForwarderUiEvent::UpsStatusChanged {
                        available: true,
                        status: Some(new_status.clone()),
                    });
                    let _ = ups_tx.send(msg);

                    last_reported_available = Some(true);
                    last_send = tokio::time::Instant::now();
                }

                last_status = Some(new_status);
            }
            Err(e) => {
                let stale_status = last_status.clone();

                status
                    .set_ups_status(UpsStatusState {
                        available: false,
                        status: stale_status.clone(),
                    })
                    .await;

                // Only log + send on transition (available -> unavailable), including
                // the initial boot state when the daemon is unreachable.
                if last_reported_available != Some(false) {
                    warn!(
                        addr = %config.daemon_addr,
                        error = %e,
                        "PiSugar daemon became unavailable"
                    );
                    warned_power_unplugged = false;
                    warned_low_battery = false;

                    let msg = ForwarderUpsStatus {
                        forwarder_id: forwarder_id.clone(),
                        available: false,
                        status: stale_status.clone(),
                    };
                    let _ = ui_tx.send(ForwarderUiEvent::UpsStatusChanged {
                        available: false,
                        status: stale_status,
                    });
                    let _ = ups_tx.send(msg);

                    last_reported_available = Some(false);
                    last_send = tokio::time::Instant::now();
                }
                // If already unavailable, stay silent (no repeated warnings)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Spawn a mock PiSugar daemon that serves `n` rounds of canned responses,
    /// each round handling the 5 standard commands.
    async fn mock_pisugar(rounds: Vec<[(&'static str, &'static str); 5]>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        tokio::spawn(async move {
            for round in rounds {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);

                for (expected_cmd, response_value) in round {
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    let cmd = line.trim();
                    let key = cmd.strip_prefix("get ").unwrap_or(cmd);
                    assert_eq!(cmd, expected_cmd, "unexpected command");
                    writer
                        .write_all(format!("{key}: {response_value}\n").as_bytes())
                        .await
                        .unwrap();
                }
            }
        });

        addr
    }

    fn standard_responses() -> [(&'static str, &'static str); 5] {
        [
            ("get battery", "85"),
            ("get battery_v", "4.05"),
            ("get battery_charging", "true"),
            ("get battery_power_plugged", "true"),
            ("get temperature", "32.0"),
        ]
    }

    #[tokio::test]
    async fn ups_task_sends_initial_status() {
        let addr = mock_pisugar(vec![standard_responses()]).await;

        let config = UpsConfig {
            enabled: true,
            daemon_addr: addr,
            poll_interval_secs: 1,
            upstream_heartbeat_secs: 60,
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Need a StatusServer — use a minimal one
        let server = crate::status_http::StatusServer::start(
            crate::status_http::StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            crate::status_http::SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let mut handle = spawn_ups_task(config, "fwd-test".to_owned(), server, shutdown_rx);

        // Should receive one status message
        let msg = tokio::time::timeout(Duration::from_secs(5), handle.ups_status_rx.recv())
            .await
            .expect("timeout waiting for UPS status")
            .expect("channel closed");

        assert_eq!(msg.forwarder_id, "fwd-test");
        assert!(msg.available);
        let status = msg.status.unwrap();
        assert_eq!(status.battery_percent, 85);
        assert!(status.power_plugged);

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn ups_task_sends_unavailable_on_connection_failure() {
        // Use a port that will refuse connections
        let config = UpsConfig {
            enabled: true,
            daemon_addr: "127.0.0.1:1".to_owned(),
            poll_interval_secs: 1,
            upstream_heartbeat_secs: 60,
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let server = crate::status_http::StatusServer::start(
            crate::status_http::StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            crate::status_http::SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let mut handle = spawn_ups_task(config, "fwd-test".to_owned(), server, shutdown_rx);

        let msg = tokio::time::timeout(Duration::from_secs(10), handle.ups_status_rx.recv())
            .await
            .expect("timeout waiting for UPS unavailable status")
            .expect("channel closed");

        assert_eq!(msg.forwarder_id, "fwd-test");
        assert!(!msg.available);
        assert!(msg.status.is_none());

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn ups_task_preserves_last_status_when_daemon_becomes_unavailable() {
        let addr = mock_pisugar(vec![standard_responses()]).await;

        let config = UpsConfig {
            enabled: true,
            daemon_addr: addr,
            poll_interval_secs: 1,
            upstream_heartbeat_secs: 60,
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let server = crate::status_http::StatusServer::start(
            crate::status_http::StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            crate::status_http::SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let mut handle = spawn_ups_task(config, "fwd-test".to_owned(), server, shutdown_rx);

        let first = tokio::time::timeout(Duration::from_secs(5), handle.ups_status_rx.recv())
            .await
            .expect("timeout waiting for initial UPS status")
            .expect("channel closed");
        assert!(first.available);
        let first_status = first.status.expect("initial status");

        let second = tokio::time::timeout(Duration::from_secs(10), handle.ups_status_rx.recv())
            .await
            .expect("timeout waiting for UPS unavailable status")
            .expect("channel closed");

        assert!(!second.available);
        let second_status = second.status.expect("stale status retained");
        assert_eq!(second_status, first_status);

        shutdown_tx.send(true).unwrap();
    }
}

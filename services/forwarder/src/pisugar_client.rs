//! Async TCP client for the pisugar-server daemon's text protocol.
//!
//! The daemon listens on a configurable host:port and responds to plain-text
//! commands. Each command is a single line; each response is `key: value\n`.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

use rt_protocol::UpsStatus;

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum PisugarError {
    #[error("connection failed: {0}")]
    Connect(io::Error),
    #[error("send failed: {0}")]
    Send(io::Error),
    #[error("read failed: {0}")]
    Read(io::Error),
    #[error("unexpected response for '{command}': {response}")]
    UnexpectedResponse { command: String, response: String },
    #[error("parse error for '{command}': {detail}")]
    Parse { command: String, detail: String },
}

/// Opens a fresh TCP connection to the pisugar-server daemon, sends five
/// commands, and returns a populated [`UpsStatus`].
pub async fn poll_status(addr: &str) -> Result<UpsStatus, PisugarError> {
    let stream = timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| {
            PisugarError::Connect(io::Error::new(io::ErrorKind::TimedOut, "connect timed out"))
        })?
        .map_err(PisugarError::Connect)?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let battery_percent = send_and_parse_u8(&mut reader, &mut writer, "get battery").await?;
    let battery_voltage_mv =
        send_and_parse_voltage_mv(&mut reader, &mut writer, "get battery_v").await?;
    let charging = send_and_parse_bool(&mut reader, &mut writer, "get battery_charging").await?;
    let power_plugged =
        send_and_parse_bool(&mut reader, &mut writer, "get battery_power_plugged").await?;
    let temperature_cdeg =
        send_and_parse_temperature_cdeg(&mut reader, &mut writer, "get temperature").await?;

    let sampled_at = chrono::Utc::now().timestamp_millis();

    Ok(UpsStatus {
        battery_percent,
        battery_voltage_mv,
        charging,
        power_plugged,
        temperature_cdeg,
        sampled_at,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_command(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    command: &str,
) -> Result<String, PisugarError> {
    writer
        .write_all(format!("{command}\n").as_bytes())
        .await
        .map_err(PisugarError::Send)?;

    let mut line = String::new();
    timeout(TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| PisugarError::Read(io::Error::new(io::ErrorKind::TimedOut, "read timed out")))?
        .map_err(PisugarError::Read)?;

    Ok(line
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string())
}

/// Extract the value portion after the `: ` separator.
fn parse_value<'a>(command: &str, response: &'a str) -> Result<&'a str, PisugarError> {
    let Some((_key, value)) = response.split_once(": ") else {
        return Err(PisugarError::UnexpectedResponse {
            command: command.to_string(),
            response: response.to_string(),
        });
    };
    Ok(value)
}

async fn send_and_parse_u8(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    command: &str,
) -> Result<u8, PisugarError> {
    let response = send_command(reader, writer, command).await?;
    let value = parse_value(command, &response)?;
    let f: f64 = value
        .parse()
        .map_err(|e: std::num::ParseFloatError| PisugarError::Parse {
            command: command.to_string(),
            detail: e.to_string(),
        })?;
    Ok(f.round() as u8)
}

async fn send_and_parse_bool(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    command: &str,
) -> Result<bool, PisugarError> {
    let response = send_command(reader, writer, command).await?;
    let value = parse_value(command, &response)?;
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(PisugarError::Parse {
            command: command.to_string(),
            detail: format!("expected 'true' or 'false', got '{value}'"),
        }),
    }
}

async fn send_and_parse_voltage_mv(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    command: &str,
) -> Result<u16, PisugarError> {
    let response = send_command(reader, writer, command).await?;
    let value = parse_value(command, &response)?;
    let f: f64 = value
        .parse()
        .map_err(|e: std::num::ParseFloatError| PisugarError::Parse {
            command: command.to_string(),
            detail: e.to_string(),
        })?;
    Ok((f * 1000.0).round() as u16)
}

async fn send_and_parse_temperature_cdeg(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    command: &str,
) -> Result<i16, PisugarError> {
    let response = send_command(reader, writer, command).await?;
    let value = parse_value(command, &response)?;
    let f: f64 = value
        .parse()
        .map_err(|e: std::num::ParseFloatError| PisugarError::Parse {
            command: command.to_string(),
            detail: e.to_string(),
        })?;
    Ok((f * 100.0).round() as i16)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Spawn a mock pisugar-server that reads commands and returns canned
    /// responses based on a provided map.
    async fn mock_server(responses: Vec<(&'static str, &'static str)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);

            for (expected_cmd, response_value) in responses {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let cmd = line.trim();
                // Extract the key from the command (strip "get " prefix)
                let key = cmd.strip_prefix("get ").unwrap_or(cmd);
                assert_eq!(cmd, expected_cmd, "unexpected command");
                writer
                    .write_all(format!("{key}: {response_value}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });

        addr
    }

    #[tokio::test]
    async fn poll_status_parses_all_five_fields() {
        let addr = mock_server(vec![
            ("get battery", "73"),
            ("get battery_v", "3.87"),
            ("get battery_charging", "true"),
            ("get battery_power_plugged", "true"),
            ("get temperature", "42.5"),
        ])
        .await;

        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.battery_percent, 73);
        assert_eq!(status.battery_voltage_mv, 3870);
        assert!(status.charging);
        assert!(status.power_plugged);
        assert_eq!(status.temperature_cdeg, 4250);
    }

    #[tokio::test]
    async fn poll_status_rounds_fractional_battery_percent() {
        let addr = mock_server(vec![
            ("get battery", "73.5"),
            ("get battery_v", "3.87"),
            ("get battery_charging", "true"),
            ("get battery_power_plugged", "true"),
            ("get temperature", "42.5"),
        ])
        .await;

        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.battery_percent, 74);
    }

    #[tokio::test]
    async fn poll_status_connection_refused() {
        // Port 1 should be refused on localhost.
        let result = poll_status("127.0.0.1:1").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PisugarError::Connect(_)));
    }

    #[tokio::test]
    async fn voltage_conversion_precision() {
        let addr = mock_server(vec![
            ("get battery", "50"),
            ("get battery_v", "3.87"),
            ("get battery_charging", "false"),
            ("get battery_power_plugged", "false"),
            ("get temperature", "25.0"),
        ])
        .await;

        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.battery_voltage_mv, 3870);
    }

    #[tokio::test]
    async fn temperature_conversion_negative() {
        let addr = mock_server(vec![
            ("get battery", "50"),
            ("get battery_v", "3.87"),
            ("get battery_charging", "false"),
            ("get battery_power_plugged", "false"),
            ("get temperature", "-5.5"),
        ])
        .await;

        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.temperature_cdeg, -550);
    }
}

use emulator::control_handler::{EmulatedReaderState, StorageState};
use emulator::scenario::ReaderScenarioConfig;
use emulator::server::{EmulatorConfig, ReadType};
use ipico_core::control::{
    Command, ReadMode, RecordingState, decode_config3, decode_date_time, decode_extended_status,
    decode_statistics, decode_tag_message_format, encode_command, parse_response,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};

fn base_reader_config() -> ReaderScenarioConfig {
    ReaderScenarioConfig {
        ip: "192.168.1.100".to_string(),
        port: 0,
        read_type: "raw".to_string(),
        chip_ids: vec![1000],
        events_per_second: 10,
        total_events: 100,
        start_delay_ms: 0,
        faults: vec![],
        initial_read_mode: None,
        initial_tto_enabled: None,
        initial_recording: None,
        stored_reads: None,
        clock_offset_ms: None,
    }
}

/// Start the emulator on an ephemeral port and return (server_handle, actual_port).
async fn start_emulator(state: EmulatedReaderState) -> (tokio::task::JoinHandle<()>, u16) {
    let (port_tx, port_rx) = tokio::sync::oneshot::channel();
    let emulator_config = EmulatorConfig {
        bind_port: 0,
        delay: 50,
        file_path: None,
        read_type: ReadType::RAW,
    };
    let handle = tokio::spawn(async move {
        emulator::server::run_with_control(emulator_config, state, Some(port_tx)).await;
    });
    let port = port_rx.await.expect("failed to receive port");
    (handle, port)
}

/// Connect to the emulator, drain the banner, and return (reader, write_half, line_buf).
async fn connect_and_drain_banner(
    port: u16,
) -> (
    BufReader<tokio::net::tcp::OwnedReadHalf>,
    tokio::net::tcp::OwnedWriteHalf,
    String,
) {
    let stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect(format!("127.0.0.1:{port}")),
    )
    .await
    .expect("connect timed out")
    .expect("connect failed");

    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line_buf = String::new();

    // Drain banner lines until we see a chip read or control frame.
    loop {
        line_buf.clear();
        let read_result = timeout(Duration::from_secs(2), reader.read_line(&mut line_buf)).await;
        match read_result {
            Ok(Ok(0)) => panic!("unexpected EOF during banner"),
            Ok(Ok(_)) => {
                let trimmed = line_buf.trim_end();
                if trimmed.starts_with("aa") || trimmed.starts_with("ab") {
                    break;
                }
            }
            Ok(Err(e)) => panic!("read error during banner: {e}"),
            Err(_) => panic!("timeout reading banner"),
        }
    }

    (reader, write_half, line_buf)
}

/// Send a command and read until we get an ab-prefixed response matching
/// the expected instruction byte. Returns the matching response line and
/// any non-ab, non-aa "extra" lines (e.g. banner text).
async fn send_and_read_response(
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    line_buf: &mut String,
    cmd: &Command,
    expected_instr_hex: &str,
) -> (String, Vec<String>) {
    let cmd_bytes = encode_command(cmd, 0x00).unwrap();
    write_half
        .write_all(&cmd_bytes)
        .await
        .expect("failed to send command");
    write_half.flush().await.expect("flush failed");

    let mut extra_lines = Vec::new();
    timeout(Duration::from_secs(5), async {
        loop {
            line_buf.clear();
            match reader.read_line(line_buf).await {
                Ok(0) => panic!("unexpected EOF waiting for response"),
                Ok(_) => {
                    let trimmed = line_buf.trim_end();
                    if trimmed.starts_with("ab") {
                        if trimmed.len() >= 8 && &trimmed[6..8] == expected_instr_hex {
                            return (trimmed.to_string(), extra_lines);
                        }
                    } else if trimmed.starts_with("aa") {
                        // Chip read, skip.
                    } else {
                        extra_lines.push(trimmed.to_string());
                    }
                }
                Err(e) => panic!("read error: {e}"),
            }
        }
    })
    .await
    .expect("timed out waiting for response")
}

// ---------------------------------------------------------------------------
// Existing tests (refactored to use ephemeral ports)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn emulator_responds_to_control_commands_over_tcp() {
    let cfg = base_reader_config();
    let state = EmulatedReaderState::from_config(&cfg, 42);
    let (server_handle, port) = start_emulator(state).await;

    let (mut reader, mut write_half, mut line_buf) = connect_and_drain_banner(port).await;

    // Send GetStatistics and verify response.
    let (stats_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetStatistics,
        "0a",
    )
    .await;
    let parsed = parse_response(stats_line.as_bytes()).expect("failed to parse response frame");
    let stats = decode_statistics(&parsed).expect("failed to decode statistics");
    assert_eq!(stats.fw_version_string(), "4.2");

    server_handle.abort();
    let _ = server_handle.await;
}

#[tokio::test]
async fn emulator_handles_full_connect_sequence() {
    let cfg = ReaderScenarioConfig {
        initial_read_mode: Some("event".to_string()),
        initial_tto_enabled: Some(true),
        initial_recording: Some(true),
        stored_reads: Some(100),
        clock_offset_ms: Some(0),
        ..base_reader_config()
    };
    let state = EmulatedReaderState::from_config(&cfg, 42);
    let (server_handle, port) = start_emulator(state).await;

    let (mut reader, mut write_half, mut line_buf) = connect_and_drain_banner(port).await;

    // 1. GetStatistics
    let (stats_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetStatistics,
        "0a",
    )
    .await;
    let stats = decode_statistics(&parse_response(stats_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(stats.fw_version_string(), "4.2");
    assert_eq!(stats.hw_code, 0x05);

    // 2. PrintBanner
    let (banner_ack_line, banner_lines) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::PrintBanner,
        "37",
    )
    .await;
    assert!(!banner_lines.is_empty());
    let banner_ack = parse_response(banner_ack_line.as_bytes()).unwrap();
    assert_eq!(banner_ack.instruction(), 0x37);
    assert!(banner_ack.data().is_empty());

    // 3. GetExtendedStatus — recording on, 100 stored reads
    let (ext_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetExtendedStatus,
        "4b",
    )
    .await;
    let ext = decode_extended_status(&parse_response(ext_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(ext.recording_state, RecordingState::On);
    assert_eq!(ext.estimated_stored_reads(), 100);

    // 4. GetConfig3 — Event mode, timeout 5
    let (cfg3_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetConfig3,
        "09",
    )
    .await;
    let (mode, tmo) = decode_config3(&parse_response(cfg3_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(mode, ReadMode::Event);
    assert_eq!(tmo, 5);

    // 5. GetTagMessageFormat — TTO enabled
    let (tmf_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetTagMessageFormat,
        "11",
    )
    .await;
    let tmf = decode_tag_message_format(&parse_response(tmf_line.as_bytes()).unwrap()).unwrap();
    assert!(tmf.tto_enabled());

    // 6. GetDateTime — decodes without error
    let (dt_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetDateTime,
        "02",
    )
    .await;
    let _dt = decode_date_time(&parse_response(dt_line.as_bytes()).unwrap()).unwrap();

    server_handle.abort();
    let _ = server_handle.await;
}

// ---------------------------------------------------------------------------
// New tests: write commands over TCP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn emulator_handles_write_commands_over_tcp() {
    let cfg = base_reader_config();
    let state = EmulatedReaderState::from_config(&cfg, 42);
    let (server_handle, port) = start_emulator(state).await;

    let (mut reader, mut write_half, mut line_buf) = connect_and_drain_banner(port).await;

    // 1. SetConfig3(Event, 8) — expect ACK
    let (ack_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::SetConfig3 {
            mode: ReadMode::Event,
            timeout: 8,
        },
        "09",
    )
    .await;
    let ack = parse_response(ack_line.as_bytes()).unwrap();
    assert!(
        ack.data().is_empty(),
        "SetConfig3 ACK should have empty data"
    );

    // 2. GetConfig3 — verify mode changed
    let (cfg3_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetConfig3,
        "09",
    )
    .await;
    let (mode, tmo) = decode_config3(&parse_response(cfg3_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(mode, ReadMode::Event);
    assert_eq!(tmo, 8);

    // 3. SetTagMessageFormat with TTO enabled — expect ACK
    let fmt = ipico_core::control::TagMessageFormat {
        field_mask: 0x80,
        id_byte_mask: 0x3f,
        ascii_header_1: 0x00,
        ascii_header_2: 0x00,
        binary_header_1: 0xaa,
        binary_header_2: 0x00,
        trailer_1: 0x0d,
        trailer_2: 0x0a,
        separator: None,
    };
    let (tmf_ack, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::SetTagMessageFormat { format: fmt },
        "11",
    )
    .await;
    let tmf_parsed = parse_response(tmf_ack.as_bytes()).unwrap();
    assert!(tmf_parsed.data().is_empty());

    // 4. GetTagMessageFormat — verify TTO bit set
    let (tmf_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetTagMessageFormat,
        "11",
    )
    .await;
    let tmf = decode_tag_message_format(&parse_response(tmf_line.as_bytes()).unwrap()).unwrap();
    assert!(tmf.tto_enabled());

    server_handle.abort();
    let _ = server_handle.await;
}

// ---------------------------------------------------------------------------
// New test: download workflow over TCP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn emulator_handles_download_workflow_over_tcp() {
    let cfg = ReaderScenarioConfig {
        initial_recording: Some(true),
        stored_reads: Some(5),
        ..base_reader_config()
    };
    let state = EmulatedReaderState::from_config(&cfg, 42);
    let (server_handle, port) = start_emulator(state).await;

    let (mut reader, mut write_half, mut line_buf) = connect_and_drain_banner(port).await;

    // 1. GetExtendedStatus — recording on, 5 stored reads, HasData
    let (ext_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetExtendedStatus,
        "4b",
    )
    .await;
    let ext = decode_extended_status(&parse_response(ext_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(ext.recording_state, RecordingState::On);
    assert_eq!(ext.estimated_stored_reads(), 5);
    assert_eq!(ext.storage_state, StorageState::HasData.wire_byte());

    // 2. SetAccessMode(on=true) — enter download mode
    let (access_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::SetAccessMode { on: true },
        "4b",
    )
    .await;
    let access_ext =
        decode_extended_status(&parse_response(access_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(access_ext.recording_state, RecordingState::Downloading);

    // 3. GetExtendedStatus — verify downloading state
    let (ext2_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetExtendedStatus,
        "4b",
    )
    .await;
    let ext2 = decode_extended_status(&parse_response(ext2_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(ext2.recording_state, RecordingState::Downloading);

    // 4. TriggerErase — clear stored reads
    let (erase_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::TriggerErase,
        "4b",
    )
    .await;
    let erase_parsed = parse_response(erase_line.as_bytes()).unwrap();
    assert!(erase_parsed.data().is_empty());

    // 5. GetExtendedStatus — verify stored_reads=0, storage_state=Empty
    let (ext3_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetExtendedStatus,
        "4b",
    )
    .await;
    let ext3 = decode_extended_status(&parse_response(ext3_line.as_bytes()).unwrap()).unwrap();
    assert_eq!(ext3.estimated_stored_reads(), 0);
    assert_eq!(ext3.storage_state, StorageState::Empty.wire_byte());

    server_handle.abort();
    let _ = server_handle.await;
}

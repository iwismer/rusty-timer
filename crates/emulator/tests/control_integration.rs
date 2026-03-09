use emulator::control_handler::EmulatedReaderState;
use emulator::scenario::ReaderScenarioConfig;
use emulator::server::{EmulatorConfig, ReadType};
use ipico_core::control::{
    Command, ReadMode, RecordingState, decode_config3, decode_date_time, decode_extended_status,
    decode_statistics, decode_tag_message_format, encode_command, parse_response,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};

fn test_reader_config() -> ReaderScenarioConfig {
    ReaderScenarioConfig {
        ip: "192.168.1.100".to_string(),
        port: 19876,
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

#[tokio::test]
async fn emulator_responds_to_control_commands_over_tcp() {
    let cfg = test_reader_config();
    let state = EmulatedReaderState::from_config(&cfg, 42);

    let emulator_config = EmulatorConfig {
        bind_port: 19876,
        delay: 50,
        file_path: None,
        read_type: ReadType::RAW,
    };

    let server_handle =
        tokio::spawn(
            async move { emulator::server::run_with_control(emulator_config, state).await },
        );

    // Give the server a moment to bind.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect as a TCP client.
    let stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect("127.0.0.1:19876"),
    )
    .await
    .expect("connect timed out")
    .expect("connect failed");

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line_buf = String::new();

    // Read banner lines (non-ab-prefixed).
    let mut banner_lines = Vec::new();
    loop {
        line_buf.clear();
        let read_result = timeout(Duration::from_secs(2), reader.read_line(&mut line_buf)).await;
        match read_result {
            Ok(Ok(0)) => panic!("unexpected EOF during banner"),
            Ok(Ok(_)) => {
                let trimmed = line_buf.trim_end().to_string();
                if trimmed.starts_with("aa") || trimmed.starts_with("ab") {
                    // We've moved past the banner into chip reads or control data.
                    break;
                }
                banner_lines.push(trimmed);
            }
            Ok(Err(e)) => panic!("read error during banner: {e}"),
            Err(_) => panic!("timeout reading banner"),
        }
    }
    assert!(
        !banner_lines.is_empty(),
        "expected at least one banner line"
    );

    // Send a GetStatistics command.
    let cmd_bytes = encode_command(&Command::GetStatistics, 0x00).unwrap();
    write_half
        .write_all(&cmd_bytes)
        .await
        .expect("failed to send command");
    write_half.flush().await.expect("flush failed");

    // Read lines until we get an ab-prefixed response with instruction 0x0a
    // (GetStatistics). Skip interleaved chip-read lines (starting with "aa").
    let response_line = timeout(Duration::from_secs(5), async {
        loop {
            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => panic!("unexpected EOF waiting for statistics response"),
                Ok(_) => {
                    let trimmed = line_buf.trim_end();
                    if trimmed.starts_with("ab") {
                        // Check if this is the statistics response (instruction byte at offset 6..8 == "0a")
                        if trimmed.len() >= 8 && &trimmed[6..8] == "0a" {
                            return trimmed.to_string();
                        }
                    }
                    // Skip chip reads and other lines.
                }
                Err(e) => panic!("read error: {e}"),
            }
        }
    })
    .await
    .expect("timed out waiting for statistics response");

    // Verify the response decodes as valid statistics.
    let parsed = parse_response(response_line.as_bytes()).expect("failed to parse response frame");
    let stats = decode_statistics(&parsed).expect("failed to decode statistics");
    assert_eq!(stats.fw_version_string(), "4.2", "expected fw_version 4.2");

    // Clean up.
    server_handle.abort();
    let _ = server_handle.await;
}

#[tokio::test]
async fn emulator_handles_full_connect_sequence() {
    // Build config with specific initial state matching the connect sequence expectations.
    let cfg = ReaderScenarioConfig {
        ip: "192.168.1.100".to_string(),
        port: 19877,
        read_type: "raw".to_string(),
        chip_ids: vec![1000],
        events_per_second: 10,
        total_events: 100,
        start_delay_ms: 0,
        faults: vec![],
        initial_read_mode: Some("event".to_string()),
        initial_tto_enabled: Some(true),
        initial_recording: Some(true),
        stored_reads: Some(100),
        clock_offset_ms: Some(0),
    };
    let state = EmulatedReaderState::from_config(&cfg, 42);

    let emulator_config = EmulatorConfig {
        bind_port: 19877,
        delay: 50,
        file_path: None,
        read_type: ReadType::RAW,
    };

    let server_handle =
        tokio::spawn(
            async move { emulator::server::run_with_control(emulator_config, state).await },
        );

    // Give the server time to bind.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect("127.0.0.1:19877"),
    )
    .await
    .expect("connect timed out")
    .expect("connect failed");

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line_buf = String::new();

    // Drain initial banner/chip-read lines until stream is flowing.
    // We need to consume the initial banner that the emulator sends on connect.
    loop {
        line_buf.clear();
        let read_result = timeout(Duration::from_secs(2), reader.read_line(&mut line_buf)).await;
        match read_result {
            Ok(Ok(0)) => panic!("unexpected EOF during initial drain"),
            Ok(Ok(_)) => {
                let trimmed = line_buf.trim_end();
                // Once we see a chip read (aa-prefixed), the banner is done.
                if trimmed.starts_with("aa") || trimmed.starts_with("ab") {
                    break;
                }
            }
            Ok(Err(e)) => panic!("read error during initial drain: {e}"),
            Err(_) => panic!("timeout during initial drain"),
        }
    }

    // Helper: send a command and read until we get an ab-prefixed response matching
    // the expected instruction byte. Returns the matching response line.
    // Also collects non-ab, non-aa lines as "extra" lines (used for banner text).
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
                            // Check instruction byte at hex offset 6..8
                            if trimmed.len() >= 8 && &trimmed[6..8] == expected_instr_hex {
                                return (trimmed.to_string(), extra_lines);
                            }
                            // Different ab-response, skip it (could be from interleaved traffic).
                        } else if trimmed.starts_with("aa") {
                            // Chip read, skip.
                        } else {
                            // Banner text or other plain text line.
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

    // 1. GetStatistics (0x0a) — expect 14-byte data
    let (stats_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetStatistics,
        "0a",
    )
    .await;
    let stats_frame = parse_response(stats_line.as_bytes()).expect("failed to parse stats frame");
    let stats = decode_statistics(&stats_frame).expect("failed to decode statistics");
    assert_eq!(stats.fw_version_string(), "4.2");
    assert_eq!(stats.hw_code, 0x05);

    // 2. PrintBanner (0x37) — expect text lines then ACK frame with instruction 0x37
    let (banner_ack_line, banner_lines) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::PrintBanner,
        "37",
    )
    .await;
    assert!(
        !banner_lines.is_empty(),
        "expected at least one banner text line"
    );
    let banner_ack =
        parse_response(banner_ack_line.as_bytes()).expect("failed to parse banner ACK");
    assert_eq!(banner_ack.instruction(), 0x37);
    assert!(
        banner_ack.data().is_empty(),
        "banner ACK should have empty data"
    );

    // 3. GetExtendedStatus (0x4b) — recording on, ~100 stored reads
    let (ext_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetExtendedStatus,
        "4b",
    )
    .await;
    let ext_frame = parse_response(ext_line.as_bytes()).expect("failed to parse ext status frame");
    let ext_status = decode_extended_status(&ext_frame).expect("failed to decode extended status");
    assert_eq!(ext_status.recording_state, RecordingState::On);
    assert_eq!(ext_status.estimated_stored_reads(), 100);

    // 4. GetConfig3 (0x09) — Event mode, timeout 5
    let (cfg3_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetConfig3,
        "09",
    )
    .await;
    let cfg3_frame = parse_response(cfg3_line.as_bytes()).expect("failed to parse config3 frame");
    let (mode, tmo) = decode_config3(&cfg3_frame).expect("failed to decode config3");
    assert_eq!(mode, ReadMode::Event);
    assert_eq!(tmo, 5);

    // 5. GetTagMessageFormat (0x11) — TTO enabled (field_mask bit 7 set)
    let (tmf_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetTagMessageFormat,
        "11",
    )
    .await;
    let tmf_frame =
        parse_response(tmf_line.as_bytes()).expect("failed to parse tag message format frame");
    let tmf = decode_tag_message_format(&tmf_frame).expect("failed to decode tag message format");
    assert!(
        tmf.field_mask & 0x80 != 0,
        "expected TTO bit set in field_mask"
    );
    assert!(tmf.tto_enabled());

    // 6. GetDateTime (0x02) — decodes without error
    let (dt_line, _) = send_and_read_response(
        &mut write_half,
        &mut reader,
        &mut line_buf,
        &Command::GetDateTime,
        "02",
    )
    .await;
    let dt_frame = parse_response(dt_line.as_bytes()).expect("failed to parse date time frame");
    let _dt = decode_date_time(&dt_frame).expect("failed to decode date time");

    // Clean up.
    server_handle.abort();
    let _ = server_handle.await;
}

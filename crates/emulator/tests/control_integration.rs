use emulator::control_handler::EmulatedReaderState;
use emulator::scenario::ReaderScenarioConfig;
use emulator::server::{EmulatorConfig, ReadType};
use ipico_core::control::{Command, decode_statistics, encode_command, parse_response};
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

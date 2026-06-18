use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use timer_core::models::Message;
use timer_core::workers::{ClientConnector, ClientPool};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, broadcast};
use tokio::time::sleep;

use crate::control_handler::{EmulatedReaderState, handle_control_frame};
use crate::read_gen::{generate_read, restamp_read};

// Re-exports for the CLI binary
pub use ipico_core::read::ReadType;
pub use timer_core::util::{is_delay, is_file, is_port};

/// A chip-read frame broadcast to the connected TCP clients.
///
/// The `ack` flag is a per-send delivery marker: a `client_write_task` sets it
/// to `true` only after it has successfully `write_all`'d **and** `flush`'d the
/// frame to its socket. The generator uses this in `pause_when_unsubscribed`
/// mode to confirm at least one client actually delivered a read before
/// advancing, so a consumer that dies mid-write never silently consumes a read.
/// In normal (non-paused) mode the marker is ignored.
#[derive(Clone)]
struct ReadFrame {
    data: Arc<str>,
    ack: Arc<AtomicBool>,
}

pub struct EmulatorConfig {
    pub bind_port: u16,
    pub delay: u64,
    pub file_path: Option<String>,
    pub read_type: ReadType,
    /// When true, file-sourced reads are emitted byte-for-byte without
    /// restamping their timestamp/checksum. This makes a file-driven scenario
    /// fully deterministic (the exact frames on disk are what readers see),
    /// which the E2E stack orchestrator relies on for exact assertions.
    pub verbatim: bool,
    /// When true, the read generator emits each file read once (or a single
    /// generated read when no file is supplied) and then stops, instead of
    /// looping forever. Combined with `verbatim` this yields a bounded,
    /// deterministic scenario.
    pub once: bool,
    /// When true, the broadcast read generator pauses before emitting each read
    /// whenever there are no connected clients, and resumes (without skipping
    /// any read) once a client reconnects. This makes a forwarder/reader
    /// power-loss scenario deterministic: reads are never emitted into the void
    /// while the consumer is down, so a restarted consumer resumes losslessly
    /// from exactly where the stream paused.
    pub pause_when_unsubscribed: bool,
}

pub async fn send_reads(
    delay: u64,
    file_reads: Vec<String>,
    bus_tx: Sender<Message>,
    read_type: ReadType,
    verbatim: bool,
    once: bool,
) {
    let mut index = 0;
    loop {
        let (mut chip_read, last) = if file_reads.is_empty() {
            (generate_read(read_type), true)
        } else {
            let read = if verbatim {
                file_reads[index].clone()
            } else {
                restamp_read(&file_reads[index])
            };
            let last = index + 1 == file_reads.len();
            index = (index + 1) % file_reads.len();
            (read, last)
        };
        chip_read.push_str("\r\n");
        bus_tx
            .send(Message::CHIP_READ(chip_read))
            .await
            .unwrap_or_else(|_| {
                println!("\r\x1b[2KError sending read to thread. Maybe no readers are conected?");
            });
        if once && last {
            return;
        }
        sleep(Duration::from_millis(delay)).await;
    }
}

pub async fn run(config: EmulatorConfig) {
    use futures::{future::FutureExt, future::select_all, pin_mut};
    use std::future::Future;
    use std::io::BufRead;
    use std::path::Path;
    use std::pin::Pin;
    use timer_core::util::signal_handler;
    use tokio::sync::mpsc;

    let file_reads: Vec<String> = config
        .file_path
        .as_ref()
        .and_then(|p| {
            std::fs::File::open(Path::new(p))
                .map_err(|e| {
                    println!("Error opening file: {e}");
                    e
                })
                .ok()
        })
        .map(|f| {
            std::io::BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .map(|line| line.trim().to_owned())
                .filter(|line| !line.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let (bus_tx, rx) = mpsc::channel::<Message>(1000);
    let client_pool = ClientPool::new(rx, None, None, false);
    let connector = ClientConnector::new(config.bind_port, bus_tx.clone()).await;

    let fut_clients = client_pool.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_sig = signal_handler().fuse();
    let fut_sender = send_reads(
        config.delay,
        file_reads,
        bus_tx.clone(),
        config.read_type,
        config.verbatim,
        config.once,
    )
    .fuse();

    pin_mut!(fut_sender, fut_clients, fut_conn, fut_sig);
    let futures: Vec<Pin<&mut dyn Future<Output = ()>>> =
        vec![fut_sender, fut_clients, fut_conn, fut_sig];
    select_all(futures).await;
    bus_tx.send(Message::SHUTDOWN).await.unwrap();
}

/// Generate chip reads and publish them to a broadcast channel.
///
/// This is analogous to `send_reads` but targets a `broadcast::Sender<String>`
/// so that multiple TCP clients can each receive a copy.
async fn broadcast_reads(
    delay: u64,
    file_reads: Vec<String>,
    tx: broadcast::Sender<ReadFrame>,
    read_type: ReadType,
    verbatim: bool,
    once: bool,
    pause_when_unsubscribed: bool,
) {
    // In bounded (`once`) scenarios, reads emitted before any client connects are
    // dropped by the broadcast channel and lost. Wait for the first consumer so
    // the exact, finite read set is delivered deterministically. The infinite
    // (looping) mode does not need this: a dropped early read is re-sent on the
    // next loop. When `pause_when_unsubscribed` is set, the delivery loop below
    // already covers the initial wait, so the start-only wait is skipped.
    if once && !pause_when_unsubscribed {
        while tx.receiver_count() == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    }
    let mut index = 0;
    loop {
        // Compute the current read *without* advancing the index yet. The index
        // only moves once the read has actually been delivered (in pause mode),
        // so a consumer that dies mid-flight does not skip a read.
        let (read_body, last, next_index) = if file_reads.is_empty() {
            (generate_read(read_type), true, 0)
        } else {
            let read = if verbatim {
                file_reads[index].clone()
            } else {
                restamp_read(&file_reads[index])
            };
            let last = index + 1 == file_reads.len();
            (read, last, (index + 1) % file_reads.len())
        };
        let mut data = read_body;
        data.push_str("\r\n");
        let data: Arc<str> = Arc::from(data);

        if pause_when_unsubscribed {
            // Deterministic, fail-safe delivery: do not advance until at least
            // one connected client has actually written+flushed this frame. If
            // every subscriber disappears before delivery (e.g. a SIGKILLed
            // forwarder), re-send the same frame to the next subscriber so no
            // read is consumed by a dying write task and lost. A client that
            // already flushed marks the ack before its receiver drops, so a
            // clean disconnect after delivery still advances.
            'deliver: loop {
                while tx.receiver_count() == 0 {
                    sleep(Duration::from_millis(10)).await;
                }
                let ack = Arc::new(AtomicBool::new(false));
                let _ = tx.send(ReadFrame {
                    data: Arc::clone(&data),
                    ack: Arc::clone(&ack),
                });
                loop {
                    sleep(Duration::from_millis(10)).await;
                    if ack.load(Ordering::Acquire) {
                        break 'deliver; // delivered: safe to advance
                    }
                    if tx.receiver_count() == 0 {
                        break; // subscribers vanished before delivery; re-send
                    }
                }
            }
        } else {
            // Normal mode: fire-and-forget. broadcast::send fails when no
            // receivers are subscribed; safe to ignore because reads are only
            // relevant while a client is connected. The ack marker is unused.
            let _ = tx.send(ReadFrame {
                data: Arc::clone(&data),
                ack: Arc::new(AtomicBool::new(false)),
            });
        }

        index = next_index;
        if once && last {
            return;
        }
        sleep(Duration::from_millis(delay)).await;
    }
}

/// Run the emulator with bidirectional TCP support.
///
/// Unlike `run()`, this function handles both outgoing chip reads **and**
/// incoming control frames on each TCP connection.  A banner is sent to
/// every new client on connect.
///
/// If `port_tx` is provided, the actual bound port is sent through it after
/// binding.  This supports ephemeral ports (`bind_port: 0`) in tests.
pub async fn run_with_control(
    config: EmulatorConfig,
    state: EmulatedReaderState,
    port_tx: Option<tokio::sync::oneshot::Sender<u16>>,
) {
    let file_reads: Vec<String> = config
        .file_path
        .as_ref()
        .and_then(|p| {
            std::fs::File::open(std::path::Path::new(p))
                .map_err(|e| {
                    println!("Error opening file: {e}");
                    e
                })
                .ok()
        })
        .map(|f| {
            std::io::BufRead::lines(std::io::BufReader::new(f))
                .map_while(Result::ok)
                .map(|line| line.trim().to_owned())
                .filter(|line| !line.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let (read_tx, _) = broadcast::channel::<ReadFrame>(1000);
    let shared_state = Arc::new(Mutex::new(state));

    // Spawn the read-generation task.
    let read_tx_clone = read_tx.clone();
    let _read_gen_handle = tokio::spawn(broadcast_reads(
        config.delay,
        file_reads,
        read_tx_clone,
        config.read_type,
        config.verbatim,
        config.once,
        config.pause_when_unsubscribed,
    ));

    let listener = TcpListener::bind(("0.0.0.0", config.bind_port))
        .await
        .expect("failed to bind TCP listener");
    let actual_port = listener.local_addr().expect("local_addr").port();
    eprintln!("[emulator] listening on 0.0.0.0:{actual_port}");

    if let Some(tx) = port_tx
        && tx.send(actual_port).is_err()
    {
        eprintln!("[emulator] port notification dropped: receiver already gone");
    }

    // Accept loop — runs until the task is aborted externally (e.g. signal or
    // test harness calling `abort()`).
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("[emulator] accept error: {e}");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };

        let state = Arc::clone(&shared_state);
        let read_rx = read_tx.subscribe();

        tokio::spawn(async move {
            handle_client(stream, state, read_rx).await;
        });
    }
}

/// Handle a single bidirectional TCP client connection.
async fn handle_client(
    stream: tokio::net::TcpStream,
    state: Arc<Mutex<EmulatedReaderState>>,
    read_rx: broadcast::Receiver<ReadFrame>,
) {
    let (read_half, write_half) = stream.into_split();

    // Per-client channel for control responses and the initial banner.
    let (client_tx, client_rx) = tokio::sync::mpsc::channel::<String>(256);

    // Send the banner through the per-client channel.
    {
        let st = state.lock().await;
        for line in st.banner().lines() {
            if client_tx.send(format!("{}\r\n", line)).await.is_err() {
                eprintln!("[emulator] banner send failed: client channel closed");
                return;
            }
        }
    }

    // Spawn the write task.
    let write_handle = tokio::spawn(client_write_task(write_half, client_rx, read_rx));

    // Run the read loop inline — when it finishes the client disconnected.
    client_read_loop(read_half, state, client_tx).await;

    // Client disconnected; clean up the write task.
    write_handle.abort();
    let _ = write_handle.await;
}

/// Read loop: reads \r\n-delimited lines, dispatches `ab`-prefixed control
/// frames, and ignores everything else.
///
/// Lines longer than `MAX_LINE_LEN` are discarded to avoid processing
/// oversized input. Note: `read_line` reads the full line into memory
/// before the length check; this does not bound allocation.
async fn client_read_loop(
    read_half: tokio::net::tcp::OwnedReadHalf,
    state: Arc<Mutex<EmulatedReaderState>>,
    client_tx: tokio::sync::mpsc::Sender<String>,
) {
    const MAX_LINE_LEN: usize = 4096;

    let mut reader = BufReader::new(read_half);
    let mut line_buf = String::new();

    loop {
        line_buf.clear();
        match reader.read_line(&mut line_buf).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                if line_buf.len() > MAX_LINE_LEN {
                    eprintln!(
                        "[emulator] dropping oversized line ({} bytes)",
                        line_buf.len()
                    );
                    continue;
                }
                let trimmed = line_buf.trim_end();
                if trimmed.starts_with("ab") {
                    let mut st = state.lock().await;
                    let responses = handle_control_frame(&mut st, trimmed);
                    drop(st);
                    for resp in responses {
                        if client_tx.send(resp).await.is_err() {
                            return; // write side gone
                        }
                    }
                }
                // Non-ab lines are silently ignored.
            }
            Err(e) => {
                eprintln!("[emulator] client read error: {e}");
                break;
            }
        }
    }
}

/// Write task: multiplexes control responses (from per-client mpsc) and
/// chip reads (from broadcast) onto the TCP write half.
async fn client_write_task(
    write_half: tokio::net::tcp::OwnedWriteHalf,
    mut client_rx: tokio::sync::mpsc::Receiver<String>,
    mut read_rx: broadcast::Receiver<ReadFrame>,
) {
    let mut writer = BufWriter::new(write_half);

    loop {
        tokio::select! {
            msg = client_rx.recv() => {
                match msg {
                    Some(data) => {
                        if let Err(e) = writer.write_all(data.as_bytes()).await {
                            eprintln!("[emulator] client write error: {e}");
                            break;
                        }
                        if let Err(e) = writer.flush().await {
                            eprintln!("[emulator] client flush error: {e}");
                            break;
                        }
                    }
                    None => break, // sender dropped
                }
            }
            result = read_rx.recv() => {
                match result {
                    Ok(frame) => {
                        if let Err(e) = writer.write_all(frame.data.as_bytes()).await {
                            eprintln!("[emulator] client write error: {e}");
                            break;
                        }
                        if let Err(e) = writer.flush().await {
                            eprintln!("[emulator] client flush error: {e}");
                            break;
                        }
                        // Mark the frame delivered only after a successful
                        // write+flush so the generator can safely advance in
                        // pause mode (deterministic, lossless delivery).
                        frame.ack.store(true, Ordering::Release);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("[emulator] client lagged, skipped {n} reads");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipico_core::read::ChipRead;
    use std::convert::TryFrom;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn send_reads_loops_file_with_restamped_timestamps() {
        let file_reads = vec!["aa400000000123450a2a01123018455927a7".to_owned()];
        let (bus_tx, mut bus_rx) = mpsc::channel(8);
        let sender_task = tokio::spawn(send_reads(
            1,
            file_reads,
            bus_tx,
            ReadType::RAW,
            false,
            false,
        ));

        let first = timeout(Duration::from_millis(100), bus_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let second = timeout(Duration::from_millis(100), bus_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match (&first, &second) {
            (Message::CHIP_READ(r1), Message::CHIP_READ(r2)) => {
                let r1 = r1.trim();
                let r2 = r2.trim();
                assert_eq!(&r1[4..20], "0000000123450a2a");
                assert_eq!(&r2[4..20], "0000000123450a2a");
                assert!(ChipRead::try_from(r1).is_ok());
                assert!(ChipRead::try_from(r2).is_ok());
            }
            _ => panic!("expected chip read messages"),
        }

        sender_task.abort();
        let _ = sender_task.await;
    }

    #[tokio::test]
    async fn broadcast_reads_verbatim_once_emits_exact_file_frames_then_stops() {
        // Two distinct frames; verbatim means they must arrive byte-for-byte
        // (only a trailing CRLF appended) and `once` means the generator stops
        // after the last one.
        let frames = vec![
            "aa400000000000010a2a01123018455900e8".to_owned(),
            "aa400000000000020a2a01123018455900e9".to_owned(),
        ];
        let (tx, mut rx) = broadcast::channel::<ReadFrame>(8);
        let gen_task = tokio::spawn(broadcast_reads(
            1,
            frames.clone(),
            tx.clone(),
            ReadType::RAW,
            true,
            true,
            false,
        ));

        let first = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let second = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.data.as_ref(), format!("{}\r\n", frames[0]));
        assert_eq!(second.data.as_ref(), format!("{}\r\n", frames[1]));

        // The generator must complete (no infinite loop) after the last read.
        timeout(Duration::from_secs(1), gen_task)
            .await
            .expect("broadcast_reads should stop in once mode")
            .unwrap();
    }

    #[tokio::test]
    async fn broadcast_reads_pause_when_unsubscribed_resumes_without_skipping() {
        // Four distinct verbatim frames. With `pause_when_unsubscribed`, the
        // generator must not advance past the next pending read while no client
        // is connected: when a client reconnects it resumes from exactly the
        // read it paused on, so a power-loss restart is lossless.
        let frames = vec![
            "aa400000000000010a2a01123018455900e8".to_owned(),
            "aa400000000000020a2a01123018455900e9".to_owned(),
            "aa400000000000030a2a01123018455900ea".to_owned(),
            "aa400000000000040a2a01123018455900eb".to_owned(),
        ];
        let delay_ms = 50;
        let (tx, _) = broadcast::channel::<ReadFrame>(16);
        let mut rx1 = tx.subscribe();
        let gen_task = tokio::spawn(broadcast_reads(
            delay_ms,
            frames.clone(),
            tx.clone(),
            ReadType::RAW,
            true,
            true,
            true,
        ));

        // Helper: receive a frame and acknowledge delivery (simulating a
        // client's successful write+flush) so the generator may advance.
        async fn recv_ack(rx: &mut broadcast::Receiver<ReadFrame>) -> String {
            let frame = timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap();
            frame.ack.store(true, Ordering::Release);
            frame.data.to_string()
        }

        // First read is delivered to (and acked by) the initial subscriber.
        let got1 = recv_ack(&mut rx1).await;
        assert_eq!(got1, format!("{}\r\n", frames[0]));

        // Consumer "dies": drop the only subscriber. The generator must pause
        // before emitting the next read rather than racing ahead.
        drop(rx1);

        // Wait several read intervals; if the generator were not pausing it
        // would emit (and drop) frames 2..4 and the `once` generator would even
        // complete during this window.
        sleep(Duration::from_millis(delay_ms * 6)).await;

        // Consumer "restarts": a fresh subscriber must receive the exact read
        // the stream paused on (frame 2), then the remainder in order.
        let mut rx2 = tx.subscribe();
        let got2 = recv_ack(&mut rx2).await;
        assert_eq!(got2, format!("{}\r\n", frames[1]));
        let got3 = recv_ack(&mut rx2).await;
        assert_eq!(got3, format!("{}\r\n", frames[2]));
        let got4 = recv_ack(&mut rx2).await;
        assert_eq!(got4, format!("{}\r\n", frames[3]));

        // The generator must complete after the last read in `once` mode.
        timeout(Duration::from_secs(1), gen_task)
            .await
            .expect("broadcast_reads should stop in once mode after resume")
            .unwrap();
    }

    #[tokio::test]
    async fn broadcast_reads_pause_retries_undelivered_frame_for_next_subscriber() {
        // A consumer connects (subscribes) but dies before ever delivering
        // (writing+flushing) the first read. In pause mode the generator must
        // NOT advance past that read: the next subscriber that reconnects must
        // receive the exact same first frame, never the second. Otherwise a
        // forwarder SIGKILLed after the receiver-count guard but before the
        // write would silently lose the in-flight read.
        let frames = vec![
            "aa400000000000010a2a01123018455900e8".to_owned(),
            "aa400000000000020a2a01123018455900e9".to_owned(),
        ];
        let delay_ms = 50;
        let (tx, _) = broadcast::channel::<ReadFrame>(16);
        // Subscribe a consumer that never reads/acks the frame (it "dies").
        let rx1 = tx.subscribe();
        let gen_task = tokio::spawn(broadcast_reads(
            delay_ms,
            frames.clone(),
            tx.clone(),
            ReadType::RAW,
            true,
            true,
            true,
        ));

        // Let the generator emit the first frame toward rx1, then the consumer
        // dies before delivering it (it never reads or acks the frame).
        sleep(Duration::from_millis(delay_ms / 2)).await;
        drop(rx1);
        // Give the generator time to observe that no subscribers remain.
        sleep(Duration::from_millis(delay_ms)).await;

        // A fresh consumer reconnects: it must receive the SAME first frame
        // (frame 0), not the second one, because the first was never delivered.
        let mut rx2 = tx.subscribe();
        let got = timeout(Duration::from_secs(1), rx2.recv())
            .await
            .expect("reconnected subscriber should receive the retried frame")
            .unwrap();
        assert_eq!(got.data.as_ref(), format!("{}\r\n", frames[0]));

        gen_task.abort();
        let _ = gen_task.await;
    }

    #[tokio::test]
    async fn send_reads_stays_alive_when_bus_receiver_is_closed() {
        let (bus_tx, bus_rx) = mpsc::channel(1);
        drop(bus_rx);

        let mut sender_task = tokio::spawn(send_reads(
            1,
            Vec::new(),
            bus_tx,
            ReadType::RAW,
            false,
            false,
        ));
        tokio::time::sleep(Duration::from_millis(15)).await;
        let still_running = timeout(Duration::from_millis(10), &mut sender_task)
            .await
            .is_err();
        assert!(still_running);
        sender_task.abort();
        assert!(sender_task.await.unwrap_err().is_cancelled());
    }
}

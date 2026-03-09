use std::sync::Arc;
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

pub struct EmulatorConfig {
    pub bind_port: u16,
    pub delay: u64,
    pub file_path: Option<String>,
    pub read_type: ReadType,
}

pub async fn send_reads(
    delay: u64,
    file_reads: Vec<String>,
    bus_tx: Sender<Message>,
    read_type: ReadType,
) {
    let mut index = 0;
    loop {
        let mut chip_read = if file_reads.is_empty() {
            generate_read(read_type)
        } else {
            let read = restamp_read(&file_reads[index]);
            index = (index + 1) % file_reads.len();
            read
        };
        chip_read.push_str("\r\n");
        bus_tx
            .send(Message::CHIP_READ(chip_read))
            .await
            .unwrap_or_else(|_| {
                println!("\r\x1b[2KError sending read to thread. Maybe no readers are conected?");
            });
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
    let fut_sender = send_reads(config.delay, file_reads, bus_tx.clone(), config.read_type).fuse();

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
    tx: broadcast::Sender<String>,
    read_type: ReadType,
) {
    let mut index = 0;
    loop {
        let mut chip_read = if file_reads.is_empty() {
            generate_read(read_type)
        } else {
            let read = restamp_read(&file_reads[index]);
            index = (index + 1) % file_reads.len();
            read
        };
        chip_read.push_str("\r\n");
        // If there are no subscribers the send will error — that is fine.
        let _ = tx.send(chip_read);
        sleep(Duration::from_millis(delay)).await;
    }
}

/// Run the emulator with bidirectional TCP support.
///
/// Unlike `run()`, this function handles both outgoing chip reads **and**
/// incoming control frames on each TCP connection.  A banner is sent to
/// every new client on connect.
pub async fn run_with_control(config: EmulatorConfig, state: EmulatedReaderState) {
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

    let (read_tx, _) = broadcast::channel::<String>(1000);
    let shared_state = Arc::new(Mutex::new(state));

    // Spawn the read-generation task.
    let read_tx_clone = read_tx.clone();
    let _read_gen_handle = tokio::spawn(broadcast_reads(
        config.delay,
        file_reads,
        read_tx_clone,
        config.read_type,
    ));

    let listener = TcpListener::bind(("0.0.0.0", config.bind_port))
        .await
        .expect("failed to bind TCP listener");

    // Accept loop — runs until the task is aborted externally (e.g. signal or
    // test harness calling `abort()`).
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };

        let state = Arc::clone(&shared_state);
        let read_rx = read_tx.subscribe();

        tokio::spawn(async move {
            handle_client(stream, state, read_rx).await;
        });
    }

    // The compiler needs this to be reachable for type-checking even though
    // `loop` above is infinite.
    #[allow(unreachable_code)]
    {
        _read_gen_handle.abort();
    }
}

/// Handle a single bidirectional TCP client connection.
async fn handle_client(
    stream: tokio::net::TcpStream,
    state: Arc<Mutex<EmulatedReaderState>>,
    read_rx: broadcast::Receiver<String>,
) {
    let (read_half, write_half) = stream.into_split();

    // Per-client channel for control responses and the initial banner.
    let (client_tx, client_rx) = tokio::sync::mpsc::channel::<String>(256);

    // Send the banner through the per-client channel.
    {
        let st = state.lock().await;
        for line in st.banner.lines() {
            let _ = client_tx.send(format!("{}\r\n", line)).await;
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
async fn client_read_loop(
    read_half: tokio::net::tcp::OwnedReadHalf,
    state: Arc<Mutex<EmulatedReaderState>>,
    client_tx: tokio::sync::mpsc::Sender<String>,
) {
    let mut reader = BufReader::new(read_half);
    let mut line_buf = String::new();

    loop {
        line_buf.clear();
        match reader.read_line(&mut line_buf).await {
            Ok(0) => break, // EOF
            Ok(_) => {
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
            Err(_) => break,
        }
    }
}

/// Write task: multiplexes control responses (from per-client mpsc) and
/// chip reads (from broadcast) onto the TCP write half.
async fn client_write_task(
    write_half: tokio::net::tcp::OwnedWriteHalf,
    mut client_rx: tokio::sync::mpsc::Receiver<String>,
    mut read_rx: broadcast::Receiver<String>,
) {
    let mut writer = BufWriter::new(write_half);

    loop {
        tokio::select! {
            msg = client_rx.recv() => {
                match msg {
                    Some(data) => {
                        if writer.write_all(data.as_bytes()).await.is_err() {
                            break;
                        }
                        if writer.flush().await.is_err() {
                            break;
                        }
                    }
                    None => break, // sender dropped
                }
            }
            result = read_rx.recv() => {
                match result {
                    Ok(data) => {
                        if writer.write_all(data.as_bytes()).await.is_err() {
                            break;
                        }
                        if writer.flush().await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
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
        let sender_task = tokio::spawn(send_reads(1, file_reads, bus_tx, ReadType::RAW));

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
    async fn send_reads_stays_alive_when_bus_receiver_is_closed() {
        let (bus_tx, bus_rx) = mpsc::channel(1);
        drop(bus_rx);

        let mut sender_task = tokio::spawn(send_reads(1, Vec::new(), bus_tx, ReadType::RAW));
        tokio::time::sleep(Duration::from_millis(15)).await;
        let still_running = timeout(Duration::from_millis(10), &mut sender_task)
            .await
            .is_err();
        assert!(still_running);
        sender_task.abort();
        assert!(sender_task.await.unwrap_err().is_cancelled());
    }
}

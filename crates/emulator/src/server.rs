use std::time::Duration;
use timer_core::models::Message;
use timer_core::workers::{ClientConnector, ClientPool};
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;

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
    use futures::{future::select_all, future::FutureExt, pin_mut};
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

#[cfg(test)]
mod tests {
    use super::*;
    use ipico_core::read::ChipRead;
    use std::convert::TryFrom;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

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

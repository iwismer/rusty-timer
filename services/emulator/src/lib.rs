use chrono::{Datelike, Timelike};
use std::fs::File;
use std::io::BufReader;
use std::time::Duration;
use timer_core::models::Message;
use timer_core::workers::{ClientConnector, ClientPool};
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;

// Re-export for main.rs
pub use timer_core::models::ReadType;
pub use timer_core::util::{is_delay, is_file, is_port};

pub struct EmulatorConfig {
    pub bind_port: u16,
    pub delay: u64,
    pub file_path: Option<String>,
    pub read_type: ReadType,
}

pub fn generate_read_for_time(read_type: ReadType, now: chrono::DateTime<chrono::Local>) -> String {
    let centiseconds = (now.nanosecond() / 10000000) as u8;
    let read = format!(
        "aa00{}{:02}{:02}{:02}{:02}{:02}{:02}{:02x}",
        "05800319aeeb0001",
        now.year() % 100,
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        centiseconds
    );
    let checksum = read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
    match read_type {
        ReadType::RAW => format!("{}{:02x}", read, checksum),
        ReadType::FSLS => format!("{}{:02x}LS", read, checksum),
    }
}

pub fn generate_read(read_type: ReadType) -> String {
    generate_read_for_time(read_type, chrono::Local::now())
}

/// Replace the timestamp in a chip read with the given time and recompute the
/// checksum. If the read doesn't look like a valid IPICO read (wrong length or
/// prefix), it is returned as-is.
pub fn restamp_read_for_time(read: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let trimmed = read.trim();
    if (trimmed.len() != 36 && trimmed.len() != 38) || !trimmed.starts_with("aa") {
        return trimmed.to_owned();
    }
    let centiseconds = (now.nanosecond() / 10000000) as u8;
    let new_timestamp = format!(
        "{:02}{:02}{:02}{:02}{:02}{:02}{:02x}",
        now.year() % 100,
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        centiseconds
    );
    let mut new_read = String::with_capacity(trimmed.len());
    new_read.push_str(&trimmed[..20]);
    new_read.push_str(&new_timestamp);
    let checksum = new_read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
    new_read.push_str(&format!("{:02x}", checksum));
    if trimmed.len() == 38 {
        new_read.push_str(&trimmed[36..38]);
    }
    new_read
}

pub fn restamp_read(read: &str) -> String {
    restamp_read_for_time(read, chrono::Local::now())
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
        // Send the read to the threads
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
    use timer_core::models::Message;
    use timer_core::util::signal_handler;
    use tokio::sync::mpsc;

    let file_reads: Vec<String> = config
        .file_path
        .as_ref()
        .and_then(|p| {
            File::open(Path::new(p))
                .map_err(|e| {
                    println!("Error opening file: {}", e);
                    e
                })
                .ok()
        })
        .map(|f| {
            BufReader::new(f)
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
    use chrono::TimeZone;
    use std::convert::TryFrom;
    use timer_core::models::ChipRead;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    #[test]
    fn generated_raw_reads_parse() {
        let read = generate_read(ReadType::RAW);
        let parsed = ChipRead::try_from(read.as_str());
        assert!(parsed.is_ok());
    }

    #[test]
    fn generated_fsls_reads_parse() {
        let read = generate_read(ReadType::FSLS);
        let parsed = ChipRead::try_from(read.as_str());
        assert!(parsed.is_ok());
    }

    #[test]
    fn generated_read_shapes_are_stable() {
        let raw = generate_read(ReadType::RAW);
        assert_eq!(raw.len(), 36);
        assert!(raw.starts_with("aa"));

        let fsls = generate_read(ReadType::FSLS);
        assert_eq!(fsls.len(), 38);
        assert!(fsls.ends_with("LS"));
    }

    #[test]
    fn generated_read_encodes_centiseconds_as_hex() {
        let now = chrono::Local.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap()
            + chrono::TimeDelta::milliseconds(990);
        let read = generate_read_for_time(ReadType::RAW, now);
        assert_eq!(&read[32..34], "63");

        let parsed = ChipRead::try_from(read.as_str()).unwrap();
        assert_eq!(parsed.time_string(), "03:04:05.990");
    }

    #[test]
    fn restamp_preserves_tag_and_updates_timestamp() {
        let original = "aa400000000123450a2a01123018455927a7";
        let now = chrono::Local
            .with_ymd_and_hms(2025, 6, 15, 10, 30, 45)
            .unwrap();
        let restamped = restamp_read_for_time(original, now);
        // Tag portion preserved
        assert_eq!(&restamped[4..20], "0000000123450a2a");
        // Timestamp updated
        let parsed = ChipRead::try_from(restamped.as_str()).unwrap();
        assert_eq!(parsed.time_string(), "10:30:45.000");
    }

    #[test]
    fn restamp_preserves_fsls_suffix() {
        let original = "aa400000000123450a2a01123018455927a7LS";
        let now = chrono::Local
            .with_ymd_and_hms(2025, 6, 15, 10, 30, 45)
            .unwrap();
        let restamped = restamp_read_for_time(original, now);
        assert!(restamped.ends_with("LS"));
        assert_eq!(restamped.len(), 38);
        assert!(ChipRead::try_from(restamped.as_str()).is_ok());
    }

    #[test]
    fn restamp_returns_invalid_reads_unchanged() {
        let now = chrono::Local
            .with_ymd_and_hms(2025, 6, 15, 10, 30, 45)
            .unwrap();
        assert_eq!(restamp_read_for_time("not a read", now), "not a read");
        assert_eq!(restamp_read_for_time("", now), "");
    }

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

        // Both reads preserve the tag portion (positions 4..20) from the file
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

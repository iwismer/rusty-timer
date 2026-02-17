use chrono::{Datelike, Timelike};
use std::fs::File;
use std::io::{BufReader, Lines};
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

pub async fn send_reads(
    delay: u64,
    mut file_reader: Option<Lines<BufReader<File>>>,
    bus_tx: Sender<Message>,
    read_type: ReadType,
) {
    loop {
        // Convert to string
        let mut chip_read: String = match file_reader.as_mut() {
            Some(lines) => match lines.next() {
                Some(line) => match line {
                    Ok(line) => line.trim().to_owned(),
                    Err(error) => {
                        eprintln!("Error reading line from file: {}", error);
                        file_reader = None;
                        generate_read(read_type)
                    }
                },
                None => {
                    file_reader = None;
                    generate_read(read_type)
                }
            },
            None => generate_read(read_type),
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

    let file_reader = config.file_path.as_ref().and_then(|p| {
        File::open(Path::new(p))
            .map(|f| BufReader::new(f).lines())
            .map_err(|e| {
                println!("Error opening file: {}", e);
                e
            })
            .ok()
    });

    let (bus_tx, rx) = mpsc::channel::<Message>(1000);
    let client_pool = ClientPool::new(rx, None, None, false);
    let connector = ClientConnector::new(config.bind_port, bus_tx.clone()).await;

    let fut_clients = client_pool.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_sig = signal_handler().fuse();
    let fut_sender = send_reads(config.delay, file_reader, bus_tx.clone(), config.read_type).fuse();

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
    use std::fs::File;
    use std::io::{BufRead, Write};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
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

    fn tmp_file_path(name_prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current time")
            .as_nanos();
        std::env::temp_dir().join(format!("{}_{}.txt", name_prefix, nonce))
    }

    #[tokio::test]
    async fn send_reads_uses_file_line_then_falls_back_to_generated() {
        let input_path = tmp_file_path("emulator_send_reads");
        let mut file = File::create(&input_path).unwrap();
        writeln!(file, "aa400000000123450a2a01123018455927a7").unwrap();

        let file = File::open(&input_path).unwrap();
        let file_reader = Some(BufReader::new(file).lines());
        let (bus_tx, mut bus_rx) = mpsc::channel(8);
        let sender_task = tokio::spawn(send_reads(1, file_reader, bus_tx, ReadType::RAW));

        let first = timeout(Duration::from_millis(100), bus_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let second = timeout(Duration::from_millis(100), bus_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match first {
            Message::CHIP_READ(read) => {
                assert_eq!(read, "aa400000000123450a2a01123018455927a7\r\n".to_owned());
            }
            _ => panic!("expected first message to be a chip read"),
        }
        match second {
            Message::CHIP_READ(read) => {
                assert!(read.ends_with("\r\n"));
                assert_ne!(read, "aa400000000123450a2a01123018455927a7\r\n");
            }
            _ => panic!("expected a chip read message"),
        }

        sender_task.abort();
        let _ = sender_task.await;
        let _ = std::fs::remove_file(&input_path);
    }

    #[tokio::test]
    async fn send_reads_stays_alive_when_bus_receiver_is_closed() {
        let (bus_tx, bus_rx) = mpsc::channel(1);
        drop(bus_rx);

        let mut sender_task = tokio::spawn(send_reads(1, None, bus_tx, ReadType::RAW));
        tokio::time::sleep(Duration::from_millis(15)).await;
        let still_running = timeout(Duration::from_millis(10), &mut sender_task)
            .await
            .is_err();
        assert!(still_running);
        sender_task.abort();
        assert!(sender_task.await.unwrap_err().is_cancelled());
    }
}

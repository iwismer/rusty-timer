use crate::models::{Message, ReadType};
use std::net::SocketAddrV4;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tokio::time::{sleep, Duration};

fn reconnect_delay_ms(connect_failures: u32) -> u64 {
    if connect_failures == 0 {
        return 0;
    }
    let shift = connect_failures.saturating_sub(1).min(6);
    (100u64 << shift).min(5000)
}

fn next_reconnect_delay_ms(connect_failures: &mut u32) -> u64 {
    *connect_failures = connect_failures.saturating_add(1);
    reconnect_delay_ms(*connect_failures)
}

/// Receives reads from the reader, then forwards them to the client pool.
#[derive(Debug)]
pub struct TimingReader {
    addr: SocketAddrV4,
    read_type: ReadType,
    stream: Option<TcpStream>,
    chip_read_bus: Sender<Message>,
}

impl TimingReader {
    pub fn new(addr: SocketAddrV4, read_type: ReadType, chip_read_bus: Sender<Message>) -> Self {
        println!("Waiting for reader: {}", addr);

        TimingReader {
            addr,
            read_type,
            stream: None::<TcpStream>,
            chip_read_bus,
        }
    }

    /// Start listening for reads.
    ///
    /// This function should never return.
    pub async fn begin(&mut self) {
        let mut input_buffer = vec![0u8; self.read_type as usize];
        let mut connect_failures = 0u32;
        loop {
            match self.stream.as_mut() {
                Some(stream) => {
                    // Get 38 bytes from the stream, which is exactly 1 read
                    match stream.read_exact(&mut input_buffer).await {
                        Ok(_) => {}
                        Err(e) => {
                            let delay_ms = next_reconnect_delay_ms(&mut connect_failures);
                            println!(
                                "\r\x1b[2KError reading from reader: {}. Retrying in {}ms",
                                e, delay_ms
                            );
                            self.stream = None::<TcpStream>;
                            sleep(Duration::from_millis(delay_ms)).await;
                            continue;
                        }
                    }
                    // Convert to string
                    let read = match std::str::from_utf8(&input_buffer) {
                        Ok(read) => read,
                        Err(error) => {
                            println!("\r\x1b[2KError parsing chip read: {}", error);
                            continue;
                        }
                    };
                    // Send the read to the threads
                    self.chip_read_bus
                        .send(Message::CHIP_READ(read.to_owned()))
                        .await
                        .unwrap_or_else(|_| {
                            println!(
                        "\r\x1b[2KError sending read to thread. Maybe no readers are connected?"
                    );
                        });
                }
                None => {
                    let stream = match TcpStream::connect(self.addr).await {
                        Ok(stream) => {
                            println!("Connected to reader: {}", self.addr);
                            connect_failures = 0;
                            stream
                        }
                        Err(error) => {
                            let delay_ms = next_reconnect_delay_ms(&mut connect_failures);
                            println!(
                                "Failed to connect to reader: {}. Retrying in {}ms",
                                error, delay_ms
                            );
                            sleep(Duration::from_millis(delay_ms)).await;
                            continue;
                        }
                    };
                    self.stream = Some(stream);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{next_reconnect_delay_ms, reconnect_delay_ms};

    #[test]
    fn reconnect_delay_starts_at_zero_and_grows() {
        assert_eq!(reconnect_delay_ms(0), 0);
        assert_eq!(reconnect_delay_ms(1), 100);
        assert_eq!(reconnect_delay_ms(2), 200);
        assert_eq!(reconnect_delay_ms(3), 400);
        assert_eq!(reconnect_delay_ms(6), 3200);
    }

    #[test]
    fn reconnect_delay_caps() {
        assert_eq!(reconnect_delay_ms(7), 5000);
        assert_eq!(reconnect_delay_ms(100), 5000);
    }

    #[test]
    fn reconnect_delay_counter_increments() {
        let mut failures = 0;
        assert_eq!(next_reconnect_delay_ms(&mut failures), 100);
        assert_eq!(failures, 1);
        assert_eq!(next_reconnect_delay_ms(&mut failures), 200);
        assert_eq!(failures, 2);
    }
}

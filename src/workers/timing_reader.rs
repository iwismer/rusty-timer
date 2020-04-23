use crate::models::{chip::ReadType, Message};
use std::net::SocketAddrV4;
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::sync::mpsc::Sender;

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
        loop {
            match self.stream.as_mut() {
                Some(stream) => {
                    // Get 38 bytes from the stream, which is exactly 1 read
                    match stream.read_exact(&mut input_buffer).await {
                        // Valid read
                        Ok(size) if size == 38 || size == 40 => {}
                        // The stream is EOF, so try to reconnect next loop
                        Ok(size) if size == 0 => {
                            self.stream = None;
                            continue;
                        }
                        // Invalid data
                        Ok(size) => {
                            println!("Didn't read enough data from stream. Read {} bytes", size);
                            continue;
                        }
                        Err(e) => {
                            println!("\r\x1b[2KError reading from reader: {}", e);
                            self.stream = None::<TcpStream>;
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
                        .send(Message::CHIP_READ(read.to_string()))
                        .await
                        .unwrap_or_else(|_| {
                            println!(
                        "\r\x1b[2KError sending read to thread. Maybe no readers are conected?"
                    );
                        });
                }
                None => {
                    let stream = match TcpStream::connect(self.addr).await {
                        Ok(stream) => {
                            println!("Connected to reader: {}", self.addr);
                            stream
                        }
                        Err(error) => {
                            println!("Failed to connect to reader: {}", error);
                            continue;
                        }
                    };
                    self.stream = Some(stream);
                }
            }
        }
    }
}

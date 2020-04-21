use crate::models::{ChipRead, Gender, Participant};
use rusqlite::Connection;
use std::fs::File;
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::path::Path;
use std::process;
use tokio::net::TcpStream;
use tokio::prelude::*;
// use tokio::sync::broadcast::Sender;
use crate::models::Message;
use tokio::sync::mpsc::Sender;

fn read_to_string(read: &str, conn: &rusqlite::Connection, read_count: &u32) -> String {
    match ChipRead::new(read.to_string()) {
        Err(desc) => format!("Error reading chip {}", desc),
        Ok(read) => {
            let mut stmt = conn
                .prepare(
                    "SELECT
                            c.id,
                            c.bib,
                            p.first_name,
                            p.last_name
                            FROM chip c
                            LEFT JOIN participant p
                            ON c.bib = p.bib
                            WHERE c.id = ?",
                )
                .unwrap();
            // Make the query and map to a participant
            let row = stmt.query_row(&[read.tag_id.as_str()], |row| {
                Ok(Participant {
                    // If there is a missing field, then map it to unknown
                    chip_id: vec![row.get(0).unwrap_or("None".to_string())],
                    bib: row.get(1).unwrap_or(0),
                    first_name: row.get(2).unwrap_or("Unknown".to_string()),
                    last_name: row.get(3).unwrap_or("Participant".to_string()),
                    gender: Gender::X,
                    age: None,
                    affiliation: None,
                    division: None,
                })
            });

            match row {
                // Bandit chip
                Err(_) => format!(
                    "Total Reads: {} Last Read: Unknown Chip {} {}",
                    read_count,
                    read.tag_id,
                    read.time_string()
                ),
                // Good chip, either good or unknown participant
                Ok(participant) => {
                    // println!("{:?}", participant);
                    format!(
                        "Total Reads: {} Last Read: {} {} {} {}",
                        read_count,
                        participant.bib,
                        participant.first_name,
                        participant.last_name,
                        read.time_string()
                    )
                }
            }
        }
    }
}

/// Receives reads from the reader, then forwards them to the client pool.
pub struct ReadBroadcaster {
    stream: TcpStream,
    file_writer: Option<File>,
    chip_read_bus: Sender<Message>,
    buffered_output: bool,
    db_conn: Connection,
}

impl ReadBroadcaster {
    pub async fn new(
        reader_ip: Ipv4Addr,
        reader_port: u16,
        chip_read_bus: Sender<Message>,
        db_conn: Connection,
        out_file: Option<String>,
        buffered_output: bool,
    ) -> Self {
        println!("Waiting for reader: {}:{}", reader_ip, reader_port);
        // Connect to the reader
        let stream = match TcpStream::connect((reader_ip, reader_port)).await {
            Ok(stream) => {
                println!("Connected to reader: {}:{}", reader_ip, reader_port);
                stream
            }
            Err(error) => {
                println!("Failed to connect to reader: {}", error);
                process::exit(1);
            }
        };

        // Check if the user has specified to save the reads to a file
        let mut file_writer: Option<File> = None;
        if out_file.is_some() {
            let path = out_file.unwrap();
            // Create the file writer for saving reads
            let file_path = Path::new(&path);
            file_writer = match File::create(file_path) {
                Ok(file) => Some(file),
                Err(error) => {
                    println!("Error creating file: {}", error);
                    process::exit(1);
                }
            };
        }

        ReadBroadcaster {
            stream,
            file_writer,
            chip_read_bus,
            buffered_output,
            db_conn,
        }
    }

    /// Start listening for reads.
    /// This function should never return.
    pub async fn begin(mut self) {
        // Check if running on windows, and set line ending.
        let line_ending = match cfg!(windows) {
            true => "\r\n",
            false => "\n",
        };

        let mut input_buffer = [0u8; 38];
        let mut read_count: u32 = 0;
        loop {
            // Get 38 bytes from the stream, which is exactly 1 read
            match self.stream.read_exact(&mut input_buffer).await {
                Ok(_) => (),
                Err(e) => {
                    println!("\r\x1b[2KError reading from reader: {}", e);
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        println!("Reader disconnected!");
                        process::exit(1);
                    }
                    continue;
                }
            }
            read_count += 1;
            // Convert to string
            let read = match std::str::from_utf8(&input_buffer) {
                Ok(read) => read,
                Err(error) => {
                    println!("\r\x1b[2KError parsing chip read: {}", error);
                    continue;
                }
            };
            // println!("'{}'", read);
            // Only write to file if a file was supplied
            if self.file_writer.is_some() {
                write!(
                    // This unwrap is safe as file_writer has been
                    // proven to be Some(T)
                    self.file_writer.as_mut().unwrap(),
                    "{}{}",
                    read.replace(|c: char| !c.is_alphanumeric(), ""),
                    // Use \r\n on a windows machine
                    line_ending
                )
                .unwrap_or_else(|e| {
                    println!("\r\x1b[2KError writing read to file: {}", e);
                });
            }
            // Lock the bus so I can send data along it
            // Send the read to the threads
            self.chip_read_bus
                .send(Message::CHIP_READ(read.to_string()))
                .await
                .unwrap_or_else(|_| {
                    println!(
                        "\r\x1b[2KError sending read to thread. Maybe no readers are conected?"
                    );
                });
            let to_print = read_to_string(&read, &self.db_conn, &read_count);
            print!("\r\x1b[2K{}", to_print);
            // only flush if the output is unbuffered
            // This can cause high CPU use on some systems
            if !self.buffered_output {
                io::stdout().flush().unwrap_or(());
            }
        }
    }
}

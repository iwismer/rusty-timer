use super::Client;
use crate::models::Message;
use crate::models::{ChipRead, Gender, Participant};
use futures::future::join_all;
use rusqlite::Connection;
use std::collections::HashSet;
use std::convert::TryFrom;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use tokio::sync::mpsc::Receiver;

fn read_to_string(read: &str, conn: &rusqlite::Connection, read_count: &u32) -> String {
    match ChipRead::try_from(read) {
        Err(desc) => format!("Error reading chip {}", desc),
        Ok(read) => {
            let mut stmt = conn
                .prepare_cached(
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
            let row = stmt.query_row([read.tag_id.as_str()], |row| {
                Ok(Participant {
                    // If there is a missing field, then map it to unknown
                    chip_id: vec![row.get(0).unwrap_or("None".to_owned())],
                    bib: row.get(1).unwrap_or(0),
                    first_name: row.get(2).unwrap_or("Unknown".to_owned()),
                    last_name: row.get(3).unwrap_or("Participant".to_owned()),
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

/// Contains a vec of all the clients and forwards reads to them
pub struct ClientPool {
    clients: Vec<Client>,
    bus: Receiver<Message>,
    file_writer: Option<File>,
    buffered_output: bool,
    db_conn: Option<Connection>,
}

impl ClientPool {
    pub fn new(
        bus: Receiver<Message>,
        db_conn: Option<Connection>,
        out_file: Option<String>,
        buffered_output: bool,
    ) -> Self {
        // Check if the user has specified to save the reads to a file
        let mut file_writer: Option<File> = None;
        if let Some(path) = out_file {
            // Create the file writer for saving reads
            let file_path = Path::new(&path);
            file_writer = match File::create(file_path) {
                Ok(file) => Some(file),
                Err(error) => {
                    // File saving is important, so panic if it fails.
                    // This should never happen, as the file location should be
                    // checked when taking in args.
                    panic!("Error creating file: {}", error);
                }
            };
        }

        ClientPool {
            clients: Vec::new(),
            bus,
            file_writer,
            buffered_output,
            db_conn,
        }
    }

    /// Begin listening for new clients and reads.
    ///
    /// This function should never return.
    pub async fn begin(mut self) {
        // Check if running on windows, and set line ending.
        let line_ending = match cfg!(windows) {
            true => "\r\n",
            false => "\n",
        };
        let mut read_count: u32 = 0;
        loop {
            let message = match self.bus.recv().await {
                Some(message) => message,
                None => return,
            };

            match message {
                Message::CHIP_READ(r) => {
                    read_count += 1;
                    // Only write to file if a file was supplied
                    if let Some(file_writer) = self.file_writer.as_mut() {
                        write!(
                            file_writer,
                            "{}{}",
                            r.replace(|c: char| !c.is_alphanumeric(), ""),
                            line_ending
                        )
                        .unwrap_or_else(|e| {
                            println!("\r\x1b[2KError writing read to file: {}", e);
                        });
                    }
                    if let Some(conn) = &self.db_conn {
                        let to_print = read_to_string(&r, conn, &read_count);
                        print!("\r\x1b[2K{}", to_print);
                        // only flush if the output is unbuffered
                        // This can cause high CPU use on some systems
                        if !self.buffered_output {
                            io::stdout().flush().unwrap_or(());
                        }
                    }

                    let mut futures = Vec::new();
                    for client in self.clients.iter_mut() {
                        futures.push(client.send_read(r.clone()));
                    }
                    let results = join_all(futures).await;
                    // If a client returned an error, remove it from future
                    // transmissions.
                    let failed_addrs: HashSet<_> = results.into_iter().filter_map(Result::err).collect();
                    if !failed_addrs.is_empty() {
                        self.clients.retain(|client| !failed_addrs.contains(&client.get_addr()));
                    }
                }
                Message::SHUTDOWN => {
                    return;
                }
                Message::CLIENT(c) => {
                    self.clients.push(c);
                }
            }
        }
    }
}

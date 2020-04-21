/*
Copyright © 2020  Isaac Wismer

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/
// use super::client_pool::CLIENTS;
use super::Client;
use crate::models::Message;
use crate::CONNECTION_COUNT;
use futures::executor::block_on;
use futures::future::Future;
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::thread;
use tokio::io::Result as t_Result;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::RecvError;
// use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc::Sender;

pub struct ClientConnector {
    listen_stream: TcpListener,
    chip_read_bus: Sender<Message>,
    // signal_bus: Sender<bool>,
}

impl ClientConnector {
    pub async fn new(
        bind_port: u16,
        chip_read_bus: Sender<Message>,
        // signal_bus: Sender<bool>,
    ) -> Self {
        // Bind to the listening port to allow other computers to connect
        let listener = TcpListener::bind(("0.0.0.0", bind_port))
            .await
            .expect("Unable to bind to port");
        println!("Bound to port: {}", listener.local_addr().unwrap().port());

        ClientConnector {
            listen_stream: listener,
            chip_read_bus,
            // signal_bus,
        }
    }

    pub async fn begin(mut self) {
        loop {
            // let futures = Vec::new();
            // futures.push(self.listen_stream.accept());
            // let chip_read_rx = self.chip_read_bus.subscribe();
            // futures.push(chip_read_rx.recv());

            // wait for a connection, then connect when it comes
            match self.listen_stream.accept().await {
                Ok((stream, addr)) => {
                    // Increment the number of connections
                    CONNECTION_COUNT.fetch_add(1, Ordering::SeqCst);
                    // Add a receiver for the connection
                    // let chip_read_rx = self.chip_read_bus.subscribe();
                    // let signal = self.signal_bus.subscribe();
                    match Client::new(stream, addr) {
                        Err(_) => eprintln!("\r\x1b[2KError connecting to client"),
                        Ok(client) => {
                            // CLIENTS.lock().await.deref().push(client);
                            self.chip_read_bus.send(Message::CLIENT(client)).await;
                            // TODO: Fix when async closures stabilize
                            // thread::spawn(|| {
                            //     let c = client.begin();
                            //     block_on(c);
                            // });
                            // clients.push(client);
                            println!("\r\x1b[2KConnected to client: {}", addr)
                        }
                    };
                }
                Err(error) => {
                    println!("Failed to connect to client: {}", error);
                }
            }
        }
    }
}

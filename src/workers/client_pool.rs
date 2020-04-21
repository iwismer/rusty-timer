use super::Client;
use crate::models::Message;
use crate::CONNECTION_COUNT;
use futures::future::join_all;
use std::sync::atomic::Ordering;
// use tokio::sync::broadcast::{Receiver, Sender};
use tokio::sync::Mutex;
use std::ops::{DerefMut, Deref};
use tokio::sync::mpsc::Receiver;

// pub static CLIENTS: Mutex<Vec<Client>> = Mutex::new(Vec::new());

pub struct ClientPool {
    clients: Vec<Client>,
    // bus_tx: Sender<Message>,
    bus_rx: Receiver<Message>,
}

impl ClientPool {
    pub fn new(bus_rx: Receiver<Message>) -> Self {
        ClientPool {
            clients: Vec::new(),
            // bus_tx,
            bus_rx,
        }
    }

    pub async fn begin(mut self) {
        loop {
            match self.bus_rx.recv().await.unwrap() {
                Message::CHIP_READ(r) => {
                    let mut futures = Vec::new();
                    for client in self.clients.iter_mut() {
                        futures.push(client.send_read(r.clone()));
                    }
                    join_all(futures).await;
                }
                Message::SHUTDOWN => {
                    for client in self.clients {
                        client.exit();
                        CONNECTION_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }
                    return;
                }
                Message::CLIENT(c) => {
                    self.clients.push(c);
                }
            }
        }
    }
}

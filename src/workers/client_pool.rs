use super::Client;
use crate::models::Message;
use futures::future::join_all;
use tokio::sync::mpsc::Receiver;

pub struct ClientPool {
    clients: Vec<Client>,
    bus_rx: Receiver<Message>,
}

impl ClientPool {
    pub fn new(bus_rx: Receiver<Message>) -> Self {
        ClientPool {
            clients: Vec::new(),
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
                    let results = join_all(futures).await;
                    for r in results.iter() {
                        if r.is_err() {
                            let pos = self
                                .clients
                                .iter()
                                .position(|c| c.get_addr() == r.err().unwrap());
                            if pos.is_some() {
                                self.clients.remove(pos.unwrap());
                            }
                        }
                    }
                }
                Message::SHUTDOWN => {
                    for client in self.clients {
                        client.exit();
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

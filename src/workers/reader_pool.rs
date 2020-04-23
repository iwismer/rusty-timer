use super::TimingReader;
use crate::models::Message;
use futures::future::join_all;
use std::net::SocketAddrV4;
use tokio::sync::mpsc::Sender;

/// Contains a vec of the readers and runs them asynchronously
#[derive(Debug)]
pub struct ReaderPool {
    readers: Vec<TimingReader>,
    bus: Sender<Message>,
}

impl ReaderPool {
    pub fn new(reader_addrs: Vec<SocketAddrV4>, bus: Sender<Message>) -> Self {
        let readers = reader_addrs
            .iter()
            .map(|a| TimingReader::new(*a, bus.clone()))
            .collect();
        ReaderPool { readers, bus }
    }

    /// Start connections to readers, and listen for new reads.
    pub async fn begin(&mut self) {
        let mut futures = Vec::new();
        for reader in self.readers.iter_mut() {
            futures.push(reader.begin());
        }
        join_all(futures).await;
    }
}

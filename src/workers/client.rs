use std::net::SocketAddr;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// Holds a connection to a single client, and forwards reads to it.
#[derive(Debug)]
pub struct Client {
    stream: TcpStream,
    addr: SocketAddr,
}

impl Client {
    pub fn new(stream: TcpStream, addr: SocketAddr) -> Result<Client, &'static str> {
        Ok(Client { stream, addr })
    }

    /// Send a single read to the connected client.
    pub async fn send_read(&mut self, read: String) -> Result<(), SocketAddr> {
        self.stream
            .write_all(read.as_bytes())
            .await
            .map_err(|_| self.addr)
    }

    pub fn get_addr(&self) -> SocketAddr {
        self.addr
    }
}

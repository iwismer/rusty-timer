use std::net::Shutdown;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::prelude::*;

/// Holds a connection to a single client, and forwards reads to it.
#[derive(Debug)]
pub struct Client {
    stream: TcpStream,
    addr: SocketAddr,
}

impl Client {
    pub fn new(stream: TcpStream, addr: SocketAddr) -> Result<Client, &'static str> {
        Ok(Client {
            stream: stream,
            addr,
        })
    }

    pub async fn send_read(&mut self, read: String) -> Result<usize, SocketAddr> {
        self.stream
            .write(read.as_bytes())
            .await
            .map_err(|_| self.addr)
    }

    pub fn exit(&self) {
        match self.stream.shutdown(Shutdown::Both) {
            Ok(_) => println!("\r\x1b[2KClient disconnected gracefully."),
            Err(e) => eprintln!("\r\x1b[2KError disconnecting: {}", e),
        };
    }

    pub fn get_addr(&self) -> SocketAddr {
        self.addr
    }
}

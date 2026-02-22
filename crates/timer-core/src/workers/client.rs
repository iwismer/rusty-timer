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

#[cfg(test)]
mod tests {
    use super::Client;
    use std::net::SocketAddr;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};

    async fn connected_pair() -> (TcpStream, SocketAddr, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let listen_addr = listener.local_addr().expect("listener addr");
        let connect = tokio::spawn(async move { TcpStream::connect(listen_addr).await });
        let (server_stream, peer_addr) = listener.accept().await.expect("accept");
        let peer_stream = connect.await.expect("join").expect("connect");
        (server_stream, peer_addr, peer_stream)
    }

    #[tokio::test]
    async fn send_read_writes_bytes_to_peer() {
        let (server_stream, peer_addr, mut peer_stream) = connected_pair().await;
        let mut client = Client::new(server_stream, peer_addr).expect("client");

        client.send_read("chip-read-1\n".to_owned()).await.unwrap();

        let mut buf = vec![0u8; "chip-read-1\n".len()];
        peer_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, b"chip-read-1\n");
    }

    #[tokio::test]
    async fn send_read_returns_client_addr_when_peer_disconnected() {
        let (server_stream, peer_addr, peer_stream) = connected_pair().await;
        let mut client = Client::new(server_stream, peer_addr).expect("client");

        drop(peer_stream);

        for _ in 0..200 {
            let result = client.send_read("x".repeat(64 * 1024)).await;
            if result.is_err() {
                assert_eq!(result, Err(peer_addr));
                return;
            }
        }

        panic!("expected disconnected peer write to fail");
    }

    #[tokio::test]
    async fn get_addr_returns_original_peer_addr() {
        let (server_stream, peer_addr, _peer_stream) = connected_pair().await;
        let client = Client::new(server_stream, peer_addr).expect("client");

        assert_eq!(client.get_addr(), peer_addr);
    }
}

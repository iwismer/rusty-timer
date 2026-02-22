use super::Client;
use crate::models::Message;
use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;

/// A worker that connects to clients and passes them along to the pool.
pub struct ClientConnector {
    listen_stream: TcpListener,
    bus: Sender<Message>,
}

impl ClientConnector {
    pub async fn new(bind_port: u16, bus: Sender<Message>) -> Self {
        // Bind to the listening port to allow other computers to connect
        let listener = TcpListener::bind(("0.0.0.0", bind_port))
            .await
            .expect("Unable to bind to port");
        println!("Bound to port: {}", listener.local_addr().unwrap().port());

        ClientConnector {
            listen_stream: listener,
            bus,
        }
    }

    /// Start listening for client connections.
    ///
    /// This function should never return.
    pub async fn begin(self) {
        loop {
            // wait for a connection, then connect when it comes
            match self.listen_stream.accept().await {
                Ok((stream, addr)) => {
                    match Client::new(stream, addr) {
                        Err(_) => eprintln!("\r\x1b[2KError connecting to client"),
                        Ok(client) => {
                            if self.bus.send(Message::CLIENT(client)).await.is_err() {
                                println!("\r\x1b[2KClient bus unavailable, stopping connector.");
                                return;
                            }
                            println!("\r\x1b[2KConnected to client: {}", addr)
                        }
                    };
                }
                Err(error) => {
                    println!("\r\x1b[2KFailed to connect to client: {}", error);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ClientConnector;
    use crate::models::Message;
    use tokio::net::TcpStream;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn begin_accepts_connection_and_dispatches_client_message() {
        let (tx, mut rx) = mpsc::channel(4);
        let connector = ClientConnector::new(0, tx).await;
        let listen_addr = connector.listen_stream.local_addr().unwrap();

        let task = tokio::spawn(connector.begin());
        let stream = TcpStream::connect(("127.0.0.1", listen_addr.port()))
            .await
            .expect("connect");
        let local_addr = stream.local_addr().expect("local_addr");

        let msg = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("recv timeout")
            .expect("message");
        match msg {
            Message::CLIENT(client) => assert_eq!(client.get_addr(), local_addr),
            other => panic!("expected CLIENT message, got: {:?}", other),
        }

        task.abort();
    }

    #[tokio::test]
    async fn begin_returns_when_client_bus_is_unavailable() {
        let (tx, rx) = mpsc::channel(1);
        let connector = ClientConnector::new(0, tx).await;
        let listen_addr = connector.listen_stream.local_addr().unwrap();
        drop(rx);

        let task = tokio::spawn(connector.begin());
        let _stream = TcpStream::connect(("127.0.0.1", listen_addr.port()))
            .await
            .expect("connect");

        timeout(Duration::from_secs(1), task)
            .await
            .expect("connector should return quickly")
            .expect("join should succeed");
    }
}

use super::Client;
use crate::models::Message;
use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;

pub struct ClientConnector {
    listen_stream: TcpListener,
    chip_read_bus: Sender<Message>,
}

impl ClientConnector {
    pub async fn new(bind_port: u16, chip_read_bus: Sender<Message>) -> Self {
        // Bind to the listening port to allow other computers to connect
        let listener = TcpListener::bind(("0.0.0.0", bind_port))
            .await
            .expect("Unable to bind to port");
        println!("Bound to port: {}", listener.local_addr().unwrap().port());

        ClientConnector {
            listen_stream: listener,
            chip_read_bus,
        }
    }

    pub async fn begin(mut self) {
        loop {
            // wait for a connection, then connect when it comes
            match self.listen_stream.accept().await {
                Ok((stream, addr)) => {
                    match Client::new(stream, addr) {
                        Err(_) => eprintln!("\r\x1b[2KError connecting to client"),
                        Ok(client) => {
                            self.chip_read_bus
                                .send(Message::CLIENT(client))
                                .await
                                .unwrap();
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

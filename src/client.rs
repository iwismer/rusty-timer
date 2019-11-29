use futures::{future::FutureExt, pin_mut, select};
use std::io::Write;
use std::net::Shutdown;
use std::thread;

pub struct Client {
    stream: std::net::TcpStream,
    addr: std::net::SocketAddr,
    recv_bus: bus::BusReader<std::string::String>,
    exit_callback: fn(),
}

impl Client {
    pub fn new(
        stream: std::net::TcpStream,
        addr: std::net::SocketAddr,
        recv_bus: bus::BusReader<std::string::String>,
        exit_callback: fn(),
    ) -> Result<Client, &'static str> {
        Ok(Client {
            stream,
            addr,
            recv_bus,
            exit_callback,
        })
    }

    async fn recv(mut self) -> Client {
        loop {
            // Receive messages and pass to client
            match self
                .stream
                .try_clone()
                .unwrap()
                .write(self.recv_bus.recv().unwrap().as_bytes())
            {
                Ok(_) => {}
                Err(_) => {
                    eprintln!("Warning: Client {} disconnected", self.addr);
                    // self.exit();
                    // end the loop, destroying the thread
                    break;
                }
            };
        }
        self
    }

    pub async fn begin(self) {
        let receive_msg = self.recv().fuse();
        pin_mut!(receive_msg);
        select! {
            (client) = receive_msg => {println!("@@@Done!@@@"); client.exit()}
        };
    }

    pub fn exit(self) {
        match self.stream.shutdown(Shutdown::Both) {
            Ok(_) => println!("\r\x1b[2KClient disconnected gracefully."),
            Err(e) => println!("\r\x1b[2KError disconnecting: {}", e),
        };
        (self.exit_callback)();
    }
}

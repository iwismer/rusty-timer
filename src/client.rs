use futures::{future::FutureExt, pin_mut, select, join};
use std::io::Write;
use std::net::Shutdown;
use std::sync::{Arc, Mutex};
use std::thread;

async fn handler(handler_bus: Arc<Mutex<bus::BusReader<bool>>>) {
    println!("---------------------------------------------------------------------------------");
    println!("\r\x1b[2KStarted Handler");
    handler_bus.lock().unwrap().recv().unwrap();
    eprintln!("\r\x1b[2KReceived SIGINT");
}

async fn forward(
    recv_bus: Arc<Mutex<bus::BusReader<std::string::String>>>,
    stream: Arc<Mutex<std::net::TcpStream>>,
    addr: std::net::SocketAddr,
) {
    println!("!!!START RECEIVING!!!");
    let t = thread::spawn(move || {
        loop {
            // Receive messages and pass to client
            match stream
                .lock()
                .unwrap()
                .write(recv_bus.lock().unwrap().recv().unwrap().as_bytes())
            {
                Ok(_) => {}
                Err(_) => {
                    eprintln!("\r\x1b[2KWarning: Client {} disconnected", addr);
                    // self.exit();
                    // end the loop, destroying the thread
                    break;
                }
            };
        }
    });
    t.join().unwrap();
}

pub struct Client {
    stream: Arc<Mutex<std::net::TcpStream>>,
    addr: std::net::SocketAddr,
    recv_bus: Arc<Mutex<bus::BusReader<std::string::String>>>,
    handler_bus: Arc<Mutex<bus::BusReader<bool>>>,
    exit_callback: fn(),
}

impl Client {
    pub fn new(
        stream: std::net::TcpStream,
        addr: std::net::SocketAddr,
        recv_bus: bus::BusReader<std::string::String>,
        handler_bus: bus::BusReader<bool>,
        exit_callback: fn(),
    ) -> Result<Client, &'static str> {
        Ok(Client {
            stream: Arc::new(Mutex::new(stream)),
            addr,
            recv_bus: Arc::new(Mutex::new(recv_bus)),
            handler_bus: Arc::new(Mutex::new(handler_bus)),
            exit_callback,
        })
    }

    pub async fn begin(self) {
        // thread::spawn(move || {
        //     handler(Arc::clone(&self.handler_bus));
        //     self.exit()
        // });
        let forward_msg = forward(
            Arc::clone(&self.recv_bus),
            Arc::clone(&self.stream),
            self.addr,
        )
        .fuse();
        let handler_interrupt = handler(Arc::clone(&self.handler_bus)).fuse();
        pin_mut!(forward_msg, handler_interrupt);
        // join!(forward_msg);
        // self.exit();
        loop {
            select! {
                () = handler_interrupt => {self.exit(); break;},
                () = forward_msg => {self.exit(); break;},
            };
        }
    }

    pub fn exit(&self) {
        println!("!!!CALLED EXIT!!!");
        match self.stream.lock().unwrap().shutdown(Shutdown::Both) {
            Ok(_) => println!("\r\x1b[2KClient disconnected gracefully."),
            Err(e) => eprintln!("\r\x1b[2KError disconnecting: {}", e),
        };
        (self.exit_callback)();
    }
}

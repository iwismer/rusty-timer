use crate::models::Message;
use futures::{future::FutureExt, pin_mut, select};
use std::net::Shutdown;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::sync::broadcast::Receiver;
// async fn handler(handler_bus: &mut Receiver<bool>) {
//     println!("\r\x1b[2KStarted Handler");
//     handler_bus.recv().await.unwrap();
//     eprintln!("\r\x1b[2KReceived SIGINT");
// }

// async fn forward(recv_bus: &mut Receiver<String>, stream: &mut TcpStream, addr: SocketAddr) {
//     println!("!!!START RECEIVING!!!");
//     loop {
//         // Receive messages and pass to client
//         match stream
//             .write(recv_bus.recv().await.unwrap().as_bytes())
//             .await
//         {
//             Ok(_) => {}
//             Err(_) => {
//                 eprintln!("\r\x1b[2KWarning: Client {} disconnected", addr);
//                 // end the loop, destroying the thread
//                 break;
//             }
//         };
//     }
// }

#[derive(Debug)]
pub struct Client {
    stream: TcpStream,
    addr: SocketAddr,
    // recv_bus: Receiver<Message>,
    // handler_bus: Receiver<bool>,
    // exit_callback: fn(),
}

impl Client {
    pub fn new(
        stream: TcpStream,
        addr: SocketAddr,
        // recv_bus: Receiver<Message>,
        // handler_bus: Receiver<bool>,
        // exit_callback: fn(),
    ) -> Result<Client, &'static str> {
        Ok(Client {
            stream: stream,
            addr,
            // recv_bus: recv_bus,
            // handler_bus: handler_bus,
            // exit_callback,
        })
    }

    pub async fn send_read(&mut self, read: String) -> Result<usize, SocketAddr> {
        self.stream
            .write(read.as_bytes())
            .await
            .map_err(|_| self.addr)
        // {
        //     Ok(_) => {}
        //     Err(_) => {
        //         eprintln!("\r\x1b[2KWarning: Client {} disconnected", self.addr);
        //         // end the loop, destroying the thread
        //     }
        // };
    }

    // pub async fn begin(mut self) {
    //     let forward_msg = forward(&mut self.recv_bus, &mut self.stream, self.addr).fuse();
    //     let handler_interrupt = handler(&mut self.handler_bus).fuse();
    //     pin_mut!(forward_msg, handler_interrupt);
    //     loop {
    //         select! {
    //             // TODO: This never runs the handler
    //             // TODO call exit again
    //             () = forward_msg => {break;},
    //             () = handler_interrupt => {break;},
    //         };
    //     }
    // }

    pub fn exit(&self) {
        println!("!!!CALLED EXIT!!!");
        match self.stream.shutdown(Shutdown::Both) {
            Ok(_) => println!("\r\x1b[2KClient disconnected gracefully."),
            Err(e) => eprintln!("\r\x1b[2KError disconnecting: {}", e),
        };
        // (self.exit_callback)();
    }

    pub fn get_addr(&self) -> SocketAddr {
        self.addr
    }
}

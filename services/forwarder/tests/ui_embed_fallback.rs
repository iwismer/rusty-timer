#![cfg(feature = "embed-ui")]

use forwarder::status_http::{StatusConfig, StatusServer, SubsystemStatus};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.expect("connect failed");
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        path
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write failed");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read failed");

    let status: u16 = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("could not parse status code");

    (status, response)
}

async fn start_server() -> StatusServer {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    tokio::time::sleep(Duration::from_millis(50)).await;
    server
}

#[tokio::test]
async fn spa_route_serves_index() {
    let server = start_server().await;
    let (status, _body) = http_get(server.local_addr(), "/config").await;
    assert_eq!(status, 200, "SPA route must serve index.html");
}

#[tokio::test]
async fn api_root_returns_not_found() {
    let server = start_server().await;
    let (status, _body) = http_get(server.local_addr(), "/api").await;
    assert_eq!(status, 404, "bare /api must return 404");
}

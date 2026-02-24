use receiver::local_proxy::LocalProxy;
use rt_protocol::ReadEvent;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

fn make_event(fwd: &str, ip: &str, seq: u64, raw: &str) -> ReadEvent {
    ReadEvent {
        forwarder_id: fwd.to_owned(),
        reader_ip: ip.to_owned(),
        stream_epoch: 1,
        seq,
        reader_timestamp: "T".to_owned(),
        raw_read_line: raw.to_owned(),
        read_type: "RAW".to_owned(),
    }
}

async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    p
}

#[tokio::test]
async fn proxy_binds_immediately_before_events() {
    let (tx, _rx): (broadcast::Sender<ReadEvent>, _) = broadcast::channel(16);
    let port = free_port().await;
    let proxy = LocalProxy::bind(port, tx)
        .await
        .expect("bind should succeed");
    tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("should connect before any events arrive");
    proxy.shutdown();
}

#[tokio::test]
async fn proxy_delivers_event_to_single_consumer() {
    let (tx, _rx): (broadcast::Sender<ReadEvent>, _) = broadcast::channel(16);
    let port = free_port().await;
    let _proxy = LocalProxy::bind(port, tx.clone()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    tx.send(make_event(
        "f",
        "192.168.1.100:10000",
        1,
        "aa01,00:01:23.456",
    ))
    .unwrap();
    let mut buf = vec![0u8; 64];
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf))
        .await
        .expect("read timed out")
        .unwrap();
    let s = std::str::from_utf8(&buf[..n]).unwrap();
    assert!(s.contains("aa01,00:01:23.456"), "got: {s:?}");
}

#[tokio::test]
async fn proxy_preserves_exact_bytes() {
    let (tx, _rx): (broadcast::Sender<ReadEvent>, _) = broadcast::channel(16);
    let port = free_port().await;
    let _proxy = LocalProxy::bind(port, tx.clone()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let raw = "aa01,00:01:23.456";
    tx.send(make_event("f", "192.168.1.100:10000", 1, raw))
        .unwrap();
    let mut buf = vec![0u8; 128];
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf))
        .await
        .expect("read timed out")
        .unwrap();
    let received = std::str::from_utf8(&buf[..n]).unwrap();
    assert_eq!(received, format!("{raw}\r\n"), "bytes must be exact");
}

#[tokio::test]
async fn proxy_multiple_consumers_all_receive() {
    let (tx, _rx): (broadcast::Sender<ReadEvent>, _) = broadcast::channel(16);
    let port = free_port().await;
    let _proxy = LocalProxy::bind(port, tx.clone()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let mut c1 = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let mut c2 = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let mut c3 = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    tx.send(make_event("f", "192.168.1.100:10000", 1, "broadcast-line"))
        .unwrap();
    let mut buf = vec![0u8; 64];
    for (i, c) in [&mut c1, &mut c2, &mut c3].iter_mut().enumerate() {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), c.read(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("consumer {i} read timed out"))
            .unwrap();
        let s = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(
            s.contains("broadcast-line"),
            "consumer {i} did not receive: {s:?}"
        );
    }
}

#[tokio::test]
async fn proxy_multiple_events_in_sequence() {
    let (tx, _rx): (broadcast::Sender<ReadEvent>, _) = broadcast::channel(16);
    let port = free_port().await;
    let _proxy = LocalProxy::bind(port, tx.clone()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    for i in 1u64..=3 {
        tx.send(make_event(
            "f",
            "192.168.1.100:10000",
            i,
            &format!("line{i}"),
        ))
        .unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let mut buf = vec![0u8; 256];
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf))
        .await
        .expect("read timed out")
        .unwrap();
    let s = std::str::from_utf8(&buf[..n]).unwrap();
    for i in 1..=3 {
        assert!(s.contains(&format!("line{i}")), "missing line{i} in: {s:?}");
    }
}

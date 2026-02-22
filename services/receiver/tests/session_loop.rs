use futures_util::{SinkExt, StreamExt};
use receiver::cache::StreamCounts;
use receiver::db::Db;
use receiver::session::{SessionError, connect, run_session_loop};
use receiver::ui_events::ReceiverUiEvent;
use rt_protocol::{ErrorMessage, ReadEvent, ReceiverEventBatch, WsMessage};
use rt_test_utils::MockWsServer;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::tungstenite::protocol::Message;

async fn run_raw_ws_server_once<F, Fut>(handler: F) -> (std::net::SocketAddr, JoinHandle<()>)
where
    F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        handler(ws).await;
    });
    (addr, task)
}

async fn join_server_task(task: JoinHandle<()>) {
    timeout(Duration::from_secs(1), task)
        .await
        .expect("server task timed out")
        .expect("server task panicked");
}

#[tokio::test]
async fn connect_returns_session_on_heartbeat_first_message() {
    let server = MockWsServer::start().await.unwrap();
    let db = Db::open_in_memory().unwrap();

    let session = connect(&format!("ws://{}", server.local_addr()), "rcv-001", &db)
        .await
        .unwrap();

    assert!(!session.session_id.is_empty());
    assert_eq!(session.device_id, "rcv-001");
}

#[tokio::test]
async fn connect_errors_when_first_message_is_non_text() {
    let (addr, task) = run_raw_ws_server_once(|mut ws| async move {
        let _ = ws.next().await;
        ws.send(Message::Binary(vec![0xde, 0xad].into()))
            .await
            .unwrap();
    })
    .await;

    let db = Db::open_in_memory().unwrap();
    let result = connect(&format!("ws://{addr}"), "rcv-002", &db).await;

    assert!(matches!(result, Err(SessionError::UnexpectedFirstMessage)));
    join_server_task(task).await;
}

#[tokio::test]
async fn connect_errors_when_server_closes_before_heartbeat() {
    let (addr, task) = run_raw_ws_server_once(|mut ws| async move {
        let _ = ws.next().await;
        ws.send(Message::Close(None)).await.unwrap();
    })
    .await;

    let db = Db::open_in_memory().unwrap();
    let result = connect(&format!("ws://{addr}"), "rcv-003", &db).await;

    assert!(matches!(result, Err(SessionError::ConnectionClosed)));
    join_server_task(task).await;
}

#[tokio::test]
async fn run_session_loop_persists_high_water_and_sends_receiver_ack() {
    let events = vec![
        ReadEvent {
            forwarder_id: "fwd-1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-01T00:00:00.000Z".to_owned(),
            raw_read_line: "raw-1".to_owned(),
            read_type: "RAW".to_owned(),
        },
        ReadEvent {
            forwarder_id: "fwd-1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 3,
            reader_timestamp: "2026-02-01T00:00:01.000Z".to_owned(),
            raw_read_line: "raw-2".to_owned(),
            read_type: "RAW".to_owned(),
        },
        ReadEvent {
            forwarder_id: "fwd-1".to_owned(),
            reader_ip: "10.0.0.2:10000".to_owned(),
            stream_epoch: 2,
            seq: 5,
            reader_timestamp: "2026-02-01T00:00:02.000Z".to_owned(),
            raw_read_line: "raw-3".to_owned(),
            read_type: "RAW".to_owned(),
        },
    ];

    let (ack_tx, ack_rx) = oneshot::channel();
    let (addr, task) = run_raw_ws_server_once(move |mut ws| {
        let events = events.clone();
        async move {
            let msg = WsMessage::ReceiverEventBatch(ReceiverEventBatch {
                session_id: "session-1".to_owned(),
                events,
            });
            ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                .await
                .unwrap();

            let incoming = ws.next().await.unwrap().unwrap();
            let text = match incoming {
                Message::Text(t) => t,
                other => panic!("expected text ack, got: {other:?}"),
            };
            let parsed = serde_json::from_str::<WsMessage>(&text).unwrap();
            let ack = match parsed {
                WsMessage::ReceiverAck(ack) => ack,
                other => panic!("expected receiver ack, got: {other:?}"),
            };
            ack_tx.send(ack).unwrap();

            ws.send(Message::Close(None)).await.unwrap();
        }
    })
    .await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .unwrap();
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);
    let (ui_tx, _ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(16);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    run_session_loop(
        ws,
        "session-1".to_owned(),
        db.clone(),
        event_tx,
        StreamCounts::new(),
        ui_tx,
        shutdown_rx,
    )
    .await
    .unwrap();

    let ack = ack_rx.await.unwrap();
    assert_eq!(ack.session_id, "session-1");

    let mut high_water = HashMap::new();
    for entry in ack.entries {
        high_water.insert(
            (entry.forwarder_id, entry.reader_ip, entry.stream_epoch),
            entry.last_seq,
        );
    }
    assert_eq!(high_water.len(), 2);
    assert_eq!(
        high_water
            .get(&("fwd-1".to_owned(), "10.0.0.1:10000".to_owned(), 1))
            .copied(),
        Some(3)
    );
    assert_eq!(
        high_water
            .get(&("fwd-1".to_owned(), "10.0.0.2:10000".to_owned(), 2))
            .copied(),
        Some(5)
    );

    let mut broadcast_count = 0;
    while event_rx.try_recv().is_ok() {
        broadcast_count += 1;
    }
    assert_eq!(broadcast_count, 3);

    let cursors = db.lock().await.load_resume_cursors().unwrap();
    assert_eq!(cursors.len(), 2);
    join_server_task(task).await;
}

#[tokio::test]
async fn run_session_loop_returns_connection_closed_on_non_retryable_error() {
    let (addr, task) = run_raw_ws_server_once(|mut ws| async move {
        let msg = WsMessage::Error(ErrorMessage {
            code: "PROTOCOL_ERROR".to_owned(),
            message: "fatal".to_owned(),
            retryable: false,
        });
        ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
            .await
            .unwrap();
    })
    .await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .unwrap();
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(4);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let (ui_tx, _ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(4);

    let result = run_session_loop(
        ws,
        "session-2".to_owned(),
        db,
        event_tx,
        StreamCounts::new(),
        ui_tx,
        shutdown_rx,
    )
    .await;
    assert!(matches!(result, Err(SessionError::ConnectionClosed)));
    join_server_task(task).await;
}

#[tokio::test]
async fn run_session_loop_exits_ok_on_retryable_error() {
    let (addr, task) = run_raw_ws_server_once(|mut ws| async move {
        let msg = WsMessage::Error(ErrorMessage {
            code: "INTERNAL_ERROR".to_owned(),
            message: "retry".to_owned(),
            retryable: true,
        });
        ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
            .await
            .unwrap();
    })
    .await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .unwrap();
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(4);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let (ui_tx, _ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(4);

    let result = run_session_loop(
        ws,
        "session-3".to_owned(),
        db,
        event_tx,
        StreamCounts::new(),
        ui_tx,
        shutdown_rx,
    )
    .await;
    assert!(result.is_ok());
    join_server_task(task).await;
}

#[tokio::test]
async fn run_session_loop_replies_to_ping_with_pong() {
    let (pong_tx, pong_rx) = oneshot::channel();
    let (addr, task) = run_raw_ws_server_once(|mut ws| async move {
        ws.send(Message::Ping(vec![1, 2, 3].into())).await.unwrap();
        let incoming = ws.next().await.unwrap().unwrap();
        match incoming {
            Message::Pong(payload) => {
                pong_tx.send(payload.to_vec()).unwrap();
            }
            other => panic!("expected pong, got: {other:?}"),
        }
        ws.send(Message::Close(None)).await.unwrap();
    })
    .await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .unwrap();
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(4);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let (ui_tx, _ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(4);

    let result = run_session_loop(
        ws,
        "session-4".to_owned(),
        db,
        event_tx,
        StreamCounts::new(),
        ui_tx,
        shutdown_rx,
    )
    .await;

    assert!(result.is_ok());
    assert_eq!(pong_rx.await.unwrap(), vec![1, 2, 3]);
    join_server_task(task).await;
}

#[tokio::test]
async fn run_session_loop_stops_on_shutdown_signal() {
    let (addr, task) = run_raw_ws_server_once(|_ws| async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    })
    .await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .unwrap();
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let (ui_tx, _ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(4);

    let handle = tokio::spawn(run_session_loop(
        ws,
        "session-5".to_owned(),
        db,
        event_tx,
        StreamCounts::new(),
        ui_tx,
        shutdown_rx,
    ));

    shutdown_tx.send(true).unwrap();

    let result = handle.await.unwrap();
    assert!(result.is_ok());
    join_server_task(task).await;
}

#[tokio::test]
async fn raw_ws_server_helper_exposes_handler_panic_via_join_handle() {
    let (addr, task) = run_raw_ws_server_once(|_ws| async move {
        panic!("intentional panic from handler");
    })
    .await;

    let (_ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .expect("connect");
    let join = timeout(Duration::from_secs(1), task)
        .await
        .expect("join timeout");
    assert!(
        join.is_err(),
        "handler panic should propagate through JoinHandle"
    );
}

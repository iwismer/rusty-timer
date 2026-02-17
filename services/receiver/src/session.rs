use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, watch};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, info, warn, error};
use rt_protocol::{AckEntry, ReceiverAck, ReceiverHello, WsMessage};
use crate::db::Db;
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("WS: {0}")] Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("JSON: {0}")] Json(#[from] serde_json::Error),
    #[error("DB: {0}")] Db(#[from] crate::db::DbError),
    #[error("UnexpectedFirstMessage")] UnexpectedFirstMessage,
    #[error("ConnectionClosed")] ConnectionClosed,
}
pub struct Session { pub session_id: String, pub device_id: String }
pub async fn connect(url: &str, rid: &str, db: &Db) -> Result<Session, SessionError> {
    let resume = db.load_resume_cursors().map_err(SessionError::Db)?;
    info!(rid, url, n=resume.len(), "connecting");
    let (mut ws, _) = connect_async(url).await?;
    let h = WsMessage::ReceiverHello(ReceiverHello{receiver_id:rid.to_owned(),resume});
    ws.send(Message::Text(serde_json::to_string(&h)?.into())).await?;
    let m = ws.next().await.ok_or(SessionError::ConnectionClosed)??;
    let text = match m { Message::Text(t) => t, _ => return Err(SessionError::UnexpectedFirstMessage) };
    match serde_json::from_str::<WsMessage>(&text)? {
        WsMessage::Heartbeat(hb) => { info!(sid=%hb.session_id,"established"); Ok(Session{session_id:hb.session_id,device_id:hb.device_id}) }
        _ => Err(SessionError::UnexpectedFirstMessage),
    }
}
pub async fn run_session_loop<S>(
    mut ws: S, session_id: String, db: Arc<Mutex<Db>>,
    event_tx: tokio::sync::broadcast::Sender<rt_protocol::ReadEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), SessionError>
where S: futures_util::Stream<Item=Result<Message,tokio_tungstenite::tungstenite::Error>>+futures_util::Sink<Message,Error=tokio_tungstenite::tungstenite::Error>+Unpin,
{
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => { if *shutdown.borrow() { break; } }
            msg = ws.next() => {
                match msg {
                    None => break,
                    Some(Err(e)) => return Err(SessionError::Ws(e)),
                    Some(Ok(Message::Text(t))) => {
                        match serde_json::from_str::<WsMessage>(&t) {
                            Ok(WsMessage::ReceiverEventBatch(b)) => {
                                debug!(n=b.events.len(),"batch");
                                for e in &b.events { let _ = event_tx.send(e.clone()); }
                                let mut hw: std::collections::HashMap<(String,String,u64),u64> = std::collections::HashMap::new();
                                for e in &b.events { let k=(e.forwarder_id.clone(),e.reader_ip.clone(),e.stream_epoch); let v=hw.entry(k).or_insert(0); if e.seq>*v{*v=e.seq;} }
                                let mut acks=Vec::new();
                                { let d=db.lock().await; for((f,i,ep),ls) in &hw { if let Err(e)=d.save_cursor(f,i,*ep,*ls){error!(error=%e);} acks.push(AckEntry{forwarder_id:f.clone(),reader_ip:i.clone(),stream_epoch:*ep,last_seq:*ls}); } }
                                let ack=WsMessage::ReceiverAck(ReceiverAck{session_id:session_id.clone(),entries:acks});
                                ws.send(Message::Text(serde_json::to_string(&ack)?.into())).await?;
                            }
                            Ok(WsMessage::Heartbeat(_)) => {}
                            Ok(WsMessage::Error(err)) => { error!(code=%err.code); if !err.retryable { return Err(SessionError::ConnectionClosed); } break; }
                            Ok(o) => debug!(?o,"ignoring"),
                            Err(e) => warn!(error=%e,"deserialize"),
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Ping(d))) => { let _ = ws.send(Message::Pong(d)).await; }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
    Ok(())
}

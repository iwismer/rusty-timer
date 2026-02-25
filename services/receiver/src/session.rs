use crate::db::Db;
use futures_util::{SinkExt, StreamExt};
use rt_protocol::{AckEntry, ReadEvent, ReceiverAck, WsMessage};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, watch};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, error, info, warn};
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("WS: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("DB: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("UnexpectedFirstMessage")]
    UnexpectedFirstMessage,
    #[error("ConnectionClosed")]
    ConnectionClosed,
}
pub struct Session {
    pub session_id: String,
    pub device_id: String,
}

fn apply_batch_counts(
    stream_counts: &crate::cache::StreamCounts,
    events: &[ReadEvent],
) -> Vec<crate::ui_events::StreamCountUpdate> {
    let mut per_epoch_seqs: HashMap<(String, String, u64), HashSet<u64>> = HashMap::new();
    for event in events {
        let seqs = per_epoch_seqs
            .entry((
                event.forwarder_id.clone(),
                event.reader_ip.clone(),
                event.stream_epoch,
            ))
            .or_default();
        let _ = seqs.insert(event.seq);
    }

    for ((forwarder_id, reader_ip, stream_epoch), seqs) in &per_epoch_seqs {
        stream_counts.record_batch(
            &crate::cache::StreamKey::new(forwarder_id.as_str(), reader_ip.as_str()),
            *stream_epoch,
            seqs.iter().copied(),
        );
    }

    let mut touched_streams: HashSet<(String, String)> = HashSet::new();
    for (forwarder_id, reader_ip, _) in per_epoch_seqs.keys() {
        let _ = touched_streams.insert((forwarder_id.clone(), reader_ip.clone()));
    }

    let mut updates = Vec::with_capacity(touched_streams.len());
    for (forwarder_id, reader_ip) in touched_streams {
        let key = crate::cache::StreamKey::new(forwarder_id.as_str(), reader_ip.as_str());
        if let Some(counts) = stream_counts.get(&key) {
            updates.push(crate::ui_events::StreamCountUpdate {
                forwarder_id,
                reader_ip,
                reads_total: counts.total,
                reads_epoch: counts.epoch,
            });
        }
    }

    updates.sort_by(|a, b| {
        a.forwarder_id
            .cmp(&b.forwarder_id)
            .then(a.reader_ip.cmp(&b.reader_ip))
    });
    updates
}

pub async fn run_session_loop<S>(
    mut ws: S,
    session_id: String,
    db: Arc<Mutex<Db>>,
    event_tx: tokio::sync::broadcast::Sender<rt_protocol::ReadEvent>,
    stream_counts: crate::cache::StreamCounts,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ReceiverUiEvent>,
    mut shutdown: watch::Receiver<bool>,
    paused_streams: Arc<RwLock<HashSet<String>>>,
    all_paused: Arc<RwLock<bool>>,
) -> Result<(), SessionError>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin,
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
                                let all_paused_now = *all_paused.read().await;
                                let paused_set = paused_streams.read().await;
                                let forwarded_events: Vec<ReadEvent> = b
                                    .events
                                    .iter()
                                    .filter(|e| {
                                        if all_paused_now {
                                            return false;
                                        }
                                        !paused_set.contains(&format!("{}/{}", e.forwarder_id, e.reader_ip))
                                    })
                                    .cloned()
                                    .collect();
                                drop(paused_set);
                                if forwarded_events.is_empty() {
                                    continue;
                                }

                                for e in &forwarded_events { let _ = event_tx.send(e.clone()); }
                                let updates = apply_batch_counts(&stream_counts, &forwarded_events);
                                if !updates.is_empty() {
                                    let _ = ui_tx.send(crate::ui_events::ReceiverUiEvent::StreamCountsUpdated {
                                        updates,
                                    });
                                }
                                let mut hw: HashMap<(String,String,u64),u64> = HashMap::new();
                                for e in &forwarded_events { let k=(e.forwarder_id.clone(),e.reader_ip.clone(),e.stream_epoch); let v=hw.entry(k).or_insert(0); if e.seq>*v{*v=e.seq;} }
                                let mut acks=Vec::new();
                                { let d=db.lock().await; for((f,i,ep),ls) in &hw { if let Err(e)=d.save_cursor(f,i,*ep,*ls){error!(error=%e);} acks.push(AckEntry{forwarder_id:f.clone(),reader_ip:i.clone(),stream_epoch:*ep,last_seq:*ls}); } }
                                let ack=WsMessage::ReceiverAck(ReceiverAck{session_id:session_id.clone(),entries:acks});
                                ws.send(Message::Text(serde_json::to_string(&ack)?.into())).await?;
                            }
                            Ok(WsMessage::ReceiverModeApplied(applied)) => {
                                info!(mode=%applied.mode_summary, streams=applied.resolved_stream_count, "server applied mode");
                                for warning in applied.warnings {
                                    warn!(warning = %warning, "server mode warning");
                                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn read_event(forwarder_id: &str, reader_ip: &str, stream_epoch: u64, seq: u64) -> ReadEvent {
        ReadEvent {
            forwarder_id: forwarder_id.to_owned(),
            reader_ip: reader_ip.to_owned(),
            stream_epoch,
            seq,
            reader_timestamp: "2026-01-01T00:00:00.000Z".to_owned(),
            raw_read_line: format!("raw-{seq}"),
            read_type: "RAW".to_owned(),
        }
    }

    #[test]
    fn apply_batch_counts_handles_mixed_epochs_without_inflating_current_epoch() {
        let stream_counts = crate::cache::StreamCounts::new();
        let events = vec![
            read_event("f1", "10.0.0.1", 10, 100),
            read_event("f1", "10.0.0.1", 9, 50),
        ];

        let updates = apply_batch_counts(&stream_counts, &events);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].forwarder_id, "f1");
        assert_eq!(updates[0].reader_ip, "10.0.0.1");
        assert_eq!(updates[0].reads_total, 2);
        assert_eq!(updates[0].reads_epoch, 1);

        let counts = stream_counts
            .get(&crate::cache::StreamKey::new("f1", "10.0.0.1"))
            .unwrap();
        assert_eq!(counts.total, 2);
        assert_eq!(counts.current_epoch, 10);
        assert_eq!(counts.epoch, 1);
    }

    #[test]
    fn apply_batch_counts_is_idempotent_for_replayed_batch() {
        let stream_counts = crate::cache::StreamCounts::new();
        let events = vec![
            read_event("f1", "10.0.0.1", 3, 10),
            read_event("f1", "10.0.0.1", 3, 11),
        ];

        let first = apply_batch_counts(&stream_counts, &events);
        let second = apply_batch_counts(&stream_counts, &events);

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].reads_total, 2);
        assert_eq!(first[0].reads_epoch, 2);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].reads_total, 2);
        assert_eq!(second[0].reads_epoch, 2);
    }

    #[test]
    fn apply_batch_counts_counts_unique_seqs_not_seq_values() {
        let stream_counts = crate::cache::StreamCounts::new();
        let events = vec![
            read_event("f1", "10.0.0.1", 5, 100),
            read_event("f1", "10.0.0.1", 5, 101),
            read_event("f1", "10.0.0.1", 5, 101),
        ];

        let updates = apply_batch_counts(&stream_counts, &events);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].reads_total, 2);
        assert_eq!(updates[0].reads_epoch, 2);
    }
}

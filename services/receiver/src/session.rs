use crate::control_api::ConnectionState;
use crate::db::Db;
use futures_util::{SinkExt, StreamExt};
use rt_protocol::{AckEntry, ReadEvent, ReceiverAck, WsMessage};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
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
    #[error("Server error: {0}")]
    ServerError(String),
    #[error("ConnectionClosed")]
    ConnectionClosed,
}
pub struct Session {
    pub session_id: String,
    pub device_id: String,
}

/// Per-forwarder chip→participant lookup.
/// Outer key is forwarder_id, inner key is chip_id (e.g. "058003700001"),
/// value is (bib, display_name).  Only forwarders with an assigned race
/// have entries; reads from other forwarders are not enriched.
pub type ChipLookup = HashMap<String, HashMap<String, (String, String)>>;

/// A request sent from a Tauri command handler to the WS session loop.
/// The session loop sends `message` over the WebSocket and routes the
/// server's response back via the oneshot `reply` channel. Responses are
/// matched to pending requests by `request_id`.
pub struct WsCommand {
    pub message: WsMessage,
    pub request_id: String,
    pub reply: oneshot::Sender<WsMessage>,
}

pub struct SessionLoopDeps {
    pub db: Arc<Mutex<Db>>,
    pub event_tx: tokio::sync::broadcast::Sender<rt_protocol::ReadEvent>,
    pub stream_counts: crate::cache::StreamCounts,
    pub ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ReceiverUiEvent>,
    pub shutdown: watch::Receiver<bool>,
    pub connection_state: watch::Receiver<ConnectionState>,
    pub chip_lookup: Arc<tokio::sync::RwLock<ChipLookup>>,
    pub ws_cmd_rx: mpsc::Receiver<WsCommand>,
}

fn apply_batch_counts(
    stream_counts: &crate::cache::StreamCounts,
    events: &[ReadEvent],
) -> Vec<crate::ui_events::StreamCountUpdate> {
    let mut per_epoch_seqs: HashMap<(String, String, i64), HashSet<i64>> = HashMap::new();
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

/// Extract the request_id from a proxy response message, if applicable.
fn proxy_response_request_id(msg: &WsMessage) -> Option<&str> {
    match msg {
        WsMessage::ReceiverProxyConfigGetResponse(r) => Some(&r.request_id),
        WsMessage::ReceiverProxyConfigSetResponse(r) => Some(&r.request_id),
        WsMessage::ReceiverProxyControlResponse(r) => Some(&r.request_id),
        _ => None,
    }
}

pub async fn run_session_loop<S>(
    mut ws: S,
    session_id: String,
    mut deps: SessionLoopDeps,
) -> Result<(), SessionError>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin,
{
    let mut pending_requests: HashMap<String, oneshot::Sender<WsMessage>> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            _ = deps.shutdown.changed() => { if *deps.shutdown.borrow() { break; } }
            Some(cmd) = deps.ws_cmd_rx.recv() => {
                let text = match serde_json::to_string(&cmd.message) {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = %e, request_id = %cmd.request_id, "failed to serialize WS command");
                        if cmd.reply.send(WsMessage::Error(rt_protocol::ErrorMessage {
                            code: "SERIALIZE_ERROR".into(),
                            message: e.to_string(),
                            retryable: false,
                        })).is_err() {
                            warn!(request_id = %cmd.request_id, "proxy error reply dropped (caller already gone)");
                        }
                        continue;
                    }
                };
                if ws.send(Message::Text(text.into())).await.is_err() {
                    warn!(request_id = %cmd.request_id, pending_count = pending_requests.len(),
                          "WS send failed for proxy command; ending session");
                    break;
                }
                pending_requests.insert(cmd.request_id, cmd.reply);
            }
            msg = ws.next() => {
                match msg {
                    None => break,
                    Some(Err(e)) => return Err(SessionError::Ws(e)),
                    Some(Ok(Message::Text(t))) => {
                        match serde_json::from_str::<WsMessage>(&t) {
                            Ok(ref parsed) if proxy_response_request_id(parsed).is_some() => {
                                let request_id = proxy_response_request_id(parsed).unwrap().to_owned();
                                if let Some(reply_tx) = pending_requests.remove(&request_id) {
                                    if reply_tx.send(parsed.clone()).is_err() {
                                        warn!(request_id = %request_id, "proxy response arrived but caller already gone (likely timeout)");
                                    }
                                } else {
                                    warn!(request_id = %request_id, "received proxy response with no pending request");
                                }
                            }
                            Ok(WsMessage::ReceiverEventBatch(b)) => {
                                debug!(n=b.events.len(),"batch");
                                let reconnect_pending =
                                    deps.connection_state.borrow().clone()
                                        != ConnectionState::Connected;
                                if reconnect_pending {
                                    continue;
                                }
                                let forwarded_events = b.events;

                                for e in &forwarded_events { let _ = deps.event_tx.send(e.clone()); }
                                let updates = apply_batch_counts(&deps.stream_counts, &forwarded_events);
                                if !updates.is_empty() {
                                    let _ = deps.ui_tx.send(crate::ui_events::ReceiverUiEvent::StreamCountsUpdated {
                                        updates,
                                    });
                                }
                                // Emit only the last read per (forwarder_id, reader_ip) to avoid SSE chatter on large batches.
                                let chip_lookup = deps.chip_lookup.read().await;
                                let mut last_reads: HashMap<(String,String), crate::ui_events::LastRead> = HashMap::new();
                                for e in &forwarded_events {
                                    let chip_id = crate::ui_events::chip_id_from_raw_frame(&e.raw_frame);
                                    let (bib, name) = chip_lookup
                                        .get(&e.forwarder_id)
                                        .and_then(|chips| chips.get(&chip_id))
                                        .map(|(b, n)| (Some(b.clone()), Some(n.clone())))
                                        .unwrap_or((None, None));
                                    last_reads.insert(
                                        (e.forwarder_id.clone(), e.reader_ip.clone()),
                                        crate::ui_events::LastRead {
                                            forwarder_id: e.forwarder_id.clone(),
                                            reader_ip: e.reader_ip.clone(),
                                            chip_id,
                                            timestamp: e.reader_timestamp.clone(),
                                            bib,
                                            name,
                                        },
                                    );
                                }
                                for last_read in last_reads.into_values() {
                                    let _ = deps.ui_tx.send(crate::ui_events::ReceiverUiEvent::LastRead(last_read));
                                }
                                let mut hw: HashMap<(String,String,i64),i64> = HashMap::new();
                                for e in &forwarded_events { let k=(e.forwarder_id.clone(),e.reader_ip.clone(),e.stream_epoch); let v=hw.entry(k).or_insert(0); if e.seq>*v{*v=e.seq;} }
                                let mut acks=Vec::new();
                                { let d=deps.db.lock().await; for((f,i,ep),ls) in &hw { if let Err(e)=d.save_cursor(f,i,*ep,*ls){error!(error=%e,forwarder_id=%f,reader_ip=%i,"save_cursor failed, withholding ack");} else { acks.push(AckEntry{forwarder_id:f.clone(),reader_ip:i.clone(),stream_epoch:*ep,last_seq:*ls}); } } }
                                if !acks.is_empty() { let ack=WsMessage::ReceiverAck(ReceiverAck{session_id:session_id.clone(),entries:acks}); ws.send(Message::Text(serde_json::to_string(&ack)?.into())).await?; }
                            }
                            Ok(WsMessage::ReceiverModeApplied(applied)) => {
                                info!(mode=%applied.mode_summary, streams=applied.resolved_stream_count, "server applied mode");
                                let _ = deps.ui_tx.send(crate::ui_events::ReceiverUiEvent::LogEntry {
                                    entry: format!(
                                        "server applied mode: {} (resolved streams: {})",
                                        applied.mode_summary, applied.resolved_stream_count
                                    ),
                                });
                                for warning in applied.warnings {
                                    warn!(warning = %warning, "server mode warning");
                                    let _ = deps.ui_tx.send(crate::ui_events::ReceiverUiEvent::LogEntry {
                                        entry: format!("server mode warning: {warning}"),
                                    });
                                }
                            }
                            Ok(WsMessage::ReaderStatusChanged(status)) => {
                                info!(
                                    stream_id = %status.stream_id,
                                    reader_ip = %status.reader_ip,
                                    connected = status.connected,
                                    "reader connection status changed"
                                );
                                if status.connected {
                                    let _ = deps.ui_tx.send(crate::ui_events::ReceiverUiEvent::LogEntry {
                                        entry: format!(
                                            "reader reconnected: {} (stream {})",
                                            status.reader_ip, status.stream_id
                                        ),
                                    });
                                } else {
                                    warn!(
                                        stream_id = %status.stream_id,
                                        reader_ip = %status.reader_ip,
                                        "reader disconnected"
                                    );
                                    let _ = deps.ui_tx.send(crate::ui_events::ReceiverUiEvent::LogEntry {
                                        entry: format!(
                                            "reader disconnected: {} (stream {})",
                                            status.reader_ip, status.stream_id
                                        ),
                                    });
                                }
                                let _ = deps
                                    .ui_tx
                                    .send(crate::ui_events::ReceiverUiEvent::Resync);
                            }
                            Ok(WsMessage::ReceiverStreamMetrics(metrics)) => {
                                let payload = crate::ui_events::StreamMetricsPayload::from_ws(&metrics);
                                let _ = deps.ui_tx.send(
                                    crate::ui_events::ReceiverUiEvent::StreamMetricsUpdated(payload),
                                );
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
    if !pending_requests.is_empty() {
        warn!(
            count = pending_requests.len(),
            "session ended with pending proxy requests"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_event(forwarder_id: &str, reader_ip: &str, stream_epoch: i64, seq: i64) -> ReadEvent {
        ReadEvent {
            forwarder_id: forwarder_id.to_owned(),
            reader_ip: reader_ip.to_owned(),
            stream_epoch,
            seq,
            reader_timestamp: "2026-01-01T00:00:00.000Z".to_owned(),
            raw_frame: format!("raw-{seq}").into_bytes(),
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

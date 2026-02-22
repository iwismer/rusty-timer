use rt_protocol::ReadEvent;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tracing::debug;
const CAP: usize = 256;

/// Per-stream read counts (in-memory only, lost on restart).
#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub total: u64,
    pub epoch: u64,
    pub current_epoch: u64,
}

/// Thread-safe container for per-stream read counts.
#[derive(Clone)]
pub struct StreamCounts {
    inner: Arc<RwLock<HashMap<StreamKey, Counts>>>,
}

impl StreamCounts {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record `n` reads for a given stream at the specified epoch.
    /// If the epoch has advanced, the epoch counter resets.
    pub fn record(&self, key: &StreamKey, stream_epoch: u64, n: u64) {
        let mut inner = self.inner.write().unwrap();
        let counts = inner.entry(key.clone()).or_default();
        counts.total += n;
        match stream_epoch.cmp(&counts.current_epoch) {
            std::cmp::Ordering::Greater => {
                counts.current_epoch = stream_epoch;
                counts.epoch = n;
            }
            std::cmp::Ordering::Equal => {
                counts.epoch += n;
            }
            std::cmp::Ordering::Less => {
                // Stale epoch reads still contribute to total, but not the active epoch counter.
            }
        }
    }

    pub fn get(&self, key: &StreamKey) -> Option<Counts> {
        self.inner.read().unwrap().get(key).cloned()
    }

    pub fn snapshot(&self) -> HashMap<StreamKey, Counts> {
        self.inner.read().unwrap().clone()
    }
}

impl Default for StreamCounts {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamKey {
    pub forwarder_id: String,
    pub reader_ip: String,
}
impl StreamKey {
    pub fn new(f: impl Into<String>, i: impl Into<String>) -> Self {
        Self {
            forwarder_id: f.into(),
            reader_ip: i.into(),
        }
    }
}
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<RwLock<HashMap<StreamKey, broadcast::Sender<ReadEvent>>>>,
}
impl EventBus {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    pub fn sender_for(&self, k: &StreamKey) -> broadcast::Sender<ReadEvent> {
        {
            let inner = self.inner.read().unwrap();
            if let Some(tx) = inner.get(k) {
                return tx.clone();
            }
        }
        let mut inner = self.inner.write().unwrap();
        inner
            .entry(k.clone())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(CAP);
                tx
            })
            .clone()
    }
    pub fn subscribe(&self, k: &StreamKey) -> broadcast::Receiver<ReadEvent> {
        self.sender_for(k).subscribe()
    }
    pub fn publish(&self, e: ReadEvent) {
        let k = StreamKey::new(&e.forwarder_id, &e.reader_ip);
        let tx = self.sender_for(&k);
        match tx.send(e) {
            Ok(n) => debug!(receivers = n, "published"),
            Err(_) => debug!("no subscribers"),
        }
    }
    pub fn remove(&self, k: &StreamKey) {
        self.inner.write().unwrap().remove(k);
    }
    pub fn stream_keys(&self) -> Vec<StreamKey> {
        self.inner.read().unwrap().keys().cloned().collect()
    }
}
impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rt_protocol::ReadEvent;
    fn ev(f: &str, i: &str, s: u64) -> ReadEvent {
        ReadEvent {
            forwarder_id: f.to_owned(),
            reader_ip: i.to_owned(),
            stream_epoch: 1,
            seq: s,
            reader_timestamp: "T".to_owned(),
            raw_read_line: format!("l{s}"),
            read_type: "RAW".to_owned(),
        }
    }
    #[test]
    fn publish_and_receive_single_event() {
        let b = EventBus::new();
        let k = StreamKey::new("f", "i");
        let mut r = b.subscribe(&k);
        b.publish(ev("f", "i", 1));
        assert_eq!(r.try_recv().unwrap().seq, 1);
    }
    #[test]
    fn multiple_subscribers_all_receive() {
        let b = EventBus::new();
        let k = StreamKey::new("f", "i");
        let mut r1 = b.subscribe(&k);
        let mut r2 = b.subscribe(&k);
        let mut r3 = b.subscribe(&k);
        b.publish(ev("f", "i", 42));
        assert_eq!(r1.try_recv().unwrap().seq, 42);
        assert_eq!(r2.try_recv().unwrap().seq, 42);
        assert_eq!(r3.try_recv().unwrap().seq, 42);
    }
    #[test]
    fn events_for_different_streams_are_isolated() {
        let b = EventBus::new();
        let k1 = StreamKey::new("f", "i1");
        let k2 = StreamKey::new("f", "i2");
        let mut r1 = b.subscribe(&k1);
        let mut r2 = b.subscribe(&k2);
        b.publish(ev("f", "i1", 10));
        b.publish(ev("f", "i2", 20));
        assert_eq!(r1.try_recv().unwrap().seq, 10);
        assert_eq!(r2.try_recv().unwrap().seq, 20);
        assert!(r1.try_recv().is_err());
        assert!(r2.try_recv().is_err());
    }
    #[test]
    fn publish_with_no_subscribers_does_not_panic() {
        let b = EventBus::new();
        b.publish(ev("f", "i", 1));
    }
    #[test]
    fn stream_keys_lists_registered_streams() {
        let b = EventBus::new();
        let _ = b.sender_for(&StreamKey::new("f", "i1"));
        let _ = b.sender_for(&StreamKey::new("f", "i2"));
        assert_eq!(b.stream_keys().len(), 2);
    }
    #[test]
    fn remove_stream_closes_channel() {
        let b = EventBus::new();
        let k = StreamKey::new("f", "i");
        let _ = b.subscribe(&k);
        b.remove(&k);
    }
    #[test]
    fn stream_counts_record_increments_total_and_epoch() {
        let sc = StreamCounts::new();
        let k = StreamKey::new("f", "i");
        sc.record(&k, 1, 5);
        let c = sc.get(&k).unwrap();
        assert_eq!(c.total, 5);
        assert_eq!(c.epoch, 5);
        assert_eq!(c.current_epoch, 1);
    }
    #[test]
    fn stream_counts_epoch_resets_on_advance() {
        let sc = StreamCounts::new();
        let k = StreamKey::new("f", "i");
        sc.record(&k, 1, 10);
        sc.record(&k, 2, 3);
        let c = sc.get(&k).unwrap();
        assert_eq!(c.total, 13);
        assert_eq!(c.epoch, 3);
        assert_eq!(c.current_epoch, 2);
    }
    #[test]
    fn stream_counts_get_returns_none_for_unknown() {
        let sc = StreamCounts::new();
        assert!(sc.get(&StreamKey::new("x", "y")).is_none());
    }
    #[test]
    fn stream_counts_stale_epoch_does_not_change_epoch_counter() {
        let sc = StreamCounts::new();
        let k = StreamKey::new("f", "i");
        sc.record(&k, 10, 4);
        sc.record(&k, 9, 7);
        let c = sc.get(&k).unwrap();
        assert_eq!(c.total, 11);
        assert_eq!(c.epoch, 4);
        assert_eq!(c.current_epoch, 10);
    }
    #[test]
    fn stream_counts_same_epoch_accumulates_epoch_counter() {
        let sc = StreamCounts::new();
        let k = StreamKey::new("f", "i");
        sc.record(&k, 3, 2);
        sc.record(&k, 3, 5);
        let c = sc.get(&k).unwrap();
        assert_eq!(c.total, 7);
        assert_eq!(c.epoch, 7);
        assert_eq!(c.current_epoch, 3);
    }
}

use std::collections::VecDeque;
use std::fmt::Display;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

/// A UI logger that formats timestamped entries, prints to tracing, broadcasts
/// to SSE subscribers, and optionally buffers for REST retrieval.
pub struct UiLogger<T: Clone + Send + 'static> {
    tx: broadcast::Sender<T>,
    map_fn: Arc<dyn Fn(String) -> T + Send + Sync>,
    buffer: Option<Arc<RwLock<VecDeque<String>>>>,
    max_entries: usize,
}

impl<T: Clone + Send> UiLogger<T> {
    /// Create a broadcast-only logger (no buffer).
    pub fn new(
        tx: broadcast::Sender<T>,
        map_fn: impl Fn(String) -> T + Send + Sync + 'static,
    ) -> Self {
        Self {
            tx,
            map_fn: Arc::new(map_fn),
            buffer: None,
            max_entries: 0,
        }
    }

    /// Create a logger with an in-memory ring buffer for REST retrieval.
    pub fn with_buffer(
        tx: broadcast::Sender<T>,
        map_fn: impl Fn(String) -> T + Send + Sync + 'static,
        max_entries: usize,
    ) -> Self {
        Self {
            tx,
            map_fn: Arc::new(map_fn),
            buffer: Some(Arc::new(RwLock::new(VecDeque::with_capacity(max_entries)))),
            max_entries,
        }
    }

    /// Format a timestamped log entry, print to tracing, broadcast, and optionally buffer.
    pub fn log(&self, msg: impl Display) {
        let entry = format!("{} {}", chrono::Utc::now().format("%H:%M:%S"), msg);
        tracing::info!("{}", entry);
        if let Some(ref buf) = self.buffer {
            if let Ok(mut entries) = buf.write() {
                entries.push_back(entry.clone());
                while entries.len() > self.max_entries {
                    entries.pop_front();
                }
            }
        }
        let _ = self.tx.send((self.map_fn)(entry));
    }

    /// Return a snapshot of buffered entries. Returns empty vec if no buffer.
    pub fn entries(&self) -> Vec<String> {
        match &self.buffer {
            Some(buf) => buf
                .read()
                .map(|b| b.iter().cloned().collect())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_sends_timestamped_entry() {
        let (tx, mut rx) = broadcast::channel::<String>(4);
        let logger = UiLogger::new(tx, |entry| entry);
        logger.log("hello world");
        let entry = rx.try_recv().unwrap();
        assert!(entry.ends_with(" hello world"), "unexpected: {entry}");
        assert_eq!(&entry[2..3], ":");
        assert_eq!(&entry[5..6], ":");
    }

    #[test]
    fn log_buffers_entries() {
        let (tx, _) = broadcast::channel::<String>(4);
        let logger = UiLogger::with_buffer(tx, |entry| entry, 3);
        logger.log("a");
        logger.log("b");
        logger.log("c");
        logger.log("d");
        let entries = logger.entries();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].ends_with(" b"));
        assert!(entries[2].ends_with(" d"));
    }

    #[test]
    fn entries_empty_without_buffer() {
        let (tx, _) = broadcast::channel::<String>(4);
        let logger = UiLogger::new(tx, |entry| entry);
        logger.log("test");
        assert!(logger.entries().is_empty());
    }

    #[test]
    fn log_with_custom_map_fn() {
        #[derive(Clone)]
        struct Event {
            entry: String,
        }
        let (tx, mut rx) = broadcast::channel::<Event>(4);
        let logger = UiLogger::new(tx, |entry| Event { entry });
        logger.log("mapped");
        let event = rx.try_recv().unwrap();
        assert!(event.entry.ends_with(" mapped"));
    }
}

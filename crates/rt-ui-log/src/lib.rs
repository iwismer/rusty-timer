use std::collections::VecDeque;
use std::fmt;
use std::fmt::Display;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

/// Log level for UI-visible entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UiLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for UiLogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trace => write!(f, "TRACE"),
            Self::Debug => write!(f, "DEBUG"),
            Self::Info => write!(f, "INFO"),
            Self::Warn => write!(f, "WARN"),
            Self::Error => write!(f, "ERROR"),
        }
    }
}

impl FromStr for UiLogLevel {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "TRACE" => Ok(Self::Trace),
            "DEBUG" => Ok(Self::Debug),
            "INFO" => Ok(Self::Info),
            "WARN" => Ok(Self::Warn),
            "ERROR" => Ok(Self::Error),
            _ => Err(()),
        }
    }
}

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

    /// Format a timestamped, level-tagged log entry, print to tracing at the
    /// appropriate level, broadcast, and optionally buffer.
    pub fn log_at(&self, level: UiLogLevel, msg: impl Display) {
        let entry = format!(
            "{} [{}] {}",
            chrono::Utc::now().format("%H:%M:%S"),
            level,
            msg,
        );
        match level {
            UiLogLevel::Trace => tracing::trace!("{}", entry),
            UiLogLevel::Debug => tracing::debug!("{}", entry),
            UiLogLevel::Info => tracing::info!("{}", entry),
            UiLogLevel::Warn => tracing::warn!("{}", entry),
            UiLogLevel::Error => tracing::error!("{}", entry),
        }
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

    /// Format a timestamped log entry at INFO level. Shorthand for `log_at(Info, msg)`.
    pub fn log(&self, msg: impl Display) {
        self.log_at(UiLogLevel::Info, msg);
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
    fn log_includes_info_tag() {
        let (tx, mut rx) = broadcast::channel::<String>(4);
        let logger = UiLogger::new(tx, |entry| entry);
        logger.log("hello");
        let entry = rx.try_recv().unwrap();
        assert!(entry.contains("[INFO]"), "unexpected: {entry}");
    }

    #[test]
    fn log_at_debug_includes_level_tag() {
        let (tx, mut rx) = broadcast::channel::<String>(4);
        let logger = UiLogger::new(tx, |entry| entry);
        logger.log_at(UiLogLevel::Debug, "test msg");
        let entry = rx.try_recv().unwrap();
        assert!(entry.contains("[DEBUG]"), "unexpected: {entry}");
        assert!(entry.ends_with(" test msg"), "unexpected: {entry}");
    }

    #[test]
    fn log_at_warn_includes_level_tag() {
        let (tx, mut rx) = broadcast::channel::<String>(4);
        let logger = UiLogger::new(tx, |entry| entry);
        logger.log_at(UiLogLevel::Warn, "oops");
        let entry = rx.try_recv().unwrap();
        assert!(entry.contains("[WARN]"), "unexpected: {entry}");
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
    fn log_at_buffers_entries() {
        let (tx, _) = broadcast::channel::<String>(4);
        let logger = UiLogger::with_buffer(tx, |entry| entry, 2);
        logger.log_at(UiLogLevel::Warn, "w");
        logger.log_at(UiLogLevel::Debug, "d");
        logger.log_at(UiLogLevel::Error, "e");
        let entries = logger.entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("[DEBUG]"));
        assert!(entries[1].contains("[ERROR]"));
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

    #[test]
    fn ui_log_level_display_roundtrip() {
        for level in [
            UiLogLevel::Trace,
            UiLogLevel::Debug,
            UiLogLevel::Info,
            UiLogLevel::Warn,
            UiLogLevel::Error,
        ] {
            let s = level.to_string();
            let parsed: UiLogLevel = s.parse().unwrap();
            assert_eq!(parsed, level);
        }
    }
}

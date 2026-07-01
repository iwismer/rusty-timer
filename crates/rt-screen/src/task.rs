use tokio::sync::watch;
use tokio::time::{Instant, interval};
use tracing::{debug, info};

use crate::state::{DisplayState, EinkConfig, LcdConfig, RefreshMode};

/// Backend-neutral configuration for the display task.
#[derive(Debug, Clone)]
pub struct ScreenTaskConfig {
    pub min_refresh_interval: std::time::Duration,
    pub telemetry_interval: std::time::Duration,
    pub refresh_policy: RefreshPolicy,
}

/// How the task decides between full and partial refreshes.
#[derive(Debug, Clone)]
pub enum RefreshPolicy {
    /// E-ink panels: alternate partial/full per the configured mode and interval.
    Eink {
        mode: RefreshMode,
        full_refresh_interval: u64,
    },
    /// LCD panels: every draw is a full draw.
    Lcd,
}

impl From<EinkConfig> for ScreenTaskConfig {
    fn from(cfg: EinkConfig) -> Self {
        Self {
            min_refresh_interval: std::time::Duration::from_millis(cfg.min_refresh_interval_ms),
            telemetry_interval: std::time::Duration::from_secs(cfg.telemetry_interval_secs),
            refresh_policy: RefreshPolicy::Eink {
                mode: cfg.refresh_mode,
                full_refresh_interval: u64::from(cfg.full_refresh_interval),
            },
        }
    }
}

impl From<LcdConfig> for ScreenTaskConfig {
    fn from(cfg: LcdConfig) -> Self {
        Self {
            min_refresh_interval: std::time::Duration::from_millis(cfg.min_refresh_interval_ms),
            telemetry_interval: std::time::Duration::from_secs(cfg.telemetry_interval_secs),
            refresh_policy: RefreshPolicy::Lcd,
        }
    }
}

/// Run the display task.
///
/// - `state_rx`: watch receiver; yields a new value whenever the display state changes.
/// - `config`: backend-neutral display configuration (refresh policy, intervals, etc.).
/// - `draw_fn`: closure called with `(&DisplayState, bool)` where the bool is `true` for a full
///   refresh and `false` for a partial refresh.
#[allow(clippy::too_many_lines)]
pub async fn run_screen_task<F>(
    mut state_rx: watch::Receiver<DisplayState>,
    mut shutdown_rx: watch::Receiver<bool>,
    config: ScreenTaskConfig,
    mut draw_fn: F,
) where
    F: FnMut(&DisplayState, bool),
{
    info!(
        refresh_policy = ?config.refresh_policy,
        min_refresh_interval_ms = config.min_refresh_interval.as_millis(),
        telemetry_interval_secs = config.telemetry_interval.as_secs(),
        "screen task started"
    );

    let min_refresh = config.min_refresh_interval;
    let mut partial_count: u32 = 0;
    let mut refresh_count: u64 = 0;
    let mut telemetry_tick = interval(config.telemetry_interval);
    // Consume the first (immediate) tick so the interval fires after the full period.
    telemetry_tick.tick().await;

    // --- Initial full refresh ---
    let mut last_refresh = {
        let state = state_rx.borrow_and_update().clone();
        info!(
            total_reads = state.total_reads,
            readers = state.readers.len(),
            p2p_connected = state.p2p_connected,
            "screen: performing initial full refresh"
        );
        draw_fn(&state, true);
        refresh_count += 1;
        Instant::now()
        // partial_count stays 0; initial draw does not count toward the partial tally
    };

    loop {
        tokio::select! {
            result = shutdown_rx.changed() => {
                if result.is_err() || *shutdown_rx.borrow() {
                    info!("screen task: shutdown requested, stopping");
                    break;
                }
            }
            result = state_rx.changed() => {
                if result.is_err() {
                    info!("screen task: watch sender dropped, stopping");
                    break;
                }

                // Debounce: if last refresh was too recent, sleep the remainder.
                let elapsed = last_refresh.elapsed();
                if elapsed < min_refresh {
                    let sleep = tokio::time::sleep(min_refresh.checked_sub(elapsed).unwrap());
                    tokio::pin!(sleep);
                    tokio::select! {
                        () = &mut sleep => {}
                        result = shutdown_rx.changed() => {
                            if result.is_err() || *shutdown_rx.borrow() {
                                info!("screen task: shutdown requested during debounce, stopping");
                                break;
                            }
                        }
                    }
                }

                let state = state_rx.borrow_and_update().clone();
                let full = decide_full(&config.refresh_policy, partial_count);
                debug!(
                    full,
                    partial_count,
                    refresh_count,
                    total_reads = state.total_reads,
                    readers = state.readers.len(),
                    "screen: refresh on state change"
                );
                draw_fn(&state, full);
                refresh_count += 1;
                last_refresh = Instant::now();
                if full {
                    partial_count = 0;
                } else {
                    partial_count += 1;
                }
            }
            _ = telemetry_tick.tick() => {
                // Periodic redraw (e.g., to update clock / telemetry even when state unchanged).
                let elapsed = last_refresh.elapsed();
                if elapsed < min_refresh {
                    let sleep = tokio::time::sleep(min_refresh.checked_sub(elapsed).unwrap());
                    tokio::pin!(sleep);
                    tokio::select! {
                        () = &mut sleep => {}
                        result = shutdown_rx.changed() => {
                            if result.is_err() || *shutdown_rx.borrow() {
                                info!("screen task: shutdown requested during debounce, stopping");
                                break;
                            }
                        }
                    }
                }
                let state = state_rx.borrow_and_update().clone();
                let full = decide_full(&config.refresh_policy, partial_count);
                debug!(
                    full,
                    partial_count,
                    refresh_count,
                    total_reads = state.total_reads,
                    "screen: refresh on telemetry tick"
                );
                draw_fn(&state, full);
                refresh_count += 1;
                last_refresh = Instant::now();
                if full {
                    partial_count = 0;
                } else {
                    partial_count += 1;
                }
            }
        }
    }

    info!(refresh_count, "screen task stopped");
}

/// Determine whether the next refresh should be full or partial.
fn decide_full(policy: &RefreshPolicy, partial_count: u32) -> bool {
    match policy {
        RefreshPolicy::Eink {
            mode,
            full_refresh_interval,
        } => match mode {
            RefreshMode::FullOnly => true,
            RefreshMode::PartialOnly => false,
            RefreshMode::Hybrid => u64::from(partial_count) >= *full_refresh_interval,
        },
        // LCD panels have no partial-refresh concept: every draw is full.
        RefreshPolicy::Lcd => true,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::sync::watch;
    use tokio::time::Duration;

    /// Build a minimal test config with no debounce and a long telemetry interval so tests are
    /// fast and deterministic.
    fn test_config() -> ScreenTaskConfig {
        EinkConfig {
            min_refresh_interval_ms: 0,
            telemetry_interval_secs: 3600,
            ..EinkConfig::default()
        }
        .into()
    }

    /// Returns a `DisplayState` with `total_reads` set to `n`.
    fn state_with_reads(n: u64) -> DisplayState {
        DisplayState {
            total_reads: n,
            ..DisplayState::initial()
        }
    }

    // Recorded draw call: (total_reads, is_full_refresh)
    type DrawLog = Arc<Mutex<Vec<(u64, bool)>>>;

    fn make_draw_fn(log: DrawLog) -> impl FnMut(&DisplayState, bool) {
        move |state: &DisplayState, full: bool| {
            log.lock().unwrap().push((state.total_reads, full));
        }
    }

    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn task_performs_initial_full_refresh() {
        let (tx, rx) = watch::channel(DisplayState::initial());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let log: DrawLog = Arc::new(Mutex::new(vec![]));
        let log_clone = log.clone();

        let handle = tokio::spawn(run_screen_task(
            rx,
            shutdown_rx,
            test_config(),
            make_draw_fn(log_clone),
        ));

        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(tx);
        drop(shutdown_tx);
        let _ = handle.await;

        let calls = log.lock().unwrap();
        assert!(!calls.is_empty(), "expected at least one draw call");
        assert_eq!(calls[0], (0, true), "first draw must be a full refresh");
    }

    #[tokio::test]
    async fn task_redraws_on_state_change() {
        let (tx, rx) = watch::channel(DisplayState::initial());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let log: DrawLog = Arc::new(Mutex::new(vec![]));
        let log_clone = log.clone();

        let handle = tokio::spawn(run_screen_task(
            rx,
            shutdown_rx,
            test_config(),
            make_draw_fn(log_clone),
        ));

        // Give the task time to perform the initial draw.
        tokio::time::sleep(Duration::from_millis(20)).await;

        tx.send(state_with_reads(42)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        drop(tx);
        drop(shutdown_tx);
        let _ = handle.await;

        let calls = log.lock().unwrap();
        assert!(
            calls.len() >= 2,
            "expected at least two draw calls, got {}",
            calls.len()
        );
        // Second draw is a partial (hybrid mode, partial_count=0 < full_refresh_interval=10)
        assert_eq!(
            calls[1],
            (42, false),
            "second draw should be partial with reads=42"
        );
    }

    #[tokio::test]
    async fn task_stops_when_sender_dropped() {
        let (tx, rx) = watch::channel(DisplayState::initial());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let log: DrawLog = Arc::new(Mutex::new(vec![]));

        let handle = tokio::spawn(run_screen_task(
            rx,
            shutdown_rx,
            test_config(),
            make_draw_fn(log),
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(tx);
        drop(shutdown_tx);

        // Task should complete well within 2 seconds after the sender is dropped.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("task did not stop within 2 seconds")
            .expect("task panicked");
        // (return type is () so no value to assert)
    }

    #[tokio::test]
    async fn shutdown_signal_stops_task_even_with_live_state_sender() {
        let (state_tx, state_rx) = watch::channel(DisplayState::initial());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let log: DrawLog = Arc::new(Mutex::new(vec![]));

        let handle = tokio::spawn(run_screen_task(
            state_rx,
            shutdown_rx,
            test_config(),
            make_draw_fn(log),
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown_tx.send(true).expect("send shutdown");

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("task did not stop within 2 seconds")
            .expect("task panicked");

        drop(state_tx);
    }

    #[tokio::test]
    async fn hybrid_mode_does_full_refresh_at_interval() {
        let config = EinkConfig {
            full_refresh_interval: 3,
            min_refresh_interval_ms: 0,
            telemetry_interval_secs: 3600,
            ..EinkConfig::default()
        };

        let (tx, rx) = watch::channel(DisplayState::initial());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let log: DrawLog = Arc::new(Mutex::new(vec![]));
        let log_clone = log.clone();

        let handle = tokio::spawn(run_screen_task(
            rx,
            shutdown_rx,
            config.into(),
            make_draw_fn(log_clone),
        ));

        // Wait for initial draw.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Send 4 state changes.
        for i in 1u64..=4 {
            tx.send(state_with_reads(i)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        drop(tx);
        drop(shutdown_tx);
        let _ = handle.await;

        let calls = log.lock().unwrap();
        // calls[0] = initial full refresh (partial_count=0, forced full)
        // calls[1] = partial_count=0 < 3 → partial  (partial_count becomes 1)
        // calls[2] = partial_count=1 < 3 → partial  (partial_count becomes 2)
        // calls[3] = partial_count=2 < 3 → partial  (partial_count becomes 3)
        // calls[4] = partial_count=3 >= 3 → full     (partial_count resets to 0)
        assert!(
            calls.len() >= 5,
            "expected at least 5 draw calls, got {}",
            calls.len()
        );
        assert!(calls[0].1, "calls[0] must be full (initial)");
        assert!(!calls[1].1, "calls[1] must be partial");
        assert!(!calls[2].1, "calls[2] must be partial");
        assert!(!calls[3].1, "calls[3] must be partial");
        assert!(calls[4].1, "calls[4] must be full (interval reached)");
    }
}

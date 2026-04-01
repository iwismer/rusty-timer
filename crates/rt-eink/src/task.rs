use tokio::sync::watch;
use tokio::time::{Duration, Instant, interval};
use tracing::{debug, info};

use crate::state::{DisplayState, EinkConfig, RefreshMode};

/// Run the e-ink display task.
///
/// - `state_rx`: watch receiver; yields a new value whenever the display state changes.
/// - `config`: e-ink configuration (refresh mode, intervals, etc.).
/// - `draw_fn`: closure called with `(&DisplayState, bool)` where the bool is `true` for a full
///   refresh and `false` for a partial refresh.
pub async fn run_eink_task<F>(
    mut state_rx: watch::Receiver<DisplayState>,
    config: EinkConfig,
    mut draw_fn: F,
) where
    F: FnMut(&DisplayState, bool),
{
    info!("eink task started");

    let min_refresh = Duration::from_millis(config.min_refresh_interval_ms);
    let mut partial_count: u32 = 0;
    let mut telemetry_tick = interval(Duration::from_secs(config.telemetry_interval_secs));
    // Consume the first (immediate) tick so the interval fires after the full period.
    telemetry_tick.tick().await;

    // --- Initial full refresh ---
    let mut last_refresh = {
        let state = state_rx.borrow_and_update().clone();
        debug!("eink initial full refresh");
        draw_fn(&state, true);
        Instant::now()
        // partial_count stays 0; initial draw does not count toward the partial tally
    };

    loop {
        tokio::select! {
            result = state_rx.changed() => {
                if result.is_err() {
                    info!("eink task: watch sender dropped, stopping");
                    break;
                }

                // Debounce: if last refresh was too recent, sleep the remainder.
                let elapsed = last_refresh.elapsed();
                if elapsed < min_refresh {
                    tokio::time::sleep(min_refresh.checked_sub(elapsed).unwrap()).await;
                }

                let state = state_rx.borrow_and_update().clone();
                let full = decide_full(&config, partial_count);
                debug!(full, "eink refresh on state change");
                draw_fn(&state, full);
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
                    tokio::time::sleep(min_refresh.checked_sub(elapsed).unwrap()).await;
                }
                let state = state_rx.borrow_and_update().clone();
                let full = decide_full(&config, partial_count);
                debug!(full, "eink refresh on telemetry tick");
                draw_fn(&state, full);
                last_refresh = Instant::now();
                if full {
                    partial_count = 0;
                } else {
                    partial_count += 1;
                }
            }
        }
    }

    info!("eink task stopped");
}

/// Determine whether the next refresh should be full or partial.
fn decide_full(config: &EinkConfig, partial_count: u32) -> bool {
    match config.refresh_mode {
        RefreshMode::FullOnly => true,
        RefreshMode::PartialOnly => false,
        RefreshMode::Hybrid => partial_count >= config.full_refresh_interval,
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

    /// Build a minimal test config with no debounce and a long telemetry interval so tests are
    /// fast and deterministic.
    fn test_config() -> EinkConfig {
        EinkConfig {
            min_refresh_interval_ms: 0,
            telemetry_interval_secs: 3600,
            ..EinkConfig::default()
        }
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
        let log: DrawLog = Arc::new(Mutex::new(vec![]));
        let log_clone = log.clone();

        let handle = tokio::spawn(run_eink_task(rx, test_config(), make_draw_fn(log_clone)));

        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(tx);
        let _ = handle.await;

        let calls = log.lock().unwrap();
        assert!(!calls.is_empty(), "expected at least one draw call");
        assert_eq!(calls[0], (0, true), "first draw must be a full refresh");
    }

    #[tokio::test]
    async fn task_redraws_on_state_change() {
        let (tx, rx) = watch::channel(DisplayState::initial());
        let log: DrawLog = Arc::new(Mutex::new(vec![]));
        let log_clone = log.clone();

        let handle = tokio::spawn(run_eink_task(rx, test_config(), make_draw_fn(log_clone)));

        // Give the task time to perform the initial draw.
        tokio::time::sleep(Duration::from_millis(20)).await;

        tx.send(state_with_reads(42)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        drop(tx);
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
        let log: DrawLog = Arc::new(Mutex::new(vec![]));

        let handle = tokio::spawn(run_eink_task(rx, test_config(), make_draw_fn(log)));

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(tx);

        // Task should complete well within 2 seconds after the sender is dropped.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("task did not stop within 2 seconds")
            .expect("task panicked");
        // (return type is () so no value to assert)
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
        let log: DrawLog = Arc::new(Mutex::new(vec![]));
        let log_clone = log.clone();

        let handle = tokio::spawn(run_eink_task(rx, config, make_draw_fn(log_clone)));

        // Wait for initial draw.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Send 4 state changes.
        for i in 1u64..=4 {
            tx.send(state_with_reads(i)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        drop(tx);
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
        assert_eq!(calls[0].1, true, "calls[0] must be full (initial)");
        assert_eq!(calls[1].1, false, "calls[1] must be partial");
        assert_eq!(calls[2].1, false, "calls[2] must be partial");
        assert_eq!(calls[3].1, false, "calls[3] must be partial");
        assert_eq!(calls[4].1, true, "calls[4] must be full (interval reached)");
    }
}

//! Background poller for the Race Director DBF import (spec §5 step B).
//!
//! Wraps the manual import (step D) with the cross-cutting modifiers from spec
//! §4: **change detection** (skip reparse when the `(mtime, size)` of all RD
//! files is unchanged), **snapshot-copy-then-parse** (copy the DBFs to a temp
//! dir and parse the copies, so a torn read while RD writes the originals is
//! contained), and **keep-last-good** (any failure logs and leaves the existing
//! tables in place — the board is never blanked mid-race; tables only swap on a
//! fully successful parse of the whole set).

use crate::control_api::AppState;
use crate::control_api::ShutdownSignal;
use crate::rd_dbf::{self, RD_FILES, RdError, RdImport};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::watch;
use tracing::{info, warn};

/// A `(mtime, size)` fingerprint of the RD files, in [`RD_FILES`] order, used to
/// skip needless reparses.
pub type FileSignature = Vec<(SystemTime, u64)>;

/// Result of a single poll pass.
#[derive(Debug)]
pub enum PollOutcome {
    /// The files were unchanged since the last successful import; nothing done.
    Skipped,
    /// A fresh import succeeded; carries the post-join resolvable-chip count.
    Imported(usize),
    /// The import failed; the previous tables/lookup are kept (keep-last-good).
    Failed(String),
}

/// Compute the `(mtime, size)` signature of all RD files in `dir`, or `None` if
/// any file's metadata cannot be read (missing file, RD closed, share
/// unmounted).
fn signature(dir: &Path) -> Option<FileSignature> {
    let mut sig = Vec::with_capacity(RD_FILES.len());
    for file in RD_FILES {
        let meta = std::fs::metadata(dir.join(file)).ok()?;
        sig.push((meta.modified().ok()?, meta.len()));
    }
    Some(sig)
}

/// Copy each RD file into a fresh temp directory and parse the copies. Copying
/// first mitigates sharing violations / torn reads while RD writes the
/// originals. Any copy or parse error is returned so the caller keeps last good.
fn snapshot_and_load(dir: &Path) -> Result<RdImport, RdError> {
    let tmp = tempfile::tempdir().map_err(|e| RdError::Io {
        file: "<tempdir>".to_owned(),
        source: e,
    })?;
    for file in RD_FILES {
        std::fs::copy(dir.join(file), tmp.path().join(file)).map_err(|e| RdError::Io {
            file: (*file).to_owned(),
            source: e,
        })?;
    }
    rd_dbf::load_from_dir(tmp.path())
}

/// Run one poll pass against `dir`.
///
/// Skips when the file signature matches `last` (change detection). Otherwise
/// snapshot-copies, parses, and — only on full success — replaces the tables and
/// updates `last`. On any failure the tables and `last` are left untouched so
/// the next tick retries without blanking the board.
pub async fn poll_once(
    state: &AppState,
    dir: &Path,
    last: &mut Option<FileSignature>,
) -> PollOutcome {
    let sig = signature(dir);
    if let (Some(new), Some(old)) = (sig.as_ref(), last.as_ref())
        && new == old
    {
        return PollOutcome::Skipped;
    }

    let dir_owned = dir.to_path_buf();
    let loaded = tokio::task::spawn_blocking(move || snapshot_and_load(&dir_owned)).await;
    match loaded {
        Ok(Ok(import)) => match crate::control_api::apply_rd_import(state, import).await {
            Ok(summary) => {
                *last = sig;
                PollOutcome::Imported(summary.resolvable_chips)
            }
            Err(e) => PollOutcome::Failed(e.to_string()),
        },
        Ok(Err(e)) => PollOutcome::Failed(e.to_string()),
        Err(e) => PollOutcome::Failed(format!("import task failed: {e}")),
    }
}

/// The long-lived poller task. Reads config each pass, polls when enabled, and
/// wakes early on a config change (new dir/interval/toggle) or shutdown.
pub async fn run(state: Arc<AppState>, mut shutdown_rx: watch::Receiver<ShutdownSignal>) {
    let mut config_rx = state.rd_import_config_rx();
    let mut last_sig: Option<FileSignature> = None;

    loop {
        if !matches!(*shutdown_rx.borrow(), ShutdownSignal::None) {
            break;
        }

        let config = {
            let db = state.storage.db.lock().await;
            db.load_rd_import_config().ok()
        };
        let interval = config
            .as_ref()
            .map_or(crate::db::DEFAULT_RD_IMPORT_INTERVAL_SECS, |c| {
                c.interval_secs.max(1)
            });

        match config {
            Some(cfg) if cfg.enabled && !cfg.dir.trim().is_empty() => {
                match poll_once(&state, Path::new(&cfg.dir), &mut last_sig).await {
                    PollOutcome::Imported(resolvable) => {
                        info!(resolvable_chips = resolvable, "RD import applied");
                    }
                    PollOutcome::Failed(err) => {
                        warn!(error = %err, "RD import failed; keeping last good data");
                    }
                    PollOutcome::Skipped => {}
                }
            }
            // Disabled (or no directory): reset the signature so a later
            // re-enable forces a fresh import rather than skipping on a stale
            // fingerprint.
            _ => last_sig = None,
        }

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            changed = config_rx.changed() => {
                if changed.is_ok() {
                    // Directory/interval/toggle may have changed; force a fresh
                    // evaluation on the next pass.
                    last_sig = None;
                }
            }
            () = tokio::time::sleep(Duration::from_secs(u64::from(interval))) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_api::AppState;
    use crate::db::{Db, RdImportConfig};
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("rd")
    }

    fn copy_fixtures_to(dir: &Path) {
        for file in RD_FILES {
            std::fs::copy(fixtures_dir().join(file), dir.join(file)).unwrap();
        }
    }

    async fn resolves_chip(state: &AppState, chip: &str) -> bool {
        let lookup = state.chip_lookup.read().await;
        lookup.values().any(|chips| chips.contains_key(chip))
    }

    #[tokio::test]
    async fn change_detection_skips_when_unchanged() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let tmp = tempfile::tempdir().unwrap();
        copy_fixtures_to(tmp.path());
        let mut last = None;

        // First pass imports.
        let first = poll_once(&state, tmp.path(), &mut last).await;
        assert!(matches!(first, PollOutcome::Imported(_)), "got {first:?}");
        assert!(last.is_some());
        assert!(resolves_chip(&state, "058003799177").await);

        // Second pass, no file changes → skipped (no reparse / churn).
        let second = poll_once(&state, tmp.path(), &mut last).await;
        assert!(matches!(second, PollOutcome::Skipped), "got {second:?}");
    }

    #[tokio::test]
    async fn keep_last_good_when_file_truncated_mid_poll() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let tmp = tempfile::tempdir().unwrap();
        copy_fixtures_to(tmp.path());
        let mut last = None;

        // Good import first.
        assert!(matches!(
            poll_once(&state, tmp.path(), &mut last).await,
            PollOutcome::Imported(_)
        ));
        assert!(resolves_chip(&state, "058003799177").await);

        // Truncate one file (simulates a torn/locked mid-write read). Its size
        // changes, so change-detection does NOT skip; the parse then fails.
        std::fs::write(tmp.path().join("RACE.DBF"), b"garbage").unwrap();
        let outcome = poll_once(&state, tmp.path(), &mut last).await;
        assert!(matches!(outcome, PollOutcome::Failed(_)), "got {outcome:?}");

        // Keep-last-good: the previously imported chip still resolves.
        assert!(
            resolves_chip(&state, "058003799177").await,
            "a failed poll must not blank the board"
        );
    }

    #[tokio::test]
    async fn keep_last_good_when_record_region_truncated() {
        // Distinct from the garbage-header case: here the header is still valid
        // but the record region is cut short (a copy captured mid-write). The
        // reader must reject it so the poll keeps the last good import.
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let tmp = tempfile::tempdir().unwrap();
        copy_fixtures_to(tmp.path());
        let mut last = None;
        assert!(matches!(
            poll_once(&state, tmp.path(), &mut last).await,
            PollOutcome::Imported(_)
        ));
        assert!(resolves_chip(&state, "058003799177").await);

        // Truncate RACE.DBF's body while leaving its header intact.
        let full = std::fs::read(tmp.path().join("RACE.DBF")).unwrap();
        std::fs::write(tmp.path().join("RACE.DBF"), &full[..full.len() - 20]).unwrap();
        let outcome = poll_once(&state, tmp.path(), &mut last).await;
        assert!(matches!(outcome, PollOutcome::Failed(_)), "got {outcome:?}");
        assert!(
            resolves_chip(&state, "058003799177").await,
            "a truncated-body poll must not blank the board"
        );
    }

    #[tokio::test]
    async fn poll_applies_divisions_and_multi_chip() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let tmp = tempfile::tempdir().unwrap();
        copy_fixtures_to(tmp.path());
        let mut last = None;
        assert!(matches!(
            poll_once(&state, tmp.path(), &mut last).await,
            PollOutcome::Imported(_)
        ));

        let lookup = state.chip_lookup.read().await;
        // bib 1's two chips both resolve to the same participant + division.
        for chip in ["058003799177", "058003799178"] {
            let entry = lookup
                .values()
                .find_map(|chips| chips.get(chip))
                .unwrap_or_else(|| panic!("chip {chip} resolves"));
            assert_eq!(entry.bib, "1");
            assert_eq!(entry.name.as_deref(), Some("John Smith"));
            assert_eq!(entry.division.as_deref(), Some("5k"));
        }
        // Spare chip (bib 900, no participant) still resolves to the bib.
        let spare = lookup
            .values()
            .find_map(|chips| chips.get("aaaaaaaaaaaa"))
            .expect("spare chip resolves to bib only");
        assert_eq!(spare.bib, "900");
        assert_eq!(spare.name, None);
        assert_eq!(spare.division, None);
    }

    #[test]
    fn rd_import_config_round_trips_in_profile() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("http://s", "t", "check-only", Some("recv-1"))
            .unwrap();
        let cfg = RdImportConfig {
            enabled: true,
            dir: "/tmp/rd".to_owned(),
            interval_secs: 30,
        };
        db.save_rd_import_config(&cfg).unwrap();
        assert_eq!(db.load_rd_import_config().unwrap(), cfg);
    }

    #[test]
    fn rd_import_config_survives_profile_save() {
        // Saving the profile does a delete+insert of the profile row; the RD
        // import config must be preserved across it (regression: it was reset).
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile("http://s", "t", "check-only", Some("recv-1"))
            .unwrap();
        let cfg = RdImportConfig {
            enabled: true,
            dir: "/tmp/rd".to_owned(),
            interval_secs: 42,
        };
        db.save_rd_import_config(&cfg).unwrap();

        // A subsequent profile save (e.g. editing server URL) must not wipe it.
        db.save_profile("http://s2", "t2", "check-only", Some("recv-1"))
            .unwrap();
        assert_eq!(db.load_rd_import_config().unwrap(), cfg);
    }
}

//! Participant, chip, and Race Director import handlers, plus the DBF output
//! config they share.

use crate::control_api::{AppState, ConnectionState};
use crate::error::ReceiverError;
use serde::Serialize;

/// Summary returned by the participant/chip import commands.
#[derive(Debug, Serialize)]
pub struct ImportSummary {
    /// Rows accepted into the table (participants or chip assignments).
    pub imported: usize,
    /// Chips that resolve to a participant after the import (post-join).
    pub resolvable_chips: usize,
}

fn decode_import_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => decode_windows_1252(e.into_bytes()),
    }
}

fn decode_windows_1252(bytes: Vec<u8>) -> String {
    bytes
        .into_iter()
        .map(|b| match b {
            0x80 => '\u{20ac}',
            0x82 => '\u{201a}',
            0x83 => '\u{0192}',
            0x84 => '\u{201e}',
            0x85 => '\u{2026}',
            0x86 => '\u{2020}',
            0x87 => '\u{2021}',
            0x88 => '\u{02c6}',
            0x89 => '\u{2030}',
            0x8a => '\u{0160}',
            0x8b => '\u{2039}',
            0x8c => '\u{0152}',
            0x8e => '\u{017d}',
            0x91 => '\u{2018}',
            0x92 => '\u{2019}',
            0x93 => '\u{201c}',
            0x94 => '\u{201d}',
            0x95 => '\u{2022}',
            0x96 => '\u{2013}',
            0x97 => '\u{2014}',
            0x98 => '\u{02dc}',
            0x99 => '\u{2122}',
            0x9a => '\u{0161}',
            0x9b => '\u{203a}',
            0x9c => '\u{0153}',
            0x9e => '\u{017e}',
            0x9f => '\u{0178}',
            _ => char::from(b),
        })
        .collect()
}

async fn read_import_file(path: String) -> Result<String, ReceiverError> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ReceiverError::BadRequest(format!("failed to read import file: {e}")))?;
    Ok(decode_import_bytes(bytes))
}

/// Import participants from `.ppl` file contents. Strict: a parse error rejects
/// the whole file and leaves the existing table untouched. On success the table
/// is replaced wholesale and the chip lookup is rebuilt.
pub async fn import_participants(
    state: &AppState,
    contents: String,
) -> Result<ImportSummary, ReceiverError> {
    let participants =
        crate::participants::parse_ppl(&contents).map_err(ReceiverError::BadRequest)?;
    let imported = participants.len();
    {
        let mut db = state.db.lock().await;
        db.replace_participants(&participants)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    let resolvable_chips = reload_chip_lookup(state).await?;
    Ok(ImportSummary {
        imported,
        resolvable_chips,
    })
}

/// Import participants from a `.ppl` file path selected by the desktop UI.
pub async fn import_participants_file(
    state: &AppState,
    path: String,
) -> Result<ImportSummary, ReceiverError> {
    import_participants(state, read_import_file(path).await?).await
}

/// Return current participant/chip counts and how they overlap, so the UI can
/// show data state without an import round-trip.
pub async fn get_data_stats(state: &AppState) -> Result<crate::db::DataStats, ReceiverError> {
    let db = state.db.lock().await;
    db.data_stats()
        .map_err(|e| ReceiverError::Internal(e.to_string()))
}

/// Import bib->chip assignments from `.bibchip` file contents. Strict, like
/// [`import_participants`].
pub async fn import_chips(
    state: &AppState,
    contents: String,
) -> Result<ImportSummary, ReceiverError> {
    let chips = crate::participants::parse_bibchip(&contents).map_err(ReceiverError::BadRequest)?;
    let imported = chips.len();
    {
        let mut db = state.db.lock().await;
        db.replace_bib_chips(&chips)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    let resolvable_chips = reload_chip_lookup(state).await?;
    Ok(ImportSummary {
        imported,
        resolvable_chips,
    })
}

/// Import bib->chip assignments from a `.bibchip` file path selected by the
/// desktop UI.
pub async fn import_chips_file(
    state: &AppState,
    path: String,
) -> Result<ImportSummary, ReceiverError> {
    import_chips(state, read_import_file(path).await?).await
}

/// Import participants + chip assignments + division names directly from a Race
/// Director working directory of `.DBF` files. This is the manual "Import from
/// Race Director" action (spec §5 step D); it works regardless of the
/// background-poll toggle and reuses the same replace-all path as the file
/// imports. Like the other imports it is all-or-nothing: a parse failure of any
/// of the three files leaves every table untouched.
pub async fn import_participants_from_rd(
    state: &AppState,
    dir: String,
) -> Result<ImportSummary, ReceiverError> {
    let import = tokio::task::spawn_blocking(move || crate::rd_dbf::load_from_dir(&dir))
        .await
        .map_err(|e| ReceiverError::Internal(e.to_string()))?
        .map_err(|e| ReceiverError::BadRequest(e.to_string()))?;
    apply_rd_import(state, import).await
}

/// Replace the participant/chip/division tables from an already-parsed RD
/// import and rebuild the chip lookup. Shared by the manual action and the
/// background poller so both honor the same replace-all + reload contract. All
/// three replacements happen under a single DB lock so the tables swap together.
pub(crate) async fn apply_rd_import(
    state: &AppState,
    import: crate::rd_dbf::RdImport,
) -> Result<ImportSummary, ReceiverError> {
    let imported = import.participants.len();
    let divisions: Vec<(i32, String)> = import.divisions.into_iter().collect();
    {
        let mut db = state.db.lock().await;
        db.replace_rd_data(&import.participants, &import.chips, &divisions)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    let resolvable_chips = reload_chip_lookup(state).await?;
    state.emit_resync();
    Ok(ImportSummary {
        imported,
        resolvable_chips,
    })
}

/// Rebuild the in-memory chip->participant lookup from the durable
/// participant/chip tables. Called at startup and after each import. Bib-only
/// chip assignments are included in the lookup; the returned count preserves
/// the import-summary meaning of chips with a participant name. The lookup uses
/// a single outer key (`"default"`); the announcer resolver searches across all
/// outer maps.
pub async fn reload_chip_lookup(state: &AppState) -> Result<usize, ReceiverError> {
    let map = {
        let db = state.db.lock().await;
        db.load_chip_to_participant()
            .map_err(|e| ReceiverError::Internal(e.to_string()))?
    };
    let count = map.values().filter(|entry| entry.name.is_some()).count();
    let mut lookup = state.chip_lookup.write().await;
    lookup.clear();
    lookup.insert("default".to_owned(), map);
    Ok(count)
}

pub async fn get_dbf_config(state: &AppState) -> Result<crate::db::DbfConfig, ReceiverError> {
    let db = state.db.lock().await;
    match db.load_dbf_config() {
        Ok(config) => Ok(config),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn put_dbf_config(
    state: &AppState,
    body: crate::db::DbfConfig,
) -> Result<(), ReceiverError> {
    if body.enabled {
        let dbf_path = shared_race_director_dbf_path(state).await?;
        if let Some(parent) = dbf_path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            return Err(ReceiverError::BadRequest(format!(
                "Race Director directory does not exist: {}",
                parent.display()
            )));
        }
    }
    let config = crate::db::DbfConfig {
        enabled: body.enabled,
        // Clamp (rather than reject) and return the stored value via a
        // subsequent get; save_dbf_config clamps identically.
        flush_interval_ms: body.flush_interval_ms.clamp(
            crate::db::DBF_FLUSH_INTERVAL_MIN_MS,
            crate::db::DBF_FLUSH_INTERVAL_MAX_MS,
        ),
    };
    let db = state.db.lock().await;
    match db.save_dbf_config(&config) {
        Ok(()) => {
            drop(db);
            state.notify_dbf_config_changed();
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

async fn shared_race_director_dbf_path(
    state: &AppState,
) -> Result<std::path::PathBuf, ReceiverError> {
    let db = state.db.lock().await;
    let rd_config = db
        .load_rd_import_config()
        .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    Ok(std::path::Path::new(&rd_config.dir).join("IPICO.DBF"))
}

pub async fn clear_dbf(state: &AppState) -> Result<(), ReceiverError> {
    let path = shared_race_director_dbf_path(state).await?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        return Err(ReceiverError::BadRequest(format!(
            "DBF directory does not exist: {}",
            parent.display()
        )));
    }
    // Reset delivery markers *before* touching the file: the next DBF pass
    // then regenerates the full file from the durable store (deliberate
    // clear-then-regenerate behavior). Marker reset followed by a failed file
    // clear is harmless — the regenerate replaces the file anyway — whereas
    // the reverse order could leave an emptied file that no pass repopulates
    // until restart.
    {
        let db = state.db.lock().await;
        db.reset_dbf_delivered_all()
            .map_err(|e| ReceiverError::Internal(format!("Failed to reset DBF markers: {e}")))?;
    }
    tokio::task::spawn_blocking(move || crate::dbf_writer::clear_dbf(&path))
        .await
        .map_err(|e| ReceiverError::Internal(format!("Failed to clear DBF: {e}")))?
        .map_err(|e| ReceiverError::Internal(format!("Failed to clear DBF: {e}")))?;
    Ok(())
}

pub async fn get_rd_import_config(
    state: &AppState,
) -> Result<crate::db::RdImportConfig, ReceiverError> {
    let db = state.db.lock().await;
    db.load_rd_import_config()
        .map_err(|e| ReceiverError::Internal(e.to_string()))
}

pub async fn put_rd_import_config(
    state: &AppState,
    body: crate::db::RdImportConfig,
) -> Result<(), ReceiverError> {
    let config = crate::db::RdImportConfig {
        enabled: body.enabled,
        dir: body.dir.trim().to_owned(),
        interval_secs: body.interval_secs,
    };
    if let Err(msg) = config.validate() {
        return Err(ReceiverError::BadRequest(msg));
    }
    // When enabled, the directory must exist so the poller has something to
    // read (mirrors the DBF-writer parent-dir check).
    if config.enabled {
        let dir = std::path::Path::new(&config.dir);
        if !dir.is_dir() {
            return Err(ReceiverError::BadRequest(format!(
                "Race Director import directory does not exist: {}",
                dir.display()
            )));
        }
    }
    {
        let db = state.db.lock().await;
        db.save_rd_import_config(&config)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    state.notify_rd_import_config_changed();
    Ok(())
}

pub async fn admin_clear_data(state: &AppState) -> Result<(), ReceiverError> {
    let current = state.connection_state.borrow().clone();
    if current != ConnectionState::Disconnected {
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        state.request_disconnect_shutdown();
    }
    let mut db = state.db.lock().await;
    match db.clear_data() {
        Ok(()) => {
            drop(db);
            state.notify_dbf_config_changed();
            state.emit_streams_snapshot().await;
            state.emit_resync();
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

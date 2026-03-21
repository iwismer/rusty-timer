//! DBF writer module for IPICO chip read records.
//!
//! Converts IPICO raw frames into Visual FoxPro DBF records and manages DBF
//! file I/O using the `dbase` crate.

use std::convert::TryFrom;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use dbase::{FieldIOError, TableWriterBuilder, WritableRecord};
use ipico_core::read::ChipRead;
use rt_protocol::ReadEvent;
use tokio::sync::{Mutex, broadcast, watch};

use crate::db::Db;

#[cfg(test)]
const VISUAL_FOXPRO_VERSION: u8 = 0x30;
const DBF_TEMPLATE_BYTES: &[u8] = include_bytes!("../../../docs/race-director/IPICO-sample.DBF");

// ---------------------------------------------------------------------------
// DbfRecord
// ---------------------------------------------------------------------------

/// A single record in the IPICO DBF output file.
///
/// Field widths match the Race Director IPICO Direct DBF schema:
/// EVENT(1), DIVISION(2), CHIP(12), TIME(8), RUNERNO(5), DAYCODE(6),
/// LAPNO(3), TPOINT(2), READER(1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbfRecord {
    /// "S" for start, "F" for finish
    pub event: String,
    /// Two-character division code (space-padded)
    pub division: String,
    /// Tag/chip ID (12 characters)
    pub chip: String,
    /// `HHMMSSHH` format (centiseconds in last two digits)
    pub time: String,
    /// Runner number (5 chars, space-padded)
    pub runerno: String,
    /// `YYMMDD` format
    pub daycode: String,
    /// Lap number (3 chars, space-padded)
    pub lapno: String,
    /// "S " or "F " (with trailing space)
    pub tpoint: String,
    /// Reader index as string (1 char)
    pub reader: String,
}

impl WritableRecord for DbfRecord {
    fn write_using<W: Write>(
        &self,
        field_writer: &mut dbase::FieldWriter<'_, W>,
    ) -> Result<(), FieldIOError> {
        field_writer.write_next_field_value(&self.event.as_str())?;
        field_writer.write_next_field_value(&self.division.as_str())?;
        field_writer.write_next_field_value(&self.chip.as_str())?;
        field_writer.write_next_field_value(&self.time.as_str())?;
        field_writer.write_next_field_value(&self.runerno.as_str())?;
        field_writer.write_next_field_value(&self.daycode.as_str())?;
        field_writer.write_next_field_value(&self.lapno.as_str())?;
        field_writer.write_next_field_value(&self.tpoint.as_str())?;
        field_writer.write_next_field_value(&self.reader.as_str())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// map_to_dbf_fields
// ---------------------------------------------------------------------------

/// Parse a raw IPICO frame and map it to a [`DbfRecord`].
///
/// Returns `None` if the frame cannot be parsed as a valid IPICO chip read.
///
/// # Arguments
///
/// * `raw_frame` – the raw bytes of the IPICO frame (ASCII hex string)
/// * `event_type` – `"start"` or `"finish"`
/// * `reader_index` – the reader index (0-based)
pub fn map_to_dbf_fields(
    raw_frame: &[u8],
    event_type: &str,
    reader_index: u8,
) -> Option<DbfRecord> {
    let frame_str = std::str::from_utf8(raw_frame).ok()?;
    let chip_read = ChipRead::try_from(frame_str).ok()?;

    let event = match event_type {
        "start" => "S",
        "finish" => "F",
        _ => "F",
    };

    let ts = &chip_read.timestamp;
    // centiseconds = millis / 10
    let centisec = ts.millis() / 10;
    // TIME: HHMMSSHH (last two digits are centiseconds)
    let time = format!(
        "{:02}{:02}{:02}{:02}",
        ts.hour(),
        ts.minute(),
        ts.second(),
        centisec,
    );
    // DAYCODE: YYMMDD
    let daycode = format!("{:02}{:02}{:02}", ts.year(), ts.month(), ts.day());

    let tpoint = format!("{} ", event);

    Some(DbfRecord {
        event: event.to_owned(),
        division: "  ".to_owned(),
        chip: chip_read.tag_id.clone(),
        time,
        runerno: "     ".to_owned(),
        daycode,
        lapno: "   ".to_owned(),
        tpoint,
        reader: reader_index.to_string(),
    })
}

fn template_writer(
    path: &Path,
) -> std::io::Result<dbase::TableWriter<std::io::BufWriter<std::fs::File>>> {
    let reader = dbase::Reader::new(Cursor::new(DBF_TEMPLATE_BYTES))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    TableWriterBuilder::from_reader(reader)
        .build_with_file_dest(path)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

fn replace_file(tmp_path: &Path, path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(tmp_path, path)
}

// ---------------------------------------------------------------------------
// create_empty_dbf
// ---------------------------------------------------------------------------

/// Create a new empty DBF file at `path` with the IPICO 9-field schema.
///
/// If a file already exists at `path` it will be overwritten.
pub fn create_empty_dbf(path: &Path) -> std::io::Result<()> {
    let mut writer = template_writer(path)?;
    writer
        .finalize()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// append_record
// ---------------------------------------------------------------------------

/// Append a [`DbfRecord`] to the DBF file at `path`.
///
/// If the file does not exist it is created first. All existing records are
/// read, the writer is rebuilt from the reader's schema, and all records
/// (existing + new) are written to a fresh file.
pub fn append_record(path: &Path, record: &DbfRecord) -> std::io::Result<()> {
    if !path.exists() {
        create_empty_dbf(path)?;
    }

    // Read all existing records and capture the table info for schema reuse.
    let mut reader =
        dbase::Reader::from_path(path).map_err(|e| std::io::Error::other(e.to_string()))?;
    let existing: Vec<dbase::Record> = reader
        .read()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let table_info = reader.into_table_info();

    // Write to a temp file next to the target, then rename atomically.
    let tmp_path = path.with_extension("dbf.tmp");
    {
        let mut writer = TableWriterBuilder::from_table_info(table_info)
            .build_with_file_dest(&tmp_path)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        for rec in &existing {
            writer
                .write_record(rec)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        writer
            .write_record(record)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        writer
            .finalize()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    replace_file(&tmp_path, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// clear_dbf
// ---------------------------------------------------------------------------

/// Rewrite the DBF file at `path` as empty (header only, zero records).
///
/// If the file does not exist it is created.
pub fn clear_dbf(path: &Path) -> std::io::Result<()> {
    create_empty_dbf(path)
}

// ---------------------------------------------------------------------------
// run_dbf_writer
// ---------------------------------------------------------------------------

/// Run the DBF writer loop. Receives ReadEvents from the global broadcast
/// channel and appends them to the DBF file.
pub async fn run_dbf_writer(
    mut event_rx: broadcast::Receiver<ReadEvent>,
    db: Arc<Mutex<Db>>,
    mut shutdown_rx: watch::Receiver<bool>,
    dbf_path: String,
) {
    let path = std::path::PathBuf::from(&dbf_path);
    tracing::debug!(path = %path.display(), "DBF writer started");

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("DBF writer shutting down");
                    break;
                }
            }
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Skip sentinel read types
                        if event.read_type.starts_with("__") {
                            continue;
                        }

                        // Look up subscription for this event to get event_type and reader index
                        let subs = {
                            let db = db.lock().await;
                            db.load_subscriptions().unwrap_or_default()
                        };

                        let sub_index = subs.iter().position(|s| {
                            s.forwarder_id == event.forwarder_id && s.reader_ip == event.reader_ip
                        });

                        let Some(idx) = sub_index else {
                            tracing::debug!(fwd = %event.forwarder_id, ip = %event.reader_ip, "no subscription for event, skipping DBF write");
                            continue;
                        };

                        // Skip if reader index > 9
                        if idx > 9 {
                            tracing::debug!(idx, "reader index > 9, skipping DBF write");
                            continue;
                        }

                        let event_type = &subs[idx].event_type;
                        let reader_index = idx as u8;

                        match map_to_dbf_fields(&event.raw_frame, event_type, reader_index) {
                            Some(record) => {
                                if let Err(e) = append_record(&path, &record) {
                                    tracing::error!(error = %e, "DBF write failed, skipping record");
                                }
                            }
                            None => {
                                tracing::warn!("failed to parse raw frame for DBF record, skipping");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "DBF writer lagged, {n} events dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("DBF writer channel closed");
                        break;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dbase::FieldValue;

    fn sample_raw_frame() -> Vec<u8> {
        b"aa400000000123450a2a01123018455927a7".to_vec()
    }

    #[test]
    fn map_to_dbf_fields_finish_event() {
        let raw = sample_raw_frame();
        let record = map_to_dbf_fields(&raw, "finish", 4).unwrap();
        assert_eq!(record.event, "F");
        assert_eq!(record.chip, "000000012345");
        assert_eq!(record.time, "18455939");
        assert_eq!(record.daycode, "011230");
        assert_eq!(record.tpoint, "F ");
        assert_eq!(record.reader, "4");
        assert_eq!(record.runerno, "     ");
        assert_eq!(record.division, "  ");
        assert_eq!(record.lapno, "   ");
    }

    #[test]
    fn map_to_dbf_fields_start_event() {
        let raw = sample_raw_frame();
        let record = map_to_dbf_fields(&raw, "start", 0).unwrap();
        assert_eq!(record.event, "S");
        assert_eq!(record.tpoint, "S ");
        assert_eq!(record.reader, "0");
    }

    #[test]
    fn map_to_dbf_fields_invalid_frame_returns_none() {
        assert!(map_to_dbf_fields(b"not a valid frame", "finish", 0).is_none());
    }

    #[test]
    fn map_to_dbf_fields_non_ipico_prefix_returns_none() {
        let mut raw = sample_raw_frame();
        raw[0] = b'b';
        assert!(map_to_dbf_fields(&raw, "finish", 0).is_none());
    }

    #[test]
    fn create_and_append_dbf_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        create_empty_dbf(&path).unwrap();
        assert!(path.exists());
        // Read back empty
        let mut reader = dbase::Reader::from_path(&path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 0);
        // Append
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, "finish", 4).unwrap();
        append_record(&path, &rec).unwrap();
        // Read back
        let mut reader = dbase::Reader::from_path(&path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(
            r.get("EVENT").and_then(|v| match v {
                FieldValue::Character(Some(s)) => Some(s.trim().to_owned()),
                _ => None,
            }),
            Some("F".to_owned())
        );
        assert_eq!(
            r.get("CHIP").and_then(|v| match v {
                FieldValue::Character(Some(s)) => Some(s.trim().to_owned()),
                _ => None,
            }),
            Some("000000012345".to_owned())
        );
    }

    #[test]
    fn clear_dbf_removes_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, "finish", 4).unwrap();
        append_record(&path, &rec).unwrap();
        append_record(&path, &rec).unwrap();
        let mut reader = dbase::Reader::from_path(&path).unwrap();
        assert_eq!(reader.read().unwrap().len(), 2);
        clear_dbf(&path).unwrap();
        let mut reader = dbase::Reader::from_path(&path).unwrap();
        assert_eq!(reader.read().unwrap().len(), 0);
    }

    #[test]
    fn created_dbf_uses_visual_foxpro_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        create_empty_dbf(&path).unwrap();

        let header = std::fs::read(&path).unwrap();
        assert_eq!(header.first().copied(), Some(VISUAL_FOXPRO_VERSION));
    }

    #[test]
    fn cleared_dbf_preserves_visual_foxpro_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, "finish", 4).unwrap();
        append_record(&path, &rec).unwrap();

        clear_dbf(&path).unwrap();

        let header = std::fs::read(&path).unwrap();
        assert_eq!(header.first().copied(), Some(VISUAL_FOXPRO_VERSION));
    }

    #[test]
    fn read_sample_dbf_file() {
        let sample_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/race-director/IPICO-sample.DBF");
        if !sample_path.exists() {
            return;
        }
        let mut reader = dbase::Reader::from_path(&sample_path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert!(records.len() > 0, "sample should have records");
        let first = &records[0];
        assert!(first.get("EVENT").is_some(), "missing EVENT field");
        assert!(first.get("CHIP").is_some(), "missing CHIP field");
        assert!(first.get("TIME").is_some(), "missing TIME field");
        assert!(first.get("DAYCODE").is_some(), "missing DAYCODE field");
        assert!(first.get("READER").is_some(), "missing READER field");
    }

    #[test]
    fn written_dbf_has_same_fields_as_sample() {
        let sample_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/race-director/IPICO-sample.DBF");
        if !sample_path.exists() {
            return;
        }
        let mut sample_reader = dbase::Reader::from_path(&sample_path).unwrap();
        let sample_records: Vec<dbase::Record> = sample_reader.read().unwrap();
        let sample_fields: Vec<String> = sample_records[0].as_ref().keys().cloned().collect();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, "finish", 4).unwrap();
        append_record(&path, &rec).unwrap();

        let mut our_reader = dbase::Reader::from_path(&path).unwrap();
        let our_records: Vec<dbase::Record> = our_reader.read().unwrap();
        let our_fields: Vec<String> = our_records[0].as_ref().keys().cloned().collect();

        let mut sample_sorted = sample_fields.clone();
        sample_sorted.sort();
        let mut our_sorted = our_fields.clone();
        our_sorted.sort();
        assert_eq!(sample_sorted, our_sorted, "field names should match");
    }

    #[tokio::test]
    async fn dbf_writer_skips_sentinel_read_types() {
        use std::sync::Arc;
        use tokio::sync::{Mutex, broadcast, watch};

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let dbf_path = dir.path().join("test.dbf");
        let db = crate::db::Db::open(&db_path).unwrap();
        db.save_subscription("f1", "10.0.0.1", None, None).unwrap();

        let db = Arc::new(Mutex::new(db));
        let (tx, _) = broadcast::channel::<rt_protocol::ReadEvent>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let rx = tx.subscribe();

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, path).await;
        });

        tx.send(rt_protocol::ReadEvent {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "T".to_owned(),
            raw_frame: sample_raw_frame(),
            read_type: "__checkpoint".to_owned(),
        })
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = shutdown_tx.send(true);
        let _ = handle.await;

        assert!(
            !dbf_path.exists(),
            "DBF file should not be created for sentinel events"
        );
    }
}

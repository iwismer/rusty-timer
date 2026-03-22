//! Maps parsed IPICO chip reads to Race Director-compatible DBF records,
//! manages low-level DBF file I/O (create, append, clear), and provides an
//! async writer task that bridges the broadcast channel to disk.
//!
//! New files are created from an embedded Visual FoxPro template
//! (`IPICO-sample.DBF`) to preserve the correct version byte and schema.
//! Each append writes directly to the end of the DBF file and updates the
//! header record count, avoiding a full file rewrite.

use std::convert::TryFrom;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use fs2::FileExt;

use dbase::{FieldIOError, TableWriterBuilder, WritableRecord};
use ipico_core::read::ChipRead;
use rt_protocol::ReadEvent;
use tokio::sync::{Mutex, broadcast, watch};

use crate::db::{Db, EventType, Subscription};

/// Reasons why a raw frame cannot be mapped to a [`DbfRecord`].
#[derive(Debug)]
pub enum DbfMappingError {
    /// The subscription index exceeds the single-digit READER field limit (0-9).
    ReaderIndexTooLarge(u8),
    /// The raw frame bytes are not valid UTF-8.
    InvalidUtf8(std::str::Utf8Error),
    /// The frame is not a valid IPICO chip read.
    InvalidChipRead(String),
    /// The parsed chip ID exceeds the 12-character CHIP field width.
    ChipIdTooLong(usize),
}

impl std::fmt::Display for DbfMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReaderIndexTooLarge(idx) => {
                write!(
                    f,
                    "subscription index {idx} exceeds DBF READER field limit (max 9)"
                )
            }
            Self::InvalidUtf8(e) => write!(f, "raw frame is not valid UTF-8: {e}"),
            Self::InvalidChipRead(e) => write!(f, "raw frame is not a valid IPICO chip read: {e}"),
            Self::ChipIdTooLong(len) => {
                write!(f, "chip ID length {len} exceeds CHIP field width (12)")
            }
        }
    }
}

#[cfg(test)]
const VISUAL_FOXPRO_VERSION: u8 = 0x30;
/// Embedded reference DBF file used to derive the Visual FoxPro schema when
/// creating new empty DBF files. The template's field definitions (9 fields,
/// version byte 0x30) are preserved by `TableWriterBuilder::from_reader()`.
const DBF_TEMPLATE_BYTES: &[u8] = include_bytes!("../../../docs/race-director/IPICO-sample.DBF");

/// Field widths for the IPICO DBF schema (inherited from the embedded
/// `docs/race-director/IPICO-sample.DBF` template).
const FIELD_WIDTHS: &[usize] = &[1, 2, 12, 8, 5, 6, 3, 2, 1]; // EVENT, DIVISION, CHIP, TIME, RUNERNO, DAYCODE, LAPNO, TPOINT, READER
const RECORD_DATA_LEN: usize = 40; // sum of FIELD_WIDTHS
const DBF_EOF_MARKER: u8 = 0x1A;
const DBF_RECORD_NOT_DELETED: u8 = 0x20;

/// A single record in the IPICO DBF output file.
///
/// Field widths match the Race Director IPICO Direct DBF schema:
/// EVENT(1), DIVISION(2), CHIP(12), TIME(8), RUNERNO(5), DAYCODE(6),
/// LAPNO(3), TPOINT(2), READER(1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbfRecord {
    /// "S" for start, "F" for finish
    event: String,
    /// Two-character division code (space-padded)
    division: String,
    /// Tag/chip ID (12 characters)
    chip: String,
    /// `HHMMSSHH` format (centiseconds in last two digits)
    time: String,
    /// Runner number (5 chars, space-padded)
    runerno: String,
    /// `YYMMDD` format
    daycode: String,
    /// Lap number (3 chars, space-padded)
    lapno: String,
    /// "S " or "F " (with trailing space)
    tpoint: String,
    /// Reader index as string (1 char)
    reader: String,
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

/// Parse a raw IPICO frame and map it to a [`DbfRecord`].
///
/// Returns an error if:
/// - `reader_index` > 9 (READER field is 1 character wide)
/// - the frame cannot be parsed as valid UTF-8
/// - the frame is not a valid IPICO chip read
/// - the parsed chip ID exceeds the 12-character CHIP field width
///
/// # Arguments
///
/// * `raw_frame` – the IPICO frame as UTF-8 encoded ASCII hex (e.g., `b"aa4000..."`)
/// * `event_type` – start or finish
/// * `reader_index` – the subscription index (0-based position in the subscription
///   list, used as the READER field value)
pub fn map_to_dbf_fields(
    raw_frame: &[u8],
    event_type: EventType,
    reader_index: u8,
) -> Result<DbfRecord, DbfMappingError> {
    if reader_index > 9 {
        return Err(DbfMappingError::ReaderIndexTooLarge(reader_index));
    }

    let frame_str = std::str::from_utf8(raw_frame).map_err(DbfMappingError::InvalidUtf8)?;
    let chip_read = ChipRead::try_from(frame_str)
        .map_err(|e| DbfMappingError::InvalidChipRead(e.to_string()))?;

    let event = match event_type {
        EventType::Start => "S",
        EventType::Finish => "F",
    };

    let ts = &chip_read.timestamp;
    // IPICO encodes centiseconds (0x00..0x63); the parser stores
    // millis = centiseconds * 10, so dividing by 10 here recovers the
    // original centisecond value losslessly.
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

    if chip_read.tag_id.len() > 12 {
        return Err(DbfMappingError::ChipIdTooLong(chip_read.tag_id.len()));
    }

    Ok(DbfRecord {
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

/// Serialize a [`DbfRecord`] into raw bytes for direct file append.
///
/// Each field is right-padded with spaces to its defined width.
/// Returns RECORD_DATA_LEN bytes (no deletion flag prefix).
fn serialize_record(record: &DbfRecord) -> [u8; RECORD_DATA_LEN] {
    let fields: [&str; 9] = [
        &record.event,
        &record.division,
        &record.chip,
        &record.time,
        &record.runerno,
        &record.daycode,
        &record.lapno,
        &record.tpoint,
        &record.reader,
    ];

    let mut buf = [b' '; RECORD_DATA_LEN]; // fill with spaces for padding
    let mut offset = 0;
    for (field, &width) in fields.iter().zip(FIELD_WIDTHS.iter()) {
        let bytes = field.as_bytes();
        debug_assert!(
            bytes.len() <= width,
            "field value '{}' ({} bytes) exceeds DBF column width ({})",
            field,
            bytes.len(),
            width
        );
        let copy_len = bytes.len().min(width);
        buf[offset..offset + copy_len].copy_from_slice(&bytes[..copy_len]);
        offset += width;
    }
    buf
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

/// Write an empty Visual FoxPro DBF header to an already-open file.
///
/// Used to initialize a newly-created file while holding an exclusive lock,
/// avoiding the TOCTOU race of check-then-create. Seeks back to file start
/// after writing so the caller can immediately read the header.
fn write_empty_header(file: &mut std::fs::File) -> std::io::Result<()> {
    let header_size = u16::from_le_bytes([DBF_TEMPLATE_BYTES[8], DBF_TEMPLATE_BYTES[9]]) as usize;
    let mut header = DBF_TEMPLATE_BYTES[..header_size].to_vec();
    // Zero the record count (bytes 4-7)
    header[4..8].copy_from_slice(&0u32.to_le_bytes());
    file.write_all(&header)?;
    file.write_all(&[DBF_EOF_MARKER])?;
    file.flush()?;
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}

/// Append a [`DbfRecord`] to the DBF file at `path` using in-place append.
///
/// If the file does not exist it is created first. An exclusive file lock
/// is held for the duration of the write to prevent concurrent readers
/// (e.g. Race Director) from seeing a partially-written record.
pub fn append_record(path: &Path, record: &DbfRecord) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;

    file.lock_exclusive()?;

    // If the file was just created (empty), write the DBF header under the lock
    if file.metadata()?.len() == 0 {
        write_empty_header(&mut file)?;
    }

    // Read header fields: record_count (bytes 4-7), header_size (bytes 8-9),
    // record_size (bytes 10-11), all little-endian.
    let mut header_buf = [0u8; 12];
    file.read_exact(&mut header_buf)?;
    let record_count =
        u32::from_le_bytes([header_buf[4], header_buf[5], header_buf[6], header_buf[7]]);
    let header_size = u16::from_le_bytes([header_buf[8], header_buf[9]]) as u64;
    let record_size = u16::from_le_bytes([header_buf[10], header_buf[11]]) as u64;

    // Sanity check: record_size should be 1 (deletion flag) + RECORD_DATA_LEN
    if record_size != (1 + RECORD_DATA_LEN as u64) {
        return Err(std::io::Error::other(format!(
            "unexpected DBF record size: expected {}, got {record_size}",
            1 + RECORD_DATA_LEN
        )));
    }

    // Seek to where the new record should go: after all existing records
    let write_pos = header_size + (record_count as u64) * record_size;
    file.seek(SeekFrom::Start(write_pos))?;

    // Write: deletion flag + record data + EOF marker
    let record_bytes = serialize_record(record);
    file.write_all(&[DBF_RECORD_NOT_DELETED])?;
    file.write_all(&record_bytes)?;
    file.write_all(&[DBF_EOF_MARKER])?;

    // Update record count in header (bytes 4-7)
    let new_count = record_count
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("DBF record count overflow"))?;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&new_count.to_le_bytes())?;

    file.flush()?;
    file.unlock()?;
    Ok(())
}

/// Rewrite the DBF file at `path` as empty (header only, zero records).
///
/// If the file does not exist it is created.
pub fn clear_dbf(path: &Path) -> std::io::Result<()> {
    create_empty_dbf(path)
}

/// Maximum consecutive I/O failures before the writer gives up and stops.
const MAX_CONSECUTIVE_WRITE_FAILURES: u32 = 10;

/// Receives ReadEvents from the global broadcast channel, filters out sentinel
/// types and unsubscribed/overflow readers, maps each event to a DBF record
/// using the subscription's event type, and appends the record to the DBF file.
///
/// Subscriptions are cached locally and refreshed every 2 seconds to avoid
/// acquiring the DB lock on every event.
pub async fn run_dbf_writer(
    mut event_rx: broadcast::Receiver<ReadEvent>,
    db: Arc<Mutex<Db>>,
    mut shutdown_rx: watch::Receiver<bool>,
    dbf_path: String,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ReceiverUiEvent>,
) {
    let path = std::path::PathBuf::from(&dbf_path);
    tracing::debug!(path = %path.display(), "DBF writer started");

    // Cache subscriptions — reload periodically rather than per-event
    let mut cached_subs: Vec<Subscription> = {
        let db = db.lock().await;
        db.load_subscriptions().unwrap_or_default()
    };
    let mut sub_refresh = tokio::time::interval(std::time::Duration::from_secs(2));
    sub_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("DBF writer shutting down");
                    break;
                }
            }
            _ = sub_refresh.tick() => {
                let db = db.lock().await;
                if let Ok(subs) = db.load_subscriptions() {
                    cached_subs = subs;
                }
            }
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Skip sentinel read types (e.g., __checkpoint)
                        if event.read_type.starts_with("__") {
                            continue;
                        }

                        let sub_index = cached_subs.iter().position(|s| {
                            s.forwarder_id == event.forwarder_id && s.reader_ip == event.reader_ip
                        });

                        let Some(idx) = sub_index else {
                            tracing::debug!(fwd = %event.forwarder_id, ip = %event.reader_ip, "no subscription for event, skipping DBF write");
                            continue;
                        };

                        // Guard against subscription index exceeding the
                        // single-character READER field limit (0-9).
                        if idx > 9 {
                            tracing::warn!(
                                forwarder_id = %event.forwarder_id,
                                reader_ip = %event.reader_ip,
                                subscription_index = idx,
                                "subscription index exceeds DBF READER field limit (max 9), skipping DBF write for this stream"
                            );
                            continue;
                        }

                        let event_type = cached_subs[idx].event_type;
                        let reader_index = idx as u8;

                        match map_to_dbf_fields(&event.raw_frame, event_type, reader_index) {
                            Ok(record) => {
                                let p = path.clone();
                                match tokio::task::spawn_blocking(move || append_record(&p, &record)).await {
                                    Ok(Ok(())) => {
                                        consecutive_failures = 0;
                                    }
                                    Ok(Err(e)) => {
                                        consecutive_failures += 1;
                                        tracing::error!(
                                            error = %e,
                                            path = %path.display(),
                                            consecutive_failures,
                                            "DBF write failed, skipping record"
                                        );
                                        if consecutive_failures >= MAX_CONSECUTIVE_WRITE_FAILURES {
                                            let msg = format!(
                                                "DBF writer stopped: {consecutive_failures} consecutive write failures (last: {e})"
                                            );
                                            tracing::error!("{msg}");
                                            let _ = ui_tx.send(
                                                crate::ui_events::ReceiverUiEvent::LogEntry { entry: msg },
                                            );
                                            break;
                                        }
                                        if consecutive_failures == 1 {
                                            let _ = ui_tx.send(
                                                crate::ui_events::ReceiverUiEvent::LogEntry {
                                                    entry: format!("DBF write error: {e}"),
                                                },
                                            );
                                        }
                                    }
                                    Err(join_err) => {
                                        tracing::error!(error = %join_err, path = %path.display(), "DBF write task panicked or was cancelled");
                                        let _ = ui_tx.send(
                                            crate::ui_events::ReceiverUiEvent::LogEntry {
                                                entry: format!("DBF writer crashed: {join_err}"),
                                            },
                                        );
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    forwarder_id = %event.forwarder_id,
                                    reader_ip = %event.reader_ip,
                                    error = %e,
                                    "failed to map raw frame to DBF record, skipping"
                                );
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
        let record = map_to_dbf_fields(&raw, EventType::Finish, 4).expect("should map");
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
        let record = map_to_dbf_fields(&raw, EventType::Start, 0).expect("should map");
        assert_eq!(record.event, "S");
        assert_eq!(record.tpoint, "S ");
        assert_eq!(record.reader, "0");
    }

    #[test]
    fn map_to_dbf_fields_invalid_frame_returns_err() {
        assert!(matches!(
            map_to_dbf_fields(b"not a valid frame", EventType::Finish, 0),
            Err(DbfMappingError::InvalidChipRead(_))
        ));
    }

    #[test]
    fn map_to_dbf_fields_non_ipico_prefix_returns_err() {
        let mut raw = sample_raw_frame();
        raw[0] = b'b';
        assert!(matches!(
            map_to_dbf_fields(&raw, EventType::Finish, 0),
            Err(DbfMappingError::InvalidChipRead(_))
        ));
    }

    #[test]
    fn map_to_dbf_fields_reader_index_over_9_returns_err() {
        let raw = sample_raw_frame();
        assert!(matches!(
            map_to_dbf_fields(&raw, EventType::Finish, 10),
            Err(DbfMappingError::ReaderIndexTooLarge(10))
        ));
        assert!(matches!(
            map_to_dbf_fields(&raw, EventType::Finish, 255),
            Err(DbfMappingError::ReaderIndexTooLarge(255))
        ));
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
        let rec = map_to_dbf_fields(&raw, EventType::Finish, 4).unwrap();
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
    fn append_record_auto_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.dbf");
        assert!(!path.exists());
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, EventType::Finish, 0).unwrap();
        append_record(&path, &rec).unwrap();
        assert!(path.exists());
        let mut reader = dbase::Reader::from_path(&path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn append_multiple_records_increments_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, EventType::Finish, 4).unwrap();
        append_record(&path, &rec).unwrap();
        append_record(&path, &rec).unwrap();
        append_record(&path, &rec).unwrap();
        let mut reader = dbase::Reader::from_path(&path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 3);
    }

    #[test]
    fn clear_dbf_removes_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, EventType::Finish, 4).unwrap();
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
        let rec = map_to_dbf_fields(&raw, EventType::Finish, 4).unwrap();
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
            eprintln!(
                "SKIPPED: sample DBF file not found at {}",
                sample_path.display()
            );
            return;
        }
        let mut reader = dbase::Reader::from_path(&sample_path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert!(!records.is_empty(), "sample should have records");
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
            eprintln!(
                "SKIPPED: sample DBF file not found at {}",
                sample_path.display()
            );
            return;
        }
        let mut sample_reader = dbase::Reader::from_path(&sample_path).unwrap();
        let sample_records: Vec<dbase::Record> = sample_reader.read().unwrap();
        let sample_fields: Vec<String> = sample_records[0].as_ref().keys().cloned().collect();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dbf");
        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, EventType::Finish, 4).unwrap();
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

    #[test]
    fn serialize_record_produces_correct_bytes() {
        let raw = sample_raw_frame();
        let record = map_to_dbf_fields(&raw, EventType::Finish, 4).unwrap();
        let bytes = serialize_record(&record);
        assert_eq!(bytes.len(), RECORD_DATA_LEN);
        // EVENT = "F" (1 byte)
        assert_eq!(bytes[0], b'F');
        // DIVISION = "  " (2 bytes)
        assert_eq!(&bytes[1..3], b"  ");
        // CHIP starts at offset 3, 12 bytes
        assert_eq!(&bytes[3..15], b"000000012345");
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
        let (ui_tx, _) = broadcast::channel(16);

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, path, ui_tx).await;
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

    #[tokio::test]
    async fn dbf_writer_writes_valid_event() {
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
        let (ui_tx, _) = broadcast::channel(16);

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, path, ui_tx).await;
        });

        tx.send(rt_protocol::ReadEvent {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "T".to_owned(),
            raw_frame: sample_raw_frame(),
            read_type: "RAW".to_owned(),
        })
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = shutdown_tx.send(true);
        let _ = handle.await;

        assert!(
            dbf_path.exists(),
            "DBF file should be created for valid events"
        );
        let mut reader = dbase::Reader::from_path(&dbf_path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 1, "should have exactly one record");
        let r = &records[0];
        assert_eq!(
            r.get("CHIP").and_then(|v| match v {
                FieldValue::Character(Some(s)) => Some(s.trim().to_owned()),
                _ => None,
            }),
            Some("000000012345".to_owned())
        );
    }

    #[test]
    fn append_record_concurrent_writers_produce_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.dbf");

        // Do NOT pre-create the file — both threads race to create it
        let path1 = path.clone();
        let path2 = path.clone();

        let raw = sample_raw_frame();
        let rec_a = map_to_dbf_fields(&raw, EventType::Start, 1).unwrap();
        let rec_b = map_to_dbf_fields(&raw, EventType::Finish, 2).unwrap();

        std::thread::scope(|s| {
            s.spawn(|| {
                for _ in 0..50 {
                    append_record(&path1, &rec_a).unwrap();
                }
            });
            s.spawn(|| {
                for _ in 0..50 {
                    append_record(&path2, &rec_b).unwrap();
                }
            });
        });

        let mut reader = dbase::Reader::from_path(&path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 100, "should have exactly 100 records");

        // Verify each record is intact (not interleaved)
        let mut start_count = 0;
        let mut finish_count = 0;
        for r in &records {
            match r.get("EVENT") {
                Some(dbase::FieldValue::Character(Some(s))) => match s.trim() {
                    "S" => {
                        start_count += 1;
                        if let Some(dbase::FieldValue::Character(Some(rd))) = r.get("READER") {
                            assert_eq!(rd.trim(), "1");
                        }
                    }
                    "F" => {
                        finish_count += 1;
                        if let Some(dbase::FieldValue::Character(Some(rd))) = r.get("READER") {
                            assert_eq!(rd.trim(), "2");
                        }
                    }
                    other => panic!("unexpected EVENT value: {other}"),
                },
                other => panic!("unexpected EVENT field: {other:?}"),
            }
        }
        assert_eq!(start_count, 50);
        assert_eq!(finish_count, 50);
    }

    #[tokio::test]
    async fn dbf_writer_skips_unsubscribed_event() {
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
        let (ui_tx, _) = broadcast::channel(16);

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, path, ui_tx).await;
        });

        // Send event for a forwarder/reader that is NOT subscribed
        tx.send(rt_protocol::ReadEvent {
            forwarder_id: "f-unknown".to_owned(),
            reader_ip: "10.0.0.99".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "T".to_owned(),
            raw_frame: sample_raw_frame(),
            read_type: "RAW".to_owned(),
        })
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = shutdown_tx.send(true);
        let _ = handle.await;

        assert!(
            !dbf_path.exists(),
            "DBF file should not be created for unsubscribed events"
        );
    }
}

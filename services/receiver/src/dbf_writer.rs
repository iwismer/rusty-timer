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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fs2::FileExt;

use dbase::{FieldIOError, TableWriterBuilder, WritableRecord};
use ipico_core::read::ChipRead;
use rt_domain::ReadEvent;
use tokio::sync::{Mutex, broadcast, watch};

use crate::db::{Db, DbError, EventType, ReceivedEvent};

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

fn template_writer_with_dest<W: Write + Seek>(dest: W) -> std::io::Result<dbase::TableWriter<W>> {
    let reader = dbase::Reader::new(Cursor::new(DBF_TEMPLATE_BYTES))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(TableWriterBuilder::from_reader(reader).build_with_dest(dest))
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
    if header_size > DBF_TEMPLATE_BYTES.len() {
        return Err(std::io::Error::other(format!(
            "DBF template header_size {header_size} exceeds template length {}",
            DBF_TEMPLATE_BYTES.len()
        )));
    }
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
    append_record_if_active(path, record, None).map(|_| ())
}

fn append_record_if_active(
    path: &Path,
    record: &DbfRecord,
    cancel_flag: Option<&AtomicBool>,
) -> std::io::Result<bool> {
    let is_cancelled = || cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst));
    if is_cancelled() {
        return Ok(false);
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;

    file.lock_exclusive()?;
    if is_cancelled() {
        file.unlock()?;
        return Ok(false);
    }

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
    if is_cancelled() {
        file.unlock()?;
        return Ok(false);
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
    file.sync_data()?;
    file.unlock()?;
    Ok(true)
}

/// Append many records in one pass: **record bytes first, header count
/// last**, one `flush`, **no fsync**.
///
/// Ordering matters because Race Director opens `IPICO.DBF` directly and
/// trusts the header record count — it does not honor our advisory sidecar
/// lock. A reader catching us mid-append sees the old count, so the partial
/// trailing bytes are invisible; only the final header update publishes the
/// new rows.
///
/// No `sync_all`: the durable source of truth is `received_events` plus the
/// delivery markers — a crash between append and mark is healed by the
/// startup regenerate. The fsync budget on old disks belongs to the SQLite
/// writer.
pub fn append_records(path: &Path, records: &[DbfRecord]) -> std::io::Result<()> {
    append_records_inner(path, records, None)
}

fn append_records_inner(
    path: &Path,
    records: &[DbfRecord],
    between_writes: Option<&dyn Fn(&Path)>,
) -> std::io::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock_exclusive()?;
    let result = append_records_locked(&mut file, records, between_writes, path);
    file.unlock()?;
    result
}

fn append_records_locked(
    file: &mut std::fs::File,
    records: &[DbfRecord],
    between_writes: Option<&dyn Fn(&Path)>,
    path: &Path,
) -> std::io::Result<()> {
    if file.metadata()?.len() == 0 {
        write_empty_header(file)?;
    }
    let mut header_buf = [0u8; 12];
    file.read_exact(&mut header_buf)?;
    let record_count =
        u32::from_le_bytes([header_buf[4], header_buf[5], header_buf[6], header_buf[7]]);
    let header_size = u16::from_le_bytes([header_buf[8], header_buf[9]]) as u64;
    let record_size = u16::from_le_bytes([header_buf[10], header_buf[11]]) as u64;
    if record_size != (1 + RECORD_DATA_LEN as u64) {
        return Err(std::io::Error::other(format!(
            "unexpected DBF record size: expected {}, got {record_size}",
            1 + RECORD_DATA_LEN
        )));
    }

    // Record bytes first (one buffered write), ending with the EOF marker.
    let write_pos = header_size + u64::from(record_count) * record_size;
    file.seek(SeekFrom::Start(write_pos))?;
    let mut buf = Vec::with_capacity(records.len() * (1 + RECORD_DATA_LEN) + 1);
    for record in records {
        buf.push(DBF_RECORD_NOT_DELETED);
        buf.extend_from_slice(&serialize_record(record));
    }
    buf.push(DBF_EOF_MARKER);
    file.write_all(&buf)?;
    file.flush()?;

    if let Some(hook) = between_writes {
        hook(path);
    }

    // Header record count last: publishes the appended rows to readers.
    let new_count = record_count
        .checked_add(
            u32::try_from(records.len())
                .map_err(|_| std::io::Error::other("DBF record count overflow"))?,
        )
        .ok_or_else(|| std::io::Error::other("DBF record count overflow"))?;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&new_count.to_le_bytes())?;
    file.flush()?;
    Ok(())
}

/// Rewrite the DBF file at `path` as empty (header only, zero records).
///
/// If the file does not exist it is created. An exclusive lock is acquired so
/// clears serialize with concurrent appends.
pub fn clear_dbf(path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;

    file.lock_exclusive()?;
    file.set_len(0)?;
    write_empty_header(&mut file)?;
    file.sync_data()?;
    file.unlock()?;
    Ok(())
}

fn durable_dbf_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.to_path_buf();
    let lock_name = path
        .file_name()
        .map(|name| format!("{}.lock", name.to_string_lossy()))
        .unwrap_or_else(|| ".dbf.lock".to_owned());
    lock_path.set_file_name(lock_name);
    lock_path
}

fn with_durable_dbf_lock<T>(
    path: &Path,
    f: impl FnOnce() -> Result<T, DbError>,
) -> Result<T, DbError> {
    let lock_path = durable_dbf_lock_path(path);
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock_file.lock_exclusive()?;
    let result = f();
    lock_file.unlock()?;
    result
}

fn validate_reader_index_for_rebuild(reader_index: u8) -> Result<(), DbError> {
    if reader_index > 9 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            DbfMappingError::ReaderIndexTooLarge(reader_index).to_string(),
        )
        .into());
    }
    Ok(())
}

fn collect_dbf_records(
    events: &[ReceivedEvent],
    event_type: EventType,
    reader_index: u8,
) -> (Vec<DbfRecord>, Vec<i64>) {
    let mut records = Vec::new();
    let mut delivered_seqs = Vec::new();
    for event in events {
        match map_to_dbf_fields(&event.raw_frame, event_type, reader_index) {
            Ok(record) => {
                records.push(record);
                delivered_seqs.push(event.seq);
            }
            Err(e) => {
                tracing::warn!(
                    stream_id = %event.stream_id,
                    seq = event.seq,
                    read_kind = %event.read_kind,
                    error = %e,
                    "skipping undeliverable durable frame for DBF write"
                );
            }
        }
    }
    (records, delivered_seqs)
}

fn write_replacement_dbf(path: &Path, records: &[DbfRecord]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".dbf-")
        .suffix(".tmp")
        .tempfile_in(parent)?;

    {
        let mut writer = template_writer_with_dest(temp.as_file_mut())?;
        for record in records {
            writer
                .write_record(record)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        writer
            .finalize()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    temp.as_file_mut().sync_all()?;
    let persisted = temp.persist(path).map_err(|e| e.error)?;
    persisted.sync_all()?;
    sync_parent_dir(path)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn mark_dbf_delivered_or_confirm(
    db: &Db,
    stream_id: &str,
    seq: i64,
    delivered_unix_ms: i64,
) -> Result<bool, DbError> {
    if db.mark_dbf_delivered(stream_id, seq, delivered_unix_ms)? {
        return Ok(true);
    }

    match db.load_received_event(stream_id, seq)? {
        Some(event) if event.dbf_delivered_unix_ms.is_some() => Ok(false),
        Some(_) => Err(std::io::Error::other(format!(
            "DBF delivery marker update affected 0 rows for stream_id={stream_id} seq={seq} while marker is still NULL"
        ))
        .into()),
        None => Err(std::io::Error::other(format!(
            "DBF delivery marker update affected 0 rows for missing stream_id={stream_id} seq={seq}"
        ))
        .into()),
    }
}

/// Deliver not-yet-marked durable events for `stream_id` to the DBF file at
/// `path`, marking each event's `dbf_delivered_unix_ms` only after the DBF file
/// has been durably replaced. Returns the number of newly-marked records.
///
/// This is the P2P durable DBF feed. It differs from the legacy broadcast writer
/// ([`run_dbf_writer`]) in three ways that match the durable-store contract:
///
/// * **Idempotent / regenerable.** When any event is pending, the DBF file is
///   rebuilt from durable `received_events` and atomically replaced. If a prior
///   run crashed after writing the DBF but before marking SQLite, the next run
///   writes the same DBF contents instead of appending duplicate rows.
/// * **No sentinel filtering.** Unlike the legacy path, `__`-prefixed types are
///   not skipped here; each frame's own parsed content (and the subscription's
///   `event_type`) determines the DBF output. Frames that are not valid chip
///   reads are logged and skipped without being marked delivered.
/// * **Reader timestamp is authoritative.** The DBF TIME/DAYCODE fields are
///   derived from the reader timestamp embedded in the frame, never from the
///   receiver receipt time (`received_unix_ms`).
pub fn deliver_durable_events_to_dbf(
    db: &Db,
    stream_id: &str,
    path: &Path,
    event_type: EventType,
    reader_index: u8,
    delivered_unix_ms: i64,
) -> Result<usize, DbError> {
    validate_reader_index_for_rebuild(reader_index)?;
    with_durable_dbf_lock(path, || {
        let pending_events = db.load_undelivered_received_events(stream_id)?;
        if pending_events.is_empty() {
            return Ok(0);
        }
        let pending_seqs = pending_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>();
        let all_events = db.load_received_events(stream_id)?;
        let (records, deliverable_seqs) =
            collect_dbf_records(&all_events, event_type, reader_index);
        let pending_deliverable_seqs = deliverable_seqs
            .into_iter()
            .filter(|seq| pending_seqs.contains(seq))
            .collect::<Vec<_>>();

        write_replacement_dbf(path, &records)?;

        let mut marked = 0usize;
        for seq in pending_deliverable_seqs {
            if mark_dbf_delivered_or_confirm(db, stream_id, seq, delivered_unix_ms)? {
                marked += 1;
            }
        }
        Ok(marked)
    })
}

/// Rebuild the DBF file at `path` entirely from durable `received_events`.
///
/// The replacement DBF is written to a temporary file, synced, and atomically
/// moved into place before delivery markers are reset and re-marked. The
/// durable store is the source of truth, so this can recover a lost or corrupt
/// DBF file without first clearing the live DBF. Returns the number of records
/// written.
pub fn regenerate_dbf_from_received_events(
    db: &Db,
    stream_id: &str,
    path: &Path,
    event_type: EventType,
    reader_index: u8,
    delivered_unix_ms: i64,
) -> Result<usize, DbError> {
    validate_reader_index_for_rebuild(reader_index)?;
    with_durable_dbf_lock(path, || {
        let all_events = db.load_received_events(stream_id)?;
        let (records, deliverable_seqs) =
            collect_dbf_records(&all_events, event_type, reader_index);
        let written = records.len();

        write_replacement_dbf(path, &records)?;
        db.reset_dbf_delivered(stream_id)?;
        for seq in deliverable_seqs {
            mark_dbf_delivered_or_confirm(db, stream_id, seq, delivered_unix_ms)?;
        }

        Ok(written)
    })
}

/// One stream's DBF delivery parameters, resolved from its subscription.
#[derive(Clone, Debug)]
pub struct DbfStreamSpec {
    pub stream_id: String,
    pub event_type: EventType,
    pub reader_index: u8,
}

/// Per-worker DBF delivery state. `regenerated == false` forces one
/// cross-stream regenerate (startup / crash reconciliation / seq regression)
/// before incremental appends resume.
#[derive(Debug, Default)]
pub struct DbfPassState {
    pub regenerated: bool,
    /// Highest seq processed per stream; appends fetch strictly above this.
    pub last_delivered: std::collections::HashMap<String, i64>,
}

/// Map events to records, also returning **every** input seq as processed.
/// Frames that are not valid chip reads produce no DBF output but are still
/// marked delivered ("processed, nothing to write") — otherwise they would
/// hold the min-undelivered probe down and force regenerates forever.
fn collect_records_and_processed(
    events: &[ReceivedEvent],
    event_type: EventType,
    reader_index: u8,
) -> (Vec<DbfRecord>, Vec<i64>) {
    let mut records = Vec::with_capacity(events.len());
    let mut processed = Vec::with_capacity(events.len());
    for event in events {
        match map_to_dbf_fields(&event.raw_frame, event_type, reader_index) {
            Ok(record) => records.push(record),
            Err(e) => {
                tracing::warn!(
                    stream_id = %event.stream_id,
                    seq = event.seq,
                    read_kind = %event.read_kind,
                    error = %e,
                    "skipping undeliverable durable frame for DBF write"
                );
            }
        }
        processed.push(event.seq);
    }
    (records, processed)
}

/// Rows fetched per incremental append chunk.
const DBF_APPEND_CHUNK_ROWS: usize = 4096;

/// Run one delivery pass for all subscribed streams against a single DBF
/// file.
///
/// First pass (or after a detected seq regression): one **cross-stream
/// regenerate** — the file is atomically replaced with every stream's
/// deliverable rows and all rows are re-marked. This is the crash
/// reconciliation for the append-then-crash-before-mark window and the reason
/// mark-before-append is never needed. Subsequent passes append only rows
/// above each stream's append point and mark them in one transaction per
/// stream.
pub fn run_dbf_delivery_pass(
    db: &mut Db,
    streams: &[DbfStreamSpec],
    path: &Path,
    state: &mut DbfPassState,
    delivered_unix_ms: i64,
) -> Result<(), DbError> {
    for spec in streams {
        validate_reader_index_for_rebuild(spec.reader_index)?;
    }
    if !state.regenerated {
        state.last_delivered = regenerate_dbf_cross_stream(db, streams, path, delivered_unix_ms)?;
        state.regenerated = true;
        return Ok(());
    }

    // Detect out-of-order arrivals below any stream's append point (late
    // redelivery after a gap, or an epoch/seq reset): the append-only file
    // cannot represent them, so fall back to one cross-stream regenerate.
    for spec in streams {
        let append_point = state
            .last_delivered
            .get(&spec.stream_id)
            .copied()
            .unwrap_or(0);
        if let Some(min_pending) = db.min_undelivered_dbf_seq(&spec.stream_id)?
            && min_pending <= append_point
        {
            tracing::info!(
                stream_id = %spec.stream_id,
                min_pending,
                append_point,
                "undelivered row below DBF append point; regenerating cross-stream"
            );
            state.last_delivered =
                regenerate_dbf_cross_stream(db, streams, path, delivered_unix_ms)?;
            return Ok(());
        }
    }

    for spec in streams {
        loop {
            let append_point = state
                .last_delivered
                .get(&spec.stream_id)
                .copied()
                .unwrap_or(0);
            let events = db.load_undelivered_received_events_after(
                &spec.stream_id,
                append_point,
                DBF_APPEND_CHUNK_ROWS,
            )?;
            if events.is_empty() {
                break;
            }
            let fetched = events.len();
            let (records, processed) =
                collect_records_and_processed(&events, spec.event_type, spec.reader_index);
            let max_seq = processed.iter().copied().max().unwrap_or(append_point);
            // Append under the receiver-side advisory lock, then mark in one
            // transaction. Append-before-mark: a crash in between is healed by
            // the startup regenerate (never the reverse — mark-before-append
            // would lose reads).
            with_durable_dbf_lock(path, || Ok(append_records(path, &records)?))?;
            db.mark_dbf_delivered_batch(&spec.stream_id, &processed, delivered_unix_ms)?;
            state.last_delivered.insert(spec.stream_id.clone(), max_seq);
            if fetched < DBF_APPEND_CHUNK_ROWS {
                break;
            }
        }
    }
    Ok(())
}

/// Replace the DBF file with **every subscribed stream's** deliverable rows
/// (ordered chronologically) and re-mark all rows delivered. Returns each
/// stream's max processed seq (the new append points).
fn regenerate_dbf_cross_stream(
    db: &mut Db,
    streams: &[DbfStreamSpec],
    path: &Path,
    delivered_unix_ms: i64,
) -> Result<std::collections::HashMap<String, i64>, DbError> {
    with_durable_dbf_lock(path, || {
        let mut all: Vec<(i64, usize, i64, DbfRecord)> = Vec::new();
        let mut append_points = std::collections::HashMap::new();
        let mut processed_per_stream: Vec<(String, Vec<i64>)> = Vec::new();
        for (stream_idx, spec) in streams.iter().enumerate() {
            let events = db.load_received_events(&spec.stream_id)?;
            let mut processed = Vec::with_capacity(events.len());
            for event in &events {
                match map_to_dbf_fields(&event.raw_frame, spec.event_type, spec.reader_index) {
                    Ok(record) => {
                        all.push((event.received_unix_ms, stream_idx, event.seq, record));
                    }
                    Err(e) => {
                        tracing::warn!(
                            stream_id = %event.stream_id,
                            seq = event.seq,
                            error = %e,
                            "skipping undeliverable durable frame during DBF regenerate"
                        );
                    }
                }
                processed.push(event.seq);
            }
            let max_seq = processed.iter().copied().max().unwrap_or(0);
            append_points.insert(spec.stream_id.clone(), max_seq);
            processed_per_stream.push((spec.stream_id.clone(), processed));
        }
        all.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
        let records: Vec<DbfRecord> = all.into_iter().map(|(_, _, _, record)| record).collect();
        tracing::info!(
            records = records.len(),
            streams = streams.len(),
            path = %path.display(),
            "regenerating DBF from durable store (cross-stream)"
        );
        write_replacement_dbf(path, &records)?;
        for (stream_id, processed) in processed_per_stream {
            db.reset_dbf_delivered(&stream_id)?;
            db.mark_dbf_delivered_batch(&stream_id, &processed, delivered_unix_ms)?;
        }
        Ok(append_points)
    })
}

/// Maximum consecutive I/O failures before the writer gives up and stops.
const MAX_CONSECUTIVE_WRITE_FAILURES: u32 = 10;

/// Receives ReadEvents from the global broadcast channel, filters out sentinel
/// types and unsubscribed/overflow readers, maps each event to a DBF record
/// using the subscription's event type, and appends the record to the DBF file.
pub async fn run_dbf_writer(
    mut event_rx: broadcast::Receiver<ReadEvent>,
    db: Arc<Mutex<Db>>,
    mut shutdown_rx: watch::Receiver<bool>,
    cancel_flag: Arc<AtomicBool>,
    dbf_path: String,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ReceiverUiEvent>,
) {
    let path = std::path::PathBuf::from(&dbf_path);
    tracing::debug!(path = %path.display(), "DBF writer started");

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
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Skip sentinel read types (e.g., __checkpoint)
                        if event.read_type.starts_with("__") {
                            continue;
                        }

                        let sub_details = {
                            let db = db.lock().await;
                            db.load_subscription_dbf_details(&event.forwarder_id, &event.reader_ip)
                        };

                        let Some((idx, event_type)) = (match sub_details {
                            Ok(details) => details,
                            Err(e) => {
                                tracing::warn!(
                                    forwarder_id = %event.forwarder_id,
                                    reader_ip = %event.reader_ip,
                                    error = %e,
                                    "failed to load subscription details for DBF write, skipping"
                                );
                                continue;
                            }
                        }) else {
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
                        let reader_index = idx as u8;

                        match map_to_dbf_fields(&event.raw_frame, event_type, reader_index) {
                            Ok(record) => {
                                let p = path.clone();
                                let cancel_flag = Arc::clone(&cancel_flag);
                                match tokio::task::spawn_blocking(move || {
                                    append_record_if_active(&p, &record, Some(cancel_flag.as_ref()))
                                }).await {
                                    Ok(Ok(true)) => {
                                        consecutive_failures = 0;
                                    }
                                    Ok(Ok(false)) => {
                                        tracing::debug!(path = %path.display(), "DBF write cancelled before commit");
                                        break;
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
    use crate::db::{Db, ReceivedEventInsert};
    use dbase::FieldValue;

    fn sample_raw_frame() -> Vec<u8> {
        b"aa400000000123450a2a01123018455927a7".to_vec()
    }

    fn insert_durable_event(db: &Db, stream_id: &str, seq: i64, raw: &[u8], received_unix_ms: i64) {
        db.insert_received_event(&ReceivedEventInsert {
            stream_id,
            seq,
            epoch: 1,
            raw_frame: raw,
            read_kind: "chip",
            reader_timestamp: None,
            received_unix_ms,
            dbf_delivered_unix_ms: None,
            chip_id: None,
        })
        .unwrap();
    }

    fn dbf_records(path: &Path) -> Vec<dbase::Record> {
        let mut reader = dbase::Reader::from_path(path).unwrap();
        reader.read().unwrap()
    }

    fn char_field(record: &dbase::Record, field: &str) -> Option<String> {
        record.get(field).and_then(|v| match v {
            FieldValue::Character(Some(s)) => Some(s.trim().to_owned()),
            _ => None,
        })
    }

    fn spec(stream_id: &str, reader_index: u8) -> DbfStreamSpec {
        DbfStreamSpec {
            stream_id: stream_id.to_owned(),
            event_type: EventType::Finish,
            reader_index,
        }
    }

    #[test]
    fn incremental_pass_appends_only_new_rows() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let mut db = Db::open_in_memory().unwrap();
        let stream_id = "s-incremental";
        let raw = sample_raw_frame();
        insert_durable_event(&db, stream_id, 1, &raw, 1_700_000_000_000);
        insert_durable_event(&db, stream_id, 2, &raw, 1_700_000_000_001);

        let specs = vec![spec(stream_id, 0)];
        let mut state = DbfPassState::default();
        run_dbf_delivery_pass(&mut db, &specs, &dbf_path, &mut state, 1_700_000_010_000).unwrap();
        assert_eq!(dbf_records(&dbf_path).len(), 2);

        // Rows persisted between passes are appended, not rebuilt.
        insert_durable_event(&db, stream_id, 3, &raw, 1_700_000_000_002);
        insert_durable_event(&db, stream_id, 4, &raw, 1_700_000_000_003);
        run_dbf_delivery_pass(&mut db, &specs, &dbf_path, &mut state, 1_700_000_020_000).unwrap();

        let records = dbf_records(&dbf_path);
        assert_eq!(records.len(), 4, "second pass appends only the new rows");
        assert!(
            db.load_undelivered_received_events(stream_id)
                .unwrap()
                .is_empty(),
            "all rows marked delivered"
        );

        // An idle pass changes nothing.
        run_dbf_delivery_pass(&mut db, &specs, &dbf_path, &mut state, 1_700_000_030_000).unwrap();
        assert_eq!(dbf_records(&dbf_path).len(), 4);
    }

    #[test]
    fn startup_regenerate_dedupes_after_crash() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let mut db = Db::open_in_memory().unwrap();
        let stream_id = "s-crash";
        let raw = sample_raw_frame();
        insert_durable_event(&db, stream_id, 1, &raw, 1_700_000_000_000);
        insert_durable_event(&db, stream_id, 2, &raw, 1_700_000_000_001);

        // Simulate append-then-crash-before-mark: the rows are already in the
        // DBF file but dbf_delivered_unix_ms is still NULL.
        let (records, _) = collect_records_and_processed(
            &db.load_received_events(stream_id).unwrap(),
            EventType::Finish,
            0,
        );
        append_records(&dbf_path, &records).unwrap();
        assert_eq!(dbf_records(&dbf_path).len(), 2);

        // Restart-shaped pass (fresh state): the cross-stream regenerate
        // yields the exact record count, no duplicates.
        let specs = vec![spec(stream_id, 0)];
        let mut state = DbfPassState::default();
        run_dbf_delivery_pass(&mut db, &specs, &dbf_path, &mut state, 1_700_000_010_000).unwrap();
        assert_eq!(
            dbf_records(&dbf_path).len(),
            2,
            "regenerate must not duplicate rows appended before the crash"
        );
        assert!(
            db.load_undelivered_received_events(stream_id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn regenerate_includes_all_streams() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let mut db = Db::open_in_memory().unwrap();
        let raw = sample_raw_frame();
        insert_durable_event(&db, "s-one", 1, &raw, 1_700_000_000_000);
        insert_durable_event(&db, "s-two", 1, &raw, 1_700_000_000_001);
        insert_durable_event(&db, "s-two", 2, &raw, 1_700_000_000_002);

        let specs = vec![spec("s-one", 0), spec("s-two", 1)];
        let mut state = DbfPassState::default();
        run_dbf_delivery_pass(&mut db, &specs, &dbf_path, &mut state, 1_700_000_010_000).unwrap();

        let records = dbf_records(&dbf_path);
        assert_eq!(records.len(), 3, "regenerate carries both streams' rows");
        let readers: Vec<String> = records
            .iter()
            .filter_map(|record| char_field(record, "READER"))
            .collect();
        assert_eq!(
            readers,
            vec!["0", "1", "1"],
            "rows are ordered chronologically and keep per-stream reader indexes"
        );

        // A gap-jump on one stream (undelivered row below its append point)
        // triggers a cross-stream regenerate that must not erase the other
        // stream's rows. Simulate by clearing a delivered marker.
        assert!(db.mark_dbf_delivered("s-one", 1, 1).is_ok());
        db.reset_dbf_delivered("s-one").unwrap();
        run_dbf_delivery_pass(&mut db, &specs, &dbf_path, &mut state, 1_700_000_020_000).unwrap();
        assert_eq!(
            dbf_records(&dbf_path).len(),
            3,
            "regenerate after one stream's reset keeps all streams' rows exactly once"
        );
    }

    #[test]
    fn mark_dbf_delivered_batch_marks_all_rows_in_one_call() {
        let mut db = Db::open_in_memory().unwrap();
        let stream_id = "s-batch";
        let raw = sample_raw_frame();
        for seq in 1..=700 {
            insert_durable_event(&db, stream_id, seq, &raw, 1_700_000_000_000 + seq);
        }
        let seqs: Vec<i64> = (1..=700).collect();
        // One call, one transaction (chunked IN lists internally).
        let marked = db
            .mark_dbf_delivered_batch(stream_id, &seqs, 1_700_000_010_000)
            .unwrap();
        assert_eq!(marked, 700);
        assert!(
            db.load_undelivered_received_events(stream_id)
                .unwrap()
                .is_empty()
        );
        // Idempotent: re-marking changes nothing.
        let again = db
            .mark_dbf_delivered_batch(stream_id, &seqs, 1_700_000_020_000)
            .unwrap();
        assert_eq!(again, 0);
    }

    #[test]
    fn append_writes_records_before_header_count() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        create_empty_dbf(&dbf_path).unwrap();
        let record = map_to_dbf_fields(&sample_raw_frame(), EventType::Finish, 0).unwrap();

        // The hook runs after the record bytes are flushed but before the
        // header count is updated: a concurrent reader (Race Director) at
        // that instant must still see the old count, hiding the partial tail.
        let observed = std::cell::RefCell::new(None);
        append_records_inner(
            &dbf_path,
            std::slice::from_ref(&record),
            Some(&|path: &Path| {
                let bytes = std::fs::read(path).unwrap();
                let count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
                *observed.borrow_mut() = Some((count, bytes.len()));
            }),
        )
        .unwrap();

        let (mid_count, mid_len) = observed.into_inner().expect("hook ran");
        assert_eq!(mid_count, 0, "header count updates only after record bytes");
        assert!(
            mid_len > (1 + RECORD_DATA_LEN),
            "record bytes were on disk before the header update"
        );
        let bytes = std::fs::read(&dbf_path).unwrap();
        let final_count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(final_count, 1);
    }

    #[test]
    fn dbf_idempotent_on_replay() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let db = Db::open_in_memory().unwrap();
        let stream_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let raw = sample_raw_frame();
        insert_durable_event(&db, stream_id, 1, &raw, 1_700_000_000_000);
        insert_durable_event(&db, stream_id, 2, &raw, 1_700_000_000_001);

        let first = deliver_durable_events_to_dbf(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            0,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(first, 2, "first run should write both pending events");

        // Replay/re-run: every event is already marked delivered, so nothing new.
        let second = deliver_durable_events_to_dbf(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            0,
            1_700_000_020_000,
        )
        .unwrap();
        assert_eq!(
            second, 0,
            "replay must not re-deliver already-delivered events"
        );

        let records = dbf_records(&dbf_path);
        assert_eq!(
            records.len(),
            2,
            "replay must not duplicate DBF rows for already-delivered (stream_id, seq)"
        );

        // Both events carry a delivery marker after a successful write.
        for seq in [1, 2] {
            let stored = db.load_received_event(stream_id, seq).unwrap().unwrap();
            assert_eq!(stored.dbf_delivered_unix_ms, Some(1_700_000_010_000));
        }
    }

    #[test]
    fn dbf_marker_null_with_existing_row_rebuilds_without_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let db = Db::open_in_memory().unwrap();
        let stream_id = "127.0.0.1:10000";
        let raw = sample_raw_frame();
        insert_durable_event(&db, stream_id, 1, &raw, 1_700_000_000_000);

        let record = map_to_dbf_fields(&raw, EventType::Finish, 0).unwrap();
        append_record(&dbf_path, &record).unwrap();
        assert_eq!(dbf_records(&dbf_path).len(), 1);
        assert_eq!(
            db.load_received_event(stream_id, 1)
                .unwrap()
                .unwrap()
                .dbf_delivered_unix_ms,
            None,
            "test setup simulates a crash after DBF write but before marker update"
        );

        let written = deliver_durable_events_to_dbf(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            0,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(written, 1);

        let records = dbf_records(&dbf_path);
        assert_eq!(
            records.len(),
            1,
            "recovery must rebuild the DBF from received_events instead of appending a duplicate row"
        );
        assert_eq!(
            db.load_received_event(stream_id, 1)
                .unwrap()
                .unwrap()
                .dbf_delivered_unix_ms,
            Some(1_700_000_010_000)
        );
    }

    #[test]
    fn dbf_regeneration_failure_preserves_existing_file_and_markers() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let db = Db::open_in_memory().unwrap();
        let stream_id = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        let raw = sample_raw_frame();
        insert_durable_event(&db, stream_id, 1, &raw, 1_700_000_000_000);
        let written = deliver_durable_events_to_dbf(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            0,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(written, 1);

        let result = regenerate_dbf_from_received_events(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            10,
            1_700_000_030_000,
        );
        assert!(
            result.is_err(),
            "invalid reader index should abort regeneration"
        );

        assert_eq!(
            dbf_records(&dbf_path).len(),
            1,
            "failed regeneration must not clear the live DBF before it can replace it"
        );
        assert_eq!(
            db.load_received_event(stream_id, 1)
                .unwrap()
                .unwrap()
                .dbf_delivered_unix_ms,
            Some(1_700_000_010_000),
            "failed regeneration must not reset stale delivery markers"
        );
    }

    #[test]
    fn dbf_uses_reader_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let db = Db::open_in_memory().unwrap();
        let stream_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        // received_unix_ms is the receiver receipt time; it must NOT influence the
        // DBF TIME/DAYCODE fields. Those come from the reader timestamp carried in
        // the frame (18:45:59.39 on day 01/12/30).
        insert_durable_event(&db, stream_id, 1, &sample_raw_frame(), 0);

        let written = deliver_durable_events_to_dbf(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            0,
            1_700_000_000_000,
        )
        .unwrap();
        assert_eq!(written, 1);

        let records = dbf_records(&dbf_path);
        assert_eq!(records.len(), 1);
        assert_eq!(char_field(&records[0], "TIME"), Some("18455939".to_owned()));
        assert_eq!(
            char_field(&records[0], "DAYCODE"),
            Some("011230".to_owned())
        );
    }

    #[test]
    fn dbf_regenerates_from_received_events() {
        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let db = Db::open_in_memory().unwrap();
        let stream_id = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        let raw = sample_raw_frame();
        insert_durable_event(&db, stream_id, 1, &raw, 1_700_000_000_000);
        insert_durable_event(&db, stream_id, 2, &raw, 1_700_000_000_001);

        let written = deliver_durable_events_to_dbf(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            0,
            1_700_000_010_000,
        )
        .unwrap();
        assert_eq!(written, 2);
        assert_eq!(dbf_records(&dbf_path).len(), 2);

        // Simulate a lost/corrupt DBF file; the durable store is the source of truth.
        std::fs::remove_file(&dbf_path).unwrap();

        let regenerated = regenerate_dbf_from_received_events(
            &db,
            stream_id,
            &dbf_path,
            EventType::Finish,
            0,
            1_700_000_030_000,
        )
        .unwrap();
        assert_eq!(
            regenerated, 2,
            "regeneration should re-emit every stored event"
        );

        let records = dbf_records(&dbf_path);
        assert_eq!(
            records.len(),
            2,
            "DBF must be fully rebuilt from received_events"
        );
        for record in &records {
            assert_eq!(char_field(record, "CHIP"), Some("000000012345".to_owned()));
        }
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
        let (tx, _) = broadcast::channel::<rt_domain::ReadEvent>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let rx = tx.subscribe();
        let (ui_tx, _) = broadcast::channel(16);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, cancel_flag, path, ui_tx).await;
        });

        tx.send(rt_domain::ReadEvent {
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
        let (tx, _) = broadcast::channel::<rt_domain::ReadEvent>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let rx = tx.subscribe();
        let (ui_tx, _) = broadcast::channel(16);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, cancel_flag, path, ui_tx).await;
        });

        tx.send(rt_domain::ReadEvent {
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

    #[tokio::test]
    async fn dbf_writer_uses_updated_event_type_without_waiting_for_cache_refresh() {
        use std::sync::Arc;
        use tokio::sync::{Mutex, broadcast, watch};

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let dbf_path = dir.path().join("test.dbf");
        let db = crate::db::Db::open(&db_path).unwrap();
        db.save_subscription("f1", "10.0.0.1", None, Some(EventType::Finish))
            .unwrap();

        let db = Arc::new(Mutex::new(db));
        let (tx, _) = broadcast::channel::<rt_domain::ReadEvent>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let rx = tx.subscribe();
        let (ui_tx, _) = broadcast::channel(16);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, cancel_flag, path, ui_tx).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        {
            let db = db.lock().await;
            db.update_subscription_event_type("f1", "10.0.0.1", EventType::Start)
                .unwrap();
        }

        tx.send(rt_domain::ReadEvent {
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

        let mut reader = dbase::Reader::from_path(&dbf_path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 1, "should have exactly one record");
        let r = &records[0];
        assert_eq!(
            r.get("EVENT").and_then(|v| match v {
                FieldValue::Character(Some(s)) => Some(s.trim().to_owned()),
                _ => None,
            }),
            Some("S".to_owned())
        );
        assert_eq!(
            r.get("TPOINT").and_then(|v| match v {
                FieldValue::Character(Some(s)) => Some(s.trim().to_owned()),
                _ => None,
            }),
            Some("S".to_owned())
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

    #[test]
    fn clear_dbf_waits_for_existing_exclusive_lock() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked-clear.dbf");

        let raw = sample_raw_frame();
        let rec = map_to_dbf_fields(&raw, EventType::Finish, 0).unwrap();
        append_record(&path, &rec).unwrap();

        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        lock_file.lock_exclusive().unwrap();

        let path_for_thread = path.clone();
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = clear_dbf(&path_for_thread);
            tx.send(result).unwrap();
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "clear_dbf should wait for the active file lock instead of rewriting immediately"
        );

        lock_file.unlock().unwrap();

        let result = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        result.unwrap();
        handle.join().unwrap();

        let mut reader = dbase::Reader::from_path(&path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(records.len(), 0);
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
        let (tx, _) = broadcast::channel::<rt_domain::ReadEvent>(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let rx = tx.subscribe();
        let (ui_tx, _) = broadcast::channel(16);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let path = dbf_path.to_str().unwrap().to_owned();
        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move {
            run_dbf_writer(rx, db_clone, shutdown_rx, cancel_flag, path, ui_tx).await;
        });

        // Send event for a forwarder/reader that is NOT subscribed
        tx.send(rt_domain::ReadEvent {
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

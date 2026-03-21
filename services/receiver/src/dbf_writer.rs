//! DBF writer module for IPICO chip read records.
//!
//! Converts IPICO raw frames into Visual FoxPro DBF records and manages DBF
//! file I/O using the `dbase` crate.

use std::convert::TryFrom;
use std::io::Write;
use std::path::Path;

use dbase::{FieldIOError, FieldName, TableWriterBuilder, WritableRecord};
use ipico_core::read::ChipRead;

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

// ---------------------------------------------------------------------------
// Schema builder helper
// ---------------------------------------------------------------------------

fn schema_builder() -> TableWriterBuilder {
    TableWriterBuilder::new()
        .add_character_field(FieldName::try_from("EVENT").unwrap(), 1)
        .add_character_field(FieldName::try_from("DIVISION").unwrap(), 2)
        .add_character_field(FieldName::try_from("CHIP").unwrap(), 12)
        .add_character_field(FieldName::try_from("TIME").unwrap(), 8)
        .add_character_field(FieldName::try_from("RUNERNO").unwrap(), 5)
        .add_character_field(FieldName::try_from("DAYCODE").unwrap(), 6)
        .add_character_field(FieldName::try_from("LAPNO").unwrap(), 3)
        .add_character_field(FieldName::try_from("TPOINT").unwrap(), 2)
        .add_character_field(FieldName::try_from("READER").unwrap(), 1)
}

// ---------------------------------------------------------------------------
// create_empty_dbf
// ---------------------------------------------------------------------------

/// Create a new empty DBF file at `path` with the IPICO 9-field schema.
///
/// If a file already exists at `path` it will be overwritten.
pub fn create_empty_dbf(path: &Path) -> std::io::Result<()> {
    schema_builder()
        .build_with_file_dest(path)
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

    std::fs::rename(&tmp_path, path)?;
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
}

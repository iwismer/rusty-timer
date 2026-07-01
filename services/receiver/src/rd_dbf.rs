//! Import participant + chip data directly from a Race Director (RD) working
//! directory of Visual FoxPro `.DBF` files.
//!
//! RD is a Visual FoxPro app; its files are DBF (v0x30/v0x32), latin1/codepage
//! text, fixed-width records, with deleted rows flagged by `*` in byte 0. We
//! read a small subset of `C`/`N` columns by name and ignore everything else
//! (including memo/`V`/`T` columns, so no sidecar `.FPT`/`.CDX` handling is
//! needed).
//!
//! Files read (fixed names, stable across RD installs — see
//! `docs/race-director/participant-dbf-import.md`):
//!
//! * `checkchip.dbf` — bib ↔ chip map (`CHECK1`=bib, `CHECK2`=chip). One row
//!   per chip, so a bib with several chips is simply several rows; row 1 is a
//!   deleted `BIB`/`CHIP` header that is skipped.
//! * `RACE.DBF` — participant table (`RUNERNO`=bib, `RUNFNAME`, `RUNLNAME`,
//!   `RUNSEX`, `RUNDIV`). This is the default participant source; `ANNRACE.DBF`
//!   is an unverified alternative (spec §3, open question #2) and is **not**
//!   used here.
//! * `DIVISION.DBF` — division names (`DIVNO`→`DIVNAME`).
//!
//! Open question #1 (spec §10): this assumes `bib == RUNERNO` (== `CHECK1`),
//! which held for the one sample event but is unverified across RD installs.

use crate::participants::Participant;
use std::collections::HashMap;
use std::path::Path;

/// Fixed RD filename for the bib↔chip map.
pub const CHECKCHIP_FILE: &str = "checkchip.dbf";
/// Fixed RD filename for the participant table (default source; see spec §3).
pub const RACE_FILE: &str = "RACE.DBF";
/// Fixed RD filename for the division-name table.
pub const DIVISION_FILE: &str = "DIVISION.DBF";

/// The set of files an RD import reads, in the order they are parsed. Used by
/// the background poller for change detection and snapshot copying.
pub const RD_FILES: &[&str] = &[CHECKCHIP_FILE, RACE_FILE, DIVISION_FILE];

/// Parsed contents of an RD working directory, in the shape the existing
/// replace-all import path consumes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RdImport {
    /// Participants from `RACE.DBF` (bib, name, gender, division code).
    pub participants: Vec<Participant>,
    /// `(bib, chip_id)` pairs from `checkchip.dbf`; chip ids are lowercased.
    pub chips: Vec<(i64, String)>,
    /// Division code → display name from `DIVISION.DBF`.
    pub divisions: HashMap<i32, String>,
}

/// Error parsing an RD directory or one of its DBF files.
#[derive(Debug)]
pub enum RdError {
    /// A required file could not be read.
    Io {
        file: String,
        source: std::io::Error,
    },
    /// A DBF header/structure was malformed.
    Malformed { file: String, reason: String },
}

impl std::fmt::Display for RdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RdError::Io { file, source } => write!(f, "reading {file}: {source}"),
            RdError::Malformed { file, reason } => write!(f, "parsing {file}: {reason}"),
        }
    }
}

impl std::error::Error for RdError {}

/// A single DBF field descriptor (name, type, and computed record offset).
struct Field {
    name: String,
    /// DBF type byte (`C`, `N`, …). Retained for callers that care; the reader
    /// itself decodes every selected field as latin1 text.
    kind: u8,
    offset: usize,
    length: usize,
}

/// A parsed DBF table: field descriptors plus the raw record region. Records
/// are decoded lazily by name so wide tables (RD's participant table has 200+
/// columns) cost only the handful of columns we actually read.
struct Dbf {
    fields: Vec<Field>,
    header_len: usize,
    record_len: usize,
    num_records: usize,
    bytes: Vec<u8>,
}

impl Dbf {
    /// Parse a DBF file generically across VFP versions: the header records the
    /// header length, record length, and record count, so the version byte is
    /// never asserted (RD ships v0x30 and v0x32 side by side).
    fn parse(file: &str, bytes: Vec<u8>) -> Result<Self, RdError> {
        let malformed = |reason: &str| RdError::Malformed {
            file: file.to_owned(),
            reason: reason.to_owned(),
        };
        if bytes.len() < 32 {
            return Err(malformed("file shorter than 32-byte header"));
        }
        let num_records = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let record_len = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
        if header_len < 33 || record_len == 0 {
            return Err(malformed("degenerate header/record length"));
        }
        if bytes.len() < header_len {
            return Err(malformed("file truncated before end of header"));
        }
        // Reject a file whose body is shorter than the header claims: a header
        // with a valid record count but a truncated record region (e.g. a copy
        // captured mid-write) must fail so the caller keeps last good rather
        // than silently importing a partial table.
        let required = num_records
            .checked_mul(record_len)
            .and_then(|body| body.checked_add(header_len));
        match required {
            Some(required) if bytes.len() >= required => {}
            _ => return Err(malformed("file truncated before end of record data")),
        }

        let mut fields = Vec::new();
        let mut offset = 1; // byte 0 of each record is the delete flag
        let mut pos = 32;
        loop {
            if pos >= bytes.len() {
                return Err(malformed("field descriptors run past end of file"));
            }
            if bytes[pos] == 0x0D {
                break; // header terminator
            }
            if pos + 32 > header_len {
                return Err(malformed("field descriptor overruns header"));
            }
            let raw_name = &bytes[pos..pos + 11];
            let name_end = raw_name.iter().position(|&b| b == 0).unwrap_or(11);
            let name = latin1(&raw_name[..name_end]);
            // Field descriptor layout: name[0..11], type[11], data address[12..16],
            // length[16], decimal count[17].
            let kind = bytes[pos + 11];
            let length = bytes[pos + 16] as usize;
            fields.push(Field {
                name,
                kind,
                offset,
                length,
            });
            offset += length;
            pos += 32;
        }

        // The delete flag + fields must fit within the declared record length.
        if offset > record_len {
            return Err(malformed("field widths exceed record length"));
        }

        Ok(Self {
            fields,
            header_len,
            record_len,
            num_records,
            bytes,
        })
    }

    fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Iterate live (non-deleted) records as raw byte slices. Records whose
    /// bytes fall outside the file (a torn/short final record) are skipped
    /// rather than panicking.
    fn records(&self) -> impl Iterator<Item = &[u8]> + '_ {
        (0..self.num_records).filter_map(move |i| {
            let start = self.header_len + i * self.record_len;
            let end = start + self.record_len;
            let rec = self.bytes.get(start..end)?;
            // Delete flag: `*` = deleted (skip), anything else = live.
            if rec.first() == Some(&b'*') {
                None
            } else {
                Some(rec)
            }
        })
    }

    /// Decode one field of a record as trimmed latin1 text, or `None` if the
    /// column is absent or is a memo/`.fpt`-backed type whose bytes are a
    /// pointer rather than a value. We only ever request `C`/`N` columns, so
    /// the memo guard is defensive.
    fn value(&self, rec: &[u8], field: &Field) -> Option<String> {
        if matches!(field.kind, b'M' | b'G' | b'P' | b'B') {
            return None;
        }
        let raw = rec.get(field.offset..field.offset + field.length)?;
        Some(latin1(raw).trim().to_owned())
    }
}

/// Decode bytes as latin1 (ISO-8859-1): each byte maps directly to the code
/// point of the same value.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Read and parse one DBF file from `dir`.
fn read_dbf(dir: &Path, file: &str) -> Result<Dbf, RdError> {
    let path = dir.join(file);
    let bytes = std::fs::read(&path).map_err(|e| RdError::Io {
        file: file.to_owned(),
        source: e,
    })?;
    Dbf::parse(file, bytes)
}

/// Parse `checkchip.dbf` into `(bib, chip_id)` pairs. Skips the deleted header
/// row and any row whose bib is non-numeric or whose chip is empty/non-hex.
/// Chip ids are lowercased to match reader/emulator frames (`{:012x}`).
fn parse_checkchip(dbf: &Dbf) -> Result<Vec<(i64, String)>, RdError> {
    let bib_field = dbf.field("CHECK1").ok_or_else(|| RdError::Malformed {
        file: CHECKCHIP_FILE.to_owned(),
        reason: "missing CHECK1 (bib) column".to_owned(),
    })?;
    let chip_field = dbf.field("CHECK2").ok_or_else(|| RdError::Malformed {
        file: CHECKCHIP_FILE.to_owned(),
        reason: "missing CHECK2 (chip) column".to_owned(),
    })?;
    let mut chips = Vec::new();
    for rec in dbf.records() {
        let Some(bib_raw) = dbf.value(rec, bib_field) else {
            continue;
        };
        let Some(chip_raw) = dbf.value(rec, chip_field) else {
            continue;
        };
        // Non-numeric bib (e.g. the "BIB" header row that survived deletion)
        // or empty chip → skip the row.
        let Ok(bib) = bib_raw.parse::<i64>() else {
            continue;
        };
        if chip_raw.is_empty() {
            continue;
        }
        if !chip_raw.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        chips.push((bib, chip_raw.to_ascii_lowercase()));
    }
    Ok(chips)
}

/// Parse the participant table (`RACE.DBF`) into [`Participant`]s. Skips rows
/// with a non-numeric `RUNERNO`.
fn parse_race(dbf: &Dbf) -> Result<Vec<Participant>, RdError> {
    let missing = |col: &str| RdError::Malformed {
        file: RACE_FILE.to_owned(),
        reason: format!("missing {col} column"),
    };
    let bib_field = dbf.field("RUNERNO").ok_or_else(|| missing("RUNERNO"))?;
    let first_field = dbf.field("RUNFNAME");
    let last_field = dbf.field("RUNLNAME");
    let sex_field = dbf.field("RUNSEX");
    let div_field = dbf.field("RUNDIV");

    let mut participants = Vec::new();
    for rec in dbf.records() {
        let Some(bib_raw) = dbf.value(rec, bib_field) else {
            continue;
        };
        let Ok(bib) = bib_raw.parse::<i64>() else {
            continue;
        };
        let first = first_field
            .and_then(|f| dbf.value(rec, f))
            .unwrap_or_default();
        let last = last_field
            .and_then(|f| dbf.value(rec, f))
            .unwrap_or_default();
        let gender = match sex_field
            .and_then(|f| dbf.value(rec, f))
            .as_deref()
            .map(str::trim)
        {
            Some("M" | "m") => "M",
            Some("F" | "f") => "F",
            _ => "X",
        }
        .to_owned();
        let division = div_field
            .and_then(|f| dbf.value(rec, f))
            .and_then(|s| s.trim().parse::<i32>().ok());
        participants.push(Participant {
            bib,
            last,
            first,
            affiliation: String::new(),
            gender,
            division,
        });
    }
    Ok(participants)
}

/// Parse `DIVISION.DBF` into a division-code → display-name map. Skips rows
/// with a non-numeric `DIVNO`.
fn parse_divisions(dbf: &Dbf) -> Result<HashMap<i32, String>, RdError> {
    let missing = |col: &str| RdError::Malformed {
        file: DIVISION_FILE.to_owned(),
        reason: format!("missing {col} column"),
    };
    let no_field = dbf.field("DIVNO").ok_or_else(|| missing("DIVNO"))?;
    let name_field = dbf.field("DIVNAME").ok_or_else(|| missing("DIVNAME"))?;
    let mut divisions = HashMap::new();
    for rec in dbf.records() {
        let Some(no_raw) = dbf.value(rec, no_field) else {
            continue;
        };
        let Ok(divno) = no_raw.parse::<i32>() else {
            continue;
        };
        let name = dbf.value(rec, name_field).unwrap_or_default();
        divisions.insert(divno, name);
    }
    Ok(divisions)
}

/// Parse a complete RD working directory into an [`RdImport`]. Reads all three
/// fixed-name files; any read/parse failure returns `Err` so a background
/// caller can keep its last good import rather than blanking the board.
pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<RdImport, RdError> {
    let dir = dir.as_ref();
    let checkchip = read_dbf(dir, CHECKCHIP_FILE)?;
    let race = read_dbf(dir, RACE_FILE)?;
    let division = read_dbf(dir, DIVISION_FILE)?;
    Ok(RdImport {
        participants: parse_race(&race)?,
        chips: parse_checkchip(&checkchip)?,
        divisions: parse_divisions(&division)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("rd")
    }

    fn load_fixture(file: &str) -> Dbf {
        read_dbf(&fixtures_dir(), file).expect("fixture parses")
    }

    /// A minimal DBF builder mirroring the committed fixtures' layout, for
    /// focused unit cases that would add little as extra binary fixtures.
    fn build_dbf(version: u8, fields: &[(&str, u8, usize)], rows: &[(bool, Vec<&str>)]) -> Vec<u8> {
        let header_len = 32 + 32 * fields.len() + 1;
        let record_len: usize = 1 + fields.iter().map(|f| f.2).sum::<usize>();
        let mut out = Vec::new();
        out.push(version);
        out.extend_from_slice(&[25, 1, 1]);
        out.extend_from_slice(&u32::try_from(rows.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&u16::try_from(header_len).unwrap().to_le_bytes());
        out.extend_from_slice(&u16::try_from(record_len).unwrap().to_le_bytes());
        out.extend_from_slice(&[0u8; 20]);
        let mut addr = 1u32;
        for (name, tchar, length) in fields {
            let nb = name.as_bytes();
            out.extend_from_slice(nb);
            out.extend(std::iter::repeat_n(0u8, 11 - nb.len()));
            out.push(*tchar);
            out.extend_from_slice(&addr.to_le_bytes());
            out.push(u8::try_from(*length).unwrap());
            out.push(0);
            out.extend_from_slice(&[0u8; 14]);
            addr += u32::try_from(*length).unwrap();
        }
        out.push(0x0D);
        for (deleted, values) in rows {
            out.push(if *deleted { b'*' } else { b' ' });
            for ((_, tchar, length), val) in fields.iter().zip(values.iter()) {
                let enc = val.as_bytes();
                let enc = &enc[..enc.len().min(*length)];
                if *tchar == b'N' {
                    out.extend(std::iter::repeat_n(b' ', length - enc.len()));
                    out.extend_from_slice(enc);
                } else {
                    out.extend_from_slice(enc);
                    out.extend(std::iter::repeat_n(b' ', length - enc.len()));
                }
            }
        }
        out.push(0x1A);
        out
    }

    #[test]
    fn header_parse_reads_field_descriptors() {
        let dbf = load_fixture(CHECKCHIP_FILE);
        assert_eq!(dbf.record_len, 1 + 10 + 12);
        assert_eq!(dbf.num_records, 5);
        assert_eq!(dbf.fields.len(), 2);
        assert_eq!(dbf.field("CHECK1").unwrap().kind, b'C');
        assert_eq!(dbf.field("CHECK2").unwrap().offset, 1 + 10);
    }

    #[test]
    fn header_parse_is_version_agnostic_v32() {
        // The v0x32 fixture must parse exactly like v0x30 (version byte differs).
        let dbf = load_fixture("eventnm_v32.dbf");
        assert_eq!(dbf.bytes[0], 0x32);
        assert_eq!(dbf.fields.len(), 1);
        let rec = dbf.records().next().unwrap();
        assert_eq!(
            dbf.value(rec, dbf.field("EVENTNM").unwrap()).unwrap(),
            "Santa Hamilton 2025"
        );
    }

    #[test]
    fn delete_flag_skips_deleted_header_row() {
        let dbf = load_fixture(CHECKCHIP_FILE);
        // 5 physical rows, row 0 is the deleted BIB/CHIP header → 4 live rows.
        assert_eq!(dbf.records().count(), 4);
    }

    #[test]
    fn field_selection_reads_named_columns_ignoring_others() {
        // RACE.DBF has ignored RUNCITY/RUNAGE columns between the ones we read.
        let dbf = load_fixture(RACE_FILE);
        let rec = dbf.records().next().unwrap();
        assert_eq!(dbf.value(rec, dbf.field("RUNERNO").unwrap()).unwrap(), "1");
        assert_eq!(
            dbf.value(rec, dbf.field("RUNLNAME").unwrap()).unwrap(),
            "Smith"
        );
        assert!(dbf.field("NOSUCHCOL").is_none());
    }

    #[test]
    fn latin1_decodes_accented_names() {
        let parts = parse_race(&load_fixture(RACE_FILE)).unwrap();
        let renee = parts.iter().find(|p| p.bib == 2).unwrap();
        assert_eq!(renee.first, "Renée");
        assert_eq!(renee.last, "Dupont");
        assert_eq!(renee.gender, "F");
        assert_eq!(renee.division, Some(2));
    }

    #[test]
    fn checkchip_maps_bibs_and_lowercases_hex_chips() {
        let chips = parse_checkchip(&load_fixture(CHECKCHIP_FILE)).unwrap();
        // Header row skipped; spare chip (bib 900) still included as a pair.
        assert!(chips.contains(&(2, "058003799abc".to_owned())));
        assert!(chips.contains(&(900, "aaaaaaaaaaaa".to_owned())));
        // No non-numeric bib survives.
        assert!(chips.iter().all(|(bib, _)| *bib >= 1));
    }

    #[test]
    fn multiple_chips_per_bib_all_map_to_that_bib() {
        let chips = parse_checkchip(&load_fixture(CHECKCHIP_FILE)).unwrap();
        let for_bib1: Vec<&String> = chips
            .iter()
            .filter(|(bib, _)| *bib == 1)
            .map(|(_, c)| c)
            .collect();
        assert_eq!(for_bib1.len(), 2, "bib 1 has two chips");
        assert!(for_bib1.contains(&&"058003799177".to_owned()));
        assert!(for_bib1.contains(&&"058003799178".to_owned()));
    }

    #[test]
    fn gender_maps_m_f_else_x() {
        let parts = parse_race(&load_fixture(RACE_FILE)).unwrap();
        assert_eq!(parts.iter().find(|p| p.bib == 1).unwrap().gender, "M");
        assert_eq!(parts.iter().find(|p| p.bib == 3).unwrap().gender, "X");
    }

    #[test]
    fn divisions_map_code_to_name() {
        let divs = parse_divisions(&load_fixture(DIVISION_FILE)).unwrap();
        assert_eq!(divs.get(&1).unwrap(), "5k");
        assert_eq!(divs.get(&2).unwrap(), "10k");
        assert_eq!(divs.get(&3).unwrap(), "test");
    }

    #[test]
    fn load_from_dir_reads_all_three_files() {
        let import = load_from_dir(fixtures_dir()).unwrap();
        assert_eq!(import.participants.len(), 3);
        assert_eq!(import.chips.len(), 4);
        assert_eq!(import.divisions.len(), 3);
    }

    #[test]
    fn missing_directory_is_an_error() {
        let err = load_from_dir(fixtures_dir().join("does-not-exist")).unwrap_err();
        assert!(matches!(err, RdError::Io { .. }));
    }

    #[test]
    fn malformed_short_file_is_rejected() {
        let Err(err) = Dbf::parse("tiny.dbf", vec![0u8; 8]) else {
            panic!("expected a malformed error");
        };
        assert!(matches!(err, RdError::Malformed { .. }));
    }

    #[test]
    fn truncated_body_with_valid_header_is_rejected() {
        // A well-formed header claiming 2 records, but the record region is
        // cut short (torn/partial copy) — must fail rather than import row 1
        // and silently drop the rest.
        let full = build_dbf(
            0x30,
            &[("CHECK1", b'C', 4), ("CHECK2", b'C', 4)],
            &[(false, vec!["1", "0abc"]), (false, vec!["2", "0abd"])],
        );
        // Drop the last few bytes so the second record is incomplete.
        let truncated = full[..full.len() - 6].to_vec();
        let Err(err) = Dbf::parse("torn.dbf", truncated) else {
            panic!("expected a malformed error for a truncated body");
        };
        assert!(matches!(err, RdError::Malformed { .. }));
    }

    #[test]
    fn built_dbf_matches_reader_expectations() {
        // Exercises the in-test builder against a deleted row + latin1 byte.
        let bytes = build_dbf(
            0x30,
            &[("CHECK1", b'C', 4), ("CHECK2", b'C', 4)],
            &[(true, vec!["BIB", "CHIP"]), (false, vec!["7", "0aBf"])],
        );
        let dbf = Dbf::parse("built.dbf", bytes).unwrap();
        let chips = parse_checkchip(&dbf).unwrap();
        assert_eq!(chips, vec![(7, "0abf".to_owned())]);
    }
}

//! Parsers for the legacy participant (`.ppl`) and chip-assignment
//! (`.bibchip`) file formats, restored so the receiver can resolve chip reads
//! to bib/name locally for the announcer.
//!
//! Formats (see `docs/file-formats.md` in repo history):
//!
//! * **`.ppl`** — headerless CSV, one participant per line:
//!   `bib, lastName, firstName, [affiliation], [reserved], [gender]`.
//!   Lines starting with `;` and blank lines are skipped. `gender` maps
//!   `M`/`F` (any case) to `M`/`F`, anything else to `X`.
//! * **`.bibchip`** — headerless CSV `bib, chipId(hex)`. Any line that does not
//!   start with an ASCII digit is skipped (so `BIB,CHIP` headers are ignored).
//!   The chip id must be non-empty and hexadecimal; a non-hex value rejects the
//!   file (it can never match a real IPICO frame).
//!
//! Bib normalization: both formats parse the bib to an `i64`, which is the
//! canonical key used to join chips to participants. Rendering the bib back to
//! a decimal string (`i64::to_string`) yields the canonical bib string used in
//! the chip lookup, so `1`, `01`, and ` 1 ` all resolve identically.

/// A participant parsed from a `.ppl` record. `gender` is normalized to one of
/// `"M"`, `"F"`, or `"X"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub bib: i64,
    pub last: String,
    pub first: String,
    pub affiliation: String,
    pub gender: String,
    /// Division code (from the Race Director `RUNDIV` column). `None` for the
    /// `.ppl` format, which has no division field. Joined to a display name via
    /// the `divisions` table on resolve.
    pub division: Option<i32>,
}

/// Parse a single `.ppl` line.
///
/// Returns `Ok(None)` for blank lines and `;` comments, `Ok(Some(_))` for a
/// valid participant, and `Err` for a malformed record (too few fields or a
/// non-integer bib).
pub fn parse_ppl_line(line: &str) -> Result<Option<Participant>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return Ok(None);
    }
    let parts: Vec<&str> = trimmed.split(',').collect();
    if parts.len() < 3 {
        return Err(format!(
            "participant record needs at least bib,last,first; got {} field(s)",
            parts.len()
        ));
    }
    let bib = parts[0]
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("invalid bib `{}`", parts[0].trim()))?;
    let last = parts[1].trim().to_owned();
    let first = parts[2].trim().to_owned();
    let affiliation = parts
        .get(3)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    let gender = match parts.get(5).map(|s| s.trim()) {
        Some("M" | "m") => "M",
        Some("F" | "f") => "F",
        _ => "X",
    }
    .to_owned();
    Ok(Some(Participant {
        bib,
        last,
        first,
        affiliation,
        gender,
        division: None,
    }))
}

/// Parse a single `.bibchip` line into `(bib, chip_id)`.
///
/// Returns `Ok(None)` for blank lines and any line not starting with an ASCII
/// digit (header rows), `Ok(Some(_))` for a valid mapping, and `Err` for a
/// data row with a non-integer bib or a missing chip id.
pub fn parse_bibchip_line(line: &str) -> Result<Option<(i64, String)>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // Skip any line that does not begin with a digit (e.g. a `BIB,CHIP` header).
    if !trimmed.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(None);
    }
    let parts: Vec<&str> = trimmed.split(',').collect();
    if parts.len() < 2 {
        return Err(format!(
            "chip record needs bib,chipId; got {} field(s)",
            parts.len()
        ));
    }
    let bib = parts[0]
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("invalid bib `{}`", parts[0].trim()))?;
    let chip = parts[1].trim().to_owned();
    if chip.is_empty() {
        return Err("chip id must not be empty".to_owned());
    }
    if !chip.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("chip id `{chip}` must be hexadecimal"));
    }
    Ok(Some((bib, chip)))
}

/// Parse a whole `.ppl` file. Strict: a single malformed data row rejects the
/// entire file (reporting the 1-based line number), matching the legacy
/// "upload replaces all / all-or-nothing" semantics.
pub fn parse_ppl(contents: &str) -> Result<Vec<Participant>, String> {
    let mut out = Vec::new();
    for (idx, line) in contents.lines().enumerate() {
        match parse_ppl_line(line) {
            Ok(Some(p)) => out.push(p),
            Ok(None) => {}
            Err(e) => return Err(format!("line {}: {e}", idx + 1)),
        }
    }
    Ok(out)
}

/// Parse a whole `.bibchip` file. Strict, like [`parse_ppl`].
pub fn parse_bibchip(contents: &str) -> Result<Vec<(i64, String)>, String> {
    let mut out = Vec::new();
    for (idx, line) in contents.lines().enumerate() {
        match parse_bibchip_line(line) {
            Ok(Some(pair)) => out.push(pair),
            Ok(None) => {}
            Err(e) => return Err(format!("line {}: {e}", idx + 1)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ppl_record_parses_bib_and_name() {
        let p = parse_ppl_line("0,Smith,John,Team Smith,,M")
            .unwrap()
            .unwrap();
        assert_eq!(p.bib, 0);
        assert_eq!(p.last, "Smith");
        assert_eq!(p.first, "John");
        assert_eq!(p.affiliation, "Team Smith");
        assert_eq!(p.gender, "M");
    }

    #[test]
    fn ppl_minimal_three_fields_ok_with_defaults() {
        let p = parse_ppl_line("7,Doe,Jane").unwrap().unwrap();
        assert_eq!(p.bib, 7);
        assert_eq!(p.affiliation, "");
        assert_eq!(p.gender, "X");
    }

    #[test]
    fn ppl_comment_and_blank_lines_skipped() {
        assert!(parse_ppl_line(";comment").unwrap().is_none());
        assert!(parse_ppl_line("").unwrap().is_none());
        assert!(parse_ppl_line("   ").unwrap().is_none());
    }

    #[test]
    fn bad_bib_is_rejected() {
        assert!(parse_ppl_line("z,Smith,John").is_err());
    }

    #[test]
    fn too_few_fields_rejected() {
        assert!(parse_ppl_line("1,Smith").is_err());
    }

    #[test]
    fn gender_normalizes_case_and_unknown() {
        assert_eq!(parse_ppl_line("1,A,B,,,f").unwrap().unwrap().gender, "F");
        assert_eq!(parse_ppl_line("1,A,B,,,Q").unwrap().unwrap().gender, "X");
    }

    #[test]
    fn bibchip_skips_header_and_parses() {
        assert!(parse_bibchip_line("BIB,CHIP").unwrap().is_none());
        let (bib, chip) = parse_bibchip_line("1,058003799177").unwrap().unwrap();
        assert_eq!(bib, 1);
        assert_eq!(chip, "058003799177");
    }

    #[test]
    fn bibchip_extra_columns_ignored_and_empty_chip_rejected() {
        let (bib, chip) = parse_bibchip_line("2,0580,extra,cols").unwrap().unwrap();
        assert_eq!(bib, 2);
        assert_eq!(chip, "0580");
        assert!(parse_bibchip_line("3,").is_err());
    }

    #[test]
    fn bibchip_non_hex_chip_rejected() {
        // Hex chip ids are accepted (incl. a-f any case)...
        assert_eq!(
            parse_bibchip_line("4,0AbF").unwrap().unwrap(),
            (4, "0AbF".to_owned())
        );
        // ...but a non-hex chip id rejects the record.
        let err = parse_bibchip_line("5,05G0").unwrap_err();
        assert!(err.contains("hexadecimal"), "got: {err}");
    }

    #[test]
    fn parse_ppl_whole_file_is_strict() {
        let ok = parse_ppl(";header\n1,A,B,,,M\n2,C,D\n").unwrap();
        assert_eq!(ok.len(), 2);
        let err = parse_ppl("1,A,B\nz,bad,row\n").unwrap_err();
        assert!(err.contains("line 2"), "got: {err}");
    }

    #[test]
    fn parse_bibchip_whole_file_skips_header_strict_on_data() {
        let ok = parse_bibchip("BIB,CHIP\n1,0580\n2,0581\n").unwrap();
        assert_eq!(ok, vec![(1, "0580".to_owned()), (2, "0581".to_owned())]);
        let err = parse_bibchip("1,0580\n2x,0581\n").unwrap_err();
        assert!(err.contains("line 2"), "got: {err}");
    }

    #[test]
    fn bib_normalization_is_canonical_via_i64() {
        // Leading zeros / whitespace normalize to the same i64 bib.
        assert_eq!(parse_ppl_line(" 01 ,A,B").unwrap().unwrap().bib, 1);
        assert_eq!(parse_bibchip_line("01,0580").unwrap().unwrap().0, 1);
    }
}

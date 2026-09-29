//! Export module for generating TAGDATA.TXT and TAGDATA.ZIP files from raw reader frames.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::storage::journal::RawExportEvent;

/// Clean a single raw frame into a valid 36+ character IPICO tag-report line ending in CRLF (`\r\n`).
///
/// Returns `None` if the frame is empty, not valid UTF-8, or does not start with `"aa"`.
pub fn render_tagdata_line(raw_frame: &[u8]) -> Option<String> {
    let trimmed = raw_frame
        .strip_suffix(b"\r\n")
        .or_else(|| raw_frame.strip_suffix(b"\n"))
        .unwrap_or(raw_frame);
    let s = std::str::from_utf8(trimmed).ok()?.trim();
    if s.starts_with("aa") && s.len() >= 36 {
        let mut line = s.to_string();
        line.push_str("\r\n");
        Some(line)
    } else {
        None
    }
}

/// Render a slice of raw events into a single merged TAGDATA text content.
pub fn render_merged_tagdata(events: &[RawExportEvent]) -> String {
    let mut out = String::new();
    for event in events {
        if let Some(line) = render_tagdata_line(&event.raw_frame) {
            out.push_str(&line);
        }
    }
    out
}

/// Sanitize a stream key or reader IP for safe usage as a directory or filename component.
pub fn sanitize_reader_name(stream_key: &str) -> String {
    stream_key
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Render raw events grouped by stream_key into separate files inside a ZIP archive.
pub fn render_separate_tagdata_zip(events: &[RawExportEvent]) -> Result<Vec<u8>, std::io::Error> {
    let mut grouped: BTreeMap<String, Vec<&RawExportEvent>> = BTreeMap::new();
    for event in events {
        grouped
            .entry(event.stream_key.clone())
            .or_default()
            .push(event);
    }

    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();

    for (stream_key, stream_events) in grouped {
        let clean_name = sanitize_reader_name(&stream_key);
        let path = format!("{clean_name}/TAGDATA.TXT");
        zip.start_file(path, options)?;

        for event in stream_events {
            if let Some(line) = render_tagdata_line(&event.raw_frame) {
                zip.write_all(line.as_bytes())?;
            }
        }
    }

    zip.finish()?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_tagdata_line() {
        let raw = b"aa00058000123b3200012606251750411376\r\n";
        assert_eq!(
            render_tagdata_line(raw),
            Some("aa00058000123b3200012606251750411376\r\n".to_string())
        );

        let raw_no_crlf = b"aa00058000123b3200012606251750411376";
        assert_eq!(
            render_tagdata_line(raw_no_crlf),
            Some("aa00058000123b3200012606251750411376\r\n".to_string())
        );

        // Control frame 'ab' is skipped
        assert_eq!(render_tagdata_line(b"ab010203\r\n"), None);

        // Short frame is skipped
        assert_eq!(render_tagdata_line(b"aa1234\r\n"), None);
    }

    #[test]
    fn test_render_merged_tagdata() {
        let events = vec![
            RawExportEvent {
                stream_key: "192.168.1.10:10000".to_string(),
                epoch: 1,
                seq: 1,
                raw_frame: b"aa00058000123b3200012606251750411376\n".to_vec(),
                reader_timestamp: None,
                received_unix_ms: 1000,
            },
            RawExportEvent {
                stream_key: "192.168.1.11:10000".to_string(),
                epoch: 1,
                seq: 2,
                raw_frame: b"aa01058000123b3200012606251750411377\r\n".to_vec(),
                reader_timestamp: None,
                received_unix_ms: 2000,
            },
        ];

        let merged = render_merged_tagdata(&events);
        assert_eq!(
            merged,
            "aa00058000123b3200012606251750411376\r\naa01058000123b3200012606251750411377\r\n"
        );
    }

    #[test]
    fn test_render_separate_tagdata_zip() {
        let events = vec![
            RawExportEvent {
                stream_key: "192.168.1.10:10000".to_string(),
                epoch: 1,
                seq: 1,
                raw_frame: b"aa00058000123b3200012606251750411376\n".to_vec(),
                reader_timestamp: None,
                received_unix_ms: 1000,
            },
            RawExportEvent {
                stream_key: "192.168.1.11:10000".to_string(),
                epoch: 1,
                seq: 2,
                raw_frame: b"aa01058000123b3200012606251750411377\r\n".to_vec(),
                reader_timestamp: None,
                received_unix_ms: 2000,
            },
        ];

        let zip_bytes = render_separate_tagdata_zip(&events).expect("zip created");
        assert!(!zip_bytes.is_empty());

        let reader = Cursor::new(zip_bytes);
        let mut zip_archive = zip::ZipArchive::new(reader).expect("valid zip");
        assert_eq!(zip_archive.len(), 2);

        let mut content1 = String::new();
        {
            let mut file1 = zip_archive
                .by_name("192.168.1.10_10000/TAGDATA.TXT")
                .expect("file 1 exists");
            std::io::Read::read_to_string(&mut file1, &mut content1).unwrap();
        }
        assert_eq!(content1, "aa00058000123b3200012606251750411376\r\n");

        let mut content2 = String::new();
        {
            let mut file2 = zip_archive
                .by_name("192.168.1.11_10000/TAGDATA.TXT")
                .expect("file 2 exists");
            std::io::Read::read_to_string(&mut file2, &mut content2).unwrap();
        }
        assert_eq!(content2, "aa01058000123b3200012606251750411377\r\n");
    }
}

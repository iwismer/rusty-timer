//! Export module for generating TAGDATA.TXT and TAGDATA.ZIP files from raw reader frames on the receiver.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::db::ReceivedEvent;

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

/// Render a slice of raw received events into a single merged TAGDATA text content.
pub fn render_merged_tagdata(events: &[ReceivedEvent]) -> String {
    let mut out = String::new();
    for event in events {
        if let Some(line) = render_tagdata_line(&event.raw_frame) {
            out.push_str(&line);
        }
    }
    out
}

/// Sanitize a stream key or reader name for safe filesystem / zip path usage.
pub fn sanitize_stream_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Render raw events grouped by stream_id into separate files inside a ZIP archive.
pub fn render_separate_tagdata_zip(events: &[ReceivedEvent]) -> Result<Vec<u8>, std::io::Error> {
    let mut grouped: BTreeMap<String, Vec<&ReceivedEvent>> = BTreeMap::new();
    for event in events {
        grouped
            .entry(event.stream_id.clone())
            .or_default()
            .push(event);
    }

    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();

    for (stream_id, stream_events) in grouped {
        let clean_name = sanitize_stream_name(&stream_id);
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

/// Write merged TAGDATA.TXT to a destination directory on disk.
/// Returns the path of the written file.
pub fn write_merged_tagdata_file(
    dest_dir: &Path,
    content: &str,
) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(dest_dir)?;
    let target = dest_dir.join("TAGDATA.TXT");
    std::fs::write(&target, content)?;
    Ok(target)
}

/// Write separate TAGDATA.TXT files per stream into subdirectories under dest_dir.
/// Returns the list of written file paths.
pub fn write_separate_tagdata_files(
    dest_dir: &Path,
    events: &[ReceivedEvent],
) -> Result<Vec<PathBuf>, std::io::Error> {
    std::fs::create_dir_all(dest_dir)?;
    let mut grouped: BTreeMap<String, Vec<&ReceivedEvent>> = BTreeMap::new();
    for event in events {
        grouped
            .entry(event.stream_id.clone())
            .or_default()
            .push(event);
    }

    let mut written = Vec::new();
    for (stream_id, stream_events) in grouped {
        let clean_name = sanitize_stream_name(&stream_id);
        let sub_dir = dest_dir.join(clean_name);
        std::fs::create_dir_all(&sub_dir)?;
        let file_path = sub_dir.join("TAGDATA.TXT");
        let mut content = String::new();
        for event in stream_events {
            if let Some(line) = render_tagdata_line(&event.raw_frame) {
                content.push_str(&line);
            }
        }
        std::fs::write(&file_path, content)?;
        written.push(file_path);
    }

    Ok(written)
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
    fn test_render_merged_and_separate_files() {
        let events = vec![
            ReceivedEvent {
                stream_id: "stream-a".into(),
                seq: 1,
                epoch: 1,
                raw_frame: b"aa00058000123b3200012606251750411376\r\n".to_vec(),
                read_kind: "tag".into(),
                reader_timestamp: None,
                received_unix_ms: 100,
                dbf_delivered_unix_ms: None,
            },
            ReceivedEvent {
                stream_id: "stream-b".into(),
                seq: 1,
                epoch: 1,
                raw_frame: b"aa00058000999b9900012606251750419999\r\n".to_vec(),
                read_kind: "tag".into(),
                reader_timestamp: None,
                received_unix_ms: 200,
                dbf_delivered_unix_ms: None,
            },
            ReceivedEvent {
                stream_id: "stream-a".into(),
                seq: 2,
                epoch: 1,
                raw_frame: b"ab010203\r\n".to_vec(),
                read_kind: "control".into(),
                reader_timestamp: None,
                received_unix_ms: 300,
                dbf_delivered_unix_ms: None,
            },
        ];

        let merged = render_merged_tagdata(&events);
        assert_eq!(
            merged,
            "aa00058000123b3200012606251750411376\r\naa00058000999b9900012606251750419999\r\n"
        );

        let zip_bytes = render_separate_tagdata_zip(&events).unwrap();
        assert!(!zip_bytes.is_empty());

        let temp_dir = tempfile::tempdir().unwrap();
        let written = write_separate_tagdata_files(temp_dir.path(), &events).unwrap();
        assert_eq!(written.len(), 2);
        assert!(temp_dir.path().join("stream-a/TAGDATA.TXT").exists());
        assert!(temp_dir.path().join("stream-b/TAGDATA.TXT").exists());
    }
}

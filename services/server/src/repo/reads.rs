use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReadRow {
    pub stream_id: Uuid,
    pub seq: i64,
    pub reader_timestamp: Option<String>,
    pub tag_id: Option<String>,
    pub received_at: chrono::DateTime<chrono::Utc>,
    pub bib: Option<i32>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupMode {
    None,
    First,
    Last,
}

impl DedupMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "first" => Some(Self::First),
            "last" => Some(Self::Last),
            _ => None,
        }
    }
}

/// Fetch reads for a single stream in its current epoch.
/// If race_id is provided, enriches with participant data via chip/participant joins.
pub async fn fetch_stream_reads(
    pool: &PgPool,
    stream_id: Uuid,
    race_id: Option<Uuid>,
) -> Result<Vec<ReadRow>, sqlx::Error> {
    let rows = match race_id {
        Some(rid) => {
            sqlx::query(
                r#"SELECT e.stream_id, e.seq, e.reader_timestamp, e.tag_id, e.received_at,
                          c.bib, p.first_name, p.last_name
                   FROM events e
                   JOIN streams s ON s.stream_id = e.stream_id
                   LEFT JOIN chips c ON c.race_id = $2 AND c.chip_id = e.tag_id
                   LEFT JOIN participants p ON p.race_id = $2 AND p.bib = c.bib
                   WHERE e.stream_id = $1 AND e.stream_epoch = s.stream_epoch
                   ORDER BY e.received_at ASC"#,
            )
            .bind(stream_id)
            .bind(rid)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query(
                r#"SELECT e.stream_id, e.seq, e.reader_timestamp, e.tag_id, e.received_at,
                          NULL::integer AS bib, NULL::text AS first_name, NULL::text AS last_name
                   FROM events e
                   JOIN streams s ON s.stream_id = e.stream_id
                   WHERE e.stream_id = $1 AND e.stream_epoch = s.stream_epoch
                   ORDER BY e.received_at ASC"#,
            )
            .bind(stream_id)
            .fetch_all(pool)
            .await?
        }
    };

    Ok(rows
        .into_iter()
        .map(|r| ReadRow {
            stream_id: r.get("stream_id"),
            seq: r.get("seq"),
            reader_timestamp: r.get("reader_timestamp"),
            tag_id: r.get("tag_id"),
            received_at: r.get("received_at"),
            bib: r.get("bib"),
            first_name: r.get("first_name"),
            last_name: r.get("last_name"),
        })
        .collect())
}

/// Fetch reads across all streams for a forwarder in their current epochs.
pub async fn fetch_forwarder_reads(
    pool: &PgPool,
    forwarder_id: &str,
    race_id: Option<Uuid>,
) -> Result<Vec<ReadRow>, sqlx::Error> {
    let rows = match race_id {
        Some(rid) => {
            sqlx::query(
                r#"SELECT e.stream_id, e.seq, e.reader_timestamp, e.tag_id, e.received_at,
                          c.bib, p.first_name, p.last_name
                   FROM events e
                   JOIN streams s ON s.stream_id = e.stream_id
                   LEFT JOIN chips c ON c.race_id = $2 AND c.chip_id = e.tag_id
                   LEFT JOIN participants p ON p.race_id = $2 AND p.bib = c.bib
                   WHERE s.forwarder_id = $1 AND e.stream_epoch = s.stream_epoch
                   ORDER BY e.received_at ASC"#,
            )
            .bind(forwarder_id)
            .bind(rid)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query(
                r#"SELECT e.stream_id, e.seq, e.reader_timestamp, e.tag_id, e.received_at,
                          NULL::integer AS bib, NULL::text AS first_name, NULL::text AS last_name
                   FROM events e
                   JOIN streams s ON s.stream_id = e.stream_id
                   WHERE s.forwarder_id = $1 AND e.stream_epoch = s.stream_epoch
                   ORDER BY e.received_at ASC"#,
            )
            .bind(forwarder_id)
            .fetch_all(pool)
            .await?
        }
    };

    Ok(rows
        .into_iter()
        .map(|r| ReadRow {
            stream_id: r.get("stream_id"),
            seq: r.get("seq"),
            reader_timestamp: r.get("reader_timestamp"),
            tag_id: r.get("tag_id"),
            received_at: r.get("received_at"),
            bib: r.get("bib"),
            first_name: r.get("first_name"),
            last_name: r.get("last_name"),
        })
        .collect())
}

/// Apply per-chip deduplication with anchored time windows.
///
/// - Reads without a tag_id pass through unaffected.
/// - For each chip (tag_id), reads are processed in received_at order:
///   - **First**: Emit the first read, skip subsequent reads within `window_secs`
///     of that anchor. When a read falls outside the window, emit it as a new anchor.
///   - **Last**: Track the last read within each window. When a read falls outside
///     the window, emit the accumulated last read from the previous window and start
///     a new window anchored at the current read.
pub fn apply_dedup(reads: Vec<ReadRow>, mode: DedupMode, window_secs: u64) -> Vec<ReadRow> {
    if mode == DedupMode::None {
        return reads;
    }

    use std::collections::HashMap;

    let mut by_chip: HashMap<String, Vec<ReadRow>> = HashMap::new();
    let mut no_tag: Vec<ReadRow> = Vec::new();

    for read in reads {
        match &read.tag_id {
            Some(tag) => by_chip.entry(tag.clone()).or_default().push(read),
            None => no_tag.push(read),
        }
    }

    let mut result: Vec<ReadRow> = no_tag;
    let window = chrono::Duration::seconds(window_secs as i64);

    for (_tag, chip_reads) in by_chip {
        match mode {
            DedupMode::First => {
                let mut anchor: Option<chrono::DateTime<chrono::Utc>> = None;
                for read in chip_reads {
                    match anchor {
                        None => {
                            anchor = Some(read.received_at);
                            result.push(read);
                        }
                        Some(a) => {
                            if read.received_at - a >= window {
                                anchor = Some(read.received_at);
                                result.push(read);
                            }
                        }
                    }
                }
            }
            DedupMode::Last => {
                let mut anchor: Option<chrono::DateTime<chrono::Utc>> = None;
                let mut current_last: Option<ReadRow> = None;
                for read in chip_reads {
                    match anchor {
                        None => {
                            anchor = Some(read.received_at);
                            current_last = Some(read);
                        }
                        Some(a) => {
                            if read.received_at - a >= window {
                                if let Some(last) = current_last.take() {
                                    result.push(last);
                                }
                                anchor = Some(read.received_at);
                                current_last = Some(read);
                            } else {
                                current_last = Some(read);
                            }
                        }
                    }
                }
                if let Some(last) = current_last {
                    result.push(last);
                }
            }
            DedupMode::None => unreachable!(),
        }
    }

    result.sort_by_key(|r| r.received_at);
    result
}

/// Apply limit+offset pagination.
pub fn paginate(reads: Vec<ReadRow>, limit: usize, offset: usize) -> (Vec<ReadRow>, usize) {
    let total = reads.len();
    let page = reads.into_iter().skip(offset).take(limit).collect();
    (page, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn make_read(tag: Option<&str>, secs: i64) -> ReadRow {
        ReadRow {
            stream_id: Uuid::nil(),
            seq: secs,
            reader_timestamp: None,
            tag_id: tag.map(|s| s.to_owned()),
            received_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
                + chrono::Duration::seconds(secs),
            bib: None,
            first_name: None,
            last_name: None,
        }
    }

    #[test]
    fn dedup_none_returns_all() {
        let reads = vec![make_read(Some("A"), 0), make_read(Some("A"), 1)];
        let result = apply_dedup(reads.clone(), DedupMode::None, 5);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn dedup_first_basic() {
        // Chip A reads at 0s, 3s, 8s with 5s window
        let reads = vec![
            make_read(Some("A"), 0),
            make_read(Some("A"), 3),
            make_read(Some("A"), 8),
        ];
        let result = apply_dedup(reads, DedupMode::First, 5);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].seq, 0); // anchor at 0
        assert_eq!(result[1].seq, 8); // 8 - 0 >= 5, new anchor
    }

    #[test]
    fn dedup_last_basic() {
        // Chip A reads at 0s, 3s, 8s with 5s window
        let reads = vec![
            make_read(Some("A"), 0),
            make_read(Some("A"), 3),
            make_read(Some("A"), 8),
        ];
        let result = apply_dedup(reads, DedupMode::Last, 5);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].seq, 3); // last in window [0,5)
        assert_eq!(result[1].seq, 8); // last (and only) in window [8,13)
    }

    #[test]
    fn dedup_multiple_chips_independent() {
        let reads = vec![
            make_read(Some("A"), 0),
            make_read(Some("B"), 1),
            make_read(Some("A"), 2),
            make_read(Some("B"), 3),
        ];
        let result = apply_dedup(reads, DedupMode::First, 5);
        // A: keep 0, skip 2; B: keep 1, skip 3
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn dedup_no_tag_passes_through() {
        let reads = vec![
            make_read(None, 0),
            make_read(Some("A"), 1),
            make_read(None, 2),
            make_read(Some("A"), 3),
        ];
        let result = apply_dedup(reads, DedupMode::First, 5);
        // 2 no-tag reads + 1 chip A read = 3
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn dedup_empty_returns_empty() {
        let result = apply_dedup(vec![], DedupMode::First, 5);
        assert!(result.is_empty());
    }

    #[test]
    fn dedup_single_read() {
        let reads = vec![make_read(Some("A"), 0)];
        let first = apply_dedup(reads.clone(), DedupMode::First, 5);
        assert_eq!(first.len(), 1);
        let last = apply_dedup(reads, DedupMode::Last, 5);
        assert_eq!(last.len(), 1);
    }

    #[test]
    fn paginate_basic() {
        let reads: Vec<ReadRow> = (0..10).map(|i| make_read(Some("A"), i)).collect();
        let (page, total) = paginate(reads, 3, 2);
        assert_eq!(total, 10);
        assert_eq!(page.len(), 3);
        assert_eq!(page[0].seq, 2);
    }
}

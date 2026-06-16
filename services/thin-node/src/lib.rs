pub mod announcer;
pub mod db;

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use chrono::{TimeZone, Utc};

    use crate::announcer::{AnnouncerInputEvent, compare_input_events};

    fn event(stream_id: &str, seq: u64, received_unix_ms: i64) -> AnnouncerInputEvent {
        AnnouncerInputEvent {
            stream_id: stream_id.to_owned(),
            seq,
            chip_id: format!("chip-{stream_id}-{seq}"),
            bib: Some(101),
            display_name: "Runner".to_owned(),
            reader_timestamp: Some("10:00:00".to_owned()),
            received_at: Utc.timestamp_millis_opt(received_unix_ms).unwrap(),
        }
    }

    #[test]
    fn announcer_orders_by_received_then_stream_then_seq() {
        let earlier = event("stream-z", 99, 999);
        let stream_b = event("stream-b", 1, 1_000);
        let stream_a_seq_7 = event("stream-a", 7, 1_000);
        let stream_a_seq_2 = event("stream-a", 2, 1_000);

        assert_eq!(compare_input_events(&earlier, &stream_b), Ordering::Less);
        assert_eq!(
            compare_input_events(&stream_a_seq_7, &stream_b),
            Ordering::Less
        );
        assert_eq!(
            compare_input_events(&stream_a_seq_2, &stream_a_seq_7),
            Ordering::Less
        );

        let mut events = vec![stream_b, stream_a_seq_7, earlier, stream_a_seq_2];
        events.sort_by(compare_input_events);

        let ordered_keys: Vec<_> = events
            .into_iter()
            .map(|event| {
                (
                    event.received_at.timestamp_millis(),
                    event.stream_id,
                    event.seq,
                )
            })
            .collect();
        assert_eq!(
            ordered_keys,
            vec![
                (999, "stream-z".to_owned(), 99),
                (1_000, "stream-a".to_owned(), 2),
                (1_000, "stream-a".to_owned(), 7),
                (1_000, "stream-b".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn db_migrates() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("thin-node.sqlite3");

        let conn = crate::db::open(&db_path).unwrap();

        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 1);

        let create_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'announcer_rows'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(create_sql.contains("stream_id TEXT NOT NULL"));
        assert!(create_sql.contains("seq INTEGER NOT NULL"));
        assert!(create_sql.contains("PRIMARY KEY(stream_id, seq)"));
    }
}

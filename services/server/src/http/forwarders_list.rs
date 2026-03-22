use crate::state::AppState;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;

#[derive(Serialize)]
struct ReaderEntry {
    reader_ip: String,
    connected: bool,
}

#[derive(Serialize)]
struct ForwarderListEntry {
    forwarder_id: String,
    display_name: Option<String>,
    online: bool,
    readers: Vec<ReaderEntry>,
    unique_chips: i64,
    total_reads: i64,
    last_read_at: Option<String>,
}

#[derive(Serialize)]
struct ForwardersListResponse {
    forwarders: Vec<ForwarderListEntry>,
}

/// GET /api/v1/forwarders — returns all known forwarders with reader connection
/// status, unique chip counts, total reads, and last-read timestamp.
/// Stats are scoped to each stream's current epoch.
pub async fn list_forwarders(State(state): State<AppState>) -> impl IntoResponse {
    let rows = match sqlx::query(
        r#"SELECT s.forwarder_id,
                  s.forwarder_display_name,
                  s.reader_ip,
                  s.online,
                  s.reader_connected,
                  s.stream_epoch
           FROM streams s
           ORDER BY s.forwarder_id, s.reader_ip"#,
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to list forwarders");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response();
        }
    };

    // Group streams by forwarder_id
    struct ForwarderInfo {
        display_name: Option<String>,
        online: bool,
        readers: Vec<ReaderEntry>,
    }

    let mut forwarders: HashMap<String, ForwarderInfo> = HashMap::new();

    for row in &rows {
        let forwarder_id: String = row.get("forwarder_id");
        let display_name: Option<String> = row.get("forwarder_display_name");
        let reader_ip: String = row.get("reader_ip");
        let online: bool = row.get("online");
        let reader_connected: bool = row.get("reader_connected");

        let entry = forwarders
            .entry(forwarder_id)
            .or_insert_with(|| ForwarderInfo {
                display_name: display_name.clone(),
                online: false,
                readers: Vec::new(),
            });

        if online {
            entry.online = true;
        }
        if display_name.is_some() && entry.display_name.is_none() {
            entry.display_name = display_name;
        }

        entry.readers.push(ReaderEntry {
            reader_ip,
            connected: reader_connected,
        });
    }

    // Fetch per-forwarder stats (distinct non-null tag_ids + total reads, scoped to each stream's current epoch)
    let stats_rows = match sqlx::query(
        r#"SELECT s.forwarder_id,
                  COUNT(DISTINCT e.tag_id) AS unique_chips,
                  COUNT(*) AS total_reads,
                  MAX(e.received_at) AS last_read_at
           FROM streams s
           JOIN events e ON e.stream_id = s.stream_id AND e.stream_epoch = s.stream_epoch
           GROUP BY s.forwarder_id"#,
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch forwarder stats");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "failed to load forwarder stats"})),
            )
                .into_response();
        }
    };

    let mut stats_map: HashMap<String, (i64, i64, Option<chrono::DateTime<chrono::Utc>>)> =
        HashMap::new();
    for row in &stats_rows {
        let fwd_id: String = row.get("forwarder_id");
        let unique_chips: i64 = row.get("unique_chips");
        let total_reads: i64 = row.get("total_reads");
        let last_read_at: Option<chrono::DateTime<chrono::Utc>> = row.get("last_read_at");
        stats_map.insert(fwd_id, (unique_chips, total_reads, last_read_at));
    }

    // Build response sorted by forwarder_id
    let mut result: Vec<ForwarderListEntry> = forwarders
        .into_iter()
        .map(|(fwd_id, info)| {
            let (unique_chips, total_reads, last_read_at) =
                stats_map.remove(&fwd_id).unwrap_or((0, 0, None));
            ForwarderListEntry {
                forwarder_id: fwd_id,
                display_name: info.display_name,
                online: info.online,
                readers: info.readers,
                unique_chips,
                total_reads,
                last_read_at: last_read_at.map(|t| t.to_rfc3339()),
            }
        })
        .collect();
    result.sort_by(|a, b| a.forwarder_id.cmp(&b.forwarder_id));

    (
        StatusCode::OK,
        Json(ForwardersListResponse { forwarders: result }),
    )
        .into_response()
}

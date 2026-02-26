use super::response::{HttpResult, internal_error, not_found};
use crate::state::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use sqlx::Row;
use uuid::Uuid;

async fn ensure_stream_exists(pool: &sqlx::PgPool, stream_id: Uuid) -> HttpResult {
    let exists = sqlx::query!(
        "SELECT 1 AS one FROM streams WHERE stream_id = $1",
        stream_id
    )
    .fetch_optional(pool)
    .await;

    match exists {
        Err(e) => Err(internal_error(e)),
        Ok(None) => Err(not_found("stream not found")),
        Ok(Some(_)) => Ok(()),
    }
}

/// `GET /api/v1/streams/{stream_id}/export.txt`
///
/// Streams canonical deduplicated events as bare `raw_frame` values,
/// one per line (`\n`-terminated), ordered by `(stream_epoch, seq)`.
pub async fn export_raw(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(response) = ensure_stream_exists(&state.pool, stream_id).await {
        return response;
    }

    let rows = sqlx::query(
        r#"SELECT raw_frame FROM events
           WHERE stream_id = $1
           ORDER BY stream_epoch ASC, seq ASC"#,
    )
    .bind(stream_id)
    .fetch_all(&state.pool)
    .await;

    match rows {
        Err(e) => internal_error(e),
        Ok(rows) => {
            let mut buf = String::new();
            for row in rows {
                let raw_frame: Vec<u8> = row.get("raw_frame");
                buf.push_str(&render_export_line(&raw_frame));
                buf.push('\n');
            }
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from(buf))
                .unwrap()
                .into_response()
        }
    }
}

/// `GET /api/v1/streams/{stream_id}/export.csv`
///
/// Streams canonical deduplicated events as CSV:
/// - Header: `stream_epoch,seq,reader_timestamp,raw_frame,read_type,chip_id`
/// - RFC 4180 quoting: fields containing comma, double-quote, or newline are
///   wrapped in double-quotes; embedded double-quotes are doubled.
/// - Ordered by `(stream_epoch, seq)`.
pub async fn export_csv(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(response) = ensure_stream_exists(&state.pool, stream_id).await {
        return response;
    }

    let rows = sqlx::query(
        r#"SELECT stream_epoch, seq, reader_timestamp, raw_frame, read_type, tag_id
           FROM events
           WHERE stream_id = $1
           ORDER BY stream_epoch ASC, seq ASC"#,
    )
    .bind(stream_id)
    .fetch_all(&state.pool)
    .await;

    match rows {
        Err(e) => internal_error(e),
        Ok(rows) => {
            let mut buf =
                String::from("stream_epoch,seq,reader_timestamp,raw_frame,read_type,chip_id\n");
            for row in rows {
                let epoch: i64 = row.get("stream_epoch");
                let seq: i64 = row.get("seq");
                let reader_timestamp: Option<String> = row.get("reader_timestamp");
                let raw_frame: Vec<u8> = row.get("raw_frame");
                let read_type: String = row.get("read_type");
                let chip_id: Option<String> = row.get("tag_id");

                let epoch = epoch.to_string();
                let seq = seq.to_string();
                let ts = reader_timestamp.as_deref().unwrap_or("");
                let line = render_export_line(&raw_frame);
                let chip_id = chip_id.as_deref().unwrap_or("");
                buf.push_str(&csv_field(&epoch));
                buf.push(',');
                buf.push_str(&csv_field(&seq));
                buf.push(',');
                buf.push_str(&csv_field(ts));
                buf.push(',');
                buf.push_str(&csv_field(&line));
                buf.push(',');
                buf.push_str(&csv_field(&read_type));
                buf.push(',');
                buf.push_str(&csv_field(chip_id));
                buf.push('\n');
            }
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
                .body(Body::from(buf))
                .unwrap()
                .into_response()
        }
    }
}

/// `GET /api/v1/streams/{stream_id}/epochs/{epoch}/export.csv`
///
/// Streams canonical deduplicated events for a single epoch as CSV:
/// - Header: `stream_epoch,seq,reader_timestamp,raw_frame,read_type,chip_id`
/// - RFC 4180 quoting (same as whole-stream export).
/// - Ordered by `seq`.
/// - Returns valid CSV with headers even when the epoch has zero reads.
pub async fn export_epoch_csv(
    State(state): State<AppState>,
    Path((stream_id, epoch)): Path<(Uuid, i64)>,
) -> impl IntoResponse {
    if let Err(response) = ensure_stream_exists(&state.pool, stream_id).await {
        return response;
    }

    let rows = sqlx::query(
        r#"SELECT stream_epoch, seq, reader_timestamp, raw_frame, read_type, tag_id
           FROM events
           WHERE stream_id = $1 AND stream_epoch = $2
           ORDER BY seq ASC"#,
    )
    .bind(stream_id)
    .bind(epoch)
    .fetch_all(&state.pool)
    .await;

    match rows {
        Err(e) => internal_error(e),
        Ok(rows) => {
            let mut buf =
                String::from("stream_epoch,seq,reader_timestamp,raw_frame,read_type,chip_id\n");
            for row in rows {
                let stream_epoch: i64 = row.get("stream_epoch");
                let seq: i64 = row.get("seq");
                let reader_timestamp: Option<String> = row.get("reader_timestamp");
                let raw_frame: Vec<u8> = row.get("raw_frame");
                let read_type: String = row.get("read_type");
                let chip_id: Option<String> = row.get("tag_id");

                let ep = stream_epoch.to_string();
                let seq = seq.to_string();
                let ts = reader_timestamp.as_deref().unwrap_or("");
                let line = render_export_line(&raw_frame);
                let chip_id = chip_id.as_deref().unwrap_or("");
                buf.push_str(&csv_field(&ep));
                buf.push(',');
                buf.push_str(&csv_field(&seq));
                buf.push(',');
                buf.push_str(&csv_field(ts));
                buf.push(',');
                buf.push_str(&csv_field(&line));
                buf.push(',');
                buf.push_str(&csv_field(&read_type));
                buf.push(',');
                buf.push_str(&csv_field(chip_id));
                buf.push('\n');
            }
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
                .body(Body::from(buf))
                .unwrap()
                .into_response()
        }
    }
}

/// `GET /api/v1/streams/{stream_id}/epochs/{epoch}/export.txt`
///
/// Streams canonical deduplicated events for a single epoch as bare
/// `raw_frame` values, one per line (`\n`-terminated), ordered by `seq`.
pub async fn export_epoch_raw(
    State(state): State<AppState>,
    Path((stream_id, epoch)): Path<(Uuid, i64)>,
) -> impl IntoResponse {
    if let Err(response) = ensure_stream_exists(&state.pool, stream_id).await {
        return response;
    }

    let rows = sqlx::query_scalar::<_, Vec<u8>>(
        r#"SELECT raw_frame FROM events
           WHERE stream_id = $1 AND stream_epoch = $2
           ORDER BY seq ASC"#,
    )
    .bind(stream_id)
    .bind(epoch)
    .fetch_all(&state.pool)
    .await;

    match rows {
        Err(e) => internal_error(e),
        Ok(raw_frames) => {
            let mut buf = String::new();
            for raw_frame in raw_frames {
                buf.push_str(&render_export_line(&raw_frame));
                buf.push('\n');
            }
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from(buf))
                .unwrap()
                .into_response()
        }
    }
}

fn render_export_line(raw_frame: &[u8]) -> String {
    let trimmed = raw_frame
        .strip_suffix(b"\r\n")
        .or_else(|| raw_frame.strip_suffix(b"\n"))
        .unwrap_or(raw_frame);
    String::from_utf8_lossy(trimmed).into_owned()
}

/// RFC 4180 CSV field quoting.
/// Wraps in double-quotes if the field contains comma, double-quote, or newline.
/// Doubles any embedded double-quotes.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_owned()
    }
}

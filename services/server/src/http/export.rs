use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rt_protocol::HttpErrorEnvelope;
use uuid::Uuid;
use crate::state::AppState;

/// `GET /api/v1/streams/{stream_id}/export.raw`
///
/// Streams canonical deduplicated events as bare `raw_read_line` values,
/// one per line (`\n`-terminated), ordered by `(stream_epoch, seq)`.
pub async fn export_raw(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify stream exists
    let exists = sqlx::query!(
        "SELECT 1 AS one FROM streams WHERE stream_id = $1",
        stream_id
    )
    .fetch_optional(&state.pool)
    .await;

    match exists {
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
            code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
        })).into_response(),
        Ok(None) => return (StatusCode::NOT_FOUND, Json(HttpErrorEnvelope {
            code: "NOT_FOUND".to_owned(), message: "stream not found".to_owned(), details: None,
        })).into_response(),
        Ok(Some(_)) => {}
    }

    let rows = sqlx::query!(
        r#"SELECT raw_read_line FROM events
           WHERE stream_id = $1
           ORDER BY stream_epoch ASC, seq ASC"#,
        stream_id
    )
    .fetch_all(&state.pool)
    .await;

    match rows {
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
            code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
        })).into_response(),
        Ok(rows) => {
            let mut buf = String::new();
            for row in &rows {
                buf.push_str(&row.raw_read_line);
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
/// - Header: `stream_epoch,seq,reader_timestamp,raw_read_line,read_type`
/// - RFC 4180 quoting: fields containing comma, double-quote, or newline are
///   wrapped in double-quotes; embedded double-quotes are doubled.
/// - Ordered by `(stream_epoch, seq)`.
pub async fn export_csv(
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify stream exists
    let exists = sqlx::query!(
        "SELECT 1 AS one FROM streams WHERE stream_id = $1",
        stream_id
    )
    .fetch_optional(&state.pool)
    .await;

    match exists {
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
            code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
        })).into_response(),
        Ok(None) => return (StatusCode::NOT_FOUND, Json(HttpErrorEnvelope {
            code: "NOT_FOUND".to_owned(), message: "stream not found".to_owned(), details: None,
        })).into_response(),
        Ok(Some(_)) => {}
    }

    let rows = sqlx::query!(
        r#"SELECT stream_epoch, seq, reader_timestamp, raw_read_line, read_type
           FROM events
           WHERE stream_id = $1
           ORDER BY stream_epoch ASC, seq ASC"#,
        stream_id
    )
    .fetch_all(&state.pool)
    .await;

    match rows {
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(HttpErrorEnvelope {
            code: "INTERNAL_ERROR".to_owned(), message: e.to_string(), details: None,
        })).into_response(),
        Ok(rows) => {
            let mut buf = String::from("stream_epoch,seq,reader_timestamp,raw_read_line,read_type\n");
            for row in &rows {
                let epoch = row.stream_epoch.to_string();
                let seq = row.seq.to_string();
                let ts = row.reader_timestamp.as_deref().unwrap_or("");
                let line = &row.raw_read_line;
                let read_type = &row.read_type;
                buf.push_str(&csv_field(&epoch));
                buf.push(',');
                buf.push_str(&csv_field(&seq));
                buf.push(',');
                buf.push_str(&csv_field(ts));
                buf.push(',');
                buf.push_str(&csv_field(line));
                buf.push(',');
                buf.push_str(&csv_field(read_type));
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

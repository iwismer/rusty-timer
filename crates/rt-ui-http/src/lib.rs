#[cfg(feature = "embed-ui")]
use axum::http::header;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
#[cfg(feature = "embed-ui")]
use std::path::Path;

fn is_blocked_path(raw_path: &str, blocked_prefix: &str) -> bool {
    raw_path == blocked_prefix
        || raw_path
            .strip_prefix(blocked_prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub fn validate_ui_request<'a>(
    method: &Method,
    uri: &'a Uri,
    blocked_prefixes: &[&str],
) -> Result<&'a str, Response> {
    let raw_path = uri.path();

    if blocked_prefixes
        .iter()
        .any(|prefix| is_blocked_path(raw_path, prefix))
    {
        return Err(StatusCode::NOT_FOUND.into_response());
    }

    if *method != Method::GET && *method != Method::HEAD {
        return Err(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }

    Ok(raw_path)
}

#[cfg(feature = "embed-ui")]
pub fn serve_embedded_ui<T: rust_embed::Embed>(raw_path: &str) -> Response {
    let path = raw_path.trim_start_matches('/');

    if let Some(file) = T::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref())],
            file.data,
        )
            .into_response();
    }

    if Path::new(path).extension().is_none() {
        if let Some(index) = T::get("index.html") {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html")],
                index.data,
            )
                .into_response();
        }
    }

    StatusCode::NOT_FOUND.into_response()
}

pub fn non_embedded_placeholder(app_name: &str) -> Response {
    Html(format!(
        "<html><body>\
         <h1>{app_name} UI not embedded</h1>\
         <p>Rebuild with <code>cargo build --features embed-ui</code> to include the web UI.</p>\
         </body></html>"
    ))
    .into_response()
}

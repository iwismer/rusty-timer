#[cfg(feature = "embed-ui")]
use axum::http::header;
use axum::http::{Method, StatusCode, Uri};
#[cfg(not(feature = "embed-ui"))]
use axum::response::Html;
use axum::response::{IntoResponse, Response};
#[cfg(feature = "embed-ui")]
use std::path::Path;

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::Embed)]
#[folder = "../../apps/receiver-ui/build"]
struct UiAssets;

/// Axum fallback handler that serves the embedded UI assets.
///
/// When `embed-ui` is enabled, serves files from the embedded SvelteKit build.
/// Unknown paths fall back to `index.html` for client-side routing.
///
/// When `embed-ui` is disabled, returns a placeholder page.
pub async fn serve_ui(method: Method, uri: Uri) -> Response {
    let raw_path = uri.path();

    // Preserve API semantics for unknown API endpoints.
    if raw_path.starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    // UI routes are read-only.
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    let path = raw_path.trim_start_matches('/');

    #[cfg(feature = "embed-ui")]
    {
        // Try the exact path first.
        if let Some(file) = UiAssets::get(path) {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                file.data,
            )
                .into_response();
        }

        // SPA fallback: serve index.html for route-like paths only.
        if Path::new(path).extension().is_none() {
            if let Some(index) = UiAssets::get("index.html") {
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

    #[cfg(not(feature = "embed-ui"))]
    {
        let _ = path; // suppress unused warning
        Html(
            "<html><body>\
             <h1>Receiver UI not embedded</h1>\
             <p>Rebuild with <code>cargo build --features embed-ui</code> to include the web UI.</p>\
             </body></html>",
        )
        .into_response()
    }
}

use axum::http::{Method, Uri};
use axum::response::{IntoResponse, Response};

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::Embed)]
#[folder = "../../apps/server-ui/build"]
struct UiAssets;

/// Axum fallback handler that serves the embedded server UI assets.
///
/// When `embed-ui` is enabled, serves files from the embedded `SvelteKit` build.
/// Unknown paths fall back to `index.html` for client-side routing.
///
/// When `embed-ui` is disabled, returns a placeholder page.
pub async fn serve_ui(method: Method, uri: Uri) -> Response {
    let raw_path = match rt_ui_http::validate_ui_request(
        &method,
        &uri,
        // Exact API paths (and their slash-delimited children). Blanket
        // "/admin" or "/announcer" entries would 404 real server-ui pages
        // under those paths, so block only the real API surface.
        &[
            "/status",
            "/healthz",
            "/register",
            "/admin/devices/approve",
            "/admin/enrollment-tokens",
            "/forwarder",
            "/forwarders",
            "/announcer/rows",
            "/announcer/takeover",
            "/allowlist",
        ],
    ) {
        Ok(path) => path,
        Err(error) => return error.into_response(),
    };

    #[cfg(feature = "embed-ui")]
    {
        rt_ui_http::serve_embedded_ui::<UiAssets>(raw_path)
    }

    #[cfg(not(feature = "embed-ui"))]
    {
        let _ = raw_path;
        rt_ui_http::non_embedded_placeholder("Server")
    }
}

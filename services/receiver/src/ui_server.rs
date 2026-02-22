use axum::http::{Method, Uri};
use axum::response::{IntoResponse, Response};

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
    let raw_path = match rt_ui_http::validate_ui_request(&method, &uri, &["/api"]) {
        Ok(path) => path,
        Err(error) => return error.into_response(),
    };

    #[cfg(feature = "embed-ui")]
    {
        rt_ui_http::serve_embedded_ui::<UiAssets>(raw_path)
    }

    #[cfg(not(feature = "embed-ui"))]
    {
        let _ = raw_path; // suppress unused warning
        rt_ui_http::non_embedded_placeholder("Receiver")
    }
}

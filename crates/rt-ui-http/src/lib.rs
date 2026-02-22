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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UiRequestError {
    NotFound,
    MethodNotAllowed,
}

impl IntoResponse for UiRequestError {
    fn into_response(self) -> Response {
        match self {
            UiRequestError::NotFound => StatusCode::NOT_FOUND.into_response(),
            UiRequestError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    }
}

pub fn validate_ui_request<'a>(
    method: &Method,
    uri: &'a Uri,
    blocked_prefixes: &[&str],
) -> Result<&'a str, UiRequestError> {
    let raw_path = uri.path();

    if blocked_prefixes
        .iter()
        .any(|prefix| is_blocked_path(raw_path, prefix))
    {
        return Err(UiRequestError::NotFound);
    }

    if *method != Method::GET && *method != Method::HEAD {
        return Err(UiRequestError::MethodNotAllowed);
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn validate_ui_request_accepts_get_and_head() {
        let get_uri = Uri::from_static("/dashboard");
        let head_uri = Uri::from_static("/healthz");

        assert_eq!(
            validate_ui_request(&Method::GET, &get_uri, &[]).unwrap(),
            "/dashboard"
        );
        assert_eq!(
            validate_ui_request(&Method::HEAD, &head_uri, &[]).unwrap(),
            "/healthz"
        );
    }

    #[test]
    fn validate_ui_request_rejects_non_get_head_methods() {
        let uri = Uri::from_static("/dashboard");
        let err = validate_ui_request(&Method::POST, &uri, &[]).unwrap_err();

        assert!(matches!(err, UiRequestError::MethodNotAllowed));
        assert_eq!(err.into_response().status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn validate_ui_request_rejects_blocked_prefixes() {
        let root = Uri::from_static("/api");
        let nested = Uri::from_static("/api/v1/readers");
        let not_blocked = Uri::from_static("/apiary");

        let root_err = validate_ui_request(&Method::GET, &root, &["/api"]).unwrap_err();
        assert!(matches!(root_err, UiRequestError::NotFound));
        assert_eq!(root_err.into_response().status(), StatusCode::NOT_FOUND);

        let nested_err = validate_ui_request(&Method::GET, &nested, &["/api"]).unwrap_err();
        assert!(matches!(nested_err, UiRequestError::NotFound));

        assert_eq!(
            validate_ui_request(&Method::GET, &not_blocked, &["/api"]).unwrap(),
            "/apiary"
        );
    }
}

pub mod cache;
pub mod control_api;
pub mod db;
pub mod local_proxy;
pub mod ports;
pub mod session;
pub mod sse;
pub mod ui_events;
pub mod ui_server;
pub use cache::{EventBus, StreamKey};
pub use db::{Db, DbError, DbResult, Profile, Subscription};
pub use ui_events::ReceiverUiEvent;

/// Converts `url` into a WebSocket client request with an
/// `Authorization: Bearer <token>` header.
///
/// Delegates to [`tungstenite::client::IntoClientRequest`] so that all
/// required WebSocket upgrade headers (`Sec-WebSocket-Key`, `Upgrade`,
/// `Connection`, `Sec-WebSocket-Version`) are populated automatically before
/// the `Authorization` header is injected.
pub fn build_authenticated_request(
    url: &str,
    token: &str,
) -> Result<
    tokio_tungstenite::tungstenite::http::Request<()>,
    Box<tokio_tungstenite::tungstenite::Error>,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header;
    url.into_client_request()
        .and_then(|mut r| {
            let hv = header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| {
                tokio_tungstenite::tungstenite::Error::Http(
                    tokio_tungstenite::tungstenite::http::Response::new(Some(e.to_string().into())),
                )
            })?;
            r.headers_mut().insert(header::AUTHORIZATION, hv);
            Ok(r)
        })
        .map_err(Box::new)
}

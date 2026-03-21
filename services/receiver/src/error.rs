#[derive(Debug, thiserror::Error, serde::Serialize)]
pub enum ReceiverError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("upstream error: {0}")]
    UpstreamError(String),
    #[error("internal error: {0}")]
    Internal(String),
}

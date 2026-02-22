use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rt_protocol::HttpErrorEnvelope;
use std::fmt::Display;

pub type HttpResponse = Response;
pub type HttpResult<T = ()> = Result<T, HttpResponse>;

pub(crate) fn json_error(
    status: StatusCode,
    code: impl Into<String>,
    message: impl Into<String>,
) -> HttpResponse {
    (
        status,
        Json(HttpErrorEnvelope {
            code: code.into(),
            message: message.into(),
            details: None,
        }),
    )
        .into_response()
}

pub fn internal_error(err: impl Display) -> HttpResponse {
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
        err.to_string(),
    )
}

pub fn bad_request(message: impl Into<String>) -> HttpResponse {
    json_error(StatusCode::BAD_REQUEST, "BAD_REQUEST", message)
}

pub fn not_found(message: impl Into<String>) -> HttpResponse {
    json_error(StatusCode::NOT_FOUND, "NOT_FOUND", message)
}

pub fn conflict(message: impl Into<String>) -> HttpResponse {
    json_error(StatusCode::CONFLICT, "CONFLICT", message)
}

pub fn gateway_timeout(message: impl Into<String>) -> HttpResponse {
    json_error(StatusCode::GATEWAY_TIMEOUT, "TIMEOUT", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn assert_error_response(
        response: Response,
        expected_status: StatusCode,
        expected_code: &str,
        expected_message: &str,
    ) {
        assert_eq!(response.status(), expected_status);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should be readable");
        let parsed: HttpErrorEnvelope =
            serde_json::from_slice(&body).expect("response body should be valid error json");

        assert_eq!(parsed.code, expected_code);
        assert_eq!(parsed.message, expected_message);
        assert_eq!(parsed.details, None);
    }

    #[tokio::test]
    async fn json_error_sets_status_code_message_and_no_details() {
        let response = json_error(
            StatusCode::BAD_GATEWAY,
            "UPSTREAM_ERROR",
            "upstream failure",
        );

        assert_error_response(
            response,
            StatusCode::BAD_GATEWAY,
            "UPSTREAM_ERROR",
            "upstream failure",
        )
        .await;
    }

    #[tokio::test]
    async fn internal_error_sets_internal_contract() {
        let response = internal_error("database unavailable");

        assert_error_response(
            response,
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "database unavailable",
        )
        .await;
    }

    #[tokio::test]
    async fn bad_request_sets_bad_request_contract() {
        let response = bad_request("invalid query");

        assert_error_response(
            response,
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
            "invalid query",
        )
        .await;
    }

    #[tokio::test]
    async fn not_found_sets_not_found_contract() {
        let response = not_found("stream missing");

        assert_error_response(
            response,
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "stream missing",
        )
        .await;
    }

    #[tokio::test]
    async fn conflict_sets_conflict_contract() {
        let response = conflict("duplicate token");

        assert_error_response(
            response,
            StatusCode::CONFLICT,
            "CONFLICT",
            "duplicate token",
        )
        .await;
    }

    #[tokio::test]
    async fn gateway_timeout_sets_timeout_contract() {
        let response = gateway_timeout("forwarder timed out");

        assert_error_response(
            response,
            StatusCode::GATEWAY_TIMEOUT,
            "TIMEOUT",
            "forwarder timed out",
        )
        .await;
    }
}

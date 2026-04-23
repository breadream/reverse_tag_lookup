use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("Failed to build HTTP client: {0}")]
    HttpClient(reqwest::Error),
    #[error("Request to LeetCode failed: {0}")]
    Request(reqwest::Error),
    #[error("Failed to access the local cache: {0}")]
    Io(std::io::Error),
    #[error("Failed to serialize the local cache: {0}")]
    Serialize(serde_json::Error),
    #[error("{0}")]
    Upstream(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::HttpClient(_)
            | AppError::Request(_)
            | AppError::Io(_)
            | AppError::Serialize(_)
            | AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
        };

        let body = Json(json!({
            "error": self.to_string(),
        }));

        (status, body).into_response()
    }
}

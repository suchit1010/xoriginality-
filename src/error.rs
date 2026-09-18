use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::fmt;

#[derive(Debug)]
pub enum AppError {
    /// Any outbound HTTP call (X API, search provider) failed.
    Http(String),
    /// Missing/invalid configuration.
    Config(String),
    /// The LLM judge call failed or returned something unparsable.
    Llm(String),
    NotFound(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Http(m) => write!(f, "upstream http error: {m}"),
            AppError::Config(m) => write!(f, "config error: {m}"),
            AppError::Llm(m) => write!(f, "llm judge error: {m}"),
            AppError::NotFound(m) => write!(f, "not found: {m}"),
        }
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            AppError::Http(m) => (StatusCode::BAD_GATEWAY, m.clone()),
            AppError::Config(m) => (StatusCode::INTERNAL_SERVER_ERROR, m.clone()),
            AppError::Llm(m) => (StatusCode::BAD_GATEWAY, m.clone()),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

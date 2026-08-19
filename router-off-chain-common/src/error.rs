//! Shared HTTP error response types.
//!
//! These types are used across both the API server and the metrics exporter
//! to produce consistent JSON error payloads.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

// ── Generic error response ────────────────────────────────────────────────────

/// A generic JSON error response body.
///
/// Serialised as `{"error": "<message>"}`.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

impl ErrorResponse {
    /// Create a new [`ErrorResponse`] with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
        }
    }
}

// ── Validation error ──────────────────────────────────────────────────────────

/// A structured validation error returned as JSON with HTTP 422.
///
/// Serialised as `{"error": "validation_error", "message": "<detail>"}`.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationError {
    pub error: &'static str,
    pub message: String,
}

impl ValidationError {
    /// Create a new [`ValidationError`] with the given detail message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: "validation_error",
            message: message.into(),
        }
    }
}

impl IntoResponse for ValidationError {
    fn into_response(self) -> Response {
        (StatusCode::UNPROCESSABLE_ENTITY, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_response_contains_message() {
        let e = ErrorResponse::new("something went wrong");
        assert_eq!(e.error, "something went wrong");
    }

    #[test]
    fn validation_error_has_fixed_kind() {
        let e = ValidationError::new("field is required");
        assert_eq!(e.error, "validation_error");
        assert_eq!(e.message, "field is required");
    }
}

//! HTTP error handling for dev server
//!
//! Provides consistent error responses that match Tauri's CommandError structure.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};

/// HTTP error response matching Tauri's CommandError structure
///
/// This ensures consistent error handling between Tauri IPC and HTTP modes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpError {
    /// User-facing error message
    pub message: String,
    /// Machine-readable error code
    pub code: String,
    /// Optional detailed error information for debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl HttpError {
    /// Create a new HTTP error
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            details: None,
        }
    }

    /// Create a new HTTP error with details
    pub fn with_details(
        message: impl Into<String>,
        code: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            details: Some(details.into()),
        }
    }

    /// Convert from anyhow::Error
    pub fn from_anyhow(err: anyhow::Error, code: impl Into<String>) -> Self {
        Self {
            message: err.to_string(),
            code: code.into(),
            details: Some(format!("{:?}", err)),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        // TODO: Consider refactoring to enum-based error codes when types grow beyond 10
        let status = match self.code.as_str() {
            "NODE_NOT_FOUND" | "RESOURCE_NOT_FOUND" => StatusCode::NOT_FOUND,
            "INVALID_INPUT" | "INVALID_NODE_TYPE" | "VALIDATION_ERROR" => StatusCode::BAD_REQUEST,
            "NOT_IMPLEMENTED" => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status, Json(self)).into_response()
    }
}

// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! API errors: a status code plus a message for the user, as JSON
//! `{"error": "...", "code": "..."}` (`code` only where the client acts on
//! it). Internal details are logged, never sent.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// An error answered to an API request.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    code: Option<&'static str>,
}

impl ApiError {
    /// An error with a status and a user-facing message.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
        }
    }

    /// Adds a machine-readable code.
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    /// The machine-readable code, if any.
    pub fn code(&self) -> Option<&'static str> {
        self.code
    }

    /// 400
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// 401: not logged in (or the session expired)
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "not logged in")
    }

    /// 403
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// 404
    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} not found"))
    }

    /// 409
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    /// 429
    pub fn too_many_requests() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many attempts, please wait a few minutes",
        )
    }

    /// 500. The message is logged; the client gets a generic one.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// The HTTP status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let message = if self.status.is_server_error() {
            tracing::error!(status = %self.status, error = %self.message, "request failed");
            "internal server error".to_owned()
        } else {
            self.message
        };
        let body = match self.code {
            Some(code) => serde_json::json!({ "error": message, "code": code }),
            None => serde_json::json!({ "error": message }),
        };
        (self.status, Json(body)).into_response()
    }
}

/// An error with all its causes, `outer: inner: innermost` (Postgres keeps
/// the useful part, e.g. "password authentication failed", in the source).
pub fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

impl From<tokio_postgres::Error> for ApiError {
    fn from(e: tokio_postgres::Error) -> Self {
        Self::internal(format!("database: {}", chain(&e)))
    }
}

impl From<deadpool_postgres::PoolError> for ApiError {
    fn from(e: deadpool_postgres::PoolError) -> Self {
        Self::internal(format!("database pool: {}", chain(&e)))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        Self::internal(format!("I/O: {e}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self::internal(format!("JSON: {e}"))
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::internal(format!("task: {e}"))
    }
}

/// Result of an API handler.
pub type ApiResult<T> = Result<T, ApiError>;

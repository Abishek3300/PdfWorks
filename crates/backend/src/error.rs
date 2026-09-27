//! Structured backend error type and its HTTP surface.
//!
//! Every request-handling path returns a [`Result<_, ApiError>`] rather than
//! panicking (the crate forbids `unwrap`/`expect`/`panic` in non-test code).
//! [`ApiError`] carries a machine code, a user-facing message, and the HTTP
//! status it maps to. Failures surface the failed Tool and reason where the
//! spec requires it (Req 33.3, 49.1, 49.4, 49.6).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// A structured, non-panicking backend failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The request Origin is not on the CORS allowlist (Req 50.3).
    OriginNotAllowed,
    /// The request body exceeded the configured maximum (Req 42.4).
    BodyTooLarge {
        /// Configured maximum body size in bytes.
        max_bytes: u64,
    },
    /// A per-client rate limit was exceeded (Req 42.1-42.3).
    RateLimited,
    /// A Source_File failed validation (Req 33.1, 33.2, 39.x, 42.5, 49.2, 50.4).
    Validation(crate::validator::ValidationError),
    /// A URL failed the SSRF / scheme guards (Req 41).
    Fetch(crate::fetcher::FetchError),
    /// The presented Job_Token did not grant access, or the Job expired
    /// (Req 43.3, 43.4).
    AccessDenied,
    /// A directory listing of the File_Store was requested (Req 43.5).
    ListingForbidden,
    /// The referenced Job or file does not exist.
    NotFound,
    /// The Job failed during processing; carries the failed Tool + reason
    /// (Req 33.3, 49.1, 49.4, 49.5, 49.6).
    Processing {
        /// Human-readable tool identifier that failed.
        tool: String,
        /// Reason for the failure.
        reason: String,
    },
    /// The File_Store could not complete the Job (e.g. storage exhausted,
    /// Req 49.4), or another backing service was unavailable.
    Unavailable {
        /// Reason for unavailability.
        reason: String,
    },
    /// A malformed request (bad JSON, missing field, etc.).
    BadRequest {
        /// Reason the request was rejected.
        reason: String,
    },
}

impl ApiError {
    /// The HTTP status this error maps to.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::OriginNotAllowed => StatusCode::FORBIDDEN,
            ApiError::BodyTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Fetch(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::AccessDenied => StatusCode::FORBIDDEN,
            ApiError::ListingForbidden => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Processing { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::BadRequest { .. } => StatusCode::BAD_REQUEST,
        }
    }

    /// A short, stable machine code used by clients and the Security_Log.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::OriginNotAllowed => "origin_not_allowed",
            ApiError::BodyTooLarge { .. } => "body_too_large",
            ApiError::RateLimited => "rate_limited",
            ApiError::Validation(_) => "validation_failed",
            ApiError::Fetch(_) => "fetch_rejected",
            ApiError::AccessDenied => "access_denied",
            ApiError::ListingForbidden => "listing_forbidden",
            ApiError::NotFound => "not_found",
            ApiError::Processing { .. } => "processing_failed",
            ApiError::Unavailable { .. } => "unavailable",
            ApiError::BadRequest { .. } => "bad_request",
        }
    }

    /// The user-facing message. Never contains file content or PII (Req 44.4,
    /// 46.2).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            ApiError::OriginNotAllowed => {
                "the request origin is not permitted".to_string()
            }
            ApiError::BodyTooLarge { max_bytes } => {
                format!("the request body exceeds the maximum of {max_bytes} bytes")
            }
            ApiError::RateLimited => {
                "the rate limit was exceeded; retry later".to_string()
            }
            ApiError::Validation(e) => e.message(),
            ApiError::Fetch(e) => e.message(),
            ApiError::AccessDenied => {
                "access to this file is not permitted or has expired".to_string()
            }
            ApiError::ListingForbidden => {
                "directory listing is not permitted".to_string()
            }
            ApiError::NotFound => "the requested resource was not found".to_string(),
            ApiError::Processing { tool, reason } => {
                format!("the {tool} tool failed: {reason}")
            }
            ApiError::Unavailable { reason } => {
                format!("the job cannot be completed at this time: {reason}")
            }
            ApiError::BadRequest { reason } => reason.clone(),
        }
    }
}

/// JSON error body returned to clients.
#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            code: self.code(),
            message: self.message(),
        };
        (self.status(), Json(body)).into_response()
    }
}

impl From<crate::validator::ValidationError> for ApiError {
    fn from(e: crate::validator::ValidationError) -> Self {
        ApiError::Validation(e)
    }
}

impl From<crate::fetcher::FetchError> for ApiError {
    fn from(e: crate::fetcher::FetchError) -> Self {
        ApiError::Fetch(e)
    }
}

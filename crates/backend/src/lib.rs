//! Axum backend service for the PDF Tools Suite (Server_Side_Processing plane).
//!
//! This crate wires together the server-plane components the design describes
//! (design §7-§10), each in its own module:
//!
//! - [`gateway`] — Security_Gateway: security headers, CORS allowlist, body
//!   limit, HTTPS assertion (Req 38, 42.4, 50.3).
//! - [`ratelimit`] — per-client Rate_Limiter backed by counters (Req 42.1-42.3).
//! - [`validator`] — content-signature typing, size/batch, zip-bomb guards
//!   (Req 33, 39, 42.5, 50.4).
//! - [`fetcher`] — URL_Fetcher with SSRF guards (Req 41).
//! - [`store`] — encrypted File_Store addressed by Job_Token (Req 43, 44.1,
//!   50.2).
//! - [`retention`] — Retention_Service (Req 32.2, 32.3, 44.2, 44.3).
//! - [`sandbox`] — sandbox worker harness (Req 40).
//! - [`dispatch`] — native engine dispatch + error surfacing (Req 31.2, 33.3,
//!   49).
//! - [`convert`] — Server_Only Office ↔ PDF / HTML → PDF via headless
//!   LibreOffice, behind a trait (Req 13-16, 19-21, 49.5).
//! - [`ocr`] — Scan to PDF assembly + OCR text layer via OCRmyPDF, behind a
//!   trait (Req 9.2, 49.5).
//! - [`render`] — PDF/A conversion + page rendering via pdfium/CLI, behind a
//!   trait (Req 8.1, 22.1-22.3).
//! - [`queue`] — Job state + dispatch queue (Req 31.4).
//! - [`jobs`] — Job intake, Job_Token issuance, Content_Scanner wiring
//!   (Req 15, 39.5, 39.6, 43).
//! - [`pipeline`] — end-to-end worker wiring intake → queue → sandbox →
//!   (engine OR Server_Only converter) → encrypted store → download
//!   (Req 3.1, 31.2, 31.4).
//! - [`logging`] — Security_Log + anomaly recording (Req 44.4, 46).
//!
//! Request-handling paths never `unwrap`/`expect`/`panic`; they return an
//! [`error::ApiError`] that maps to the correct HTTP status.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod config;
pub mod convert;
pub mod dispatch;
pub mod error;
pub mod fetcher;
pub mod gateway;
pub mod jobs;
pub mod logging;
pub mod ocr;
pub mod pipeline;
pub mod queue;
pub mod ratelimit;
pub mod render;
pub mod retention;
pub mod sandbox;
pub mod store;
pub mod validator;

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::config::Config;
use crate::logging::{AnomalyConfig, SecurityLog};

/// Shared application state threaded through handlers.
pub struct AppState {
    /// Resolved configuration.
    pub config: Config,
    /// Security_Log sink (Req 46).
    pub security_log: SecurityLog,
}

impl AppState {
    /// Build state from a [`Config`], creating a Security_Log gated by the
    /// `SECURITY_LOG_OPERATOR_TOKEN` env var (Req 46.3).
    #[must_use]
    pub fn new(config: Config) -> Self {
        let operator_token =
            std::env::var("SECURITY_LOG_OPERATOR_TOKEN").unwrap_or_else(|_| "operator".to_string());
        Self {
            config,
            security_log: SecurityLog::new(operator_token, AnomalyConfig::default()),
        }
    }
}

/// Build the Axum [`Router`] with the Security_Gateway middleware applied
/// (Req 38, 42.4, 50.3).
#[must_use]
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_gateway_layer,
        ))
        .with_state(state)
}

/// Liveness probe (also confirms the native engine links).
async fn healthz() -> Json<serde_json::Value> {
    let _tool = pdf_engine::ToolId::Merge;
    Json(serde_json::json!({ "status": "ok" }))
}

/// The Security_Gateway middleware: enforces the CORS allowlist (Req 50.3),
/// then attaches the security headers (Req 38.2-38.6) to the response.
async fn security_gateway_layer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // Reject non-allowlisted cross-origin requests before doing any work
    // (Req 50.3).
    let origin = gateway::origin_of(&headers);
    let cors = match gateway::cors_headers(origin.as_deref(), &state.config.cors_allowlist) {
        Ok(h) => h,
        Err(status) => {
            state.security_log.record(logging::SecurityEvent {
                client_ref: SecurityLog::client_ref(origin.as_deref().unwrap_or("unknown")),
                kind: logging::SecurityEventKind::RequestRejected,
                reason: error::ApiError::OriginNotAllowed.code().to_string(),
            });
            return status.into_response();
        }
    };

    let mut response = next.run(request).await;

    // Attach security + CORS headers to the outgoing response.
    for (name, value) in gateway::security_headers() {
        response.headers_mut().insert(name, value);
    }
    for (name, value) in cors {
        response.headers_mut().insert(name, value);
    }
    response
}

/// Entry point used by the binary; kept in the library so it is unit-covered.
///
/// # Errors
///
/// Returns an error string if configuration is invalid or the listener cannot
/// bind.
pub async fn run(addr: std::net::SocketAddr) -> Result<(), String> {
    let config = Config::from_env()?;
    let state = Arc::new(AppState::new(config));
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("server error: {e}"))
}

/// Small helper so the binary can format a status without pulling axum types.
#[must_use]
pub fn status_ok() -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_builds() {
        let state = Arc::new(AppState::new(Config::for_tests()));
        let _router = build_router(state);
    }

    #[test]
    fn status_helper() {
        assert_eq!(status_ok(), StatusCode::OK);
    }
}

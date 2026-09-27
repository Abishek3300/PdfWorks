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

pub mod api;
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

use crate::api::Services;
use crate::config::{Config, MAX_REQUEST_BODY_BYTES};
use crate::logging::{AnomalyConfig, SecurityLog};
use crate::ratelimit::{InMemoryCounterStore, LimitedAction, RateLimiter};

/// Shared application state threaded through handlers.
pub struct AppState {
    /// Resolved configuration.
    pub config: Config,
    /// Security_Log sink (Req 46).
    pub security_log: SecurityLog,
    /// Runtime services (File_Store, queue, executors) — the wired pipeline.
    pub services: Services,
    /// Per-client Rate_Limiter for Job submissions + uploads (Req 42.1-42.3).
    pub rate_limiter: RateLimiter<InMemoryCounterStore>,
}

impl AppState {
    /// Build state from a [`Config`], creating a Security_Log gated by the
    /// `SECURITY_LOG_OPERATOR_TOKEN` env var (Req 46.3) and assembling the
    /// runtime services from the environment.
    #[must_use]
    pub fn new(config: Config) -> Self {
        let operator_token =
            std::env::var("SECURITY_LOG_OPERATOR_TOKEN").unwrap_or_else(|_| "operator".to_string());
        let services = Services::from_env(&config);
        let rate_limiter = RateLimiter::new(InMemoryCounterStore::new(), config.rate_limit);
        Self {
            config,
            security_log: SecurityLog::new(operator_token, AnomalyConfig::default()),
            services,
            rate_limiter,
        }
    }
}

/// Build the Axum [`Router`] with the API routes mounted behind the
/// Security_Gateway + Rate_Limiter middleware (Req 38, 42.1-42.4, 50.3).
#[must_use]
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .merge(api::api_routes())
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

    // Short-circuit a CORS preflight (Req 50.3). A browser sends
    // `OPTIONS` carrying `Access-Control-Request-Method` before the actual
    // cross-origin request; the router has no OPTIONS route, so letting it fall
    // through returns 405 without the Access-Control-Allow-* headers and the
    // browser blocks the real request. From an allowed origin we answer the
    // preflight directly with 204 and the already-built CORS headers (plus a
    // Max-Age so the browser caches the decision). A preflight from a
    // disallowed origin never reaches here — it was rejected with 403 above,
    // which correctly makes the browser block the request. Preflights carry no
    // body and must not be throttled, so this runs before the body-size check
    // and the rate limiter.
    if is_cors_preflight(&request, &headers) {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        // Attach the CORS headers plus the preflight cache duration and the
        // standard security headers (harmless on a 204).
        for (name, value) in gateway::security_headers() {
            resp.headers_mut().insert(name, value);
        }
        for (name, value) in &cors {
            resp.headers_mut().insert(name.clone(), value.clone());
        }
        resp.headers_mut().insert(
            axum::http::header::ACCESS_CONTROL_MAX_AGE,
            axum::http::header::HeaderValue::from_static("600"),
        );
        return resp;
    }

    // Enforce the maximum request body size from the declared Content-Length
    // before the body is read, bounding an oversized upload (Req 42.4). The
    // multipart extractor additionally bounds each streamed field.
    if let Some(len) = content_length(&headers) {
        if len > MAX_REQUEST_BODY_BYTES {
            state.security_log.record(logging::SecurityEvent {
                client_ref: SecurityLog::client_ref(origin.as_deref().unwrap_or("unknown")),
                kind: logging::SecurityEventKind::RequestRejected,
                reason: error::ApiError::BodyTooLarge {
                    max_bytes: MAX_REQUEST_BODY_BYTES,
                }
                .code()
                .to_string(),
            });
            let resp = error::ApiError::BodyTooLarge {
                max_bytes: MAX_REQUEST_BODY_BYTES,
            }
            .into_response();
            return with_cors(resp, &cors);
        }
    }

    // Rate-limit Job submissions per client (Req 42.1, 42.3). Only the intake
    // POST counts against the Job-submission budget; status/result reads are
    // not throttled here so a client can poll a running Job.
    if is_job_submission(&request) {
        let client = SecurityLog::client_ref(origin.as_deref().unwrap_or("unknown"));
        let now = now_ms();
        let allowed = state
            .rate_limiter
            .check(&client, LimitedAction::JobSubmission, now)
            .await
            .unwrap_or(true);
        if !allowed {
            state.security_log.record(logging::SecurityEvent {
                client_ref: client,
                kind: logging::SecurityEventKind::RequestRejected,
                reason: error::ApiError::RateLimited.code().to_string(),
            });
            return with_cors(error::ApiError::RateLimited.into_response(), &cors);
        }
    }

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

/// Attach CORS headers to an already-built (short-circuit) response so a
/// rejection still carries the allowlist echo the browser expects.
fn with_cors(
    mut resp: Response,
    cors: &[(axum::http::header::HeaderName, axum::http::header::HeaderValue)],
) -> Response {
    for (name, value) in gateway::security_headers() {
        resp.headers_mut().insert(name, value);
    }
    for (name, value) in cors {
        resp.headers_mut().insert(name.clone(), value.clone());
    }
    resp
}

/// The declared request body size from the `Content-Length` header, if any.
fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Whether a request is a Job submission (`POST /api/jobs`) for rate-limiting.
fn is_job_submission(request: &axum::extract::Request) -> bool {
    request.method() == axum::http::Method::POST && request.uri().path() == "/api/jobs"
}

/// Whether a request is a CORS preflight: an `OPTIONS` request carrying the
/// `Access-Control-Request-Method` header the browser adds before an actual
/// cross-origin request (Req 50.3).
fn is_cors_preflight(request: &axum::extract::Request, headers: &HeaderMap) -> bool {
    request.method() == axum::http::Method::OPTIONS
        && headers.contains_key(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD)
}

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    // Drain the dispatch queue on a background worker so the intake path never
    // blocks on processing (Req 31.4).
    api::spawn_worker(state.clone());
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

/// Minimal self-check backing the `--health-check` CLI flag and the Docker
/// HEALTHCHECK. Confirms the native engine links (a Job_Token can be issued
/// from the CSPRNG) without binding a socket or touching the network. Returns
/// `true` when the process is healthy.
#[must_use]
pub fn health_check() -> bool {
    // Issuing a token exercises the engine link + the platform entropy source;
    // a failure here means the runtime is not healthy.
    pdf_engine::generate_job_token().is_ok()
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

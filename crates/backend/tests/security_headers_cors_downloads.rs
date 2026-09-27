//! Task 22.2 — Integration tests for security headers, CORS, and downloads.
//!
//! These drive the real Axum [`build_router`] through
//! `tower::ServiceExt::oneshot` so the Security_Gateway middleware runs against
//! live responses. They assert the design's response-header contract and the
//! CORS allowlist behavior, plus the download presentation headers.
//!
//! Requirements: 38.1, 38.2, 38.3, 38.4, 38.5, 38.6, 38.7, 50.2, 50.3.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for `oneshot`

use backend::config::{Config, HSTS_MAX_AGE_SECS};
use backend::gateway;
use backend::store::DownloadHeaders;
use backend::{build_router, AppState};

/// Build the router with the test config (allowlist = https://app.example).
fn router() -> axum::Router {
    let state = Arc::new(AppState::new(Config::for_tests()));
    build_router(state)
}

/// A GET /healthz request, optionally carrying an Origin header.
fn request(origin: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri("/healthz").method("GET");
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    b.body(Body::empty()).expect("build request")
}

// -------------------------------------------------------------------------
// Security response headers (Req 38.2–38.6) on a live response.
// -------------------------------------------------------------------------

#[tokio::test]
async fn response_carries_all_security_headers() {
    let resp = router().oneshot(request(None)).await.expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers();

    // HSTS with max-age >= one year (Req 38.2).
    let hsts = h
        .get("strict-transport-security")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        hsts.contains(&format!("max-age={HSTS_MAX_AGE_SECS}")),
        "HSTS header missing/short: {hsts:?}"
    );

    // CSP present and WITHOUT inline script (Req 38.3).
    let csp = h
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(csp.contains("script-src 'self'"), "csp: {csp}");
    assert!(!csp.contains("unsafe-inline"), "csp must forbid inline script");
    assert!(csp.contains("frame-ancestors 'self'"), "csp: {csp}"); // Req 38.5

    // nosniff (Req 38.4).
    assert_eq!(
        h.get("x-content-type-options").and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    // Clickjacking protection (Req 38.5).
    assert_eq!(
        h.get("x-frame-options").and_then(|v| v.to_str().ok()),
        Some("DENY")
    );
    // Referrer policy (Req 38.6).
    assert_eq!(
        h.get("referrer-policy").and_then(|v| v.to_str().ok()),
        Some("strict-origin-when-cross-origin")
    );
}

// -------------------------------------------------------------------------
// CORS allowlist (Req 50.3).
// -------------------------------------------------------------------------

#[tokio::test]
async fn allowlisted_origin_is_echoed() {
    let resp = router()
        .oneshot(request(Some("https://app.example")))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("https://app.example")
    );
}

#[tokio::test]
async fn non_allowlisted_origin_is_rejected() {
    let resp = router()
        .oneshot(request(Some("https://evil.example")))
        .await
        .expect("response");
    // The gateway short-circuits a cross-origin request from a non-allowlisted
    // Origin with 403 (Req 50.3).
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(resp
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn same_origin_request_without_origin_is_allowed() {
    let resp = router().oneshot(request(None)).await.expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
}

/// A CORS preflight: `OPTIONS` with an Origin + `Access-Control-Request-Method`.
fn preflight(origin: &str) -> Request<Body> {
    Request::builder()
        .uri("/api/jobs")
        .method("OPTIONS")
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type, x-job-token")
        .body(Body::empty())
        .expect("build preflight request")
}

#[tokio::test]
async fn cors_preflight_from_allowed_origin_is_answered() {
    // The router has no OPTIONS route; the gateway must answer the preflight
    // itself with the Access-Control-Allow-* headers so the browser proceeds
    // with the actual POST /api/jobs (Req 50.3).
    let resp = router()
        .oneshot(preflight("https://app.example"))
        .await
        .expect("response");

    // 204 No Content (or 200) with the allow headers present.
    assert!(
        resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::OK,
        "preflight status: {}",
        resp.status()
    );
    let h = resp.headers();
    assert_eq!(
        h.get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("https://app.example")
    );
    let methods = h
        .get("access-control-allow-methods")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(methods.contains("POST"), "allow-methods: {methods}");
    let allow_headers = h
        .get("access-control-allow-headers")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        allow_headers.contains("content-type") && allow_headers.contains("x-job-token"),
        "allow-headers: {allow_headers}"
    );
    // Preflight caching hint is present.
    assert!(h.get("access-control-max-age").is_some());
}

#[tokio::test]
async fn cors_preflight_from_disallowed_origin_is_rejected() {
    // A preflight from a non-allowlisted Origin is rejected with 403 and no
    // allow header, which correctly makes the browser block the request.
    let resp = router()
        .oneshot(preflight("https://evil.example"))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(resp
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

// -------------------------------------------------------------------------
// Secure cookie attributes (Req 38.7).
// -------------------------------------------------------------------------

#[test]
fn issued_cookies_are_secure_httponly_samesite() {
    let cookie = gateway::secure_cookie("session", "abc123");
    assert!(cookie.contains("Secure"), "cookie: {cookie}");
    assert!(cookie.contains("HttpOnly"), "cookie: {cookie}");
    assert!(cookie.contains("SameSite=Strict"), "cookie: {cookie}");
}

// -------------------------------------------------------------------------
// TLS assertion posture (Req 38.1).
// -------------------------------------------------------------------------

#[test]
fn production_requires_https_forwarded_scheme() {
    let mut cfg = Config::for_tests();
    cfg.production = true;
    // TLS 1.2+ is terminated at the reverse proxy; the app asserts the
    // forwarded scheme is https in production (Req 38.1).
    assert!(gateway::assert_https(&cfg, Some("https")).is_ok());
    assert!(gateway::assert_https(&cfg, Some("http")).is_err());
    assert!(gateway::assert_https(&cfg, None).is_err());
}

// -------------------------------------------------------------------------
// Download presentation headers (Req 50.2).
// -------------------------------------------------------------------------

#[test]
fn downloads_force_attachment_and_non_executable_type() {
    // A normal PDF keeps its type but is served as an attachment (Req 50.2).
    let pdf = DownloadHeaders::for_download("report.pdf", "application/pdf");
    assert_eq!(
        pdf.content_disposition,
        "attachment; filename=\"report.pdf\""
    );
    assert_eq!(pdf.content_type, "application/pdf");

    // Executable/markup content types are coerced so the browser cannot run
    // them (Req 50.2): HTML, SVG, JavaScript.
    for (name, ct) in [
        ("x.html", "text/html"),
        ("x.svg", "image/svg+xml"),
        ("x.js", "application/javascript"),
        ("x.xhtml", "application/xhtml+xml"),
    ] {
        let h = DownloadHeaders::for_download(name, ct);
        assert!(
            h.content_disposition.starts_with("attachment;"),
            "{name} not attachment"
        );
        assert_eq!(
            h.content_type, "application/octet-stream",
            "{ct} should be coerced to octet-stream"
        );
    }
}

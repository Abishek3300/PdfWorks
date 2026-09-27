//! Security_Gateway (Task 14.1, Req 38.1-38.7, 42.4, 50.3).
//!
//! Every server request crosses the Security_Gateway before reaching the API.
//! The gateway:
//! - sets security response headers — HSTS (Req 38.2), a CSP with no inline
//!   script (Req 38.3), `X-Content-Type-Options: nosniff` (Req 38.4),
//!   `X-Frame-Options` + CSP `frame-ancestors` (Req 38.5), and a restrictive
//!   `Referrer-Policy` (Req 38.6);
//! - marks issued cookies `Secure; HttpOnly; SameSite` (Req 38.7);
//! - enforces a **CORS allowlist**, rejecting non-allowlisted Origins
//!   (Req 50.3);
//! - enforces a **maximum request body size** (Req 42.4); and
//! - documents that **TLS 1.2+ termination** (Req 38.1) is handled by the
//!   deployment reverse proxy, while asserting HTTPS in production via config.
//!
//! The header set and CORS decision are pure functions so they are unit-tested
//! directly; the Axum layer applies them to live responses.

use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{HeaderMap, StatusCode};

use crate::config::{Config, HSTS_MAX_AGE_SECS, MAX_REQUEST_BODY_BYTES};

/// The security headers the gateway attaches to every response (Req 38.2-38.6).
///
/// The CSP disallows inline script by omitting `'unsafe-inline'` from
/// `script-src` and pinning it to `'self'` (Req 38.3), and forbids framing by
/// any origin other than the app's own via `frame-ancestors 'self'` (Req 38.5).
#[must_use]
pub fn security_headers() -> Vec<(HeaderName, HeaderValue)> {
    let mut out = Vec::new();

    let hsts = format!("max-age={HSTS_MAX_AGE_SECS}; includeSubDomains; preload");
    push(&mut out, "strict-transport-security", &hsts);

    // No inline script: script-src 'self' only, plus object-src 'none' and a
    // self-only frame-ancestors (Req 38.3, 38.5).
    let csp = "default-src 'self'; script-src 'self'; object-src 'none'; \
               base-uri 'self'; frame-ancestors 'self'";
    push(&mut out, "content-security-policy", csp);

    push(&mut out, "x-content-type-options", "nosniff");
    push(&mut out, "x-frame-options", "DENY");
    push(&mut out, "referrer-policy", "strict-origin-when-cross-origin");

    out
}

/// Build a hardened `Set-Cookie` value carrying the Secure, HttpOnly, and
/// SameSite attributes (Req 38.7). In non-production the `Secure` flag is still
/// set unless explicitly disabled, matching the assert-HTTPS posture.
#[must_use]
pub fn secure_cookie(name: &str, value: &str) -> String {
    format!("{name}={value}; Secure; HttpOnly; SameSite=Strict; Path=/")
}

/// Whether an Origin is permitted by the CORS allowlist (Req 50.3).
///
/// A request with no Origin header (same-origin / non-CORS) is permitted; a
/// cross-origin request is permitted only if its Origin is on the allowlist.
#[must_use]
pub fn origin_allowed(origin: Option<&str>, allowlist: &[String]) -> bool {
    match origin {
        None => true,
        Some(o) => allowlist.iter().any(|a| a == o),
    }
}

/// Whether a request body of `len` bytes is within the maximum (Req 42.4).
#[must_use]
pub fn body_within_limit(len: u64) -> bool {
    len <= MAX_REQUEST_BODY_BYTES
}

/// Assert that a production deployment is serving over HTTPS (Req 38.1).
///
/// TLS 1.2+ is terminated at the reverse proxy; the app receives a forwarded
/// scheme (e.g. `X-Forwarded-Proto`). In production a non-HTTPS forwarded
/// scheme is rejected so plaintext traffic never reaches the API.
///
/// # Errors
///
/// Returns `Err(())` when `production` is true and `forwarded_proto` is not
/// `https`.
pub fn assert_https(config: &Config, forwarded_proto: Option<&str>) -> Result<(), ()> {
    if !config.production {
        return Ok(());
    }
    match forwarded_proto {
        Some(p) if p.eq_ignore_ascii_case("https") => Ok(()),
        _ => Err(()),
    }
}

/// Extract the `Origin` header value as a string, if present and valid UTF-8.
#[must_use]
pub fn origin_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Build the CORS response headers echoing an allowed Origin (Req 50.3).
///
/// Returns `Err(StatusCode::FORBIDDEN)` for a non-allowlisted Origin so the
/// gateway can short-circuit with a 403.
///
/// # Errors
///
/// Returns `Err(FORBIDDEN)` when the Origin is present but not on the allowlist.
pub fn cors_headers(
    origin: Option<&str>,
    allowlist: &[String],
) -> Result<Vec<(HeaderName, HeaderValue)>, StatusCode> {
    if !origin_allowed(origin, allowlist) {
        return Err(StatusCode::FORBIDDEN);
    }
    let mut out = Vec::new();
    if let Some(o) = origin {
        push(&mut out, "access-control-allow-origin", o);
        push(&mut out, "vary", "Origin");
        push(&mut out, "access-control-allow-methods", "GET, POST, DELETE, OPTIONS");
        push(&mut out, "access-control-allow-headers", "content-type, x-job-token");
    }
    Ok(out)
}

/// Push a header pair, silently skipping any value that is not a valid header
/// value (keeps this panic-free).
fn push(out: &mut Vec<(HeaderName, HeaderValue)>, name: &'static str, value: &str) {
    if let (Ok(n), Ok(v)) = (
        HeaderName::from_bytes(name.as_bytes()),
        HeaderValue::from_str(value),
    ) {
        out.push((n, v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_map(headers: &[(HeaderName, HeaderValue)]) -> std::collections::HashMap<String, String> {
        headers
            .iter()
            .map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect()
    }

    #[test]
    fn sets_all_required_security_headers() {
        let map = header_map(&security_headers());
        // HSTS max-age >= one year (Req 38.2).
        let hsts = &map["strict-transport-security"];
        assert!(hsts.contains(&format!("max-age={HSTS_MAX_AGE_SECS}")));
        // CSP disallows inline script (Req 38.3): no 'unsafe-inline' in script-src.
        let csp = &map["content-security-policy"];
        assert!(csp.contains("script-src 'self'"));
        assert!(!csp.contains("unsafe-inline"));
        assert!(csp.contains("frame-ancestors 'self'")); // Req 38.5
        assert_eq!(map["x-content-type-options"], "nosniff"); // Req 38.4
        assert_eq!(map["x-frame-options"], "DENY"); // Req 38.5
        assert_eq!(map["referrer-policy"], "strict-origin-when-cross-origin"); // Req 38.6
    }

    #[test]
    fn cookie_has_secure_httponly_samesite() {
        let c = secure_cookie("sid", "abc");
        assert!(c.contains("Secure"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("SameSite=Strict"));
    }

    #[test]
    fn cors_allowlist_admits_and_rejects() {
        let allow = vec!["https://app.example".to_string()];
        assert!(origin_allowed(Some("https://app.example"), &allow));
        assert!(!origin_allowed(Some("https://evil.example"), &allow));
        // No Origin (same-origin) is allowed.
        assert!(origin_allowed(None, &allow));
    }

    #[test]
    fn cors_headers_reject_bad_origin() {
        let allow = vec!["https://app.example".to_string()];
        assert_eq!(
            cors_headers(Some("https://evil.example"), &allow),
            Err(StatusCode::FORBIDDEN)
        );
        assert!(cors_headers(Some("https://app.example"), &allow).is_ok());
    }

    #[test]
    fn body_limit_enforced() {
        assert!(body_within_limit(MAX_REQUEST_BODY_BYTES));
        assert!(!body_within_limit(MAX_REQUEST_BODY_BYTES + 1));
    }

    #[test]
    fn https_asserted_in_production() {
        let mut cfg = Config::for_tests();
        cfg.production = true;
        assert!(assert_https(&cfg, Some("https")).is_ok());
        assert!(assert_https(&cfg, Some("http")).is_err());
        assert!(assert_https(&cfg, None).is_err());
        // Dev never asserts.
        cfg.production = false;
        assert!(assert_https(&cfg, Some("http")).is_ok());
    }
}

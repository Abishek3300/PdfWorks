//! Task 22.5 — SSRF security suite for the URL_Fetcher.
//!
//! A consolidated, clearly-organized suite over the public [`backend::fetcher`]
//! policy surface. It complements the in-module property tests (Properties 17 &
//! 18) with the concrete cases the design's security-test section calls out:
//! scheme rejection, Private_IP_Range blocking on the initial address AND on
//! every redirect hop, the redirect cap, and the fetch timeout.
//!
//! Requirements: 41.1, 41.2, 41.3, 41.4, 41.5, 41.6.
//!
//! Network I/O is behind the `Resolver` + `HttpClient` traits so the
//! security-relevant guard logic is exercised deterministically without
//! touching the network.

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::IpAddr;

use backend::config::FetchConfig;
use backend::fetcher::{fetch, validate_url, FetchError, HttpClient, HttpResponse, Resolver};

/// A resolver mapping known hosts to fixed addresses; unknown hosts fail.
struct MapResolver(HashMap<String, Vec<IpAddr>>);
impl Resolver for MapResolver {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, FetchError> {
        self.0
            .get(host)
            .cloned()
            .ok_or(FetchError::ResolutionFailed)
    }
}

fn resolver_for(host: &str, addr: [u8; 4]) -> MapResolver {
    let mut m = HashMap::new();
    m.insert(host.to_string(), vec![IpAddr::from(addr)]);
    MapResolver(m)
}

/// A client that replays a fixed sequence of responses.
struct SeqClient {
    steps: RefCell<Vec<Result<HttpResponse, FetchError>>>,
}
impl SeqClient {
    fn new(steps: Vec<Result<HttpResponse, FetchError>>) -> Self {
        Self {
            steps: RefCell::new(steps),
        }
    }
}
impl HttpClient for SeqClient {
    fn request(&self, _url: &str) -> Result<HttpResponse, FetchError> {
        let mut steps = self.steps.borrow_mut();
        if steps.is_empty() {
            return Ok(HttpResponse::Body(vec![]));
        }
        steps.remove(0)
    }
}

// -------------------------------------------------------------------------
// Suite: scheme rejection (Req 41.1, 41.2).
// -------------------------------------------------------------------------

#[test]
fn suite_scheme_rejection() {
    let r = resolver_for("host", [93, 184, 216, 34]);
    // Non-http(s) schemes carrying a `scheme://authority` form are rejected
    // specifically as scheme-not-permitted (Req 41.2).
    for url in ["file:///etc/passwd", "ftp://host/x", "gopher://host/"] {
        assert_eq!(
            validate_url(url, &r),
            Err(FetchError::SchemeNotPermitted),
            "{url} should be rejected by scheme"
        );
    }
    // Opaque non-http(s) schemes without an authority (`data:`, `javascript:`)
    // have no `://` and so fail closed as malformed — still rejected, which is
    // the security-relevant outcome (Req 41.1).
    for url in ["data:text/html,hi", "javascript:alert(1)", "mailto:a@b.c"] {
        assert!(
            matches!(
                validate_url(url, &r),
                Err(FetchError::SchemeNotPermitted) | Err(FetchError::Malformed)
            ),
            "{url} should be rejected"
        );
    }
    // http/https to a public host are accepted.
    let r2 = resolver_for("example.com", [93, 184, 216, 34]);
    assert_eq!(validate_url("http://example.com/", &r2), Ok(()));
    assert_eq!(validate_url("https://example.com/", &r2), Ok(()));
}

// -------------------------------------------------------------------------
// Suite: Private_IP_Range blocking at the initial address (Req 41.3).
// -------------------------------------------------------------------------

#[test]
fn suite_private_range_blocked_at_initial_address() {
    let r = resolver_for("x", [93, 184, 216, 34]);
    for url in [
        "http://127.0.0.1/",
        "http://10.0.0.5/",
        "http://192.168.1.1/",
        "http://172.16.0.1/",
        "http://169.254.169.254/latest/meta-data/", // cloud metadata
        "http://[::1]/",
        "http://100.64.0.1/", // carrier-grade NAT
        "http://0.0.0.0/",
    ] {
        assert_eq!(
            validate_url(url, &r),
            Err(FetchError::DestinationNotPermitted),
            "{url} should be blocked"
        );
    }
}

#[test]
fn suite_private_range_blocked_after_dns_resolution() {
    // A public-looking name that resolves to a private address is still blocked
    // (defends against DNS rebinding).
    let r = resolver_for("sneaky.example", [10, 1, 2, 3]);
    assert_eq!(
        validate_url("https://sneaky.example/", &r),
        Err(FetchError::DestinationNotPermitted)
    );
}

// -------------------------------------------------------------------------
// Suite: Private_IP_Range blocking on redirect hops (Req 41.4).
// -------------------------------------------------------------------------

#[test]
fn suite_redirect_into_private_range_is_blocked() {
    let r = resolver_for("public.example", [93, 184, 216, 34]);
    // First hop redirects to the cloud metadata endpoint; the fetcher must
    // re-validate the hop and reject it (Req 41.4).
    let client = SeqClient::new(vec![Ok(HttpResponse::Redirect(
        "http://169.254.169.254/".to_string(),
    ))]);
    assert_eq!(
        fetch("https://public.example/", &r, &client, &FetchConfig::default()),
        Err(FetchError::DestinationNotPermitted)
    );
}

#[test]
fn suite_public_redirect_chain_reaches_body() {
    let r = resolver_for("public.example", [93, 184, 216, 34]);
    let client = SeqClient::new(vec![
        Ok(HttpResponse::Redirect("https://public.example/step".to_string())),
        Ok(HttpResponse::Body(b"<html>ok</html>".to_vec())),
    ]);
    assert_eq!(
        fetch("https://public.example/", &r, &client, &FetchConfig::default()),
        Ok(b"<html>ok</html>".to_vec())
    );
}

// -------------------------------------------------------------------------
// Suite: redirect cap (Req 41.5).
// -------------------------------------------------------------------------

#[test]
fn suite_redirect_cap_enforced() {
    let r = resolver_for("public.example", [93, 184, 216, 34]);
    let client = SeqClient::new(vec![
        Ok(HttpResponse::Redirect("https://public.example/1".to_string())),
        Ok(HttpResponse::Redirect("https://public.example/2".to_string())),
        Ok(HttpResponse::Redirect("https://public.example/3".to_string())),
        Ok(HttpResponse::Body(b"too late".to_vec())),
    ]);
    let cfg = FetchConfig {
        max_redirects: 2,
        ..FetchConfig::default()
    };
    assert_eq!(
        fetch("https://public.example/", &r, &client, &cfg),
        Err(FetchError::TooManyRedirects)
    );
}

// -------------------------------------------------------------------------
// Suite: fetch timeout (Req 41.6).
// -------------------------------------------------------------------------

#[test]
fn suite_fetch_timeout_surfaces() {
    let r = resolver_for("slow.example", [93, 184, 216, 34]);
    // The HTTP client honors the configured per-request timeout; a timed-out
    // request surfaces as a Timeout error (Req 41.6).
    let client = SeqClient::new(vec![Err(FetchError::Timeout)]);
    assert_eq!(
        fetch("https://slow.example/", &r, &client, &FetchConfig::default()),
        Err(FetchError::Timeout)
    );
}

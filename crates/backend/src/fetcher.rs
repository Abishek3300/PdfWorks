//! URL_Fetcher with SSRF guards (Task 15.5, Req 16.4, 41.1-41.6).
//!
//! The HTML to PDF Tool can fetch a remote URL. Left unguarded, that lets a
//! client coerce the Backend into requesting internal addresses (SSRF). This
//! module enforces, as pure and directly-testable policy:
//!
//! - **Scheme allowlist** — only `http`/`https` (Req 41.1, 41.2, Property 18).
//! - **Private range blocking** — RFC 1918, loopback, link-local, and the
//!   cloud metadata endpoint `169.254.169.254` are refused, both at the initial
//!   address and at *every* redirect hop (Req 41.3, 41.4, Property 17).
//! - **Redirect cap** — a configured maximum number of hops (Req 41.5).
//! - **Fetch timeout** — enforced per request (Req 41.6).
//!
//! Actual network I/O is behind the [`Resolver`] + [`HttpClient`] traits so the
//! guard logic — the security-relevant part — is exercised deterministically in
//! tests without touching the network. A production implementation supplies a
//! DNS resolver and an HTTP client that honor the same policy.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::config::FetchConfig;

/// Why a fetch was rejected. Each maps to a spec message (Req 41.2, 41.3, 41.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The URL scheme is not http/https (Req 41.2).
    SchemeNotPermitted,
    /// The URL could not be parsed.
    Malformed,
    /// The URL resolved to a Private_IP_Range address (Req 41.3, 41.4).
    DestinationNotPermitted,
    /// The redirect cap was exceeded (Req 41.5).
    TooManyRedirects,
    /// The fetch timed out (Req 41.6).
    Timeout,
    /// The host could not be resolved.
    ResolutionFailed,
}

impl FetchError {
    /// User-facing message (Req 41.2, 41.3, 41.6).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            FetchError::SchemeNotPermitted => {
                "the URL scheme is not permitted".to_string()
            }
            FetchError::Malformed => "the URL is malformed".to_string(),
            FetchError::DestinationNotPermitted => {
                "the URL destination is not permitted".to_string()
            }
            FetchError::TooManyRedirects => {
                "the URL redirected too many times".to_string()
            }
            FetchError::Timeout => "the fetch timed out".to_string(),
            FetchError::ResolutionFailed => {
                "the URL host could not be resolved".to_string()
            }
        }
    }
}

/// The parsed pieces of a URL the guard needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    /// Lower-cased scheme (e.g. `http`).
    pub scheme: String,
    /// Host (a name or a literal IP).
    pub host: String,
}

/// Parse the scheme and host from a URL without pulling a URL crate.
///
/// Deliberately conservative: it recognizes `scheme://host[:port][/...]` and
/// rejects anything it cannot cleanly split, so a weird input fails closed.
///
/// # Errors
///
/// Returns [`FetchError::Malformed`] for inputs without a `scheme://host`.
pub fn parse_url(url: &str) -> Result<ParsedUrl, FetchError> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://").ok_or(FetchError::Malformed)?;
    if scheme.is_empty() {
        return Err(FetchError::Malformed);
    }
    // Authority ends at the first '/', '?', or '#'.
    let authority_end = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // Strip userinfo if present (user:pass@host).
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // Split host from port. Bracketed IPv6 literals are handled first.
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        // [::1]:port  ->  ::1
        stripped
            .split(']')
            .next()
            .ok_or(FetchError::Malformed)?
            .to_string()
    } else {
        host_port
            .rsplit_once(':')
            .map_or(host_port, |(h, _port)| h)
            .to_string()
    };
    if host.is_empty() {
        return Err(FetchError::Malformed);
    }
    Ok(ParsedUrl {
        scheme: scheme.to_ascii_lowercase(),
        host,
    })
}

/// Whether a scheme is permitted (Req 41.1, 41.2, Property 18).
#[must_use]
pub fn scheme_permitted(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

/// The cloud instance-metadata endpoint that must always be blocked
/// (Private_IP_Range glossary entry).
const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);

/// Whether an IP address is in a Private_IP_Range and must be refused
/// (Req 41.3, Property 17).
///
/// Covers RFC 1918 private ranges, loopback, link-local, unspecified, the
/// carrier-grade NAT range, and the IPv6 unique-local / loopback / link-local
/// ranges, plus the cloud metadata address explicitly.
#[must_use]
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => is_private_v6(v6),
    }
}

/// IPv4 private/reserved classification.
fn is_private_v4(v4: Ipv4Addr) -> bool {
    if v4 == METADATA_V4 {
        return true;
    }
    v4.is_private()          // 10/8, 172.16/12, 192.168/16
        || v4.is_loopback()  // 127/8
        || v4.is_link_local()// 169.254/16
        || v4.is_unspecified() // 0.0.0.0
        || v4.is_broadcast() // 255.255.255.255
        || is_cgnat_v4(v4)   // 100.64/10 (carrier-grade NAT)
        || v4.octets()[0] == 0 // 0.0.0.0/8 reserved
}

/// Carrier-grade NAT range 100.64.0.0/10 (RFC 6598), not publicly routable.
fn is_cgnat_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// IPv6 private/reserved classification.
fn is_private_v6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() {
        return true;
    }
    // IPv4-mapped (::ffff:a.b.c.d) — classify by the embedded v4 address.
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_private_v4(v4);
    }
    // Some environments also map v4 into the deprecated compatible range.
    if let Some(v4) = v6.to_ipv4() {
        if v4.octets()[0] != 0 || v6.segments()[..7].iter().any(|&s| s != 0) {
            // Only treat as embedded v4 when it is the compatible form.
        }
        if is_private_v4(v4) {
            return true;
        }
    }
    let seg = v6.segments();
    // Unique local addresses fc00::/7.
    if (seg[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // Link-local fe80::/10.
    if (seg[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    false
}

/// Resolves a host name to one or more IP addresses.
pub trait Resolver {
    /// Resolve `host` to its addresses.
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::ResolutionFailed`] if the host cannot be resolved.
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, FetchError>;
}

/// The outcome of one HTTP request: either the final body, or a redirect to a
/// new location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpResponse {
    /// A terminal response carrying the fetched body bytes.
    Body(Vec<u8>),
    /// A redirect to the given absolute URL.
    Redirect(String),
}

/// Performs a single HTTP request with no redirect following of its own and
/// honoring the configured timeout (the caller drives redirects so each hop is
/// re-validated, Req 41.4).
pub trait HttpClient {
    /// Perform one request to an already-validated URL.
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::Timeout`] on timeout, or another [`FetchError`].
    fn request(&self, url: &str) -> Result<HttpResponse, FetchError>;
}

/// Validate a single URL against the scheme + private-range policy.
///
/// # Errors
///
/// Returns [`FetchError::SchemeNotPermitted`], [`FetchError::Malformed`],
/// [`FetchError::ResolutionFailed`], or [`FetchError::DestinationNotPermitted`].
pub fn validate_url<R: Resolver>(url: &str, resolver: &R) -> Result<(), FetchError> {
    // Reject a disallowed scheme first, before the host is parsed: schemes such
    // as `file://` carry an empty authority, which `parse_url` would otherwise
    // report as `Malformed`, masking the scheme-not-permitted policy (Req 41.2,
    // Property 18).
    if let Some((scheme, _)) = url.trim().split_once("://") {
        if !scheme.is_empty() && !scheme_permitted(&scheme.to_ascii_lowercase()) {
            return Err(FetchError::SchemeNotPermitted);
        }
    }
    let parsed = parse_url(url)?;
    if !scheme_permitted(&parsed.scheme) {
        return Err(FetchError::SchemeNotPermitted);
    }
    // A literal IP host is checked directly; a name is resolved and *every*
    // resolved address must be public (defends against DNS rebinding).
    if let Ok(ip) = parsed.host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(FetchError::DestinationNotPermitted);
        }
        return Ok(());
    }
    let addrs = resolver.resolve(&parsed.host)?;
    if addrs.is_empty() {
        return Err(FetchError::ResolutionFailed);
    }
    if addrs.iter().copied().any(is_private_ip) {
        return Err(FetchError::DestinationNotPermitted);
    }
    Ok(())
}

/// Fetch `url`, following redirects while re-validating each hop against the
/// scheme + private-range policy (Req 41.4) and capping the number of hops
/// (Req 41.5).
///
/// # Errors
///
/// Returns the first [`FetchError`] the policy or the client produces.
pub fn fetch<R: Resolver, C: HttpClient>(
    url: &str,
    resolver: &R,
    client: &C,
    config: &FetchConfig,
) -> Result<Vec<u8>, FetchError> {
    let mut current = url.to_string();
    let mut hops: u32 = 0;
    loop {
        // Re-validate the current URL BEFORE issuing the request (Req 41.4).
        validate_url(&current, resolver)?;
        match client.request(&current)? {
            HttpResponse::Body(bytes) => return Ok(bytes),
            HttpResponse::Redirect(next) => {
                hops += 1;
                if hops > config.max_redirects {
                    return Err(FetchError::TooManyRedirects);
                }
                current = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::collections::HashMap;

    struct MapResolver(HashMap<String, Vec<IpAddr>>);
    impl Resolver for MapResolver {
        fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, FetchError> {
            self.0
                .get(host)
                .cloned()
                .ok_or(FetchError::ResolutionFailed)
        }
    }

    fn public_resolver(host: &str) -> MapResolver {
        let mut m = HashMap::new();
        m.insert(host.to_string(), vec![IpAddr::from([93, 184, 216, 34])]);
        MapResolver(m)
    }

    #[test]
    fn parses_scheme_and_host() {
        let p = parse_url("https://user:pass@example.com:8443/path?q=1").unwrap();
        assert_eq!(p.scheme, "https");
        assert_eq!(p.host, "example.com");
    }

    #[test]
    fn parses_ipv6_literal() {
        let p = parse_url("http://[::1]:80/").unwrap();
        assert_eq!(p.host, "::1");
    }

    #[test]
    fn rejects_non_http_schemes() {
        for url in ["file:///etc/passwd", "ftp://host/x", "gopher://host"] {
            let r = public_resolver("host");
            assert_eq!(validate_url(url, &r), Err(FetchError::SchemeNotPermitted));
        }
    }

    #[test]
    fn blocks_literal_private_ips() {
        let r = public_resolver("x");
        for url in [
            "http://127.0.0.1/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
        ] {
            assert_eq!(
                validate_url(url, &r),
                Err(FetchError::DestinationNotPermitted),
                "{url} should be blocked"
            );
        }
    }

    #[test]
    fn blocks_names_resolving_to_private() {
        let mut m = HashMap::new();
        m.insert("evil.example".to_string(), vec![IpAddr::from([10, 1, 2, 3])]);
        let r = MapResolver(m);
        assert_eq!(
            validate_url("https://evil.example/", &r),
            Err(FetchError::DestinationNotPermitted)
        );
    }

    #[test]
    fn allows_public_destination() {
        let r = public_resolver("example.com");
        assert_eq!(validate_url("https://example.com/", &r), Ok(()));
    }

    struct SeqClient {
        steps: std::cell::RefCell<Vec<HttpResponse>>,
    }
    impl HttpClient for SeqClient {
        fn request(&self, _url: &str) -> Result<HttpResponse, FetchError> {
            Ok(self.steps.borrow_mut().remove(0))
        }
    }

    #[test]
    fn redirect_to_private_is_blocked_at_hop() {
        let mut m = HashMap::new();
        m.insert("public.example".to_string(), vec![IpAddr::from([93, 184, 216, 34])]);
        let r = MapResolver(m);
        let client = SeqClient {
            steps: std::cell::RefCell::new(vec![HttpResponse::Redirect(
                "http://169.254.169.254/".to_string(),
            )]),
        };
        let err = fetch(
            "https://public.example/",
            &r,
            &client,
            &FetchConfig::default(),
        );
        assert_eq!(err, Err(FetchError::DestinationNotPermitted));
    }

    #[test]
    fn redirect_cap_enforced() {
        let mut m = HashMap::new();
        m.insert("public.example".to_string(), vec![IpAddr::from([93, 184, 216, 34])]);
        let r = MapResolver(m);
        let steps = vec![
            HttpResponse::Redirect("https://public.example/1".to_string()),
            HttpResponse::Redirect("https://public.example/2".to_string()),
            HttpResponse::Redirect("https://public.example/3".to_string()),
        ];
        let client = SeqClient {
            steps: std::cell::RefCell::new(steps),
        };
        let cfg = FetchConfig {
            max_redirects: 2,
            ..FetchConfig::default()
        };
        assert_eq!(
            fetch("https://public.example/", &r, &client, &cfg),
            Err(FetchError::TooManyRedirects)
        );
    }

    #[test]
    fn follows_redirect_to_body() {
        let mut m = HashMap::new();
        m.insert("public.example".to_string(), vec![IpAddr::from([93, 184, 216, 34])]);
        let r = MapResolver(m);
        let client = SeqClient {
            steps: std::cell::RefCell::new(vec![
                HttpResponse::Redirect("https://public.example/final".to_string()),
                HttpResponse::Body(b"<html></html>".to_vec()),
            ]),
        };
        let out = fetch("https://public.example/", &r, &client, &FetchConfig::default());
        assert_eq!(out, Ok(b"<html></html>".to_vec()));
    }
}

// -------------------------------------------------------------------------
// Property 17 & 18.
// -------------------------------------------------------------------------
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashMap;

    struct FixedResolver(Vec<IpAddr>);
    impl Resolver for FixedResolver {
        fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, FetchError> {
            Ok(self.0.clone())
        }
    }

    /// Generate an IPv4 address inside a Private_IP_Range.
    fn private_v4() -> impl Strategy<Value = Ipv4Addr> {
        prop_oneof![
            (0u8..=255, 0u8..=255).prop_map(|(c, d)| Ipv4Addr::new(10, 0, c, d)),
            (16u8..=31, 0u8..=255, 0u8..=255).prop_map(|(b, c, d)| Ipv4Addr::new(172, b, c, d)),
            (0u8..=255).prop_map(|d| Ipv4Addr::new(192, 168, 0, d)),
            (0u8..=255).prop_map(|d| Ipv4Addr::new(127, 0, 0, d.max(1))),
            (0u8..=255).prop_map(|d| Ipv4Addr::new(169, 254, 0, d)),
            Just(Ipv4Addr::new(169, 254, 169, 254)),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        // Feature: pdf-tools-suite, Property 17: URL fetches to private ranges are rejected.
        //
        // For all URLs that resolve to a Private_IP_Range address — at the
        // initial address or any redirect hop — the URL_Fetcher rejects the Job.
        // Validates: Requirements 41.3, 41.4.
        #[test]
        fn private_ranges_rejected(v4 in private_v4()) {
            // (a) literal-IP URL.
            let url = format!("http://{v4}/");
            let resolver = FixedResolver(vec![IpAddr::V4(v4)]);
            prop_assert_eq!(
                validate_url(&url, &resolver),
                Err(FetchError::DestinationNotPermitted)
            );

            // (b) name resolving to the private address.
            prop_assert_eq!(
                validate_url("http://host.example/", &resolver),
                Err(FetchError::DestinationNotPermitted)
            );

            // (c) redirect hop into the private address is caught by fetch().
            struct RedirectOnce(String, std::cell::Cell<bool>);
            impl HttpClient for RedirectOnce {
                fn request(&self, _u: &str) -> Result<HttpResponse, FetchError> {
                    if self.1.replace(true) {
                        Ok(HttpResponse::Body(vec![]))
                    } else {
                        Ok(HttpResponse::Redirect(self.0.clone()))
                    }
                }
            }
            let mut m = HashMap::new();
            m.insert("public.example".to_string(), vec![IpAddr::from([93, 184, 216, 34])]);
            struct MapR(HashMap<String, Vec<IpAddr>>, IpAddr);
            impl Resolver for MapR {
                fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, FetchError> {
                    if host == "public.example" {
                        Ok(vec![IpAddr::from([93, 184, 216, 34])])
                    } else {
                        Ok(vec![self.1])
                    }
                }
            }
            let r = MapR(m, IpAddr::V4(v4));
            let client = RedirectOnce(url.clone(), std::cell::Cell::new(false));
            prop_assert_eq!(
                fetch("https://public.example/", &r, &client, &FetchConfig::default()),
                Err(FetchError::DestinationNotPermitted)
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        // Feature: pdf-tools-suite, Property 18: Only http/https URL schemes are accepted.
        //
        // For all URLs whose scheme is not http or https, the URL_Fetcher
        // rejects the Job with a scheme-not-permitted message.
        // Validates: Requirements 41.1, 41.2.
        #[test]
        fn non_http_schemes_rejected(
            scheme in "[a-z][a-z0-9+.-]{0,10}",
            host in "[a-z]{1,10}\\.example",
        ) {
            prop_assume!(scheme != "http" && scheme != "https");
            let url = format!("{scheme}://{host}/");
            let resolver = FixedResolver(vec![IpAddr::from([93, 184, 216, 34])]);
            prop_assert_eq!(
                validate_url(&url, &resolver),
                Err(FetchError::SchemeNotPermitted)
            );
        }
    }
}

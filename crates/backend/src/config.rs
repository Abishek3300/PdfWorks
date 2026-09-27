//! Backend configuration and the spec-defined constants.
//!
//! Values map directly to the requirements glossary: `Max_File_Size` (100 MB),
//! `Max_Batch_Count` (50), and `Retention_Period` (60 min). Deployment-specific
//! knobs (CORS allowlist, HTTPS enforcement, encryption key) are read from the
//! environment so the same binary runs in dev and prod.

use std::time::Duration;

/// `Max_File_Size`: the maximum size of a single Source_File — 100 MB
/// (glossary, Req 39.2).
pub const MAX_FILE_SIZE_BYTES: u64 = 100 * 1024 * 1024;

/// `Max_Batch_Count`: the maximum number of Source_Files per Job — 50
/// (glossary, Req 39.3).
pub const MAX_BATCH_COUNT: usize = 50;

/// `Retention_Period`: how long files live after Job completion — 60 minutes
/// (glossary, Req 32.2, 44.2).
pub const RETENTION_PERIOD: Duration = Duration::from_secs(60 * 60);

/// Minimum HSTS `max-age` mandated by Req 38.2 (one year, in seconds).
pub const HSTS_MAX_AGE_SECS: u64 = 31_536_000;

/// Maximum request body size accepted by the Security_Gateway (Req 42.4).
///
/// Sized just above `Max_File_Size` to leave room for multipart framing while
/// still bounding the blast radius of an oversized upload.
pub const MAX_REQUEST_BODY_BYTES: u64 = MAX_FILE_SIZE_BYTES + 1024 * 1024;

/// Rate-limit window and per-window ceilings (Req 42.1, 42.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// Length of the sliding window.
    pub window: Duration,
    /// Maximum Job submissions per client per window (Req 42.1).
    pub max_jobs_per_window: u32,
    /// Maximum uploads per client per window (Req 42.2).
    pub max_uploads_per_window: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(60),
            max_jobs_per_window: 30,
            max_uploads_per_window: 100,
        }
    }
}

/// SSRF / fetch limits for the URL_Fetcher (Req 41.5, 41.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchConfig {
    /// Maximum number of redirects followed for one Job (Req 41.5).
    pub max_redirects: u32,
    /// Per-request fetch timeout (Req 41.6).
    pub timeout: Duration,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            max_redirects: 5,
            timeout: Duration::from_secs(10),
        }
    }
}

/// Archive (zip-bomb) limits for DOCX/XLSX/PPTX Source_Files (Req 50.4, 42.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    /// Maximum total uncompressed size across all entries (Req 50.4).
    pub max_total_uncompressed: u64,
    /// Maximum ratio of uncompressed to compressed bytes (Req 42.5).
    pub max_expansion_ratio: u64,
    /// Maximum nesting depth of archives-within-archives (Req 50.4).
    pub max_nesting_depth: u32,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            // 500 MB of expanded content is far beyond any legitimate Office
            // document while still bounding a decompression bomb.
            max_total_uncompressed: 500 * 1024 * 1024,
            // A 100x expansion ratio flags classic zip bombs (which reach
            // thousands-to-one) without tripping normal compression.
            max_expansion_ratio: 100,
            max_nesting_depth: 3,
        }
    }
}

/// Sandbox resource limits (Req 40.3, 40.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxLimits {
    /// Maximum wall-clock time for one Job.
    pub wall_clock: Duration,
    /// Maximum resident memory for one Job, in bytes.
    pub max_memory_bytes: u64,
    /// Maximum CPU time for one Job.
    pub max_cpu: Duration,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            wall_clock: Duration::from_secs(30),
            max_memory_bytes: 512 * 1024 * 1024,
            max_cpu: Duration::from_secs(25),
        }
    }
}

/// Fully-resolved backend configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Origins permitted by the CORS allowlist (Req 50.3).
    pub cors_allowlist: Vec<String>,
    /// Whether the deployment is production. When true, HTTPS is asserted
    /// (Req 38.1) and cookies always carry Secure (Req 38.7).
    pub production: bool,
    /// Per-deployment 256-bit key for at-rest encryption (Req 44.1).
    pub encryption_key: [u8; 32],
    /// Rate-limit configuration.
    pub rate_limit: RateLimitConfig,
    /// URL_Fetcher configuration.
    pub fetch: FetchConfig,
    /// Archive-bomb limits.
    pub archive: ArchiveLimits,
    /// Sandbox resource limits.
    pub sandbox: SandboxLimits,
}

impl Config {
    /// Build a configuration suitable for local development and tests, with a
    /// deterministic zero key and a permissive single-origin allowlist.
    #[must_use]
    pub fn for_tests() -> Self {
        Self {
            cors_allowlist: vec!["https://app.example".to_string()],
            production: false,
            encryption_key: [0u8; 32],
            rate_limit: RateLimitConfig::default(),
            fetch: FetchConfig::default(),
            archive: ArchiveLimits::default(),
            sandbox: SandboxLimits::default(),
        }
    }

    /// Load configuration from the environment.
    ///
    /// - `CORS_ALLOWLIST`: comma-separated permitted origins (Req 50.3).
    /// - `BACKEND_PRODUCTION`: `1`/`true` to enable production hardening.
    /// - `FILE_STORE_KEY_HEX`: 64 hex chars (32 bytes) for at-rest encryption.
    ///
    /// # Errors
    ///
    /// Returns an error string if the encryption key is missing/malformed in a
    /// production deployment, so startup fails loudly rather than encrypting
    /// with a weak key.
    pub fn from_env() -> Result<Self, String> {
        let cors_allowlist = std::env::var("CORS_ALLOWLIST")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();

        let production = matches!(
            std::env::var("BACKEND_PRODUCTION").unwrap_or_default().as_str(),
            "1" | "true" | "TRUE"
        );

        let encryption_key = match std::env::var("FILE_STORE_KEY_HEX") {
            Ok(hex) => decode_key_hex(&hex)?,
            Err(_) if !production => [0u8; 32],
            Err(_) => {
                return Err(
                    "FILE_STORE_KEY_HEX is required in production (Req 44.1)".to_string(),
                )
            }
        };

        Ok(Self {
            cors_allowlist,
            production,
            encryption_key,
            rate_limit: RateLimitConfig::default(),
            fetch: FetchConfig::default(),
            archive: ArchiveLimits::default(),
            sandbox: SandboxLimits::default(),
        })
    }
}

/// Decode exactly 32 bytes from a 64-char hex string.
fn decode_key_hex(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err("FILE_STORE_KEY_HEX must be 64 hex characters (32 bytes)".to_string());
    }
    let mut out = [0u8; 32];
    let bytes = hex.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = hex_val(bytes[i * 2]).ok_or("invalid hex digit")?;
        let lo = hex_val(bytes[i * 2 + 1]).ok_or("invalid hex digit")?;
        *slot = (hi << 4) | lo;
    }
    Ok(out)
}

/// Map an ASCII hex digit to its 0-15 value.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn constants_match_glossary() {
        assert_eq!(MAX_FILE_SIZE_BYTES, 104_857_600);
        assert_eq!(MAX_BATCH_COUNT, 50);
        assert_eq!(RETENTION_PERIOD, Duration::from_secs(3600));
        assert!(HSTS_MAX_AGE_SECS >= 31_536_000);
    }

    #[test]
    fn decodes_valid_key() {
        let hex = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let key = decode_key_hex(hex).unwrap();
        assert_eq!(key[0], 0x00);
        assert_eq!(key[1], 0x11);
        assert_eq!(key[31], 0xff);
    }

    #[test]
    fn rejects_short_key() {
        assert!(decode_key_hex("00").is_err());
    }
}

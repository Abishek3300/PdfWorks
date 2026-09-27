//! Job_Token generation and token-gated, expiry-bound access checking
//! (Task 6.6).
//!
//! A Job_Token is the sole credential guarding a Job's Source_Files and
//! Output_Files (Req 43.1-43.4, 32.4, 36.6). It must be unguessable, so it
//! carries at least 128 bits of entropy drawn from a cryptographically secure
//! generator (Req 43.2) and is encoded URL-safe so it can travel in a link.
//!
//! Both operations live in the shared engine so the *same* generation and
//! access-check logic backs the WASM and native builds:
//!
//! - [`generate_job_token`] draws random bytes via `getrandom` (a CSPRNG whose
//!   backend is selected per target in `.cargo/config.toml`: the Web Crypto API
//!   on wasm32, a CPU RDRAND source on windows-gnu) and encodes them URL-safe.
//! - [`check_access`] grants access iff a presented token equals the expected
//!   token (compared in constant time to avoid timing side channels) AND the
//!   caller-supplied `now` is strictly before the Job's `expiry`.
//!
//! Time is modeled as a plain `u64` millisecond count supplied by the caller, so
//! this module stays pure and I/O-free (no system clock) and therefore
//! WASM-compatible.

use crate::model::EngineError;

/// Number of random bytes per Job_Token.
///
/// 16 bytes = 128 bits of entropy, the minimum the design mandates (Req 43.2).
/// We use a larger margin so even after URL-safe encoding the token comfortably
/// exceeds the floor.
const TOKEN_ENTROPY_BYTES: usize = 32;

/// The minimum entropy, in bits, every Job_Token must carry (Req 43.2).
pub const MIN_TOKEN_ENTROPY_BITS: usize = 128;

/// Generate a fresh, URL-safe Job_Token with at least 128 bits of entropy
/// (Req 43.1, 43.2, Property 19).
///
/// Draws [`TOKEN_ENTROPY_BYTES`] bytes from the platform CSPRNG via `getrandom`
/// and encodes them with URL-safe base64 **without padding**, so the result
/// contains only `A-Z`, `a-z`, `0-9`, `-`, and `_` and is safe to embed in a URL
/// path or query without escaping.
///
/// # Errors
///
/// Returns [`EngineError::NoOutputProduced`] if the platform entropy source is
/// unavailable, so callers observe a structured error rather than a panic.
pub fn generate_job_token() -> Result<String, EngineError> {
    let mut bytes = [0u8; TOKEN_ENTROPY_BYTES];
    // `getrandom::fill` is backed by a CSPRNG (Req 43.2). A failure here means
    // the platform entropy source is unavailable; surface it structurally.
    getrandom::fill(&mut bytes).map_err(|_| EngineError::NoOutputProduced)?;
    Ok(encode_url_safe_no_pad(&bytes))
}

/// Decide whether a file-access request is authorized (Req 43.3, 43.4, 32.4,
/// 36.6, Property 20).
///
/// Access is granted **if and only if**:
/// 1. `presented` equals `expected` (the Job_Token issued for that Job), and
/// 2. `now < expiry` (the current time is strictly before the Job's expiry).
///
/// The token comparison is constant-time (see [`constant_time_eq`]) so an
/// attacker cannot learn a valid token byte-by-byte from response timing. Time
/// is expressed in the same arbitrary `u64` unit (e.g. milliseconds since an
/// epoch) for `now` and `expiry`; the caller supplies both, keeping this pure.
#[must_use]
pub fn check_access(expected: &str, presented: &str, expiry: u64, now: u64) -> bool {
    // Expiry is a strict bound: at or after `expiry` the token is rejected
    // (Req 43.4). Evaluate it first, but still run the constant-time compare so
    // the timing profile does not reveal whether expiry or token mismatch was
    // the cause.
    let not_expired = now < expiry;
    let tokens_match = constant_time_eq(expected.as_bytes(), presented.as_bytes());
    not_expired && tokens_match
}

/// Compare two byte slices for equality in constant time with respect to their
/// contents.
///
/// Returns `false` immediately when the lengths differ (length is not secret),
/// otherwise accumulates the bitwise difference across every byte so the running
/// time depends only on the length, never on where the first mismatch occurs.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// URL-safe base64 alphabet (RFC 4648 §5): `-` and `_` replace `+` and `/`.
const URL_SAFE_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encode `bytes` as URL-safe base64 without padding.
///
/// Implemented inline (rather than pulling in a base64 crate) to keep the
/// engine's dependency surface minimal and unambiguously WASM-compatible. The
/// output uses only URL-safe characters and omits `=` padding, so it drops
/// straight into a URL (Req 43.1).
fn encode_url_safe_no_pad(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        // Assemble up to 24 bits from this 1-3 byte chunk.
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;

        // Always emit the first two sextets; emit the 3rd/4th only when the
        // corresponding input bytes exist (no-padding encoding).
        out.push(URL_SAFE_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(URL_SAFE_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(URL_SAFE_ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(URL_SAFE_ALPHABET[(n & 0x3f) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    // Test code may unwrap/expect freely; the crate-wide deny is relaxed here.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn generated_token_is_url_safe_and_high_entropy() {
        let token = generate_job_token().unwrap();
        // URL-safe base64 (no padding): only A-Z a-z 0-9 - _
        assert!(token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(!token.contains('='));
        // 32 random bytes -> 256 bits, comfortably above the 128-bit floor.
        // Compile-time guarantee that the entropy budget clears the floor.
        const _: () = assert!(TOKEN_ENTROPY_BYTES * 8 >= MIN_TOKEN_ENTROPY_BITS);
    }

    #[test]
    fn access_granted_only_for_correct_unexpired_token() {
        let token = "the-issued-token";
        // Correct token, before expiry -> granted.
        assert!(check_access(token, token, 100, 50));
        // Correct token, exactly at expiry -> denied (strict bound, Req 43.4).
        assert!(!check_access(token, token, 100, 100));
        // Correct token, after expiry -> denied.
        assert!(!check_access(token, token, 100, 150));
        // Wrong token, before expiry -> denied.
        assert!(!check_access(token, "wrong", 100, 50));
    }

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn base64_encoding_is_correct() {
        // Known RFC 4648 vectors (URL-safe, no padding).
        assert_eq!(encode_url_safe_no_pad(b""), "");
        assert_eq!(encode_url_safe_no_pad(b"f"), "Zg");
        assert_eq!(encode_url_safe_no_pad(b"fo"), "Zm8");
        assert_eq!(encode_url_safe_no_pad(b"foo"), "Zm9v");
        assert_eq!(encode_url_safe_no_pad(b"foob"), "Zm9vYg");
    }
}

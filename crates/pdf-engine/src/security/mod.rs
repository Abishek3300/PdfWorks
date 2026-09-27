//! Shared security helpers for the PDF Tools Suite (Task 6).
//!
//! These pure, I/O-free helpers back security guarantees that must hold
//! identically on both processing planes (Client_Side via WASM and Server_Side
//! via native), so they live in the shared engine and are exercised once by the
//! design's security Properties 14-20:
//!
//! - [`filename`] — path-traversal-safe file-name sanitization (Req 50.1,
//!   Property 15) and unique output-name assignment (Req 49.3, Property 14).
//! - [`scanner`] — PDF active-content detection and removal/rejection
//!   (Req 39.5, 39.6, Property 16).
//! - [`token`] — Job_Token generation (Req 43.1, 43.2, Property 19) and
//!   token-gated, expiry-bound access checking (Req 43.3, 43.4, 32.4, 36.6,
//!   Property 20).

pub mod filename;
pub mod scanner;
pub mod token;

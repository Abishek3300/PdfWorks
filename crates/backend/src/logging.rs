//! Security_Log and anomaly recording (Task 20.1, Req 44.4, 46.1-46.4).
//!
//! The Security_Log records security-relevant events (Req 46.1) while excluding
//! PII (Req 46.2) and Source_File / Output_File content (Req 44.4). Read access
//! is restricted to authorized operators (Req 46.3). When a client's rejected-
//! request rate crosses a threshold, an anomaly event is recorded (Req 46.4).
//!
//! Events carry only a coarse, non-identifying `client_ref` (a hashed client
//! key), an event kind, and a short reason — never raw IPs, file names, or
//! bytes. The store keeps events in memory here; a production deployment ships
//! them to an operator-only sink.

use std::collections::HashMap;
use std::sync::Mutex;

/// The category of a security event (Req 46.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEventKind {
    /// A request was rejected by validation, the gateway, or a guard.
    RequestRejected,
    /// A file-access request presented an invalid/expired Job_Token.
    AccessDenied,
    /// A URL fetch was blocked by the SSRF guards.
    FetchBlocked,
    /// A client's rejected-request rate exceeded the threshold (Req 46.4).
    Anomaly,
    /// A Job's files were deleted (retention or on-demand).
    FilesDeleted,
}

/// A single, PII-free security event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityEvent {
    /// Non-identifying client reference (a hash, never a raw IP — Req 46.2).
    pub client_ref: String,
    /// The event category.
    pub kind: SecurityEventKind,
    /// A short machine reason code (e.g. an [`crate::error::ApiError::code`]).
    /// Never contains file content or PII (Req 44.4, 46.2).
    pub reason: String,
}

/// Threshold configuration for anomaly detection (Req 46.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnomalyConfig {
    /// Number of rejected requests from one client that triggers an anomaly.
    pub rejected_threshold: u32,
}

impl Default for AnomalyConfig {
    fn default() -> Self {
        Self {
            rejected_threshold: 20,
        }
    }
}

/// The in-memory Security_Log with operator-gated reads.
pub struct SecurityLog {
    events: Mutex<Vec<SecurityEvent>>,
    rejected_counts: Mutex<HashMap<String, u32>>,
    anomaly_flagged: Mutex<HashMap<String, bool>>,
    config: AnomalyConfig,
    /// Opaque operator token required to read the log (Req 46.3).
    operator_token: String,
}

impl SecurityLog {
    /// Create a log requiring `operator_token` for reads (Req 46.3).
    #[must_use]
    pub fn new(operator_token: String, config: AnomalyConfig) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            rejected_counts: Mutex::new(HashMap::new()),
            anomaly_flagged: Mutex::new(HashMap::new()),
            config,
            operator_token,
        }
    }

    /// Derive a non-identifying client reference from a raw client key (e.g. an
    /// IP or API key). Uses a stable non-cryptographic hash so the same client
    /// maps to the same ref without storing the raw value (Req 46.2).
    #[must_use]
    pub fn client_ref(raw_client_key: &str) -> String {
        // FNV-1a 64-bit — deterministic, dependency-free, and one-way enough
        // that the log never stores the raw identifier.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for b in raw_client_key.as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("c{hash:016x}")
    }

    /// Record a security event (Req 46.1). If the event is a rejection, update
    /// the per-client counter and emit an anomaly event once the threshold is
    /// crossed (Req 46.4).
    pub fn record(&self, event: SecurityEvent) {
        let is_rejection = matches!(
            event.kind,
            SecurityEventKind::RequestRejected
                | SecurityEventKind::AccessDenied
                | SecurityEventKind::FetchBlocked
        );
        let client_ref = event.client_ref.clone();

        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }

        if is_rejection {
            self.bump_and_maybe_flag(&client_ref);
        }
    }

    /// Increment a client's rejection count and record an anomaly the first
    /// time it crosses the threshold (Req 46.4).
    fn bump_and_maybe_flag(&self, client_ref: &str) {
        let crossed = {
            let mut counts = match self.rejected_counts.lock() {
                Ok(c) => c,
                Err(_) => return,
            };
            let n = counts.entry(client_ref.to_string()).or_insert(0);
            *n += 1;
            *n >= self.config.rejected_threshold
        };

        if crossed {
            let already = {
                let mut flagged = match self.anomaly_flagged.lock() {
                    Ok(f) => f,
                    Err(_) => return,
                };
                flagged.insert(client_ref.to_string(), true).unwrap_or(false)
            };
            if !already {
                if let Ok(mut events) = self.events.lock() {
                    events.push(SecurityEvent {
                        client_ref: client_ref.to_string(),
                        kind: SecurityEventKind::Anomaly,
                        reason: "rejected_rate_threshold_exceeded".to_string(),
                    });
                }
            }
        }
    }

    /// Read the recorded events. Restricted to callers presenting the operator
    /// token (Req 46.3).
    ///
    /// # Errors
    ///
    /// Returns `Err(())` when the presented token does not match.
    pub fn read(&self, operator_token: &str) -> Result<Vec<SecurityEvent>, ()> {
        if !pdf_engine::constant_time_eq(
            self.operator_token.as_bytes(),
            operator_token.as_bytes(),
        ) {
            return Err(());
        }
        self.events.lock().map(|e| e.clone()).map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn log() -> SecurityLog {
        SecurityLog::new("op-secret".to_string(), AnomalyConfig::default())
    }

    #[test]
    fn client_ref_is_not_the_raw_key() {
        let r = SecurityLog::client_ref("203.0.113.7");
        assert!(!r.contains("203.0.113.7"));
        // Deterministic.
        assert_eq!(r, SecurityLog::client_ref("203.0.113.7"));
    }

    #[test]
    fn read_requires_operator_token() {
        let l = log();
        l.record(SecurityEvent {
            client_ref: SecurityLog::client_ref("x"),
            kind: SecurityEventKind::RequestRejected,
            reason: "validation_failed".to_string(),
        });
        assert!(l.read("wrong").is_err());
        assert_eq!(l.read("op-secret").unwrap().len(), 1);
    }

    #[test]
    fn anomaly_recorded_after_threshold() {
        let l = SecurityLog::new("op".to_string(), AnomalyConfig { rejected_threshold: 3 });
        let cref = SecurityLog::client_ref("noisy");
        for _ in 0..3 {
            l.record(SecurityEvent {
                client_ref: cref.clone(),
                kind: SecurityEventKind::RequestRejected,
                reason: "rate_limited".to_string(),
            });
        }
        let events = l.read("op").unwrap();
        let anomalies = events
            .iter()
            .filter(|e| e.kind == SecurityEventKind::Anomaly)
            .count();
        assert_eq!(anomalies, 1, "exactly one anomaly should be recorded");
    }

    #[test]
    fn anomaly_recorded_only_once() {
        let l = SecurityLog::new("op".to_string(), AnomalyConfig { rejected_threshold: 2 });
        let cref = SecurityLog::client_ref("noisy");
        for _ in 0..10 {
            l.record(SecurityEvent {
                client_ref: cref.clone(),
                kind: SecurityEventKind::RequestRejected,
                reason: "rate_limited".to_string(),
            });
        }
        let events = l.read("op").unwrap();
        let anomalies = events
            .iter()
            .filter(|e| e.kind == SecurityEventKind::Anomaly)
            .count();
        assert_eq!(anomalies, 1);
    }
}

//! Retention_Service (Task 16.2, Req 32.2, 32.3, 33.4, 44.2, 44.3).
//!
//! Files live in the File_Store for at most the `Retention_Period` (60 min)
//! after a Job completes, after which they are securely deleted (Req 32.2,
//! 44.2). A User can also request immediate deletion (Req 32.3, 44.3). On any
//! Job failure, no partial Output_File is retained (Req 33.4, 49.4).
//!
//! Deletion is expressed as a pure decision (`is_expired`) plus store-backed
//! sweeps, so the timing logic is unit-testable without a real clock and the
//! deletion itself is exercised against the in-memory store.

use std::time::Duration;

use crate::store::{EncryptedFileStore, ObjectStore, StoreError};

/// Whether a Job's files are past their retention deadline (Req 32.2, 44.2).
///
/// Time is expressed as arbitrary millisecond counts supplied by the caller so
/// this stays pure and clock-free. `completed_at` is when the Job finished;
/// `now` is the current time; `period` is the `Retention_Period`.
#[must_use]
pub fn is_expired(completed_at_ms: u64, now_ms: u64, period: Duration) -> bool {
    let deadline = completed_at_ms.saturating_add(period.as_millis() as u64);
    now_ms >= deadline
}

/// A record the sweeper consults: a Job_Token and when the Job completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionRecord {
    /// Job_Token scoping the files to delete.
    pub job_token: String,
    /// Completion time in milliseconds.
    pub completed_at_ms: u64,
}

/// Securely delete all files for `job_token` immediately (Req 32.3, 44.3).
///
/// # Errors
///
/// Propagates store errors.
pub async fn delete_now<S: ObjectStore>(
    store: &EncryptedFileStore<S>,
    job_token: &str,
) -> Result<usize, StoreError> {
    store.delete_job(job_token).await
}

/// Sweep `records`, deleting every Job whose retention deadline has passed as
/// of `now_ms` (Req 32.2, 44.2). Returns the tokens that were deleted.
///
/// # Errors
///
/// Returns the first store error encountered; already-deleted Jobs are not
/// rolled back (deletion is idempotent).
pub async fn sweep_expired<S: ObjectStore>(
    store: &EncryptedFileStore<S>,
    records: &[RetentionRecord],
    now_ms: u64,
    period: Duration,
) -> Result<Vec<String>, StoreError> {
    let mut deleted = Vec::new();
    for rec in records {
        if is_expired(rec.completed_at_ms, now_ms, period) {
            store.delete_job(&rec.job_token).await?;
            deleted.push(rec.job_token.clone());
        }
    }
    Ok(deleted)
}

/// Delete any partial output for a Job that failed mid-processing (Req 33.4,
/// 49.4). Called on the failure path so no partial Output_File is retained.
///
/// # Errors
///
/// Propagates store errors.
pub async fn discard_partial<S: ObjectStore>(
    store: &EncryptedFileStore<S>,
    job_token: &str,
) -> Result<(), StoreError> {
    store.delete_job(job_token).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::config::RETENTION_PERIOD;
    use crate::store::{scoped_key, InMemoryObjectStore};

    fn store() -> EncryptedFileStore<InMemoryObjectStore> {
        EncryptedFileStore::new(InMemoryObjectStore::new(), &[5u8; 32])
    }

    #[test]
    fn expiry_is_strict_at_deadline() {
        let period = Duration::from_secs(60);
        // Exactly at the deadline -> expired.
        assert!(is_expired(0, 60_000, period));
        // One ms before -> not expired.
        assert!(!is_expired(0, 59_999, period));
    }

    #[tokio::test]
    async fn delete_now_removes_job_files() {
        let s = store();
        s.put_encrypted(&scoped_key("job1", "src"), &[0u8; 12], b"data")
            .await
            .unwrap();
        let n = delete_now(&s, "job1").await.unwrap();
        assert_eq!(n, 1);
        assert!(s.get_decrypted(&scoped_key("job1", "src")).await.is_err());
    }

    #[tokio::test]
    async fn sweep_deletes_only_expired() {
        let s = store();
        s.put_encrypted(&scoped_key("old", "f"), &[0u8; 12], b"x").await.unwrap();
        s.put_encrypted(&scoped_key("fresh", "f"), &[1u8; 12], b"y").await.unwrap();
        let records = vec![
            RetentionRecord { job_token: "old".to_string(), completed_at_ms: 0 },
            RetentionRecord {
                job_token: "fresh".to_string(),
                completed_at_ms: 10_000_000,
            },
        ];
        // now = period + 1ms after epoch: "old" expired, "fresh" not.
        let now = RETENTION_PERIOD.as_millis() as u64 + 1;
        let deleted = sweep_expired(&s, &records, now, RETENTION_PERIOD).await.unwrap();
        assert_eq!(deleted, vec!["old".to_string()]);
        assert!(s.get_decrypted(&scoped_key("old", "f")).await.is_err());
        assert!(s.get_decrypted(&scoped_key("fresh", "f")).await.is_ok());
    }

    #[tokio::test]
    async fn discard_partial_leaves_nothing() {
        let s = store();
        s.put_encrypted(&scoped_key("failed", "partial"), &[0u8; 12], b"z")
            .await
            .unwrap();
        discard_partial(&s, "failed").await.unwrap();
        assert!(s.get_decrypted(&scoped_key("failed", "partial")).await.is_err());
    }
}

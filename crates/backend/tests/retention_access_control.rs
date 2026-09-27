//! Task 22.3 — Retention and access-control integration tests.
//!
//! These exercise the Retention_Service + File_Store + Job_Token access checks
//! together: files are deleted when the Retention_Period elapses and on demand,
//! and a Job_Token is rejected after expiry and can never enumerate a Job's
//! files (no directory listing).
//!
//! Requirements: 32.2, 32.3, 43.4, 43.5, 44.2, 44.3.

mod common;

use std::time::Duration;

use backend::config::RETENTION_PERIOD;
use backend::error::ApiError;
use backend::jobs::{job_result, job_status};
use backend::queue::{InMemoryJobBackend, JobRecord, JobStatus, JobStore};
use backend::retention::{delete_now, is_expired, sweep_expired, RetentionRecord};
use backend::store::{scoped_key, StoreError};

use common::encrypted_store;

/// Store a Job record + one encrypted output for a token.
async fn seed_stored_job(
    store: &backend::store::EncryptedFileStore<backend::store::InMemoryObjectStore>,
    jobs: &InMemoryJobBackend,
    token: &str,
    created_at_ms: u64,
) -> String {
    let file_id = "out-0-result.pdf";
    let key = scoped_key(token, file_id);
    store
        .put_encrypted(&key, &[1u8; 12], b"%PDF-1.7 result")
        .await
        .expect("store output");
    jobs.put(JobRecord {
        job_token: token.to_string(),
        status: JobStatus::Succeeded,
        output_ids: vec![file_id.to_string()],
        created_at_ms,
        expires_at_ms: created_at_ms + RETENTION_PERIOD.as_millis() as u64,
    })
    .await
    .expect("store record");
    file_id.to_string()
}

// -------------------------------------------------------------------------
// Retention: deletion at expiry (Req 32.2, 44.2).
// -------------------------------------------------------------------------

#[tokio::test]
async fn files_deleted_when_retention_period_elapses() {
    let store = encrypted_store();
    let jobs = InMemoryJobBackend::new();
    let file_id = seed_stored_job(&store, &jobs, "expiring", 0).await;

    // Before the deadline the file is present.
    let key = scoped_key("expiring", &file_id);
    assert!(store.get_decrypted(&key).await.is_ok());

    // The sweeper deletes the Job once its deadline passes (Req 32.2, 44.2).
    let now = RETENTION_PERIOD.as_millis() as u64 + 1;
    let deleted = sweep_expired(
        &store,
        &[RetentionRecord {
            job_token: "expiring".to_string(),
            completed_at_ms: 0,
        }],
        now,
        RETENTION_PERIOD,
    )
    .await
    .expect("sweep");
    assert_eq!(deleted, vec!["expiring".to_string()]);
    assert_eq!(store.get_decrypted(&key).await, Err(StoreError::NotFound));
}

#[tokio::test]
async fn sweeper_keeps_jobs_within_retention() {
    let store = encrypted_store();
    let jobs = InMemoryJobBackend::new();
    let file_id = seed_stored_job(&store, &jobs, "fresh", 0).await;

    // Just before the deadline, nothing is deleted.
    let now = RETENTION_PERIOD.as_millis() as u64 - 1;
    let deleted = sweep_expired(
        &store,
        &[RetentionRecord {
            job_token: "fresh".to_string(),
            completed_at_ms: 0,
        }],
        now,
        RETENTION_PERIOD,
    )
    .await
    .expect("sweep");
    assert!(deleted.is_empty());
    assert!(store
        .get_decrypted(&scoped_key("fresh", &file_id))
        .await
        .is_ok());
}

#[test]
fn expiry_boundary_is_inclusive() {
    let period = Duration::from_secs(60);
    assert!(!is_expired(0, 59_999, period));
    assert!(is_expired(0, 60_000, period));
}

// -------------------------------------------------------------------------
// Retention: deletion on demand (Req 32.3, 44.3).
// -------------------------------------------------------------------------

#[tokio::test]
async fn files_deleted_on_demand() {
    let store = encrypted_store();
    let jobs = InMemoryJobBackend::new();
    let file_id = seed_stored_job(&store, &jobs, "ondemand", 0).await;
    let key = scoped_key("ondemand", &file_id);
    assert!(store.get_decrypted(&key).await.is_ok());

    let n = delete_now(&store, "ondemand").await.expect("delete now");
    assert_eq!(n, 1);
    assert_eq!(store.get_decrypted(&key).await, Err(StoreError::NotFound));
}

#[tokio::test]
async fn on_demand_delete_only_touches_that_job() {
    let store = encrypted_store();
    let jobs = InMemoryJobBackend::new();
    let a = seed_stored_job(&store, &jobs, "jobA", 0).await;
    let b = seed_stored_job(&store, &jobs, "jobB", 0).await;

    delete_now(&store, "jobA").await.expect("delete A");
    assert_eq!(
        store.get_decrypted(&scoped_key("jobA", &a)).await,
        Err(StoreError::NotFound)
    );
    // jobB is untouched.
    assert!(store
        .get_decrypted(&scoped_key("jobB", &b))
        .await
        .is_ok());
}

// -------------------------------------------------------------------------
// Access control: token rejected after expiry (Req 43.4).
// -------------------------------------------------------------------------

#[tokio::test]
async fn token_rejected_after_expiry() {
    let store = encrypted_store();
    let jobs = InMemoryJobBackend::new();
    let file_id = seed_stored_job(&store, &jobs, "expTok", 0).await;

    // Within retention, status + result are accessible with the token.
    assert!(job_status(&jobs, "expTok", 0).await.is_ok());
    assert!(job_result(&store, &jobs, "expTok", &file_id, 0).await.is_ok());

    // After expiry, the same token no longer grants access (Req 43.4).
    let past = RETENTION_PERIOD.as_millis() as u64 + 1;
    assert_eq!(
        job_status(&jobs, "expTok", past).await,
        Err(ApiError::AccessDenied)
    );
    assert_eq!(
        job_result(&store, &jobs, "expTok", &file_id, past).await,
        Err(ApiError::AccessDenied)
    );
}

#[tokio::test]
async fn unknown_token_is_denied() {
    let jobs = InMemoryJobBackend::new();
    assert_eq!(
        job_status(&jobs, "never-issued", 0).await,
        Err(ApiError::AccessDenied)
    );
}

// -------------------------------------------------------------------------
// Access control: no directory listing (Req 43.5).
// -------------------------------------------------------------------------

#[tokio::test]
async fn directory_listing_is_refused() {
    let store = encrypted_store();
    // The File_Store never exposes a listing — access is only ever by an exact
    // (token, file_id) pair (Req 43.5).
    assert_eq!(store.list().await, Err(StoreError::ListingForbidden));
}

#[tokio::test]
async fn result_access_requires_exact_file_id_not_enumeration() {
    let store = encrypted_store();
    let jobs = InMemoryJobBackend::new();
    let file_id = seed_stored_job(&store, &jobs, "listTok", 0).await;

    // A wrong/guessed file id under a valid token yields NotFound, never a
    // listing of what exists (Req 43.5).
    assert_eq!(
        job_result(&store, &jobs, "listTok", "does-not-exist", 0).await,
        Err(ApiError::NotFound)
    );
    // The real id works.
    assert!(job_result(&store, &jobs, "listTok", &file_id, 0).await.is_ok());
}

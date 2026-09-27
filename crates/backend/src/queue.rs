//! Job state persistence + dispatch queue (Task 15.1, Req 31.4, 43.x).
//!
//! Server-side Jobs are persisted so their status survives across requests and
//! so a worker can pick them up without blocking the intake path (Req 31.4).
//! The design specifies Redis for job status + the dispatch queue; this module
//! abstracts that behind [`JobStore`] + [`JobQueue`] so tests use an in-memory
//! implementation and production uses Redis, with identical semantics.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Lifecycle status of a Job (design Data Models `JobStatus`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobStatus {
    /// Accepted and waiting for a worker.
    Queued,
    /// Being processed by a worker.
    Running,
    /// Completed; output files are available.
    Succeeded,
    /// Failed with a reason (Req 33.3).
    Failed {
        /// Human-readable failure reason.
        reason: String,
    },
}

/// Persisted Job record, addressed by its Job_Token (Req 43.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    /// The unguessable Job_Token (Req 43.1, 43.2).
    pub job_token: String,
    /// Current status.
    pub status: JobStatus,
    /// Output file ids produced (opaque, scoped to the token).
    pub output_ids: Vec<String>,
    /// Job creation time in ms.
    pub created_at_ms: u64,
    /// Expiry time in ms = created_at + Retention_Period (Req 32.2, 43.4).
    pub expires_at_ms: u64,
}

/// Errors from the Job store/queue layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// No Job exists for the token.
    NotFound,
    /// The backing store failed.
    Backend(String),
}

/// Persists Job records keyed by Job_Token.
#[async_trait]
pub trait JobStore: Send + Sync {
    /// Create or overwrite a Job record.
    async fn put(&self, record: JobRecord) -> Result<(), QueueError>;
    /// Fetch a Job record by token.
    async fn get(&self, job_token: &str) -> Result<JobRecord, QueueError>;
    /// Update a Job's status.
    async fn set_status(&self, job_token: &str, status: JobStatus) -> Result<(), QueueError>;
    /// Remove a Job record (on retention deletion).
    async fn remove(&self, job_token: &str) -> Result<(), QueueError>;
}

/// A FIFO dispatch queue of Job_Tokens awaiting a worker (Req 31.4).
#[async_trait]
pub trait JobQueue: Send + Sync {
    /// Enqueue a Job_Token for dispatch.
    async fn enqueue(&self, job_token: &str) -> Result<(), QueueError>;
    /// Pop the next Job_Token, or `None` if the queue is empty.
    async fn dequeue(&self) -> Result<Option<String>, QueueError>;
    /// Current queue depth.
    async fn depth(&self) -> Result<usize, QueueError>;
}

/// In-memory [`JobStore`] + [`JobQueue`] for tests and single-node dev.
#[derive(Default)]
pub struct InMemoryJobBackend {
    records: Mutex<HashMap<String, JobRecord>>,
    queue: Mutex<VecDeque<String>>,
}

impl InMemoryJobBackend {
    /// Create an empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl JobStore for InMemoryJobBackend {
    async fn put(&self, record: JobRecord) -> Result<(), QueueError> {
        self.records
            .lock()
            .map_err(|_| QueueError::Backend("lock".to_string()))?
            .insert(record.job_token.clone(), record);
        Ok(())
    }

    async fn get(&self, job_token: &str) -> Result<JobRecord, QueueError> {
        self.records
            .lock()
            .map_err(|_| QueueError::Backend("lock".to_string()))?
            .get(job_token)
            .cloned()
            .ok_or(QueueError::NotFound)
    }

    async fn set_status(&self, job_token: &str, status: JobStatus) -> Result<(), QueueError> {
        let mut recs = self
            .records
            .lock()
            .map_err(|_| QueueError::Backend("lock".to_string()))?;
        let rec = recs.get_mut(job_token).ok_or(QueueError::NotFound)?;
        rec.status = status;
        Ok(())
    }

    async fn remove(&self, job_token: &str) -> Result<(), QueueError> {
        self.records
            .lock()
            .map_err(|_| QueueError::Backend("lock".to_string()))?
            .remove(job_token);
        Ok(())
    }
}

#[async_trait]
impl JobQueue for InMemoryJobBackend {
    async fn enqueue(&self, job_token: &str) -> Result<(), QueueError> {
        self.queue
            .lock()
            .map_err(|_| QueueError::Backend("lock".to_string()))?
            .push_back(job_token.to_string());
        Ok(())
    }

    async fn dequeue(&self) -> Result<Option<String>, QueueError> {
        Ok(self
            .queue
            .lock()
            .map_err(|_| QueueError::Backend("lock".to_string()))?
            .pop_front())
    }

    async fn depth(&self) -> Result<usize, QueueError> {
        Ok(self
            .queue
            .lock()
            .map_err(|_| QueueError::Backend("lock".to_string()))?
            .len())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn record(tok: &str) -> JobRecord {
        JobRecord {
            job_token: tok.to_string(),
            status: JobStatus::Queued,
            output_ids: vec![],
            created_at_ms: 0,
            expires_at_ms: 3_600_000,
        }
    }

    #[tokio::test]
    async fn persists_and_updates_status() {
        let b = InMemoryJobBackend::new();
        b.put(record("t1")).await.unwrap();
        assert_eq!(b.get("t1").await.unwrap().status, JobStatus::Queued);
        b.set_status("t1", JobStatus::Succeeded).await.unwrap();
        assert_eq!(b.get("t1").await.unwrap().status, JobStatus::Succeeded);
    }

    #[tokio::test]
    async fn fifo_queue_order() {
        let b = InMemoryJobBackend::new();
        b.enqueue("a").await.unwrap();
        b.enqueue("b").await.unwrap();
        assert_eq!(b.depth().await.unwrap(), 2);
        assert_eq!(b.dequeue().await.unwrap(), Some("a".to_string()));
        assert_eq!(b.dequeue().await.unwrap(), Some("b".to_string()));
        assert_eq!(b.dequeue().await.unwrap(), None);
    }

    #[tokio::test]
    async fn missing_job_is_not_found() {
        let b = InMemoryJobBackend::new();
        assert_eq!(b.get("nope").await, Err(QueueError::NotFound));
    }
}

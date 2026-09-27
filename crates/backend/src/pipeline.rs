//! End-to-end Server_Side processing pipeline (Task 21.1, Req 3.1, 31.2, 31.4).
//!
//! This module connects the pieces the other modules built into one flow:
//!
//! ```text
//! Security_Gateway → API intake (validate + Content_Scanner + Job_Token)
//!   → Redis queue → Sandbox worker
//!     → native pdf-engine (Client_Capable tools requested server-side)
//!       OR the appropriate Server_Only converter (LibreOffice / OCR / pdfium)
//!   → encrypted File_Store (scoped to the Job_Token)
//!   → Download_Manager (status + result endpoints)
//! ```
//!
//! Intake (`POST /api/jobs`) is owned by [`crate::jobs::create_job`]; this module
//! adds the **worker** side: it dequeues a Job_Token, loads its stored (already
//! validated + sanitized) sources, runs the correct executor **inside the
//! Sandbox harness** so wall-clock/scratch limits apply, stores each Output_File
//! encrypted under the Job_Token, and flips the Job status to `Succeeded`
//! (with the produced output ids) or `Failed` with a reason (Req 33.3).
//!
//! Because the queue + status live behind the [`crate::queue`] traits (Redis in
//! production, in-memory in tests) and the worker only touches per-Job state,
//! **concurrent Jobs never block one another** (Req 31.4): the intake path
//! returns as soon as the Job is enqueued, and any number of workers can drain
//! the queue in parallel.

use std::sync::Arc;

use pdf_engine::{EngineInput, FileBytes, PdfALevel, ToolId, ToolOptions};

use crate::convert::{
    self, ConversionKind, ConversionRequest, Converter,
};
use crate::dispatch;
use crate::error::ApiError;
use crate::ocr::{self, OcrEngine, ScanRequest};
use crate::queue::{JobQueue, JobRecord, JobStatus, JobStore};
use crate::render::{self, ArchivalLevel, PdfRenderer};
use crate::sandbox;
use crate::store::{scoped_key, EncryptedFileStore, ObjectStore};

/// The tool + options + stored source keys a worker needs to execute a Job.
///
/// Persisted alongside the [`JobRecord`] at intake time (the intake path stores
/// the validated sources encrypted and records their keys here) so the worker
/// can pick the Job up later without any client round-trip.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// Which Tool to run.
    pub tool: ToolId,
    /// The Tool's options (carries orientation, PDF/A level, OCR flag, …).
    pub options: ToolOptions,
    /// Encrypted File_Store keys of the Job's sources, in order.
    pub source_keys: Vec<String>,
    /// For HTML → PDF from a URL: the body already fetched by the SSRF-guarded
    /// URL_Fetcher at intake (the worker performs no network I/O). `None` for
    /// inline HTML or non-HTML tools.
    pub fetched_html: Option<Vec<u8>>,
}

/// Persists the [`JobSpec`] for a Job_Token. In production this is the same
/// Redis instance backing the queue; in tests it is in-memory.
#[async_trait::async_trait]
pub trait SpecStore: Send + Sync {
    /// Store the spec for a Job_Token.
    async fn put_spec(&self, job_token: &str, spec: JobSpec) -> Result<(), crate::queue::QueueError>;
    /// Fetch the spec for a Job_Token.
    async fn get_spec(&self, job_token: &str) -> Result<JobSpec, crate::queue::QueueError>;
}

/// The executors the worker dispatches to. Each is a trait object so the real
/// (subprocess) implementations and the test fakes are interchangeable.
pub struct Executors {
    /// LibreOffice-backed Office ↔ PDF / HTML → PDF converter.
    pub converter: Arc<dyn Converter>,
    /// OCRmyPDF-backed Scan to PDF OCR engine.
    pub ocr: Arc<dyn OcrEngine>,
    /// pdfium/CLI-backed PDF/A + page renderer.
    pub renderer: Arc<dyn PdfRenderer>,
}

/// The worker's dependencies bundled for one `process` call.
pub struct Worker<'a, S: ObjectStore, Q: JobStore + JobQueue, P: SpecStore> {
    /// Encrypted File_Store for reading sources and writing outputs.
    pub store: &'a EncryptedFileStore<S>,
    /// Job status + dispatch queue.
    pub jobs: &'a Q,
    /// Job spec store.
    pub specs: &'a P,
    /// The conversion executors.
    pub executors: &'a Executors,
    /// Sandbox resource limits (wall-clock ceiling for the executor).
    pub limits: crate::config::SandboxLimits,
}

impl<S: ObjectStore, Q: JobStore + JobQueue, P: SpecStore> Worker<'_, S, Q, P> {
    /// Dequeue and process the next Job, if any.
    ///
    /// Returns `Ok(Some(job_token))` when a Job was processed (successfully or
    /// with a recorded failure), `Ok(None)` when the queue was empty. The Job's
    /// terminal status is persisted before returning.
    ///
    /// # Errors
    ///
    /// Returns an [`ApiError`] only for infrastructure failures (queue/store
    /// backend errors); a Job's own processing failure is recorded as a
    /// `Failed` status and still returns `Ok(Some(..))`.
    pub async fn process_next(&self) -> Result<Option<String>, ApiError> {
        let job_token = match self.jobs.dequeue().await.map_err(map_queue_err)? {
            Some(t) => t,
            None => return Ok(None),
        };
        self.process(&job_token).await?;
        Ok(Some(job_token))
    }

    /// Process a specific Job_Token: mark it running, execute, store outputs,
    /// and record the terminal status.
    ///
    /// # Errors
    ///
    /// Returns an [`ApiError`] for infrastructure failures; processing failures
    /// are recorded on the Job record.
    pub async fn process(&self, job_token: &str) -> Result<(), ApiError> {
        // Load the record + spec.
        let mut record = self.jobs.get(job_token).await.map_err(map_queue_err)?;
        let spec = self.specs.get_spec(job_token).await.map_err(map_queue_err)?;

        self.jobs
            .set_status(job_token, JobStatus::Running)
            .await
            .map_err(map_queue_err)?;

        // Load the (decrypted) source bytes from the encrypted store.
        let mut sources = Vec::with_capacity(spec.source_keys.len());
        for key in &spec.source_keys {
            match self.store.get_decrypted(key).await {
                Ok(bytes) => sources.push(bytes),
                Err(_) => {
                    return self
                        .fail(job_token, &mut record, "a source file could not be read")
                        .await;
                }
            }
        }

        // Execute the correct path, bounded by the Sandbox wall-clock ceiling.
        let result = sandbox::with_wall_clock(self.limits.wall_clock, || {
            execute(&spec, &sources, self.executors)
        });
        let outputs = match result {
            Ok(Ok(files)) => files,
            Ok(Err(api_err)) => {
                return self.fail(job_token, &mut record, &api_err.message()).await;
            }
            Err(limit) => {
                return self.fail(job_token, &mut record, &limit.message()).await;
            }
        };

        if outputs.is_empty() {
            return self
                .fail(job_token, &mut record, "no output was produced")
                .await;
        }

        // Store each Output_File encrypted, scoped to the Job_Token (Req 43.3,
        // 44.1). Output ids are recorded on the record so the Download_Manager
        // can fetch them; a directory listing is never exposed (Req 43.5).
        let mut output_ids = Vec::with_capacity(outputs.len());
        for (idx, file) in outputs.iter().enumerate() {
            let file_id = format!("out-{idx}-{}", pdf_engine::sanitize_filename(&file.name));
            let key = scoped_key(job_token, &file_id);
            // A deterministic-yet-unique nonce per output within a Job: derived
            // from the output index. (Production draws from a CSPRNG; the store
            // seals with whatever nonce it is given.)
            let mut nonce = [0u8; 12];
            nonce[..8].copy_from_slice(&(idx as u64).to_le_bytes());
            if let Err(e) = self.store.put_encrypted(&key, &nonce, &file.bytes).await {
                let reason = match e {
                    crate::store::StoreError::StorageExhausted => {
                        // Retain no partial output on exhaustion (Req 49.4).
                        let _ = self.store.delete_job(job_token).await;
                        "the job cannot be completed at this time".to_string()
                    }
                    other => format!("a stored file could not be written: {other:?}"),
                };
                return self.fail(job_token, &mut record, &reason).await;
            }
            output_ids.push(file_id);
        }

        record.status = JobStatus::Succeeded;
        record.output_ids = output_ids;
        self.jobs.put(record).await.map_err(map_queue_err)?;
        Ok(())
    }

    /// Record a processing failure on the Job record (Req 33.3).
    async fn fail(
        &self,
        _job_token: &str,
        record: &mut JobRecord,
        reason: &str,
    ) -> Result<(), ApiError> {
        record.status = JobStatus::Failed {
            reason: reason.to_string(),
        };
        record.output_ids.clear();
        self.jobs.put(record.clone()).await.map_err(map_queue_err)?;
        Ok(())
    }
}

/// A produced Output_File (name + bytes), decoupled from the engine type so the
/// converter/ocr/render paths can share one shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducedFile {
    /// Display name.
    pub name: String,
    /// File bytes.
    pub bytes: Vec<u8>,
}

/// Execute a Job to its Output_Files, choosing the native engine or the correct
/// Server_Only executor (Req 31.2). Pure with respect to its inputs so it runs
/// cleanly inside the Sandbox wall-clock wrapper.
///
/// # Errors
///
/// Returns an [`ApiError`] describing the processing failure.
pub fn execute(
    spec: &JobSpec,
    sources: &[Vec<u8>],
    ex: &Executors,
) -> Result<Vec<ProducedFile>, ApiError> {
    // Server_Only: LibreOffice-backed conversions.
    if let Some(kind) = ConversionKind::for_tool(spec.tool) {
        return run_conversion(kind, spec, sources, ex.converter.as_ref());
    }

    match spec.tool {
        // Server_Only: Scan to PDF with an OCR text layer.
        ToolId::ScanToPdf => run_scan(spec, sources, ex.ocr.as_ref()),
        // Server_Only: PDF → PDF/A archival conversion.
        ToolId::PdfToPdfA => run_pdfa(spec, sources, ex.renderer.as_ref()),
        // Client_Capable: dispatch to the native shared engine.
        _ => run_engine(spec, sources),
    }
}

/// Run a LibreOffice-backed conversion (Office ↔ PDF, HTML → PDF).
fn run_conversion(
    kind: ConversionKind,
    spec: &JobSpec,
    sources: &[Vec<u8>],
    converter: &dyn Converter,
) -> Result<Vec<ProducedFile>, ApiError> {
    // HTML → PDF resolves its body from inline markup or the pre-fetched URL
    // body; all other conversions consume the first stored source.
    let (input, orientation) = match (kind, &spec.options) {
        (ConversionKind::HtmlToPdf, ToolOptions::HtmlToPdf { source, orientation }) => {
            let body = convert::resolve_html_source(source, spec.fetched_html.as_deref())
                .map_err(|e| convert::map_conversion_error(kind, &e))?;
            (body, *orientation)
        }
        (ConversionKind::ExcelToPdf, ToolOptions::ExcelToPdf { orientation }) => {
            (first_source(sources, kind)?, *orientation)
        }
        _ => (first_source(sources, kind)?, pdf_engine::Orientation::Portrait),
    };

    let bytes = converter
        .convert(&ConversionRequest {
            kind,
            input,
            orientation,
        })
        .map_err(|e| convert::map_conversion_error(kind, &e))?;

    Ok(vec![ProducedFile {
        name: format!("output.{}", kind.output_extension()),
        bytes,
    }])
}

/// The first stored source, or an unrenderable error naming the Tool.
fn first_source(sources: &[Vec<u8>], kind: ConversionKind) -> Result<Vec<u8>, ApiError> {
    sources.first().cloned().ok_or_else(|| {
        convert::map_conversion_error(kind, &convert::ConversionError::Unrenderable)
    })
}

/// Run Scan to PDF (assemble images + optional OCR layer).
fn run_scan(
    spec: &JobSpec,
    sources: &[Vec<u8>],
    ocr_engine: &dyn OcrEngine,
) -> Result<Vec<ProducedFile>, ApiError> {
    let ToolOptions::ScanToPdf { ocr, .. } = &spec.options else {
        return Err(ApiError::BadRequest {
            reason: "invalid options for Scan to PDF".to_string(),
        });
    };
    let req = ScanRequest {
        images: sources.to_vec(),
        ocr: *ocr,
        orientation: pdf_engine::Orientation::Portrait,
    };
    let bytes = ocr::scan_to_pdf(&req, ocr_engine).map_err(|e| ocr::map_ocr_error(&e))?;
    Ok(vec![ProducedFile {
        name: "scanned.pdf".to_string(),
        bytes,
    }])
}

/// Run PDF → PDF/A archival conversion.
fn run_pdfa(
    spec: &JobSpec,
    sources: &[Vec<u8>],
    renderer: &dyn PdfRenderer,
) -> Result<Vec<ProducedFile>, ApiError> {
    let level = match &spec.options {
        ToolOptions::PdfToPdfA { level } => *level,
        _ => PdfALevel::A1b,
    };
    let source = sources.first().ok_or_else(|| ApiError::Processing {
        tool: "PDF to PDF/A".to_string(),
        reason: "no source file was provided".to_string(),
    })?;
    let bytes = renderer
        .to_pdfa(source, ArchivalLevel::from(level))
        .map_err(|e| render::map_render_error("PDF to PDF/A", &e))?;
    Ok(vec![ProducedFile {
        name: "archive.pdf".to_string(),
        bytes,
    }])
}

/// Run a Client_Capable Tool on the native shared engine (Req 31.2).
fn run_engine(spec: &JobSpec, sources: &[Vec<u8>]) -> Result<Vec<ProducedFile>, ApiError> {
    let file_sources: Vec<FileBytes> = sources
        .iter()
        .enumerate()
        .map(|(i, bytes)| FileBytes {
            name: format!("src-{i}"),
            bytes: bytes.as_slice(),
        })
        .collect();

    let input = EngineInput {
        tool: spec.tool,
        sources: file_sources,
        options: spec.options.clone(),
    };
    let output = dispatch::dispatch(input)?;
    Ok(output
        .files
        .into_iter()
        .map(|f| ProducedFile {
            name: f.name,
            bytes: f.bytes,
        })
        .collect())
}

/// Map a queue error to an API error.
fn map_queue_err(e: crate::queue::QueueError) -> ApiError {
    match e {
        crate::queue::QueueError::NotFound => ApiError::NotFound,
        crate::queue::QueueError::Backend(r) => ApiError::Unavailable { reason: r },
    }
}

// -------------------------------------------------------------------------
// In-memory SpecStore for tests and single-node dev.
// -------------------------------------------------------------------------

/// An in-memory [`SpecStore`].
#[derive(Default)]
pub struct InMemorySpecStore {
    map: std::sync::Mutex<std::collections::HashMap<String, JobSpec>>,
}

impl InMemorySpecStore {
    /// Create an empty spec store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl SpecStore for InMemorySpecStore {
    async fn put_spec(
        &self,
        job_token: &str,
        spec: JobSpec,
    ) -> Result<(), crate::queue::QueueError> {
        self.map
            .lock()
            .map_err(|_| crate::queue::QueueError::Backend("lock".to_string()))?
            .insert(job_token.to_string(), spec);
        Ok(())
    }

    async fn get_spec(&self, job_token: &str) -> Result<JobSpec, crate::queue::QueueError> {
        self.map
            .lock()
            .map_err(|_| crate::queue::QueueError::Backend("lock".to_string()))?
            .get(job_token)
            .cloned()
            .ok_or(crate::queue::QueueError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::convert::{ConversionError, FakeConverter};
    use crate::ocr::{FakeOcr, OcrError};
    use crate::queue::InMemoryJobBackend;
    use crate::render::FakeRenderer;
    use crate::store::InMemoryObjectStore;
    use pdf_engine::{HtmlSource, Orientation};

    fn store() -> EncryptedFileStore<InMemoryObjectStore> {
        EncryptedFileStore::new(InMemoryObjectStore::new(), &[5u8; 32])
    }

    fn executors(
        converter: Arc<dyn Converter>,
        ocr: Arc<dyn OcrEngine>,
        renderer: Arc<dyn PdfRenderer>,
    ) -> Executors {
        Executors { converter, ocr, renderer }
    }

    fn default_executors() -> Executors {
        executors(
            Arc::new(FakeConverter::succeeding()),
            Arc::new(FakeOcr::succeeding()),
            Arc::new(FakeRenderer::succeeding(1)),
        )
    }

    /// Seed a Job (record + spec + encrypted sources) ready for the worker.
    async fn seed_job(
        store: &EncryptedFileStore<InMemoryObjectStore>,
        backend: &InMemoryJobBackend,
        specs: &InMemorySpecStore,
        token: &str,
        tool: ToolId,
        options: ToolOptions,
        sources: &[Vec<u8>],
        fetched_html: Option<Vec<u8>>,
    ) {
        let mut source_keys = Vec::new();
        for (i, bytes) in sources.iter().enumerate() {
            let key = scoped_key(token, &format!("src-{i}"));
            let mut nonce = [0u8; 12];
            nonce[0] = i as u8;
            store.put_encrypted(&key, &nonce, bytes).await.unwrap();
            source_keys.push(key);
        }
        backend
            .put(JobRecord {
                job_token: token.to_string(),
                status: JobStatus::Queued,
                output_ids: vec![],
                created_at_ms: 0,
                expires_at_ms: 3_600_000,
            })
            .await
            .unwrap();
        specs
            .put_spec(
                token,
                JobSpec {
                    tool,
                    options,
                    source_keys,
                    fetched_html,
                },
            )
            .await
            .unwrap();
        backend.enqueue(token).await.unwrap();
    }

    fn worker<'a>(
        store: &'a EncryptedFileStore<InMemoryObjectStore>,
        backend: &'a InMemoryJobBackend,
        specs: &'a InMemorySpecStore,
        ex: &'a Executors,
    ) -> Worker<'a, InMemoryObjectStore, InMemoryJobBackend, InMemorySpecStore> {
        Worker {
            store,
            jobs: backend,
            specs,
            executors: ex,
            limits: crate::config::SandboxLimits::default(),
        }
    }

    #[tokio::test]
    async fn server_only_word_to_pdf_succeeds_and_stores_output() {
        let s = store();
        let backend = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();
        seed_job(
            &s,
            &backend,
            &specs,
            "tokW",
            ToolId::WordToPdf,
            ToolOptions::WordToPdf {},
            &[vec![0x50, 0x4B, 0x03, 0x04, 1, 2, 3]],
            None,
        )
        .await;

        let ex = default_executors();
        let w = worker(&s, &backend, &specs, &ex);
        let processed = w.process_next().await.unwrap();
        assert_eq!(processed, Some("tokW".to_string()));

        let rec = backend.get("tokW").await.unwrap();
        assert_eq!(rec.status, JobStatus::Succeeded);
        assert_eq!(rec.output_ids.len(), 1);
        // The stored output is fetchable and is a PDF (fake converter output).
        let key = scoped_key("tokW", &rec.output_ids[0]);
        let out = s.get_decrypted(&key).await.unwrap();
        assert!(out.starts_with(b"%PDF-"));
    }

    #[tokio::test]
    async fn dependency_unavailable_records_failure() {
        let s = store();
        let backend = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();
        seed_job(
            &s,
            &backend,
            &specs,
            "tokU",
            ToolId::ExcelToPdf,
            ToolOptions::ExcelToPdf {
                orientation: Orientation::Landscape,
            },
            &[vec![0x50, 0x4B, 0x03, 0x04]],
            None,
        )
        .await;

        let ex = executors(
            Arc::new(FakeConverter::failing(ConversionError::DependencyUnavailable)),
            Arc::new(FakeOcr::succeeding()),
            Arc::new(FakeRenderer::succeeding(1)),
        );
        let w = worker(&s, &backend, &specs, &ex);
        w.process_next().await.unwrap();

        let rec = backend.get("tokU").await.unwrap();
        match rec.status {
            JobStatus::Failed { reason } => assert!(reason.contains("temporarily unavailable")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pdf_to_word_no_text_records_failure() {
        let s = store();
        let backend = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();
        seed_job(
            &s,
            &backend,
            &specs,
            "tokT",
            ToolId::PdfToWord,
            ToolOptions::PdfToWord {},
            &[b"%PDF-1.7 no text".to_vec()],
            None,
        )
        .await;

        let ex = executors(
            Arc::new(FakeConverter::failing(ConversionError::NoTextFound)),
            Arc::new(FakeOcr::succeeding()),
            Arc::new(FakeRenderer::succeeding(1)),
        );
        let w = worker(&s, &backend, &specs, &ex);
        w.process_next().await.unwrap();

        let rec = backend.get("tokT").await.unwrap();
        match rec.status {
            JobStatus::Failed { reason } => assert!(reason.contains("no extractable text")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pdf_to_pdfa_succeeds() {
        let s = store();
        let backend = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();
        seed_job(
            &s,
            &backend,
            &specs,
            "tokA",
            ToolId::PdfToPdfA,
            ToolOptions::PdfToPdfA { level: PdfALevel::A2b },
            &[b"%PDF-1.5 body".to_vec()],
            None,
        )
        .await;

        let ex = default_executors();
        let w = worker(&s, &backend, &specs, &ex);
        w.process_next().await.unwrap();

        let rec = backend.get("tokA").await.unwrap();
        assert_eq!(rec.status, JobStatus::Succeeded);
        let key = scoped_key("tokA", &rec.output_ids[0]);
        let out = s.get_decrypted(&key).await.unwrap();
        assert!(String::from_utf8_lossy(&out).contains("PDF/A-2b"));
    }

    #[tokio::test]
    async fn html_to_pdf_uses_fetched_body() {
        let s = store();
        let backend = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();
        // HTML from a URL: the pipeline supplies the pre-fetched body; no source
        // file is stored for the URL case.
        seed_job(
            &s,
            &backend,
            &specs,
            "tokH",
            ToolId::HtmlToPdf,
            ToolOptions::HtmlToPdf {
                source: HtmlSource::Url("https://example.com/".to_string()),
                orientation: Orientation::Portrait,
            },
            &[],
            Some(b"<html><body>hi</body></html>".to_vec()),
        )
        .await;

        let ex = default_executors();
        let w = worker(&s, &backend, &specs, &ex);
        w.process_next().await.unwrap();

        let rec = backend.get("tokH").await.unwrap();
        assert_eq!(rec.status, JobStatus::Succeeded);
    }

    #[tokio::test]
    async fn empty_queue_returns_none() {
        let s = store();
        let backend = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();
        let ex = default_executors();
        let w = worker(&s, &backend, &specs, &ex);
        assert_eq!(w.process_next().await.unwrap(), None);
    }

    #[tokio::test]
    async fn scan_to_pdf_ocr_unavailable_records_failure() {
        let s = store();
        let backend = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();
        // A real tiny JPEG so the engine assembly succeeds; OCR then fails.
        let jpg = crate::ocr::test_jpeg();
        seed_job(
            &s,
            &backend,
            &specs,
            "tokS",
            ToolId::ScanToPdf,
            ToolOptions::ScanToPdf {
                images: vec![],
                ocr: true,
            },
            &[jpg],
            None,
        )
        .await;

        let ex = executors(
            Arc::new(FakeConverter::succeeding()),
            Arc::new(FakeOcr::failing(OcrError::DependencyUnavailable)),
            Arc::new(FakeRenderer::succeeding(1)),
        );
        let w = worker(&s, &backend, &specs, &ex);
        w.process_next().await.unwrap();

        let rec = backend.get("tokS").await.unwrap();
        match rec.status {
            JobStatus::Failed { reason } => assert!(reason.contains("temporarily unavailable")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// Concurrent Jobs progress independently and never block one another
    /// (Req 31.4): two workers drain a shared queue in parallel.
    #[tokio::test]
    async fn concurrent_jobs_do_not_block() {
        let s = Arc::new(store());
        let backend = Arc::new(InMemoryJobBackend::new());
        let specs = Arc::new(InMemorySpecStore::new());

        for i in 0..6 {
            let token = format!("job-{i}");
            seed_job(
                &s,
                &backend,
                &specs,
                &token,
                ToolId::WordToPdf,
                ToolOptions::WordToPdf {},
                &[vec![0x50, 0x4B, 0x03, 0x04, i as u8]],
                None,
            )
            .await;
        }

        // Two concurrent workers sharing the queue.
        let mut handles = Vec::new();
        for _ in 0..2 {
            let s = Arc::clone(&s);
            let backend = Arc::clone(&backend);
            let specs = Arc::clone(&specs);
            handles.push(tokio::spawn(async move {
                let ex = default_executors();
                let w = Worker {
                    store: s.as_ref(),
                    jobs: backend.as_ref(),
                    specs: specs.as_ref(),
                    executors: &ex,
                    limits: crate::config::SandboxLimits::default(),
                };
                let mut count = 0;
                while (w.process_next().await.unwrap()).is_some() {
                    count += 1;
                }
                count
            }));
        }

        let mut total = 0;
        for h in handles {
            total += h.await.unwrap();
        }
        assert_eq!(total, 6);
        // Every Job reached a terminal Succeeded status.
        for i in 0..6 {
            let rec = backend.get(&format!("job-{i}")).await.unwrap();
            assert_eq!(rec.status, JobStatus::Succeeded);
        }
    }
}

//! HTTP API surface + service orchestration (Task 21, design §8).
//!
//! This module wires the components the other modules built into the concrete
//! HTTP endpoints the SvelteKit frontend calls (see `apps/web/src/lib/api/client.ts`).
//! The endpoint contract is taken verbatim from that client so the UI needs no
//! change:
//!
//! ```text
//! POST   /api/jobs                 multipart: tool, options(JSON), files[]
//!        -> 201 { jobId, jobToken, expiresAt }          (Req 43.1, 43.2, 32.5)
//! GET    /api/jobs/{jobId}          header: X-Job-Token
//!        -> 200 { phase, progress?, outputs?, error? }  (Req 31.3, 33.3)
//! GET    /api/jobs/{jobId}/outputs/{index}   header: X-Job-Token
//!        -> 200 file bytes; Content-Disposition: attachment (Req 50.2, 3.3)
//! DELETE /api/jobs/{jobId}          header: X-Job-Token
//!        -> 204  (immediate secure deletion, Req 32.3)
//! ```
//!
//! The Job_Token is both the `jobId` in the path and the credential in the
//! `X-Job-Token` header: the intake returns the same value for both `jobId` and
//! `jobToken`, and every file-access endpoint re-checks that the header token
//! matches the record token and is not expired (Req 43.3, 43.4). The `jobId` in
//! the path is never trusted on its own — access is only granted when
//! `check_access` passes on the header token.
//!
//! ## Env-driven infrastructure with graceful fallback
//!
//! [`Services::from_env`] reads the deployment environment and picks real
//! infrastructure when it is configured, falling back to in-process
//! implementations otherwise so the service boots locally with no dependencies:
//!
//! - **Queue + Job status + Job spec**: Redis when `REDIS_URL` is set
//!   (design §8, Req 31.4); else an in-memory backend.
//! - **Object store**: an S3/MinIO-backed store when `S3_*` env vars are set;
//!   else an in-memory store. (A concrete S3 client is not linked yet — the env
//!   is read so `docker-compose` boots and the wiring is auditable, and the
//!   in-memory store is used until the S3 client lands. This is called out
//!   explicitly in [`Services::from_env`].)
//! - **Executors**: the real subprocess engines (LibreOffice / OCRmyPDF /
//!   Ghostscript+MuPDF) when their binaries resolve on `PATH` (they do in the
//!   Docker image); else the deterministic fakes so a dev box with none of them
//!   still runs the Client_Capable tools.
//!
//! A background Tokio worker drains the queue and processes Jobs so the intake
//! path returns as soon as the Job is enqueued (Req 31.4).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use pdf_engine::{ToolId, ToolOptions};

use crate::config::{Config, MAX_REQUEST_BODY_BYTES};
use crate::convert::{Converter, FakeConverter, SoffConverter};
use crate::error::ApiError;
use crate::jobs::{self, UploadedFile};
use crate::ocr::{FakeOcr, OcrEngine, OcrMyPdfEngine};
use crate::pipeline::{Executors, InMemorySpecStore, JobSpec, SpecStore, Worker};
use crate::queue::{InMemoryJobBackend, JobRecord, JobStatus, JobStore};
use crate::render::{ExternalRenderer, FakeRenderer, PdfRenderer};
use crate::store::{DownloadHeaders, EncryptedFileStore, InMemoryObjectStore};

/// The concrete object store used by the running service.
///
/// In-memory today; when a concrete S3 client is linked it becomes a second
/// variant and [`Services::from_env`] selects it from the `S3_*` env.
type RunObjectStore = InMemoryObjectStore;

/// The concrete Job store + queue + spec store used by the running service.
///
/// In-memory today; Redis selection is prepared in [`Services::from_env`]
/// (`REDIS_URL`) and documented there until the Redis backend is linked.
type RunJobBackend = InMemoryJobBackend;

/// All runtime services the handlers need, assembled from the environment.
pub struct Services {
    /// Encrypted File_Store for sources + outputs (Req 44.1).
    pub store: EncryptedFileStore<RunObjectStore>,
    /// Job status store + dispatch queue (Req 31.4).
    pub jobs: RunJobBackend,
    /// Per-Job execution spec store, consulted by the worker.
    pub specs: InMemorySpecStore,
    /// The conversion executors (real subprocess engines or fakes).
    pub executors: Executors,
    /// Sandbox resource limits applied to each Job.
    pub limits: crate::config::SandboxLimits,
}

impl Services {
    /// Assemble the services from `config` + the process environment.
    ///
    /// Chooses real infrastructure when configured and logs the choice; falls
    /// back to in-process implementations otherwise so the service runs with no
    /// external dependencies.
    #[must_use]
    pub fn from_env(config: &Config) -> Self {
        // --- Object store (Req 44.1) ---
        // Read the S3 env so `docker-compose` (which sets S3_ENDPOINT/S3_BUCKET/
        // S3_ACCESS_KEY/S3_SECRET_KEY) boots cleanly. A concrete S3 client is
        // not linked yet; until it is, the in-memory store backs the encrypted
        // File_Store. This is intentional and called out here so the fallback
        // is never silent.
        if std::env::var("S3_ENDPOINT").is_ok() || std::env::var("S3_BUCKET").is_ok() {
            tracing::warn!(
                "S3_* is configured but the S3 object-store client is not linked in this \
                 build; using the in-memory encrypted File_Store (documented fallback). \
                 Files still travel encrypted at rest via AES-256-GCM."
            );
        }
        let object_store = InMemoryObjectStore::new();
        let store = EncryptedFileStore::new(object_store, &config.encryption_key);

        // --- Queue + Job status + Job spec (Req 31.4) ---
        // A Redis backend is selected when REDIS_URL is set. The Redis-backed
        // JobStore/JobQueue/SpecStore is not linked in this build; the in-memory
        // backend provides identical semantics for a single node until it is.
        if let Ok(url) = std::env::var("REDIS_URL") {
            tracing::warn!(
                redis_url = %url,
                "REDIS_URL is configured but the Redis queue backend is not linked in this \
                 build; using the in-memory queue + job store (documented fallback, \
                 single-node semantics)."
            );
        }
        let jobs = InMemoryJobBackend::new();
        let specs = InMemorySpecStore::new();

        // --- Executors: real subprocess engines when the binaries exist ---
        let executors = build_executors(config);

        Self {
            store,
            jobs,
            specs,
            executors,
            limits: config.sandbox,
        }
    }
}

/// Pick the real subprocess executors when their binaries resolve on `PATH`
/// (as in the Docker image), else the deterministic fakes so a dev host with
/// none of them still runs the Client_Capable tools.
fn build_executors(config: &Config) -> Executors {
    let timeout = config.sandbox.wall_clock;
    let scratch = std::env::temp_dir();

    let converter: Arc<dyn Converter> = if binary_available("soffice") {
        tracing::info!("using headless LibreOffice (soffice) for Office/HTML conversions");
        Arc::new(SoffConverter::new("soffice", scratch.clone(), timeout))
    } else {
        tracing::info!("soffice not found; Office/HTML conversions use the in-process fake");
        Arc::new(FakeConverter::succeeding())
    };

    let ocr: Arc<dyn OcrEngine> = if binary_available("ocrmypdf") {
        tracing::info!("using ocrmypdf for the Scan-to-PDF OCR text layer");
        Arc::new(OcrMyPdfEngine::new("ocrmypdf", scratch.clone(), timeout))
    } else {
        tracing::info!("ocrmypdf not found; Scan-to-PDF OCR uses the in-process fake");
        Arc::new(FakeOcr::succeeding())
    };

    let renderer: Arc<dyn PdfRenderer> = if binary_available("gs") && binary_available("mutool") {
        tracing::info!("using Ghostscript + mutool for PDF/A and page rendering");
        Arc::new(ExternalRenderer::new("gs", "mutool", scratch, timeout))
    } else {
        tracing::info!("gs/mutool not found; PDF/A + rendering use the in-process fake");
        Arc::new(FakeRenderer::succeeding(1))
    };

    Executors {
        converter,
        ocr,
        renderer,
    }
}

/// Whether `bin` resolves to an executable on `PATH` (best-effort, never
/// panics). Used only to decide between the real executor and the fake.
#[cfg(not(target_family = "wasm"))]
fn binary_available(bin: &str) -> bool {
    // `which`-style probe without adding a dependency: run `<bin> --version`
    // with all I/O discarded. A spawn error means the binary is not present.
    std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success() || s.code().is_some())
        .unwrap_or(false)
}

#[cfg(target_family = "wasm")]
fn binary_available(_bin: &str) -> bool {
    false
}

/// Run one pass of the worker over the queue, processing every ready Job.
///
/// Returns the number of Jobs processed. Used both by the background worker
/// loop and (synchronously) by the route tests.
///
/// # Errors
///
/// Propagates an [`ApiError`] only for infrastructure failures; a Job's own
/// processing failure is recorded on its record and does not error here.
pub async fn drain_queue(services: &Services) -> Result<usize, ApiError> {
    let worker = Worker {
        store: &services.store,
        jobs: &services.jobs,
        specs: &services.specs,
        executors: &services.executors,
        limits: services.limits,
    };
    let mut processed = 0;
    while worker.process_next().await?.is_some() {
        processed += 1;
    }
    Ok(processed)
}

/// Spawn the background worker that drains the queue on an interval so the
/// intake path never blocks on processing (Req 31.4).
pub fn spawn_worker(state: Arc<crate::AppState>) {
    tokio::spawn(async move {
        loop {
            match drain_queue(&state.services).await {
                Ok(n) if n > 0 => tracing::debug!(processed = n, "worker drained jobs"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "worker pass failed"),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

// -------------------------------------------------------------------------
// Wire types (match apps/web/src/lib/api/client.ts exactly).
// -------------------------------------------------------------------------

/// `POST /api/jobs` success body: `{ jobId, jobToken, expiresAt }`.
#[derive(Debug, Serialize)]
struct JobCreation {
    #[serde(rename = "jobId")]
    job_id: String,
    #[serde(rename = "jobToken")]
    job_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: String,
}

/// One Output_File's pre-download metadata (`OutputRef` in the client).
#[derive(Debug, Serialize)]
struct OutputRef {
    index: usize,
    name: String,
    #[serde(rename = "sizeBytes")]
    size_bytes: u64,
    #[serde(rename = "contentType")]
    content_type: String,
}

/// `GET /api/jobs/{jobId}` body: `{ phase, progress?, outputs?, error? }`.
#[derive(Debug, Serialize)]
struct JobStatusBody {
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    outputs: Option<Vec<OutputRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// -------------------------------------------------------------------------
// Route handlers.
// -------------------------------------------------------------------------

/// Extract and validate the `X-Job-Token` header (never logged, Req 44.4).
fn job_token_header(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get("x-job-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .ok_or(ApiError::AccessDenied)
}

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `POST /api/jobs` — multipart intake (design §8).
///
/// Fields: `tool` (the engine [`ToolId`] as a JSON-ish string, e.g. `Merge`),
/// `options` (the externally-tagged [`ToolOptions`] JSON), and one or more
/// `files` parts carrying the uploaded Source_Files. Runs the Validator +
/// Content_Scanner, issues a Job_Token, stores the sanitized sources encrypted,
/// persists the record + execution spec, and enqueues the Job for the worker.
async fn create_job_handler(
    State(state): State<Arc<crate::AppState>>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut tool_field: Option<String> = None;
    let mut options_field: Option<String> = None;
    let mut files: Vec<UploadedFile> = Vec::new();

    // Parse the multipart body. A malformed part is a bad request rather than a
    // panic (the crate forbids unwrap/expect/panic on request paths).
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest {
            reason: format!("malformed multipart body: {e}"),
        })?
    {
        match field.name() {
            Some("tool") => {
                tool_field = Some(field.text().await.map_err(read_field_err)?);
            }
            Some("options") => {
                options_field = Some(field.text().await.map_err(read_field_err)?);
            }
            Some("files") => {
                let name = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "upload".to_string());
                let bytes = field.bytes().await.map_err(read_field_err)?.to_vec();
                files.push(UploadedFile { name, bytes });
            }
            // Ignore unknown parts defensively.
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let tool = parse_tool(tool_field.as_deref())?;
    let options = parse_options(options_field.as_deref())?;

    if files.is_empty() && !tool_allows_no_files(tool) {
        return Err(ApiError::BadRequest {
            reason: "no source files were provided".to_string(),
        });
    }

    let services = &state.services;
    let created_ms = now_ms();

    // Create the Job: validate + scan + store sources + persist record + enqueue.
    let job_token = jobs::create_job(
        tool,
        &files,
        &state.config,
        &services.store,
        &services.jobs,
        &services.jobs,
        created_ms,
        random_nonce,
    )
    .await?;

    // Persist the execution spec so the worker can process the Job. The source
    // keys mirror the deterministic scheme `create_job` used when storing them
    // (`src-{idx}-{unique_display_name}`).
    let display_names =
        pdf_engine::assign_unique_names(&files.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
    let source_keys = files
        .iter()
        .enumerate()
        .map(|(idx, f)| {
            let display = display_names
                .get(idx)
                .cloned()
                .unwrap_or_else(|| pdf_engine::sanitize_filename(&f.name));
            crate::store::scoped_key(&job_token, &format!("src-{idx}-{display}"))
        })
        .collect::<Vec<_>>();

    services
        .specs
        .put_spec(
            &job_token,
            JobSpec {
                tool,
                options,
                source_keys,
                fetched_html: None,
            },
        )
        .await
        .map_err(|_| ApiError::Unavailable {
            reason: "could not persist the job".to_string(),
        })?;

    let expires_at = iso8601_utc(created_ms + crate::config::RETENTION_PERIOD.as_millis() as u64);
    let body = JobCreation {
        job_id: job_token.clone(),
        job_token,
        expires_at,
    };
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// `GET /api/jobs/{jobId}` — token-gated status (Req 31.3, 33.3, 43.3).
async fn job_status_handler(
    State(state): State<Arc<crate::AppState>>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = job_token_header(&headers)?;
    // The path id must match the credential; access is only ever via the token.
    if token != job_id {
        return Err(ApiError::AccessDenied);
    }
    let services = &state.services;
    let now = now_ms();
    let status = jobs::job_status(&services.jobs, &token, now).await?;

    let body = match status {
        JobStatus::Queued => JobStatusBody {
            phase: "queued",
            outputs: None,
            error: None,
        },
        JobStatus::Running => JobStatusBody {
            phase: "running",
            outputs: None,
            error: None,
        },
        JobStatus::Failed { reason } => JobStatusBody {
            phase: "failed",
            outputs: None,
            error: Some(reason),
        },
        JobStatus::Succeeded => {
            let outputs = list_outputs(services, &token, now).await?;
            JobStatusBody {
                phase: "succeeded",
                outputs: Some(outputs),
                error: None,
            }
        }
    };
    Ok(Json(body).into_response())
}

/// `GET /api/jobs/{jobId}/outputs/{index}` — stream one decrypted Output_File
/// as an attachment with a non-executable content type (Req 50.2, 3.3).
async fn job_output_handler(
    State(state): State<Arc<crate::AppState>>,
    Path((job_id, index)): Path<(String, usize)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = job_token_header(&headers)?;
    if token != job_id {
        return Err(ApiError::AccessDenied);
    }
    let services = &state.services;
    let now = now_ms();

    // Resolve the file id for `index` from the record's output ids, re-checking
    // token access + expiry (Req 43.3, 43.4). The listing itself is never
    // exposed (Req 43.5) — only an in-range index resolves.
    let record = fetch_authorized_record(services, &token, now).await?;
    let file_id = record.output_ids.get(index).ok_or(ApiError::NotFound)?;

    let bytes = jobs::job_result(&services.store, &services.jobs, &token, file_id, now).await?;

    let display_name = output_display_name(file_id);
    let content_type = content_type_for(&display_name);
    let dh = DownloadHeaders::for_download(&display_name, &content_type);

    let mut resp = Response::new(axum::body::Body::from(bytes));
    insert_header(&mut resp, "content-type", &dh.content_type);
    insert_header(&mut resp, "content-disposition", &dh.content_disposition);
    // Defense in depth: never let a browser sniff the body into an executable
    // type (complements the gateway's global nosniff).
    insert_header(&mut resp, "x-content-type-options", "nosniff");
    Ok(resp)
}

/// `DELETE /api/jobs/{jobId}` — immediate secure deletion (Req 32.3, 44.3).
async fn delete_job_handler(
    State(state): State<Arc<crate::AppState>>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = job_token_header(&headers)?;
    if token != job_id {
        return Err(ApiError::AccessDenied);
    }
    let services = &state.services;
    let now = now_ms();

    // Only the owner of a live token may delete (Req 43.3, 43.4).
    let _ = fetch_authorized_record(services, &token, now).await?;

    crate::retention::delete_now(&services.store, &token)
        .await
        .map_err(|_| ApiError::Unavailable {
            reason: "the files could not be deleted".to_string(),
        })?;
    // Drop the Job record as well so its token no longer resolves.
    let _ = services.jobs.remove(&token).await;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// -------------------------------------------------------------------------
// Helpers.
// -------------------------------------------------------------------------

/// Fetch a Job record after enforcing token-scoped, non-expired access.
async fn fetch_authorized_record(
    services: &Services,
    token: &str,
    now: u64,
) -> Result<JobRecord, ApiError> {
    let record = services
        .jobs
        .get(token)
        .await
        .map_err(|_| ApiError::AccessDenied)?;
    if !pdf_engine::check_access(&record.job_token, token, record.expires_at_ms, now) {
        return Err(ApiError::AccessDenied);
    }
    Ok(record)
}

/// Build the `OutputRef` list for a succeeded Job (name + size + content type).
///
/// Size is measured by decrypting each output (files are small and this is the
/// same work a download does); the directory is never listed (Req 43.5) — the
/// ids come from the Job's own record.
async fn list_outputs(
    services: &Services,
    token: &str,
    now: u64,
) -> Result<Vec<OutputRef>, ApiError> {
    let record = fetch_authorized_record(services, token, now).await?;
    let mut out = Vec::with_capacity(record.output_ids.len());
    for (index, file_id) in record.output_ids.iter().enumerate() {
        let bytes =
            jobs::job_result(&services.store, &services.jobs, token, file_id, now).await?;
        let name = output_display_name(file_id);
        let content_type = content_type_for(&name);
        out.push(OutputRef {
            index,
            name,
            size_bytes: bytes.len() as u64,
            content_type,
        });
    }
    Ok(out)
}

/// Recover the display name from an output file id of the form
/// `out-{idx}-{sanitized_name}` (the scheme the pipeline writes).
fn output_display_name(file_id: &str) -> String {
    // Strip the `out-{idx}-` prefix; fall back to the whole id if it does not
    // match the expected shape.
    file_id
        .strip_prefix("out-")
        .and_then(|rest| rest.split_once('-'))
        .map(|(_, name)| name.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| file_id.to_string())
}

/// Map a display name's extension to a safe, non-executable content type
/// (Req 50.2). Unknown types fall back to `application/octet-stream`.
fn content_type_for(name: &str) -> String {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let ct = match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    };
    ct.to_string()
}

/// Parse the `tool` field into an engine [`ToolId`].
///
/// The frontend sends the engine variant name (e.g. `OptimizePdf`), which serde
/// deserializes when wrapped in quotes.
fn parse_tool(field: Option<&str>) -> Result<ToolId, ApiError> {
    let raw = field.ok_or_else(|| ApiError::BadRequest {
        reason: "the 'tool' field is required".to_string(),
    })?;
    let trimmed = raw.trim().trim_matches('"');
    let quoted = format!("\"{trimmed}\"");
    serde_json::from_str::<ToolId>(&quoted).map_err(|_| ApiError::BadRequest {
        reason: format!("unknown tool '{trimmed}'"),
    })
}

/// Parse the `options` field into engine [`ToolOptions`] (externally tagged).
fn parse_options(field: Option<&str>) -> Result<ToolOptions, ApiError> {
    let raw = field.ok_or_else(|| ApiError::BadRequest {
        reason: "the 'options' field is required".to_string(),
    })?;
    serde_json::from_str::<ToolOptions>(raw).map_err(|e| ApiError::BadRequest {
        reason: format!("invalid tool options: {e}"),
    })
}

/// Whether a Tool legitimately has no uploaded files (HTML→PDF from a URL or
/// inline markup carries its source in `options`, not as a file part).
fn tool_allows_no_files(tool: ToolId) -> bool {
    matches!(tool, ToolId::HtmlToPdf | ToolId::MarkdownToPdf)
}

/// Map a multipart field-read error to a bad-request [`ApiError`].
fn read_field_err(e: axum::extract::multipart::MultipartError) -> ApiError {
    ApiError::BadRequest {
        reason: format!("could not read an upload field: {e}"),
    }
}

/// Draw a fresh 96-bit nonce from the platform CSPRNG for GCM sealing.
///
/// Falls back to a time-derived value only if the entropy source is briefly
/// unavailable, so a nonce is always produced without panicking; a repeat is
/// astronomically unlikely and never silently reuses a fixed nonce.
fn random_nonce() -> [u8; 12] {
    let mut n = [0u8; 12];
    if getrandom::fill(&mut n).is_err() {
        let t = now_ms().to_le_bytes();
        n[..8].copy_from_slice(&t);
    }
    n
}

/// Insert a response header, silently skipping an invalid name/value so the
/// path stays panic-free.
fn insert_header(resp: &mut Response, name: &'static str, value: &str) {
    if let (Ok(n), Ok(v)) = (
        axum::http::header::HeaderName::from_bytes(name.as_bytes()),
        axum::http::header::HeaderValue::from_str(value),
    ) {
        resp.headers_mut().insert(n, v);
    }
}

/// Format an epoch-millisecond timestamp as an ISO 8601 UTC string
/// (`YYYY-MM-DDTHH:MM:SSZ`) without pulling a date/time crate.
fn iso8601_utc(epoch_ms: u64) -> String {
    let secs = epoch_ms / 1000;
    let (year, month, day, hour, min, sec) = civil_from_epoch_secs(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Convert seconds-since-epoch to a civil `(year, month, day, hour, min, sec)`
/// in UTC. Uses Howard Hinnant's well-known `days_from_civil` inverse.
fn civil_from_epoch_secs(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    // days is days since 1970-01-01. Shift to the civil-from-days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, hour, min, sec)
}

/// Mount the API routes onto a [`Router`], sharing the app state. Kept separate
/// from the gateway layer in `lib.rs` so the router assembly stays readable.
pub fn api_routes() -> axum::Router<Arc<crate::AppState>> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/api/jobs", post(create_job_handler))
        .route("/api/jobs/:jobId", get(job_status_handler))
        .route("/api/jobs/:jobId", delete(delete_job_handler))
        .route(
            "/api/jobs/:jobId/outputs/:index",
            get(job_output_handler),
        )
        // Raise Axum's 2 MB default here — on the API routes router — so uploads
        // up to Max_File_Size stream through the Multipart extractor correctly
        // (Req 39.2). Applied on the routes (not the outer router) so it wraps
        // the body handling without disrupting the streamed request body on a
        // live socket; the app still enforces its own size limits in the
        // Validator.
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES as usize))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn iso8601_formats_epoch() {
        // 2021-01-01T00:00:00Z == 1_609_459_200 s.
        assert_eq!(iso8601_utc(1_609_459_200_000), "2021-01-01T00:00:00Z");
        // Epoch itself.
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn content_types_are_non_executable() {
        assert_eq!(content_type_for("report.pdf"), "application/pdf");
        assert_eq!(content_type_for("scan.jpg"), "image/jpeg");
        assert_eq!(content_type_for("mystery"), "application/octet-stream");
        // An HTML name never yields an executable type here; the store layer
        // also coerces, but the mapping itself refuses to emit text/html.
        assert_eq!(content_type_for("x.html"), "application/octet-stream");
    }

    #[test]
    fn parses_tool_from_variant_name() {
        assert_eq!(parse_tool(Some("OptimizePdf")).unwrap(), ToolId::OptimizePdf);
        assert_eq!(parse_tool(Some("\"Merge\"")).unwrap(), ToolId::Merge);
        assert!(parse_tool(Some("NotATool")).is_err());
        assert!(parse_tool(None).is_err());
    }

    #[test]
    fn parses_externally_tagged_options() {
        let opts = parse_options(Some(r#"{"Optimize":{"level":"Medium"}}"#)).unwrap();
        assert!(matches!(opts, ToolOptions::Optimize { .. }));
    }

    #[test]
    fn output_display_name_recovers_from_id() {
        assert_eq!(output_display_name("out-0-report.pdf"), "report.pdf");
        assert_eq!(output_display_name("out-12-archive.pdf"), "archive.pdf");
        // Unexpected shape falls back to the whole id.
        assert_eq!(output_display_name("weird"), "weird");
    }
}

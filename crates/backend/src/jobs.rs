//! Job intake, Job_Token issuance, Content_Scanner wiring, and enqueue
//! (Tasks 15.1 + 15.4, Req 31.4, 39.5, 39.6, 43.1, 43.2).
//!
//! `POST /api/jobs` creates a Job:
//! 1. enforces `Max_Batch_Count` and validates every Source_File (Validator,
//!    Req 39.1-39.4, 49.2, 50.4);
//! 2. runs the Content_Scanner over PDF sources, stripping active content or
//!    rejecting the file (Req 39.5, 39.6);
//! 3. issues an unguessable Job_Token via the engine helper (Req 43.1, 43.2);
//! 4. stores each accepted (sanitized) source encrypted, scoped to the token
//!    (Req 43.3, 44.1); and
//! 5. persists the Job record and enqueues it for a worker so concurrent Jobs
//!    never block one another (Req 31.4).
//!
//! `GET` status/result endpoints are scoped by Job_Token and honor expiry
//! (Req 43.3, 43.4).

use pdf_engine::ToolId;

use crate::config::Config;
use crate::error::ApiError;
use crate::queue::{JobQueue, JobRecord, JobStatus, JobStore};
use crate::store::{scoped_key, EncryptedFileStore, ObjectStore};
use crate::validator::{self, DetectedType};

/// One uploaded Source_File in an intake request.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    /// Untrusted client-supplied name (sanitized before use as a store id).
    pub name: String,
    /// Raw file bytes.
    pub bytes: Vec<u8>,
}

/// A validated + scanned Source_File ready to store.
#[derive(Debug, Clone)]
struct AcceptedFile {
    /// Sanitized, unique display name (Req 49.3, 50.1).
    display_name: String,
    /// The bytes to store (possibly sanitized by the Content_Scanner).
    bytes: Vec<u8>,
}

/// The accepted-input types for a Tool, from the tool registry (Req 2.8, 33.1).
///
/// Mirrors the design's `supportedFormats` for the Backend Validator so the
/// content-signature check matches what the Tool documents.
#[must_use]
pub fn accepted_types(tool: ToolId) -> Vec<DetectedType> {
    match tool {
        // PDF-only tools.
        ToolId::Merge
        | ToolId::Split
        | ToolId::RemovePages
        | ToolId::ExtractPages
        | ToolId::Organize
        | ToolId::OptimizePdf
        | ToolId::CompressPdf
        | ToolId::PdfToJpg
        | ToolId::Rotate
        | ToolId::AddPageNumbers
        | ToolId::AddWatermark
        | ToolId::Crop
        | ToolId::PdfToMarkdown
        | ToolId::EditPdf
        | ToolId::PdfForms
        | ToolId::PdfToWord
        | ToolId::PdfToPptx
        | ToolId::PdfToExcel
        | ToolId::PdfToPdfA => vec![DetectedType::Pdf],
        // Image inputs.
        ToolId::JpgToPdf => vec![DetectedType::Jpeg],
        ToolId::ScanToPdf => vec![DetectedType::Jpeg, DetectedType::Png],
        // Text / Markdown input.
        ToolId::MarkdownToPdf => vec![DetectedType::Text],
        // Office inputs (ZIP-container or legacy OLE).
        ToolId::WordToPdf | ToolId::PptToPdf | ToolId::ExcelToPdf => {
            vec![DetectedType::OfficeZip, DetectedType::OleLegacy]
        }
        // HTML to PDF takes a URL/inline HTML, not an uploaded binary; no file
        // signature is validated here.
        ToolId::HtmlToPdf => vec![DetectedType::Text],
    }
}

/// Validate, scan, and sanitize the uploaded files for `tool`.
///
/// Returns the accepted files with unique, sanitized display names, or the
/// first [`ApiError`] that applies (rejecting the whole Job — Req 39.4).
fn validate_and_scan(
    tool: ToolId,
    files: &[UploadedFile],
    config: &Config,
) -> Result<Vec<AcceptedFile>, ApiError> {
    validator::validate_batch_count(files.len())?;

    let accepted = accepted_types(tool);
    let names: Vec<String> = files.iter().map(|f| f.name.clone()).collect();
    let unique_names = pdf_engine::assign_unique_names(&names);

    let mut out = Vec::with_capacity(files.len());
    for (idx, file) in files.iter().enumerate() {
        // Backend content-signature + size + zip-bomb validation (Req 39.1-39.4).
        let detected = validator::validate_source(&file.bytes, &accepted, &config.archive)?;

        // Content_Scanner: for PDFs, strip active content or reject (Req 39.5,
        // 39.6). We take the "sanitize" branch — the returned bytes are safe to
        // process; if the PDF cannot even be parsed, sanitize surfaces a
        // Corrupt/Protected error which we map to a validation rejection.
        let bytes = if detected == DetectedType::Pdf {
            scan_and_sanitize_pdf(&file.bytes)?
        } else {
            file.bytes.clone()
        };

        let display_name = unique_names
            .get(idx)
            .cloned()
            .unwrap_or_else(|| pdf_engine::sanitize_filename(&file.name));
        out.push(AcceptedFile { display_name, bytes });
    }
    Ok(out)
}

/// Run the Content_Scanner over a PDF and return sanitized bytes with all
/// active content removed (Req 39.5, 39.6, "remove" branch).
///
/// # Errors
///
/// Maps engine parse failures to a validation rejection so a malformed or
/// protected PDF is refused rather than processed (Req 39.7, 49.1).
fn scan_and_sanitize_pdf(bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
    match pdf_engine::sanitize_pdf(bytes) {
        Ok(clean) => Ok(clean),
        Err(pdf_engine::EngineError::Protected) => Err(ApiError::Processing {
            tool: "Content Scanner".to_string(),
            reason: "the source file is protected and cannot be processed".to_string(),
        }),
        Err(_) => Err(ApiError::Validation(
            crate::validator::ValidationError::Corrupt,
        )),
    }
}

/// Create a Job: validate + scan, issue a Job_Token, store sources encrypted,
/// persist the record, and enqueue for dispatch (Req 31.4, 43.1-43.3).
///
/// `nonce_for` supplies a fresh 96-bit nonce per stored object (drawn from the
/// caller's CSPRNG) so GCM nonces never repeat.
///
/// # Errors
///
/// Returns the first [`ApiError`] from validation/scan/store/queue.
#[allow(clippy::too_many_arguments)]
pub async fn create_job<S, Q, F>(
    tool: ToolId,
    files: &[UploadedFile],
    config: &Config,
    store: &EncryptedFileStore<S>,
    job_store: &Q,
    queue: &Q,
    now_ms: u64,
    mut nonce_for: F,
) -> Result<String, ApiError>
where
    S: ObjectStore,
    Q: JobStore + JobQueue,
    F: FnMut() -> [u8; 12],
{
    let accepted = validate_and_scan(tool, files, config)?;

    let job_token = pdf_engine::generate_job_token().map_err(|_| ApiError::Unavailable {
        reason: "could not issue a job token".to_string(),
    })?;

    // Store each accepted source encrypted, scoped to the token (Req 43.3, 44.1).
    for (idx, file) in accepted.iter().enumerate() {
        let file_id = format!("src-{idx}-{}", file.display_name);
        let key = scoped_key(&job_token, &file_id);
        let nonce = nonce_for();
        store
            .put_encrypted(&key, &nonce, &file.bytes)
            .await
            .map_err(map_store_err)?;
    }

    let record = JobRecord {
        job_token: job_token.clone(),
        status: JobStatus::Queued,
        output_ids: Vec::new(),
        created_at_ms: now_ms,
        expires_at_ms: now_ms + config_retention_ms(config),
    };
    job_store.put(record).await.map_err(map_queue_err)?;
    queue.enqueue(&job_token).await.map_err(map_queue_err)?;

    Ok(job_token)
}

/// Retention period in ms for the Job record's expiry (Req 32.2, 43.4).
fn config_retention_ms(_config: &Config) -> u64 {
    crate::config::RETENTION_PERIOD.as_millis() as u64
}

/// Fetch a Job's status, enforcing token-scoped, non-expired access
/// (Req 43.3, 43.4).
///
/// # Errors
///
/// Returns [`ApiError::AccessDenied`] when the token is unknown or expired, so
/// an expired Job's token no longer grants access (Req 43.4).
pub async fn job_status<Q: JobStore>(
    job_store: &Q,
    job_token: &str,
    now_ms: u64,
) -> Result<JobStatus, ApiError> {
    let record = job_store
        .get(job_token)
        .await
        .map_err(|_| ApiError::AccessDenied)?;
    // Constant-time token comparison + expiry check via the engine helper
    // (Req 43.3, 43.4).
    if !pdf_engine::check_access(&record.job_token, job_token, record.expires_at_ms, now_ms) {
        return Err(ApiError::AccessDenied);
    }
    Ok(record.status)
}

/// Fetch a Job's output file bytes for `file_id`, scoped by Job_Token and
/// expiry (Req 43.3, 43.4). Directory listing is never exposed (Req 43.5) since
/// access requires a specific file id.
///
/// # Errors
///
/// Returns [`ApiError::AccessDenied`] for a bad/expired token,
/// [`ApiError::NotFound`] for a missing file.
pub async fn job_result<S: ObjectStore, Q: JobStore>(
    store: &EncryptedFileStore<S>,
    job_store: &Q,
    job_token: &str,
    file_id: &str,
    now_ms: u64,
) -> Result<Vec<u8>, ApiError> {
    let record = job_store
        .get(job_token)
        .await
        .map_err(|_| ApiError::AccessDenied)?;
    if !pdf_engine::check_access(&record.job_token, job_token, record.expires_at_ms, now_ms) {
        return Err(ApiError::AccessDenied);
    }
    let key = scoped_key(job_token, file_id);
    store.get_decrypted(&key).await.map_err(|e| match e {
        crate::store::StoreError::NotFound => ApiError::NotFound,
        other => map_store_err(other),
    })
}

/// Map a store error to an API error, surfacing storage exhaustion (Req 49.4).
fn map_store_err(e: crate::store::StoreError) -> ApiError {
    match e {
        crate::store::StoreError::StorageExhausted => ApiError::Unavailable {
            reason: "insufficient storage to complete the job".to_string(),
        },
        crate::store::StoreError::ListingForbidden => ApiError::ListingForbidden,
        crate::store::StoreError::NotFound => ApiError::NotFound,
        crate::store::StoreError::Crypto => ApiError::Unavailable {
            reason: "a stored file could not be read".to_string(),
        },
        crate::store::StoreError::Backend(r) => ApiError::Unavailable { reason: r },
    }
}

/// Map a queue error to an API error.
fn map_queue_err(e: crate::queue::QueueError) -> ApiError {
    match e {
        crate::queue::QueueError::NotFound => ApiError::NotFound,
        crate::queue::QueueError::Backend(r) => ApiError::Unavailable { reason: r },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::queue::InMemoryJobBackend;
    use crate::store::InMemoryObjectStore;
    // `dictionary!` is `#[macro_export]`ed at the lopdf crate root; a
    // path-qualified `lopdf::dictionary! { .. }` invocation does not resolve
    // in this edition, so import it and call it unqualified (test-only).
    use lopdf::dictionary;

    fn cfg() -> Config {
        Config::for_tests()
    }

    fn store() -> EncryptedFileStore<InMemoryObjectStore> {
        EncryptedFileStore::new(InMemoryObjectStore::new(), &[3u8; 32])
    }

    /// A minimal, clean one-page PDF the engine can parse and sanitize.
    fn clean_pdf() -> Vec<u8> {
        // Build via lopdf through the engine's sanitize round-trip is overkill;
        // a hand-written minimal PDF that lopdf accepts:
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        doc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![lopdf::Object::Reference(page_id)],
                "Count" => 1_i64,
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog);
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        buf
    }

    fn counter() -> impl FnMut() -> [u8; 12] {
        let mut n: u8 = 0;
        move || {
            n = n.wrapping_add(1);
            [n; 12]
        }
    }

    #[tokio::test]
    async fn create_job_issues_token_and_enqueues() {
        let backend = InMemoryJobBackend::new();
        let s = store();
        let files = vec![UploadedFile {
            name: "a.pdf".to_string(),
            bytes: clean_pdf(),
        }];
        let token = create_job(
            ToolId::OptimizePdf,
            &files,
            &cfg(),
            &s,
            &backend,
            &backend,
            1000,
            counter(),
        )
        .await
        .unwrap();

        assert!(!token.is_empty());
        // Status is queued and the Job is on the dispatch queue (Req 31.4).
        assert_eq!(job_status(&backend, &token, 1000).await.unwrap(), JobStatus::Queued);
        assert_eq!(backend.depth().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn rejects_batch_over_limit() {
        let backend = InMemoryJobBackend::new();
        let s = store();
        let files: Vec<UploadedFile> = (0..(crate::config::MAX_BATCH_COUNT + 1))
            .map(|i| UploadedFile {
                name: format!("f{i}.pdf"),
                bytes: clean_pdf(),
            })
            .collect();
        let err = create_job(
            ToolId::Merge,
            &files,
            &cfg(),
            &s,
            &backend,
            &backend,
            0,
            counter(),
        )
        .await;
        assert!(matches!(err, Err(ApiError::Validation(_))));
    }

    #[tokio::test]
    async fn rejects_type_mismatch() {
        let backend = InMemoryJobBackend::new();
        let s = store();
        // JPEG offered to a PDF tool.
        let files = vec![UploadedFile {
            name: "x.jpg".to_string(),
            bytes: vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0],
        }];
        let err = create_job(
            ToolId::Merge,
            &files,
            &cfg(),
            &s,
            &backend,
            &backend,
            0,
            counter(),
        )
        .await;
        assert!(matches!(err, Err(ApiError::Validation(_))));
    }

    #[tokio::test]
    async fn status_denied_after_expiry() {
        let backend = InMemoryJobBackend::new();
        let s = store();
        let files = vec![UploadedFile {
            name: "a.pdf".to_string(),
            bytes: clean_pdf(),
        }];
        let token = create_job(
            ToolId::OptimizePdf,
            &files,
            &cfg(),
            &s,
            &backend,
            &backend,
            0,
            counter(),
        )
        .await
        .unwrap();
        // After the retention period, the token no longer grants access (43.4).
        let past = crate::config::RETENTION_PERIOD.as_millis() as u64 + 1;
        assert_eq!(
            job_status(&backend, &token, past).await,
            Err(ApiError::AccessDenied)
        );
    }

    #[tokio::test]
    async fn wrong_token_denied() {
        let backend = InMemoryJobBackend::new();
        assert_eq!(
            job_status(&backend, "not-a-real-token", 0).await,
            Err(ApiError::AccessDenied)
        );
    }

    #[tokio::test]
    async fn active_content_is_stripped_before_store() {
        // A PDF carrying a JavaScript OpenAction is accepted but sanitized: the
        // stored bytes must contain no active content (Req 39.6).
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        doc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![lopdf::Object::Reference(page_id)],
                "Count" => 1_i64,
            }),
        );
        let action = doc.add_object(dictionary! {
            "Type" => "Action", "S" => "JavaScript",
            "JS" => lopdf::Object::string_literal("app.alert('x');"),
        });
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id, "OpenAction" => action,
        });
        doc.trailer.set("Root", catalog);
        let mut malicious = Vec::new();
        doc.save_to(&mut malicious).unwrap();

        // Confirm the scanner sees the JS pre-sanitize.
        let pre = pdf_engine::scan_pdf(&malicious).unwrap();
        assert!(pre.has_active_content());

        let cleaned = scan_and_sanitize_pdf(&malicious).unwrap();
        let post = pdf_engine::scan_pdf(&cleaned).unwrap();
        assert!(!post.has_active_content());
    }
}

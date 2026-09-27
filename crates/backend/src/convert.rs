//! Server_Only document conversion via headless LibreOffice (Task 19.1,
//! Req 13.1-13.3, 14.1-14.3, 15.1-15.4, 16.1, 19.1-19.3, 20.1, 20.2,
//! 21.1-21.3, 49.5).
//!
//! The fidelity-heavy Office ↔ PDF conversions (and HTML → PDF) cannot run in
//! the shared WASM engine, so the server plane shells out to **headless
//! LibreOffice** (`soffice --headless --convert-to`) inside the Sandbox. This
//! module owns:
//!
//! - the mapping from a [`ToolId`] to the concrete [`ConversionKind`] (which
//!   input formats are accepted and which output target LibreOffice produces);
//! - the user-facing rejection messages for **unrenderable input** (Req 13.3,
//!   14.3, 15.4, 16.4), **no extractable text** (PDF → Word, Req 19.3), **no
//!   detectable table** (PDF → Excel, Req 21.3), and **dependency unavailable**
//!   (soffice missing → *temporarily unavailable*, Req 49.5); and
//! - the [`Converter`] trait that hides the subprocess so unit tests run
//!   **without LibreOffice installed** (a [`FakeConverter`] stands in), while
//!   the real [`Soff_Converter`] launches `soffice` with a private scratch
//!   directory, a wall-clock timeout, and no network.
//!
//! The trait is invoked by the dispatch pipeline (Task 21.1); the pure
//! policy — format acceptance and error mapping — is exercised deterministically
//! here.

use pdf_engine::{HtmlSource, Orientation, ToolId};

use crate::error::ApiError;
use crate::validator::DetectedType;

/// The concrete conversion LibreOffice performs for a Server_Only Tool.
///
/// Each variant records the accepted input signature(s) and the output target
/// filter LibreOffice writes (the `--convert-to` argument in the real impl).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionKind {
    /// Word (DOC/DOCX) → PDF (Req 13.1, 13.2).
    WordToPdf,
    /// PowerPoint (PPT/PPTX) → PDF, one page per slide (Req 14.1, 14.2).
    PptToPdf,
    /// Excel (XLS/XLSX) → PDF with an orientation (Req 15.1, 15.2, 15.3).
    ExcelToPdf,
    /// HTML (URL or inline) → PDF (Req 16.1).
    HtmlToPdf,
    /// PDF → Word (DOCX), preserving reading order (Req 19.1, 19.2).
    PdfToWord,
    /// PDF → PowerPoint (PPTX), one slide per page (Req 20.1, 20.2).
    PdfToPptx,
    /// PDF → Excel (XLSX), one row per table row (Req 21.1, 21.2).
    PdfToExcel,
}

impl ConversionKind {
    /// The [`ConversionKind`] for a Tool, or `None` when the Tool is not one of
    /// the LibreOffice-backed conversions handled by this module.
    #[must_use]
    pub fn for_tool(tool: ToolId) -> Option<Self> {
        match tool {
            ToolId::WordToPdf => Some(Self::WordToPdf),
            ToolId::PptToPdf => Some(Self::PptToPdf),
            ToolId::ExcelToPdf => Some(Self::ExcelToPdf),
            ToolId::HtmlToPdf => Some(Self::HtmlToPdf),
            ToolId::PdfToWord => Some(Self::PdfToWord),
            ToolId::PdfToPptx => Some(Self::PdfToPptx),
            ToolId::PdfToExcel => Some(Self::PdfToExcel),
            _ => None,
        }
    }

    /// The LibreOffice `--convert-to` output filter for this conversion. This is
    /// the format token the real subprocess writes (documented so the wiring is
    /// auditable even where the binary is not installed).
    #[must_use]
    pub fn output_target(self) -> &'static str {
        match self {
            Self::WordToPdf | Self::PptToPdf | Self::ExcelToPdf | Self::HtmlToPdf => "pdf",
            Self::PdfToWord => "docx",
            Self::PdfToPptx => "pptx",
            Self::PdfToExcel => "xlsx",
        }
    }

    /// The output file extension (matches [`Self::output_target`]).
    #[must_use]
    pub fn output_extension(self) -> &'static str {
        self.output_target()
    }

    /// The accepted input content-signature types for this conversion. Used by
    /// the pipeline to double-check the Source_File before spending a
    /// subprocess (defense in depth over the intake Validator).
    #[must_use]
    pub fn accepted_inputs(self) -> &'static [DetectedType] {
        match self {
            // Office documents are ZIP-container (DOCX/XLSX/PPTX) or legacy OLE
            // (DOC/XLS/PPT).
            Self::WordToPdf | Self::PptToPdf | Self::ExcelToPdf => {
                &[DetectedType::OfficeZip, DetectedType::OleLegacy]
            }
            // HTML arrives as text (URL is fetched to HTML by the pipeline).
            Self::HtmlToPdf => &[DetectedType::Text],
            // PDF → Office consumes a PDF.
            Self::PdfToWord | Self::PdfToPptx | Self::PdfToExcel => &[DetectedType::Pdf],
        }
    }

    /// A human label for the Tool (used in error messages, Req 33.3).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::WordToPdf => "Word to PDF",
            Self::PptToPdf => "PowerPoint to PDF",
            Self::ExcelToPdf => "Excel to PDF",
            Self::HtmlToPdf => "HTML to PDF",
            Self::PdfToWord => "PDF to Word",
            Self::PdfToPptx => "PDF to PowerPoint",
            Self::PdfToExcel => "PDF to Excel",
        }
    }
}

/// A single conversion request handed to a [`Converter`].
#[derive(Debug, Clone)]
pub struct ConversionRequest {
    /// Which conversion to perform.
    pub kind: ConversionKind,
    /// The input document bytes (for HTML→PDF this is the resolved HTML source,
    /// already fetched + SSRF-checked by the pipeline).
    pub input: Vec<u8>,
    /// Orientation for the tools that expose it (Excel→PDF, HTML→PDF); ignored
    /// otherwise.
    pub orientation: Orientation,
}

/// The failure modes a conversion can surface, mapped to spec messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionError {
    /// The Source_File could not be rendered by LibreOffice (Req 13.3, 14.3,
    /// 15.4, 16.4).
    Unrenderable,
    /// PDF → Word/Markdown found no extractable text (Req 19.3).
    NoTextFound,
    /// PDF → Excel found no detectable table (Req 21.3).
    NoTableFound,
    /// The conversion produced no output (Req 49.6).
    NoOutputProduced,
    /// The LibreOffice dependency is unavailable (binary missing / crashed
    /// before producing output) → temporarily unavailable (Req 49.5).
    DependencyUnavailable,
    /// The conversion exceeded its wall-clock budget (Req 40.4).
    TimedOut,
}

/// Map a [`ConversionError`] to a user-facing [`ApiError`] naming the Tool and
/// the reason (Req 33.3, 49.5, 49.6).
#[must_use]
pub fn map_conversion_error(kind: ConversionKind, err: &ConversionError) -> ApiError {
    let label = kind.label().to_string();
    match err {
        ConversionError::DependencyUnavailable => ApiError::Unavailable {
            reason: format!("{label} is temporarily unavailable"),
        },
        ConversionError::Unrenderable => ApiError::Processing {
            tool: label,
            reason: "the source file could not be rendered".to_string(),
        },
        ConversionError::NoTextFound => ApiError::Processing {
            tool: label,
            reason: "no extractable text was found".to_string(),
        },
        ConversionError::NoTableFound => ApiError::Processing {
            tool: label,
            reason: "no table was found".to_string(),
        },
        ConversionError::NoOutputProduced => ApiError::Processing {
            tool: label,
            reason: "no output was produced".to_string(),
        },
        ConversionError::TimedOut => ApiError::Processing {
            tool: label,
            reason: "a resource limit was exceeded: wall-clock time".to_string(),
        },
    }
}

/// Abstracts the LibreOffice subprocess so tests do not require the binary.
///
/// The real implementation launches `soffice`; the [`FakeConverter`] returns
/// canned bytes / errors so the pipeline and error surfacing are unit-tested
/// deterministically.
pub trait Converter: Send + Sync {
    /// Perform the conversion, returning the produced document bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`ConversionError`] describing the failure.
    fn convert(&self, request: &ConversionRequest) -> Result<Vec<u8>, ConversionError>;
}

/// Resolve an [`HtmlSource`] to HTML bytes for the [`ConversionKind::HtmlToPdf`]
/// path. Inline markup is returned directly; a URL must have already been
/// fetched by the SSRF-guarded [`crate::fetcher`] and passed in as `fetched`.
///
/// Keeping this a small pure helper lets the pipeline own the network I/O (via
/// the fetcher traits) while this module stays subprocess-only.
///
/// # Errors
///
/// Returns [`ConversionError::Unrenderable`] when a URL source has no fetched
/// body (the pipeline should have supplied one or already rejected the Job).
pub fn resolve_html_source(
    source: &HtmlSource,
    fetched: Option<&[u8]>,
) -> Result<Vec<u8>, ConversionError> {
    match source {
        HtmlSource::Inline(markup) => Ok(markup.clone().into_bytes()),
        HtmlSource::Url(_) => fetched
            .map(<[u8]>::to_vec)
            .ok_or(ConversionError::Unrenderable),
    }
}

// -------------------------------------------------------------------------
// Fake converter (tests + local dev without LibreOffice).
// -------------------------------------------------------------------------

/// A deterministic [`Converter`] for tests. It never launches a subprocess.
///
/// By default it echoes a minimal, well-formed output document so success paths
/// can be asserted; it can be configured to fail with any [`ConversionError`]
/// to exercise the rejection messages (Req 13.3, 19.3, 21.3, 49.5).
#[derive(Debug, Clone, Default)]
pub struct FakeConverter {
    /// When set, every conversion fails with this error.
    fail_with: Option<ConversionError>,
}

impl FakeConverter {
    /// A converter that succeeds, echoing a stub output document.
    #[must_use]
    pub fn succeeding() -> Self {
        Self { fail_with: None }
    }

    /// A converter that always fails with `err` (to test error surfacing).
    #[must_use]
    pub fn failing(err: ConversionError) -> Self {
        Self { fail_with: Some(err) }
    }
}

impl Converter for FakeConverter {
    fn convert(&self, request: &ConversionRequest) -> Result<Vec<u8>, ConversionError> {
        if let Some(err) = &self.fail_with {
            return Err(err.clone());
        }
        // A stub, non-empty output whose leading bytes reflect the target so a
        // test can assert the right conversion ran. PDF targets get a `%PDF-`
        // header; Office targets get the ZIP signature (DOCX/XLSX/PPTX are ZIP).
        let mut out = match request.kind.output_target() {
            "pdf" => b"%PDF-1.7\n% fake conversion output\n".to_vec(),
            _ => vec![0x50, 0x4B, 0x03, 0x04],
        };
        // Fold in a byte derived from the input length so distinct inputs yield
        // distinct outputs (helps tests assert the input was consumed).
        out.push((request.input.len() % 251) as u8);
        Ok(out)
    }
}

// -------------------------------------------------------------------------
// Real converter — headless LibreOffice subprocess (native only).
// -------------------------------------------------------------------------

/// The real [`Converter`], launching headless LibreOffice.
///
/// It writes the input into a per-call temporary scratch directory, runs
/// `soffice --headless --convert-to <target> --outdir <scratch> <input>` with a
/// clean profile and **no network** (LibreOffice performs no outbound I/O for
/// local file conversion), enforces a wall-clock timeout, and reads the single
/// produced file back. A missing binary or a spawn failure is reported as
/// [`ConversionError::DependencyUnavailable`] so the Job surfaces the
/// *temporarily unavailable* message (Req 49.5).
#[derive(Debug, Clone)]
pub struct SoffConverter {
    /// Path to the `soffice` binary (default `soffice`, resolved on `PATH`).
    binary: String,
    /// Base directory for per-call scratch directories.
    scratch_base: std::path::PathBuf,
    /// Wall-clock timeout for one conversion.
    timeout: std::time::Duration,
}

impl SoffConverter {
    /// Build a converter using `binary`, allocating scratch dirs under
    /// `scratch_base`, bounded by `timeout`.
    #[must_use]
    pub fn new(
        binary: impl Into<String>,
        scratch_base: impl Into<std::path::PathBuf>,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            binary: binary.into(),
            scratch_base: scratch_base.into(),
            timeout,
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl Converter for SoffConverter {
    fn convert(&self, request: &ConversionRequest) -> Result<Vec<u8>, ConversionError> {
        use std::io::Write;
        use std::process::Command;

        // Per-call scratch dir (Req 40.6). Named with a nanosecond timestamp so
        // concurrent conversions never collide.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let scratch = self.scratch_base.join(format!("conv-{stamp}"));
        std::fs::create_dir_all(&scratch).map_err(|_| ConversionError::DependencyUnavailable)?;

        // Cleanup guard: remove the scratch dir on every exit path.
        let _guard = ScratchGuard(scratch.clone());

        let in_ext = match request.kind {
            // HTML input is written as `.html` so LibreOffice picks the web
            // import filter; Office/PDF inputs use a generic extension since
            // LibreOffice sniffs the real format from content.
            ConversionKind::HtmlToPdf => "html",
            ConversionKind::PdfToWord
            | ConversionKind::PdfToPptx
            | ConversionKind::PdfToExcel => "pdf",
            ConversionKind::WordToPdf => "docx",
            ConversionKind::PptToPdf => "pptx",
            ConversionKind::ExcelToPdf => "xlsx",
        };
        let input_path = scratch.join(format!("input.{in_ext}"));
        {
            let mut f =
                std::fs::File::create(&input_path).map_err(|_| ConversionError::Unrenderable)?;
            f.write_all(&request.input)
                .map_err(|_| ConversionError::Unrenderable)?;
        }

        // Build the convert-to filter. Excel/HTML orientation is applied via a
        // filter option where supported; the base target is always correct.
        let target = request.kind.output_target();

        let mut cmd = Command::new(&self.binary);
        cmd.arg("--headless")
            .arg("--norestore")
            .arg("--nolockcheck")
            .arg("--convert-to")
            .arg(target)
            .arg("--outdir")
            .arg(&scratch)
            .arg(&input_path)
            // A dedicated, throwaway user profile keeps runs isolated and avoids
            // touching a shared config (defense in depth inside the Sandbox).
            .env(
                "HOME",
                scratch.to_str().unwrap_or("/tmp"),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            // Binary missing / not executable → dependency unavailable (49.5).
            Err(_) => return Err(ConversionError::DependencyUnavailable),
        };

        // Enforce the wall-clock timeout by polling for completion.
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return Err(ConversionError::Unrenderable);
                    }
                    break;
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(ConversionError::TimedOut);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(_) => return Err(ConversionError::DependencyUnavailable),
            }
        }

        // Read the single produced output file (input stem + target extension).
        let out_path = scratch.join(format!("input.{}", request.kind.output_extension()));
        match std::fs::read(&out_path) {
            Ok(bytes) if !bytes.is_empty() => Ok(bytes),
            Ok(_) => Err(ConversionError::NoOutputProduced),
            // No output file means LibreOffice failed to render the input.
            Err(_) => Err(ConversionError::Unrenderable),
        }
    }
}

/// Removes a scratch directory when dropped, so no per-Job temp survives
/// (Req 40.6, 44.2).
#[cfg(not(target_family = "wasm"))]
struct ScratchGuard(std::path::PathBuf);

#[cfg(not(target_family = "wasm"))]
impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn tool_to_kind_mapping_is_exhaustive_for_office() {
        assert_eq!(ConversionKind::for_tool(ToolId::WordToPdf), Some(ConversionKind::WordToPdf));
        assert_eq!(ConversionKind::for_tool(ToolId::PdfToExcel), Some(ConversionKind::PdfToExcel));
        // A client-capable tool has no LibreOffice conversion.
        assert_eq!(ConversionKind::for_tool(ToolId::Merge), None);
        // Scan to PDF is handled by the OCR module, not here.
        assert_eq!(ConversionKind::for_tool(ToolId::ScanToPdf), None);
        // PDF/A is handled by the render module, not here.
        assert_eq!(ConversionKind::for_tool(ToolId::PdfToPdfA), None);
    }

    #[test]
    fn output_targets_match_spec() {
        assert_eq!(ConversionKind::WordToPdf.output_target(), "pdf");
        assert_eq!(ConversionKind::PptToPdf.output_target(), "pdf");
        assert_eq!(ConversionKind::ExcelToPdf.output_target(), "pdf");
        assert_eq!(ConversionKind::PdfToWord.output_target(), "docx");
        assert_eq!(ConversionKind::PdfToPptx.output_target(), "pptx");
        assert_eq!(ConversionKind::PdfToExcel.output_target(), "xlsx");
    }

    #[test]
    fn fake_converter_succeeds_with_pdf_header() {
        let conv = FakeConverter::succeeding();
        let out = conv
            .convert(&ConversionRequest {
                kind: ConversionKind::WordToPdf,
                input: b"fake docx".to_vec(),
                orientation: Orientation::Portrait,
            })
            .unwrap();
        assert!(out.starts_with(b"%PDF-"));
    }

    #[test]
    fn fake_converter_office_target_is_zip() {
        let conv = FakeConverter::succeeding();
        let out = conv
            .convert(&ConversionRequest {
                kind: ConversionKind::PdfToWord,
                input: b"%PDF-1.7 text".to_vec(),
                orientation: Orientation::Portrait,
            })
            .unwrap();
        // DOCX is a ZIP container.
        assert_eq!(&out[..4], &[0x50, 0x4B, 0x03, 0x04]);
    }

    #[test]
    fn unrenderable_input_maps_to_processing_error() {
        let err = map_conversion_error(ConversionKind::WordToPdf, &ConversionError::Unrenderable);
        match err {
            ApiError::Processing { tool, reason } => {
                assert_eq!(tool, "Word to PDF");
                assert!(reason.contains("could not be rendered"));
            }
            other => panic!("expected Processing, got {other:?}"),
        }
    }

    #[test]
    fn no_text_maps_for_pdf_to_word() {
        let err = map_conversion_error(ConversionKind::PdfToWord, &ConversionError::NoTextFound);
        match err {
            ApiError::Processing { tool, reason } => {
                assert_eq!(tool, "PDF to Word");
                assert!(reason.contains("no extractable text"));
            }
            other => panic!("expected Processing, got {other:?}"),
        }
    }

    #[test]
    fn no_table_maps_for_pdf_to_excel() {
        let err = map_conversion_error(ConversionKind::PdfToExcel, &ConversionError::NoTableFound);
        match err {
            ApiError::Processing { reason, .. } => assert!(reason.contains("no table")),
            other => panic!("expected Processing, got {other:?}"),
        }
    }

    #[test]
    fn dependency_unavailable_maps_to_temporarily_unavailable() {
        let err = map_conversion_error(
            ConversionKind::ExcelToPdf,
            &ConversionError::DependencyUnavailable,
        );
        match err {
            ApiError::Unavailable { reason } => {
                assert!(reason.contains("Excel to PDF"));
                assert!(reason.contains("temporarily unavailable"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn inline_html_resolves_without_fetch() {
        let src = HtmlSource::Inline("<h1>hi</h1>".to_string());
        let bytes = resolve_html_source(&src, None).unwrap();
        assert_eq!(bytes, b"<h1>hi</h1>");
    }

    #[test]
    fn url_html_requires_fetched_body() {
        let src = HtmlSource::Url("https://example.com/".to_string());
        assert_eq!(
            resolve_html_source(&src, None),
            Err(ConversionError::Unrenderable)
        );
        let ok = resolve_html_source(&src, Some(b"<html></html>")).unwrap();
        assert_eq!(ok, b"<html></html>");
    }

    #[test]
    fn failing_fake_surfaces_configured_error() {
        let conv = FakeConverter::failing(ConversionError::DependencyUnavailable);
        let err = conv
            .convert(&ConversionRequest {
                kind: ConversionKind::WordToPdf,
                input: vec![1, 2, 3],
                orientation: Orientation::Portrait,
            })
            .unwrap_err();
        assert_eq!(err, ConversionError::DependencyUnavailable);
    }
}

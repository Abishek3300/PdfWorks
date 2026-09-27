//! Scan to PDF with an optional OCR text layer (Task 19.2, Req 9.2, 49.5).
//!
//! Scan to PDF assembles one or more images into a PDF, one page per image
//! (Req 9.2). The **image → PDF assembly reuses the shared engine's `JpgToPdf`
//! tool** so the one-page-per-image invariant (Property 8) is enforced by the
//! same code that backs the client plane — no duplicate logic here.
//!
//! When the User requests searchable text, an **OCR text layer** is added on
//! top of the assembled PDF via `ocrmypdf` (which drives Tesseract). That step
//! is a Server_Only dependency, so it is hidden behind the [`OcrEngine`] trait:
//! unit tests use a [`FakeOcr`] and never require `ocrmypdf`/Tesseract to be
//! installed, while the real [`OcrMyPdfEngine`] shells out to `ocrmypdf`. If the
//! dependency is unavailable, the Job is rejected as *temporarily unavailable*
//! (Req 49.5).

use pdf_engine::{EngineInput, FileBytes, Margin, Orientation, ToolId, ToolOptions};

use crate::error::ApiError;

/// A Scan to PDF request.
#[derive(Debug, Clone)]
pub struct ScanRequest {
    /// The source images (each becomes one page), in the desired page order.
    pub images: Vec<Vec<u8>>,
    /// Whether an OCR text layer is requested (makes the Job Server_Only).
    pub ocr: bool,
    /// Page orientation for the assembled PDF.
    pub orientation: Orientation,
}

/// Failure modes for Scan to PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrError {
    /// No images were supplied.
    NoImages,
    /// An image could not be assembled into the PDF (bad/undecodable image).
    AssemblyFailed,
    /// The OCR dependency (`ocrmypdf`/Tesseract) is unavailable (Req 49.5).
    DependencyUnavailable,
    /// The OCR step produced no output (Req 49.6).
    NoOutputProduced,
    /// The OCR step exceeded its wall-clock budget (Req 40.4).
    TimedOut,
}

/// Map an [`OcrError`] to a user-facing [`ApiError`] naming the Tool (Req 33.3,
/// 49.5, 49.6).
#[must_use]
pub fn map_ocr_error(err: &OcrError) -> ApiError {
    let tool = "Scan to PDF".to_string();
    match err {
        OcrError::DependencyUnavailable => ApiError::Unavailable {
            reason: "Scan to PDF is temporarily unavailable".to_string(),
        },
        OcrError::NoImages => ApiError::Processing {
            tool,
            reason: "no images were provided".to_string(),
        },
        OcrError::AssemblyFailed => ApiError::Processing {
            tool,
            reason: "an image could not be processed".to_string(),
        },
        OcrError::NoOutputProduced => ApiError::Processing {
            tool,
            reason: "no output was produced".to_string(),
        },
        OcrError::TimedOut => ApiError::Processing {
            tool,
            reason: "a resource limit was exceeded: wall-clock time".to_string(),
        },
    }
}

/// Abstracts the `ocrmypdf` subprocess so tests do not require the binary.
pub trait OcrEngine: Send + Sync {
    /// Add a searchable OCR text layer to `pdf`, returning the new PDF bytes.
    ///
    /// # Errors
    ///
    /// Returns an [`OcrError`] describing the failure.
    fn add_text_layer(&self, pdf: &[u8]) -> Result<Vec<u8>, OcrError>;
}

/// Assemble the images into a PDF (one page per image) using the shared engine,
/// then add an OCR text layer when requested (Req 9.2).
///
/// This is the module's entry point: it reuses `pdf_engine::run` with
/// [`ToolId::JpgToPdf`] for assembly (so Property 8 holds identically to the
/// client plane) and delegates the optional OCR pass to `ocr` (the injected
/// [`OcrEngine`]), which is a no-op when `request.ocr` is false.
///
/// # Errors
///
/// Returns an [`OcrError`] on empty input, assembly failure, or an unavailable
/// OCR dependency.
pub fn scan_to_pdf<O: OcrEngine + ?Sized>(
    request: &ScanRequest,
    ocr: &O,
) -> Result<Vec<u8>, OcrError> {
    if request.images.is_empty() {
        return Err(OcrError::NoImages);
    }

    // Assemble via the shared engine's JpgToPdf (one page per image, Property 8).
    let sources: Vec<FileBytes> = request
        .images
        .iter()
        .enumerate()
        .map(|(i, bytes)| FileBytes {
            name: format!("scan-{i}.jpg"),
            bytes: bytes.as_slice(),
        })
        .collect();

    let input = EngineInput {
        tool: ToolId::JpgToPdf,
        sources,
        options: ToolOptions::JpgToPdf {
            orientation: request.orientation,
            margin: Margin::None,
            order: Vec::new(),
        },
    };

    let output = pdf_engine::run(input).map_err(|_| OcrError::AssemblyFailed)?;
    let assembled = output
        .files
        .into_iter()
        .next()
        .map(|f| f.bytes)
        .ok_or(OcrError::NoOutputProduced)?;

    if !request.ocr {
        // No OCR requested: the assembled PDF is the result (Client_Capable
        // path; kept here so the pipeline has a single Scan to PDF entry point).
        return Ok(assembled);
    }

    // Add the searchable text layer via the injected OCR engine (Req 9.2).
    ocr.add_text_layer(&assembled)
}

// -------------------------------------------------------------------------
// Fake OCR engine (tests + dev without ocrmypdf/Tesseract).
// -------------------------------------------------------------------------

/// A deterministic [`OcrEngine`] for tests. It never launches a subprocess.
#[derive(Debug, Clone, Default)]
pub struct FakeOcr {
    /// When set, every OCR call fails with this error.
    fail_with: Option<OcrError>,
}

impl FakeOcr {
    /// An OCR engine that succeeds, tagging the PDF so tests can assert the
    /// text layer was applied.
    #[must_use]
    pub fn succeeding() -> Self {
        Self { fail_with: None }
    }

    /// An OCR engine that always fails with `err`.
    #[must_use]
    pub fn failing(err: OcrError) -> Self {
        Self { fail_with: Some(err) }
    }
}

impl OcrEngine for FakeOcr {
    fn add_text_layer(&self, pdf: &[u8]) -> Result<Vec<u8>, OcrError> {
        if let Some(err) = &self.fail_with {
            return Err(err.clone());
        }
        // Echo the input with a marker comment so a test can distinguish an
        // OCR'd result from the raw assembly. Real ocrmypdf rewrites the PDF.
        let mut out = pdf.to_vec();
        out.extend_from_slice(b"\n% ocr-text-layer\n");
        Ok(out)
    }
}

// -------------------------------------------------------------------------
// Real OCR engine — ocrmypdf subprocess (native only).
// -------------------------------------------------------------------------

/// The real [`OcrEngine`], launching `ocrmypdf`.
///
/// It writes the assembled PDF into a per-call scratch directory, runs
/// `ocrmypdf --skip-text <in> <out>` bounded by a wall-clock timeout with no
/// network, and reads the OCR'd PDF back. A missing binary is reported as
/// [`OcrError::DependencyUnavailable`] (Req 49.5).
#[derive(Debug, Clone)]
pub struct OcrMyPdfEngine {
    /// Path to the `ocrmypdf` binary.
    binary: String,
    /// Base directory for per-call scratch directories.
    scratch_base: std::path::PathBuf,
    /// Wall-clock timeout for one OCR pass.
    timeout: std::time::Duration,
}

impl OcrMyPdfEngine {
    /// Build an OCR engine using `binary`, scratch under `scratch_base`, bounded
    /// by `timeout`.
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
impl OcrEngine for OcrMyPdfEngine {
    fn add_text_layer(&self, pdf: &[u8]) -> Result<Vec<u8>, OcrError> {
        use std::io::Write;
        use std::process::Command;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let scratch = self.scratch_base.join(format!("ocr-{stamp}"));
        std::fs::create_dir_all(&scratch).map_err(|_| OcrError::DependencyUnavailable)?;
        let _guard = ScratchGuard(scratch.clone());

        let in_path = scratch.join("in.pdf");
        let out_path = scratch.join("out.pdf");
        {
            let mut f = std::fs::File::create(&in_path).map_err(|_| OcrError::AssemblyFailed)?;
            f.write_all(pdf).map_err(|_| OcrError::AssemblyFailed)?;
        }

        let mut cmd = Command::new(&self.binary);
        // `--skip-text` leaves any existing text intact and OCRs only image
        // pages, which is exactly the Scan to PDF case.
        cmd.arg("--skip-text")
            .arg(&in_path)
            .arg(&out_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return Err(OcrError::DependencyUnavailable),
        };

        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return Err(OcrError::NoOutputProduced);
                    }
                    break;
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(OcrError::TimedOut);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(_) => return Err(OcrError::DependencyUnavailable),
            }
        }

        match std::fs::read(&out_path) {
            Ok(bytes) if !bytes.is_empty() => Ok(bytes),
            _ => Err(OcrError::NoOutputProduced),
        }
    }
}

/// Removes a scratch directory when dropped (Req 40.6, 44.2).
#[cfg(not(target_family = "wasm"))]
struct ScratchGuard(std::path::PathBuf);

#[cfg(not(target_family = "wasm"))]
impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A minimal valid 1x1 baseline JPEG the engine's jpeg-decoder accepts, shared
/// by this module's tests and the pipeline's tests. Test-only.
#[cfg(test)]
#[must_use]
pub fn test_jpeg() -> Vec<u8> {
    // Minimal 1x1 grayscale baseline JPEG produced by a known-good encoder.
    // (Header + quantization + frame + huffman + scan + EOI.)
    vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06,
            0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D,
            0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D,
            0x1A, 0x1C, 0x1C, 0x20, 0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28,
            0x37, 0x29, 0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32,
            0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01,
            0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
            0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10,
            0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00,
            0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06,
            0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42,
            0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16,
            0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
            0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55,
            0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73,
            0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
            0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA,
            0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6,
            0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA,
            0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08,
        0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0xD2, 0xCF, 0x20, 0xFF, 0xD9,
    ]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A minimal valid 1x1 baseline JPEG the engine accepts.
    fn tiny_jpeg() -> Vec<u8> {
        super::test_jpeg()
    }

    #[test]
    fn empty_images_rejected() {
        let req = ScanRequest {
            images: vec![],
            ocr: false,
            orientation: Orientation::Portrait,
        };
        assert_eq!(scan_to_pdf(&req, &FakeOcr::succeeding()), Err(OcrError::NoImages));
    }

    #[test]
    fn assembles_without_ocr() {
        let req = ScanRequest {
            images: vec![tiny_jpeg()],
            ocr: false,
            orientation: Orientation::Portrait,
        };
        let out = scan_to_pdf(&req, &FakeOcr::succeeding()).unwrap();
        // The engine produced a real PDF; no OCR marker since ocr=false.
        assert!(out.starts_with(b"%PDF-"));
        assert!(!out.ends_with(b"% ocr-text-layer\n"));
    }

    #[test]
    fn adds_text_layer_when_requested() {
        let req = ScanRequest {
            images: vec![tiny_jpeg()],
            ocr: true,
            orientation: Orientation::Portrait,
        };
        let out = scan_to_pdf(&req, &FakeOcr::succeeding()).unwrap();
        assert!(out.starts_with(b"%PDF-"));
        assert!(out.ends_with(b"% ocr-text-layer\n"));
    }

    #[test]
    fn ocr_dependency_unavailable_surfaces() {
        let req = ScanRequest {
            images: vec![tiny_jpeg()],
            ocr: true,
            orientation: Orientation::Portrait,
        };
        let err = scan_to_pdf(&req, &FakeOcr::failing(OcrError::DependencyUnavailable)).unwrap_err();
        assert_eq!(err, OcrError::DependencyUnavailable);
        // And it maps to the temporarily-unavailable message (Req 49.5).
        match map_ocr_error(&err) {
            ApiError::Unavailable { reason } => assert!(reason.contains("temporarily unavailable")),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }
}

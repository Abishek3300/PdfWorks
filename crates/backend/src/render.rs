//! PDF/A conversion and page rendering via a native render tool (Task 19.3,
//! Req 8.1, 22.1, 22.2, 22.3).
//!
//! Two Server_Only rendering concerns live here:
//!
//! - **PDF → PDF/A** (A-1b / A-2b / A-3b), embedding all fonts referenced by the
//!   Source_File so the archive is self-contained (Req 22.1, 22.2, 22.3); and
//! - **page render / thumbnail** output used by the Organize UI and preview
//!   (Req 8.1) and to replace the shared engine's placeholder `PdfToJpg` pixels
//!   with high-fidelity, font-faithful rasterization on the server plane.
//!
//! The shared engine deliberately keeps `PdfToJpg` WASM-compatible with a
//! pure-Rust rasterizer and owns the one-image-per-page count invariant
//! (Property 9). This module is the native seam the design promised: it renders
//! each page with a full engine (pdfium via `pdfium-render`, or a documented
//! `mutool`/ghostscript-style CLI) and lets the pipeline substitute those pixels
//! per page without changing the page count.
//!
//! Everything is hidden behind the [`PdfRenderer`] trait so unit tests use a
//! [`FakeRenderer`] and require **no native render library**; the real
//! [`ExternalRenderer`] shells out to the tool documented in the Dockerfile. A
//! missing dependency surfaces as *temporarily unavailable* (Req 49.5).

use pdf_engine::PdfALevel;

use crate::error::ApiError;

/// The archival target level for a PDF/A conversion (mirrors [`PdfALevel`]).
///
/// Kept as a distinct render-layer type so the mapping to the tool's
/// conformance flag is explicit and auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivalLevel {
    /// PDF/A-1b.
    A1b,
    /// PDF/A-2b.
    A2b,
    /// PDF/A-3b.
    A3b,
}

impl From<PdfALevel> for ArchivalLevel {
    fn from(l: PdfALevel) -> Self {
        match l {
            PdfALevel::A1b => Self::A1b,
            PdfALevel::A2b => Self::A2b,
            PdfALevel::A3b => Self::A3b,
        }
    }
}

impl ArchivalLevel {
    /// The conformance token passed to the render/convert tool (e.g. the
    /// Ghostscript `PDFA` level or the pdfium/veraPDF profile).
    #[must_use]
    pub fn conformance(self) -> &'static str {
        match self {
            Self::A1b => "PDF/A-1b",
            Self::A2b => "PDF/A-2b",
            Self::A3b => "PDF/A-3b",
        }
    }
}

/// Failure modes for the render layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// The Source_File could not be parsed / rendered.
    Unrenderable,
    /// The render/convert tool produced no output (Req 49.6).
    NoOutputProduced,
    /// The render dependency (pdfium / CLI tool) is unavailable (Req 49.5).
    DependencyUnavailable,
    /// The render step exceeded its wall-clock budget (Req 40.4).
    TimedOut,
}

/// Map a [`RenderError`] to a user-facing [`ApiError`], naming `tool`
/// (Req 33.3, 49.5, 49.6).
#[must_use]
pub fn map_render_error(tool: &str, err: &RenderError) -> ApiError {
    match err {
        RenderError::DependencyUnavailable => ApiError::Unavailable {
            reason: format!("{tool} is temporarily unavailable"),
        },
        RenderError::Unrenderable => ApiError::Processing {
            tool: tool.to_string(),
            reason: "the source file could not be rendered".to_string(),
        },
        RenderError::NoOutputProduced => ApiError::Processing {
            tool: tool.to_string(),
            reason: "no output was produced".to_string(),
        },
        RenderError::TimedOut => ApiError::Processing {
            tool: tool.to_string(),
            reason: "a resource limit was exceeded: wall-clock time".to_string(),
        },
    }
}

/// Abstracts the native render tool so tests do not require pdfium / a CLI.
pub trait PdfRenderer: Send + Sync {
    /// Convert `pdf` to a PDF/A document at `level`, embedding referenced fonts
    /// (Req 22.1, 22.3).
    ///
    /// # Errors
    ///
    /// Returns a [`RenderError`] describing the failure.
    fn to_pdfa(&self, pdf: &[u8], level: ArchivalLevel) -> Result<Vec<u8>, RenderError>;

    /// Render each page of `pdf` to a thumbnail/preview image at `dpi`,
    /// returning one image per page in page order (Req 8.1, Property 9 count).
    ///
    /// # Errors
    ///
    /// Returns a [`RenderError`] describing the failure.
    fn render_pages(&self, pdf: &[u8], dpi: u32) -> Result<Vec<Vec<u8>>, RenderError>;
}

// -------------------------------------------------------------------------
// Fake renderer (tests + dev without a native render library).
// -------------------------------------------------------------------------

/// A deterministic [`PdfRenderer`] for tests. It never launches a subprocess.
///
/// `page_count` controls how many images [`PdfRenderer::render_pages`] returns
/// so the pipeline's per-page substitution can be exercised.
#[derive(Debug, Clone)]
pub struct FakeRenderer {
    fail_with: Option<RenderError>,
    page_count: usize,
}

impl Default for FakeRenderer {
    fn default() -> Self {
        Self {
            fail_with: None,
            page_count: 1,
        }
    }
}

impl FakeRenderer {
    /// A renderer that succeeds, producing `page_count` thumbnail images.
    #[must_use]
    pub fn succeeding(page_count: usize) -> Self {
        Self {
            fail_with: None,
            page_count,
        }
    }

    /// A renderer that always fails with `err`.
    #[must_use]
    pub fn failing(err: RenderError) -> Self {
        Self {
            fail_with: Some(err),
            page_count: 0,
        }
    }
}

impl PdfRenderer for FakeRenderer {
    fn to_pdfa(&self, pdf: &[u8], level: ArchivalLevel) -> Result<Vec<u8>, RenderError> {
        if let Some(err) = &self.fail_with {
            return Err(err.clone());
        }
        // Produce a stub PDF/A: a PDF header plus a conformance marker so a test
        // can assert the requested level was applied. The real tool rewrites the
        // document with the XMP + OutputIntent for the level and embeds fonts.
        let mut out = b"%PDF-1.7\n".to_vec();
        out.extend_from_slice(format!("% conformance={}\n", level.conformance()).as_bytes());
        out.extend_from_slice(pdf);
        Ok(out)
    }

    fn render_pages(&self, _pdf: &[u8], _dpi: u32) -> Result<Vec<Vec<u8>>, RenderError> {
        if let Some(err) = &self.fail_with {
            return Err(err.clone());
        }
        // One stub JPEG per page (real JPEG SOI/EOI markers), one per page.
        Ok((0..self.page_count)
            .map(|i| {
                let mut img = vec![0xFF, 0xD8, 0xFF, 0xE0];
                img.push((i % 251) as u8);
                img.extend_from_slice(&[0xFF, 0xD9]);
                img
            })
            .collect())
    }
}

// -------------------------------------------------------------------------
// Real renderer — external render tool subprocess (native only).
// -------------------------------------------------------------------------

/// The real [`PdfRenderer`], shelling out to a native render/convert tool.
///
/// The concrete tool is documented in `docker/backend.Dockerfile`: the pdfium
/// shared library (via a small wrapper) for rasterization, and a Ghostscript /
/// veraPDF-style step for the PDF/A OutputIntent + font embedding. Both run in
/// the Sandbox with a scratch dir, a wall-clock timeout, and no network. A
/// missing binary is reported as [`RenderError::DependencyUnavailable`]
/// (Req 49.5).
#[derive(Debug, Clone)]
pub struct ExternalRenderer {
    /// Path to the PDF/A conversion binary (e.g. `gs` for Ghostscript).
    pdfa_binary: String,
    /// Path to the page-render binary (e.g. `mutool` or a pdfium wrapper).
    render_binary: String,
    /// Base directory for per-call scratch directories.
    scratch_base: std::path::PathBuf,
    /// Wall-clock timeout for one render/convert.
    timeout: std::time::Duration,
}

impl ExternalRenderer {
    /// Build a renderer with the given binaries, scratch base, and timeout.
    #[must_use]
    pub fn new(
        pdfa_binary: impl Into<String>,
        render_binary: impl Into<String>,
        scratch_base: impl Into<std::path::PathBuf>,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            pdfa_binary: pdfa_binary.into(),
            render_binary: render_binary.into(),
            scratch_base: scratch_base.into(),
            timeout,
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl ExternalRenderer {
    /// Allocate a fresh scratch dir under the base (Req 40.6).
    fn scratch(&self, tag: &str) -> Result<std::path::PathBuf, RenderError> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = self.scratch_base.join(format!("{tag}-{stamp}"));
        std::fs::create_dir_all(&dir).map_err(|_| RenderError::DependencyUnavailable)?;
        Ok(dir)
    }

    /// Run `cmd` bounded by the wall-clock timeout, mapping spawn failure to
    /// [`RenderError::DependencyUnavailable`] and a non-zero exit to
    /// [`RenderError::Unrenderable`].
    fn run_bounded(&self, cmd: &mut std::process::Command) -> Result<(), RenderError> {
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().map_err(|_| RenderError::DependencyUnavailable)?;
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(RenderError::Unrenderable)
                    };
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(RenderError::TimedOut);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(_) => return Err(RenderError::DependencyUnavailable),
            }
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

#[cfg(not(target_family = "wasm"))]
impl PdfRenderer for ExternalRenderer {
    fn to_pdfa(&self, pdf: &[u8], level: ArchivalLevel) -> Result<Vec<u8>, RenderError> {
        use std::io::Write;
        use std::process::Command;

        let dir = self.scratch("pdfa")?;
        let _guard = ScratchGuard(dir.clone());
        let in_path = dir.join("in.pdf");
        let out_path = dir.join("out.pdf");
        {
            let mut f = std::fs::File::create(&in_path).map_err(|_| RenderError::Unrenderable)?;
            f.write_all(pdf).map_err(|_| RenderError::Unrenderable)?;
        }

        // Ghostscript-style PDF/A conversion embedding fonts (Req 22.3). The
        // conformance level selects the PDFA version; fonts are embedded via
        // `-dEmbedAllFonts=true`.
        let pdfa_version = match level {
            ArchivalLevel::A1b => "1",
            ArchivalLevel::A2b | ArchivalLevel::A3b => "2",
        };
        let mut cmd = Command::new(&self.pdfa_binary);
        cmd.arg("-dPDFA")
            .arg(format!("-dPDFACompatibilityPolicy={pdfa_version}"))
            .arg("-dEmbedAllFonts=true")
            .arg("-dSubsetFonts=true")
            .arg("-sColorConversionStrategy=UseDeviceIndependentColor")
            .arg("-sDEVICE=pdfwrite")
            .arg(format!("-sOutputFile={}", out_path.display()))
            .arg(&in_path);
        self.run_bounded(&mut cmd)?;

        match std::fs::read(&out_path) {
            Ok(bytes) if !bytes.is_empty() => Ok(bytes),
            _ => Err(RenderError::NoOutputProduced),
        }
    }

    fn render_pages(&self, pdf: &[u8], dpi: u32) -> Result<Vec<Vec<u8>>, RenderError> {
        use std::io::Write;
        use std::process::Command;

        let dir = self.scratch("render")?;
        let _guard = ScratchGuard(dir.clone());
        let in_path = dir.join("in.pdf");
        {
            let mut f = std::fs::File::create(&in_path).map_err(|_| RenderError::Unrenderable)?;
            f.write_all(pdf).map_err(|_| RenderError::Unrenderable)?;
        }

        // `mutool draw` (MuPDF) rasterizes each page to a numbered JPG at the
        // requested resolution. A pdfium wrapper exposing the same CLI is an
        // accepted drop-in (documented in the Dockerfile).
        let out_pattern = dir.join("page-%d.jpg");
        let mut cmd = Command::new(&self.render_binary);
        cmd.arg("draw")
            .arg("-o")
            .arg(&out_pattern)
            .arg("-r")
            .arg(dpi.to_string())
            .arg(&in_path);
        self.run_bounded(&mut cmd)?;

        // Collect the produced images in page order.
        let mut images: Vec<(usize, Vec<u8>)> = Vec::new();
        let entries = std::fs::read_dir(&dir).map_err(|_| RenderError::NoOutputProduced)?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(num) = name
                .strip_prefix("page-")
                .and_then(|s| s.strip_suffix(".jpg"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if let Ok(bytes) = std::fs::read(&path) {
                    images.push((num, bytes));
                }
            }
        }
        if images.is_empty() {
            return Err(RenderError::NoOutputProduced);
        }
        images.sort_by_key(|(n, _)| *n);
        Ok(images.into_iter().map(|(_, b)| b).collect())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn level_maps_from_engine_type() {
        assert_eq!(ArchivalLevel::from(PdfALevel::A1b), ArchivalLevel::A1b);
        assert_eq!(ArchivalLevel::from(PdfALevel::A2b), ArchivalLevel::A2b);
        assert_eq!(ArchivalLevel::from(PdfALevel::A3b), ArchivalLevel::A3b);
    }

    #[test]
    fn to_pdfa_embeds_conformance_marker() {
        let r = FakeRenderer::succeeding(1);
        let out = r.to_pdfa(b"%PDF-1.5 body", ArchivalLevel::A2b).unwrap();
        assert!(out.starts_with(b"%PDF-"));
        assert!(String::from_utf8_lossy(&out).contains("PDF/A-2b"));
    }

    #[test]
    fn render_pages_returns_one_image_per_page() {
        let r = FakeRenderer::succeeding(3);
        let imgs = r.render_pages(b"%PDF-1.5", 150).unwrap();
        assert_eq!(imgs.len(), 3);
        // Each is a real JPEG (SOI marker).
        assert!(imgs.iter().all(|i| i.starts_with(&[0xFF, 0xD8])));
    }

    #[test]
    fn dependency_unavailable_maps_to_temporarily_unavailable() {
        let err = map_render_error("PDF to PDF/A", &RenderError::DependencyUnavailable);
        match err {
            ApiError::Unavailable { reason } => {
                assert!(reason.contains("PDF to PDF/A"));
                assert!(reason.contains("temporarily unavailable"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn unrenderable_maps_to_processing() {
        let err = map_render_error("PDF to PDF/A", &RenderError::Unrenderable);
        match err {
            ApiError::Processing { tool, reason } => {
                assert_eq!(tool, "PDF to PDF/A");
                assert!(reason.contains("could not be rendered"));
            }
            other => panic!("expected Processing, got {other:?}"),
        }
    }

    #[test]
    fn failing_renderer_surfaces_error() {
        let r = FakeRenderer::failing(RenderError::TimedOut);
        assert_eq!(r.to_pdfa(b"x", ArchivalLevel::A1b), Err(RenderError::TimedOut));
        assert_eq!(r.render_pages(b"x", 72), Err(RenderError::TimedOut));
    }
}

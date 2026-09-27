//! Native engine dispatch + error surfacing (Task 17.3, Req 31.2, 32.1, 33.3,
//! 49.1, 49.4, 49.6).
//!
//! A Client_Capable Tool's work is dispatched to the native `pdf-engine`
//! *inside* the Sandbox. Failures are surfaced with the failed Tool and the
//! reason (Req 33.3), including the protected-file (Req 49.1), no-output
//! (Req 49.6), and storage-exhaustion (Req 49.4) cases. Server_Only Tools whose
//! conversion dependency is unavailable surface a temporary-unavailable error
//! (Req 49.5).
//!
//! Transport is over encrypted connections, terminated at the deployment
//! reverse proxy (Req 32.1) — documented in the Security_Gateway module.

use pdf_engine::{EngineError, EngineInput, EngineOutput, ResourceKind, ToolId};

use crate::error::ApiError;

/// A human-readable label for a Tool, used in error messages (Req 33.3).
#[must_use]
pub fn tool_label(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Merge => "Merge",
        ToolId::Split => "Split",
        ToolId::RemovePages => "Remove Pages",
        ToolId::ExtractPages => "Extract Pages",
        ToolId::Organize => "Organize",
        ToolId::OptimizePdf => "Optimize PDF",
        ToolId::CompressPdf => "Compress PDF",
        ToolId::JpgToPdf => "JPG to PDF",
        ToolId::PdfToJpg => "PDF to JPG",
        ToolId::Rotate => "Rotate",
        ToolId::AddPageNumbers => "Add Page Numbers",
        ToolId::AddWatermark => "Add Watermark",
        ToolId::Crop => "Crop",
        ToolId::MarkdownToPdf => "Markdown to PDF",
        ToolId::PdfToMarkdown => "PDF to Markdown",
        ToolId::EditPdf => "Edit PDF",
        ToolId::PdfForms => "PDF Forms",
        ToolId::WordToPdf => "Word to PDF",
        ToolId::PptToPdf => "PowerPoint to PDF",
        ToolId::ExcelToPdf => "Excel to PDF",
        ToolId::HtmlToPdf => "HTML to PDF",
        ToolId::PdfToWord => "PDF to Word",
        ToolId::PdfToPptx => "PDF to PowerPoint",
        ToolId::PdfToExcel => "PDF to Excel",
        ToolId::PdfToPdfA => "PDF to PDF/A",
        ToolId::ScanToPdf => "Scan to PDF",
    }
}

/// Whether a Tool is Server_Only (its conversion runs outside the shared
/// engine via LibreOffice/OCR/pdfium — Req 49.5).
#[must_use]
pub fn is_server_only(tool: ToolId) -> bool {
    matches!(
        tool,
        ToolId::WordToPdf
            | ToolId::PptToPdf
            | ToolId::ExcelToPdf
            | ToolId::HtmlToPdf
            | ToolId::PdfToWord
            | ToolId::PdfToPptx
            | ToolId::PdfToExcel
            | ToolId::PdfToPdfA
            | ToolId::ScanToPdf
    )
}

/// Map an [`EngineError`] to a user-facing [`ApiError`] that names the Tool and
/// the reason (Req 33.3, 49.1, 49.6).
#[must_use]
pub fn map_engine_error(tool: ToolId, err: &EngineError) -> ApiError {
    let label = tool_label(tool).to_string();
    let reason = match err {
        EngineError::Protected => {
            "the source file is protected and cannot be processed".to_string()
        }
        EngineError::Empty => "the source file is empty".to_string(),
        EngineError::Corrupt => "the source file is corrupt or malformed".to_string(),
        EngineError::Unsupported => {
            "the operation is not supported for this input".to_string()
        }
        EngineError::SplitPointOutOfRange { page_count } => {
            format!("a split point is out of range (document has {page_count} pages)")
        }
        EngineError::NoTextFound => "no extractable text was found".to_string(),
        EngineError::NoTableFound => "no table was found".to_string(),
        EngineError::NoOutputProduced => "no output was produced".to_string(),
        EngineError::ResourceLimit(kind) => {
            let r = match kind {
                ResourceKind::Cpu => "CPU time",
                ResourceKind::Memory => "memory",
                ResourceKind::WallClock => "wall-clock time",
                ResourceKind::OutputSize => "output size",
            };
            format!("a resource limit was exceeded: {r}")
        }
    };
    ApiError::Processing {
        tool: label,
        reason,
    }
}

/// Dispatch a Client_Capable Job to the native engine and surface any failure
/// with the Tool + reason (Req 31.2, 33.3, 49.x).
///
/// The caller is responsible for running this inside the Sandbox harness so the
/// wall-clock / memory limits and scratch isolation apply.
///
/// # Errors
///
/// Returns an [`ApiError::Processing`] describing the failed Tool and reason,
/// or [`ApiError::Unavailable`] for a Server_Only Tool whose dependency is not
/// wired here (Req 49.5).
pub fn dispatch(input: EngineInput) -> Result<EngineOutput, ApiError> {
    let tool = input.tool;
    if is_server_only(tool) {
        // Server_Only conversions are handled by out-of-process subprocesses
        // (LibreOffice / OCR / pdfium). When that dependency is not available
        // the Job is rejected as temporarily unavailable (Req 49.5).
        return Err(ApiError::Unavailable {
            reason: format!(
                "{} requires a server conversion dependency that is unavailable",
                tool_label(tool)
            ),
        });
    }
    pdf_engine::run(input).map_err(|e| map_engine_error(tool, &e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_engine::ToolOptions;

    #[test]
    fn protected_file_surfaces_tool_and_reason() {
        let err = map_engine_error(ToolId::Merge, &EngineError::Protected);
        match err {
            ApiError::Processing { tool, reason } => {
                assert_eq!(tool, "Merge");
                assert!(reason.contains("protected"));
            }
            other => panic!("expected Processing, got {other:?}"),
        }
    }

    #[test]
    fn no_output_surfaces_reason() {
        let err = map_engine_error(ToolId::PdfToJpg, &EngineError::NoOutputProduced);
        match err {
            ApiError::Processing { tool, reason } => {
                assert_eq!(tool, "PDF to JPG");
                assert!(reason.contains("no output"));
            }
            other => panic!("expected Processing, got {other:?}"),
        }
    }

    #[test]
    fn server_only_is_unavailable() {
        let input = EngineInput {
            tool: ToolId::WordToPdf,
            sources: vec![],
            options: ToolOptions::WordToPdf {},
        };
        match dispatch(input) {
            Err(ApiError::Unavailable { reason }) => {
                assert!(reason.contains("Word to PDF"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }
}

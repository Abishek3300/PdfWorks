//! Shared PDF processing engine.
//!
//! This crate is compiled to **two** targets from one codebase:
//! - `wasm32-unknown-unknown` for Client_Side_Processing inside a browser Web Worker.
//! - the native target for Server_Side_Processing inside the sandboxed backend.
//!
//! The public [`run`] entry point is **I/O-free, deterministic, and panic-free**:
//! the caller supplies bytes and options and receives bytes back. This keeps the
//! same logic backing both processing planes and makes it directly property-testable
//! (design Properties 1-13).
//!
//! The complete public API surface and data models are defined in [`model`],
//! matching the design's "Data Models" section exactly. The per-tool algorithms
//! are implemented in later tasks; every dispatch arm here is panic-free and
//! returns a structured [`EngineError`] placeholder until then.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod model;
mod pdf;
mod security;

// WASM binding layer (Task 7.1, Req 37.5, 47.1). Compiled ONLY for the browser
// target so the native build and `cargo test -p pdf-engine` are unaffected.
#[cfg(target_family = "wasm")]
mod wasm;

pub use model::{
    Angle, Element, EngineError, EngineInput, EngineOutput, FileBytes, FileRef, FormField,
    FormFieldKind, HtmlSource, Level, Margin, Orientation, OutputFile, PageScope, PdfALevel,
    Position, Rect, ResourceKind, ShapeKind, ToolId, ToolOptions,
};

// --- Shared security helpers (Task 6, design Properties 14-20) ---
pub use security::filename::{assign_unique_names, sanitize_filename};
pub use security::scanner::{scan_pdf, sanitize_pdf, ActiveContent, ScanReport};
pub use security::token::{
    check_access, constant_time_eq, generate_job_token, MIN_TOKEN_ENTROPY_BITS,
};

/// Single entry point shared by the WASM and native builds.
///
/// Pure with respect to `input`; performs no I/O. Returns a structured
/// [`EngineError`] rather than panicking on any invalid input.
///
/// Each `Client_Capable` tool routes to its own handler (implemented in later
/// tasks). `Server_Only` tools are not executed here — their conversions run in
/// the server layer via LibreOffice / OCR / pdfium subprocesses — so those arms
/// return [`EngineError::Unsupported`].
///
/// # Errors
///
/// Returns an [`EngineError`] describing why the Job could not be completed
/// (unsupported tool, invalid input, resource limit, or no output produced).
pub fn run(input: EngineInput) -> Result<EngineOutput, EngineError> {
    match input.tool {
        // --- Client_Capable tools (implemented by the shared engine) ---
        // Tasks 2.2-4.4 replace each stub with the real algorithm. Until then
        // they report no output rather than panicking.
        ToolId::Merge => run_merge(&input),
        ToolId::Split => run_split(&input),
        ToolId::RemovePages => run_remove_pages(&input),
        ToolId::ExtractPages => run_extract_pages(&input),
        ToolId::Organize => run_organize(&input),
        ToolId::OptimizePdf => run_optimize(&input),
        ToolId::CompressPdf => run_compress(&input),
        ToolId::JpgToPdf => run_jpg_to_pdf(&input),
        ToolId::PdfToJpg => run_pdf_to_jpg(&input),
        ToolId::Rotate => run_rotate(&input),
        ToolId::AddPageNumbers => run_add_page_numbers(&input),
        ToolId::AddWatermark => run_add_watermark(&input),
        ToolId::Crop => run_crop(&input),
        ToolId::MarkdownToPdf => run_markdown_to_pdf(&input),
        ToolId::PdfToMarkdown => run_pdf_to_markdown(&input),
        ToolId::EditPdf => run_edit_pdf(&input),
        ToolId::PdfForms => run_pdf_forms(&input),

        // --- Server_Only tools (never executed inside the shared engine) ---
        ToolId::WordToPdf
        | ToolId::PptToPdf
        | ToolId::ExcelToPdf
        | ToolId::HtmlToPdf
        | ToolId::PdfToWord
        | ToolId::PdfToPptx
        | ToolId::PdfToExcel
        | ToolId::PdfToPdfA
        | ToolId::ScanToPdf => Err(EngineError::Unsupported),
    }
}

// -------------------------------------------------------------------------
// Per-tool routing stubs.
//
// Each handler is a panic-free placeholder implemented fully in a later task
// (see tasks.md 2.2-4.4). They return `NoOutputProduced` so callers observe a
// structured error rather than a panic while the engine is being built out.
// -------------------------------------------------------------------------

/// Merge — concatenate the pages of two or more Source_Files into one
/// Output_File, in the caller-specified source `order` (Req 4.1, 4.2).
///
/// The Output_File's page count equals the sum of the source page counts, and
/// [`EngineOutput::source_page_counts`] records each source's page count in
/// original (unordered) slice position for invariant checks (Req 30.3).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if fewer than two sources are provided (Merge
///   requires at least two, Req 4.4) or the `order` permutation is invalid.
/// - Parse errors ([`EngineError::Empty`] / [`EngineError::Protected`] /
///   [`EngineError::Corrupt`]) surface from the offending Source_File.
fn run_merge(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::Merge { order } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    if input.sources.len() < 2 {
        return Err(EngineError::Unsupported);
    }

    // Parse every source once; record page counts by original slice position.
    let docs: Vec<lopdf::Document> = input
        .sources
        .iter()
        .map(|f| pdf::parse(f.bytes))
        .collect::<Result<_, _>>()?;
    let source_page_counts: Vec<u32> = docs.iter().map(pdf::page_count).collect();

    // Resolve the output ordering: an explicit permutation of source indices, or
    // natural slice order when none/empty was supplied.
    let ordered: Vec<&lopdf::Document> = if order.is_empty() {
        docs.iter().collect()
    } else {
        resolve_permutation(order, docs.len())?
            .into_iter()
            .map(|i| &docs[i])
            .collect()
    };

    let mut merged = pdf::concatenate(&ordered)?;
    let bytes = pdf::serialize(&mut merged)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "merged.pdf".to_string(),
            bytes,
        }],
        source_page_counts,
    })
}

/// Split — partition a single Source_File into one Output_File per resulting
/// range, defined by explicit `split_points` and/or a `fixed_size` (Req 5.1,
/// 5.2). Concatenating the ranges in order reproduces the original page
/// sequence with no page lost or duplicated (Property 2).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - [`EngineError::SplitPointOutOfRange`] carrying the actual page count if any
///   split point exceeds the page count (Req 5.3).
fn run_split(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::Split {
        split_points,
        fixed_size,
    } = &input.options
    else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    // Reject any split point beyond the document (Req 5.3), reporting the count.
    for &point in split_points {
        if point > total {
            return Err(EngineError::SplitPointOutOfRange { page_count: total });
        }
    }

    let ranges = compute_split_ranges(total, split_points, *fixed_size);

    let mut files = Vec::with_capacity(ranges.len());
    for (idx, range) in ranges.iter().enumerate() {
        let mut part = pdf::build_from_pages(&doc, range)?;
        let bytes = pdf::serialize(&mut part)?;
        files.push(OutputFile {
            name: format!("split_{}.pdf", idx + 1),
            bytes,
        });
    }

    Ok(EngineOutput {
        files,
        source_page_counts: vec![total],
    })
}

/// Remove Pages — produce one Output_File that excludes the selected 1-based
/// `pages` and retains the remaining pages in original order (Req 6.1). This is
/// the complement of [`run_extract_pages`].
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - [`EngineError::NoOutputProduced`] if every page was selected for removal
///   (at least one page must remain).
fn run_remove_pages(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::RemovePages { pages } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    let remove: std::collections::BTreeSet<u32> = pages.iter().copied().collect();
    let retained: Vec<u32> = (1..=total).filter(|p| !remove.contains(p)).collect();
    if retained.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }

    let mut out = pdf::build_from_pages(&doc, &retained)?;
    let bytes = pdf::serialize(&mut out)?;
    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "removed.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

/// Extract Pages — produce one Output_File containing only the selected 1-based
/// `pages`, in original document order; the output page count equals the number
/// of distinct selected pages (Req 7.1, 30.3, Property 5).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - [`EngineError::NoOutputProduced`] if the selection is empty or references
///   no existing page.
fn run_extract_pages(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::ExtractPages { pages } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    // Keep distinct, in-range pages in their original document order (Req 7.1).
    let selected: std::collections::BTreeSet<u32> =
        pages.iter().copied().filter(|&p| p >= 1 && p <= total).collect();
    let ordered: Vec<u32> = (1..=total).filter(|p| selected.contains(p)).collect();
    if ordered.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }

    let mut out = pdf::build_from_pages(&doc, &ordered)?;
    let bytes = pdf::serialize(&mut out)?;
    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "extracted.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

/// Organize — apply a permutation (`order`), per-page `rotations`, and `deletes`
/// in a single pass, producing one Output_File whose page sequence matches the
/// requested order (Req 8.2, 8.3, 8.4, Property 6).
///
/// `order` holds 0-based source page indices. Any index in `deletes` (1-based)
/// is dropped from the result; `rotations` sets the `/Rotate` entry of the named
/// 1-based source page. When `order` is empty the natural page order is used.
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided or the
///   permutation is invalid.
/// - [`EngineError::NoOutputProduced`] if no page remains after deletion.
fn run_organize(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::Organize {
        order,
        rotations,
        deletes,
    } = &input.options
    else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    // Resolve the requested page order as 1-based page numbers.
    let ordered_1based: Vec<u32> = if order.is_empty() {
        (1..=total).collect()
    } else {
        resolve_permutation(order, total as usize)?
            .into_iter()
            .map(|i| (i as u32) + 1)
            .collect()
    };

    // Drop deleted pages while preserving the requested order.
    let deleted: std::collections::BTreeSet<u32> = deletes.iter().copied().collect();
    let kept: Vec<u32> = ordered_1based
        .into_iter()
        .filter(|p| !deleted.contains(p))
        .collect();
    if kept.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }

    let mut out = pdf::build_from_pages(&doc, &kept)?;
    pdf::apply_rotations(&mut out, &kept, rotations);
    let bytes = pdf::serialize(&mut out)?;
    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "organized.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

// -------------------------------------------------------------------------
// Shared helpers for the page-algebra tools.
// -------------------------------------------------------------------------

/// Require exactly one Source_File, returning it or [`EngineError::Unsupported`].
fn single_source<'a, 'b>(input: &'b EngineInput<'a>) -> Result<&'b FileBytes<'a>, EngineError> {
    match input.sources.as_slice() {
        [only] => Ok(only),
        _ => Err(EngineError::Unsupported),
    }
}

/// Validate that `order` is a permutation of `0..len` and return it.
///
/// # Errors
///
/// Returns [`EngineError::Unsupported`] if `order` has the wrong length, an
/// out-of-range index, or a duplicate.
fn resolve_permutation(order: &[usize], len: usize) -> Result<Vec<usize>, EngineError> {
    if order.len() != len {
        return Err(EngineError::Unsupported);
    }
    let mut seen = vec![false; len];
    for &idx in order {
        let slot = seen.get_mut(idx).ok_or(EngineError::Unsupported)?;
        if *slot {
            return Err(EngineError::Unsupported); // duplicate index
        }
        *slot = true;
    }
    Ok(order.to_vec())
}

/// Compute the ordered list of 1-based page ranges a Split produces.
///
/// Explicit `split_points` (1-based page numbers *after* which to cut) take
/// precedence; when none are given, `fixed_size` chunks the document into equal
/// ranges (the final range may be shorter). With neither, the whole document is
/// a single range. The concatenation of the returned ranges is always exactly
/// `1..=total` in order (Property 2).
fn compute_split_ranges(
    total: u32,
    split_points: &[u32],
    fixed_size: Option<u32>,
) -> Vec<Vec<u32>> {
    if total == 0 {
        return Vec::new();
    }

    if !split_points.is_empty() {
        // Sort + dedup cut points, ignoring 0 and >= total (nothing after them).
        let mut cuts: Vec<u32> = split_points
            .iter()
            .copied()
            .filter(|&p| p >= 1 && p < total)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();

        let mut ranges = Vec::new();
        let mut start = 1u32;
        for cut in cuts {
            ranges.push((start..=cut).collect());
            start = cut + 1;
        }
        ranges.push((start..=total).collect());
        return ranges;
    }

    if let Some(size) = fixed_size {
        if size >= 1 {
            let mut ranges = Vec::new();
            let mut start = 1u32;
            while start <= total {
                let end = (start + size - 1).min(total);
                ranges.push((start..=end).collect());
                start = end + 1;
            }
            return ranges;
        }
    }

    // No split configuration: the whole document is one range.
    vec![(1..=total).collect()]
}

/// Optimize — reduce a PDF's structural overhead at the chosen Level while
/// guaranteeing the Output_File is no larger than the Source_File and its page
/// count is unchanged (Task 3.1, Req 10.1, 10.3, Property 7).
///
/// The single Source_File is losslessly re-serialized (pruned, stream-compressed,
/// and packed per the Level's strategy). If that rewrite does not shrink the
/// file, the original bytes are returned unchanged, so the output never grows.
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors ([`EngineError::Empty`] / [`EngineError::Protected`] /
///   [`EngineError::Corrupt`]) surface from the Source_File.
fn run_optimize(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::Optimize { level } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    optimize_like(input, *level, "optimized.pdf")
}

/// Compress — reduce a PDF's size at the chosen Level with the same guarantees
/// as Optimize: the Output_File is no larger than the Source_File and the page
/// count is preserved (Task 3.1, Req 11.1, 11.3, Property 7).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
fn run_compress(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::Compress { level } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    optimize_like(input, *level, "compressed.pdf")
}

/// Shared body of Optimize and Compress: both are lossless size-reduction passes
/// over a single Source_File that must never grow the file and must preserve the
/// page count (Req 10.1, 10.3, 11.1, 11.3, Property 7).
fn optimize_like(
    input: &EngineInput,
    level: Level,
    output_name: &str,
) -> Result<EngineOutput, EngineError> {
    let source = single_source(input)?;
    let strategy = pdf::OptimizeStrategy::from_level(level);
    let (bytes, page_count) = pdf::optimize_bytes(source.bytes, strategy)?;
    Ok(EngineOutput {
        files: vec![OutputFile {
            name: output_name.to_string(),
            bytes,
        }],
        source_page_counts: vec![page_count],
    })
}

/// JPG to PDF — build a PDF with exactly one page per JPG Source_File, honoring
/// the requested `orientation`, `margin`, and `order` (Task 3.3, Req 12.1, 12.2,
/// 12.3, Property 8).
///
/// Each source's pixel dimensions are read with a pure-Rust JPEG decoder (no
/// native/system dependency, so this stays WASM-compatible). The image is drawn
/// scaled to fit inside the page's margin box; the Output_File's page count
/// equals the number of JPG sources.
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if no sources are provided or `order` is not a
///   valid permutation of the sources.
/// - [`EngineError::Empty`] for a zero-byte source.
/// - [`EngineError::Corrupt`] if a source is not a decodable JPEG.
fn run_jpg_to_pdf(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::JpgToPdf {
        orientation,
        margin,
        order,
    } = &input.options
    else {
        return Err(EngineError::Unsupported);
    };
    if input.sources.is_empty() {
        return Err(EngineError::Unsupported);
    }

    // Resolve the page order: an explicit permutation of source indices, or the
    // natural upload order when none was supplied (Req 12.4).
    let indices: Vec<usize> = if order.is_empty() {
        (0..input.sources.len()).collect()
    } else {
        resolve_permutation(order, input.sources.len())?
    };

    let ordered: Vec<&FileBytes> = indices.iter().map(|&i| &input.sources[i]).collect();
    let mut doc = pdf::build_pdf_from_jpgs(&ordered, *orientation, *margin)?;
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "images.pdf".to_string(),
            bytes,
        }],
        source_page_counts: Vec::new(),
    })
}

/// PDF to JPG — produce exactly one JPG Output_File per page of the single PDF
/// Source_File (Task 3.3, Req 18.1, 18.2, Property 9).
///
/// ## WASM-compatibility note / Task 19.3 seam
///
/// The design specifies pdfium (`pdfium-render`) for high-fidelity page
/// rasterization. pdfium is a **native** library and is *not* available on
/// `wasm32-unknown-unknown`; linking it into this shared crate would break the
/// browser build that Client_Side_Processing depends on. To keep the shared
/// engine pure and WASM-compatible, this in-engine implementation renders each
/// page to a real, self-contained baseline JPG using a pure-Rust rasterizer
/// (see [`pdf::rasterize_pages_to_jpg`]). This guarantees Property 9 (one image
/// per page) holds identically on both the WASM and native builds.
///
/// Pixel-perfect, font-faithful rasterization is intentionally deferred to the
/// server plane: Task 19.3 wires pdfium in the (native-only) server layer and
/// may substitute its output per page. The one-JPG-per-page count invariant is
/// owned here and is unaffected by that substitution.
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
/// - [`EngineError::NoOutputProduced`] if the document has no pages.
fn run_pdf_to_jpg(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::PdfToJpg { dpi } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let doc = pdf::parse(source.bytes)?;
    let page_count = pdf::page_count(&doc);
    if page_count == 0 {
        return Err(EngineError::NoOutputProduced);
    }

    let images = pdf::rasterize_pages_to_jpg(&doc, *dpi)?;
    // One JPG Output_File per page, with pairwise-distinct names (Req 18.1).
    let files: Vec<OutputFile> = images
        .into_iter()
        .enumerate()
        .map(|(idx, bytes)| OutputFile {
            name: format!("page_{}.jpg", idx + 1),
            bytes,
        })
        .collect();

    Ok(EngineOutput {
        files,
        source_page_counts: vec![page_count],
    })
}

/// Rotate — rotate the scoped pages by the chosen [`Angle`] (Task 4.1, Req 24.1,
/// 24.2, 24.3, Property 12).
///
/// Supports rotating every page in a single action ([`PageScope::All`], Req
/// 24.2) or an individual page ([`PageScope::Pages`], Req 24.3). The rotation is
/// accumulated onto each targeted page's `/Rotate` entry, normalized to
/// `0..360`, so four 90° turns restore the original orientation (Property 12).
/// The page count is preserved.
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
fn run_rotate(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::Rotate { angle, pages } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let mut doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    let targets = pdf::resolve_scope(&doc, pages);
    pdf::rotate_pages(&mut doc, &targets, *angle);
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "rotated.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

/// Add Page Numbers — stamp a consecutive page number on every page at the
/// chosen [`Position`], numbering from `start` (Task 4.1, Req 25.1, 25.2, 25.3,
/// Property 13). The page count is preserved and every page receives a number.
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
fn run_add_page_numbers(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::AddPageNumbers { position, start } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let mut doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    pdf::add_page_numbers(&mut doc, *position, *start)?;
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "numbered.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

/// Add Watermark — overlay a text and/or image watermark on every page at the
/// given `opacity` and `rotation_deg` (Task 4.1, Req 26.1, 26.2, 26.3, 26.4,
/// Property 13). The page count is preserved and every page is marked.
///
/// A text watermark is drawn fully in-engine. An image watermark's bytes are
/// resolved by the server layer from the File_Store (the pure engine holds no
/// bytes for it); when only an image is requested the engine still marks every
/// page with a placeholder so the per-page invariant holds on both planes and
/// the server layer substitutes the real image (see [`pdf::add_watermark`]).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided or
///   neither text nor image was supplied.
/// - Parse errors surface from the Source_File.
fn run_add_watermark(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::AddWatermark {
        text,
        image,
        opacity,
        rotation_deg,
    } = &input.options
    else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let mut doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    pdf::add_watermark(
        &mut doc,
        text.as_deref(),
        image.is_some(),
        *opacity,
        *rotation_deg,
    )?;
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "watermarked.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

/// Crop — set the `/CropBox` of the targeted pages to `region` (Task 4.1, Req
/// 27.1, 27.3, Property 13). When `all_pages` is true every page is cropped in a
/// single action; otherwise only the first page. The page count is preserved.
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
fn run_crop(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::Crop { region, all_pages } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let mut doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    pdf::crop_pages(&mut doc, *region, *all_pages);
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "cropped.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

/// Markdown to PDF — render Markdown `text` (headings, lists, code blocks,
/// tables, links) to a paginated PDF Output_File (Task 3.7, Req 17.1, 17.3,
/// Property 11).
///
/// The Markdown is parsed with a pure-Rust CommonMark parser and laid out as
/// text lines with lopdf, keeping the whole path WASM-compatible. The output is
/// a single `document.pdf`.
///
/// # Errors
///
/// - [`EngineError::NoOutputProduced`] if the Markdown yields no renderable
///   content or serialization fails.
fn run_markdown_to_pdf(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::MarkdownToPdf { text } = &input.options else {
        return Err(EngineError::Unsupported);
    };

    let lines = pdf::parse_markdown(text);
    if lines.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }
    let mut doc = pdf::build_pdf_from_markdown(&lines)?;
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "document.pdf".to_string(),
            bytes,
        }],
        source_page_counts: Vec::new(),
    })
}

/// PDF to Markdown — extract the single Source_File's text as Markdown,
/// representing detected headings and lists with Markdown syntax (Task 3.7,
/// Req 23.1, 23.2, 23.3, Property 11).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
/// - [`EngineError::NoTextFound`] if the document has no extractable text
///   (Req 19.3-style rejection).
fn run_pdf_to_markdown(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::PdfToMarkdown {} = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let doc = pdf::parse(source.bytes)?;
    let markdown = pdf::extract_markdown(&doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "document.md".to_string(),
            bytes: markdown.into_bytes(),
        }],
        source_page_counts: vec![pdf::page_count(&doc)],
    })
}

/// Edit PDF — apply the added text / image / shape [`Element`]s at their
/// specified positions on their pages, preserving the existing pages (Task 4.4,
/// Req 28.4).
///
/// Text and shapes are drawn fully in-engine as overlay content streams. Image
/// elements reference a File_Store [`FileRef`] whose bytes the server layer
/// resolves; the pure engine draws a labeled placeholder at the element's box so
/// the element is present on both planes (see [`pdf::apply_elements`]).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
fn run_edit_pdf(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::EditPdf { elements } = &input.options else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let mut doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    pdf::apply_elements(&mut doc, elements)?;
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "edited.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

/// PDF Forms — write `field_values` into existing form fields and add
/// `added_fields` as interactive AcroForm fields in the Output_File (Task 4.4,
/// Req 29.2, 29.4).
///
/// Added fields are widget-annotation form fields placed on their page's
/// `/Annots` and registered in the document's `/AcroForm` `/Fields`, so they are
/// interactive in the output (see [`pdf::apply_form`]).
///
/// # Errors
///
/// - [`EngineError::Unsupported`] if not exactly one source is provided.
/// - Parse errors surface from the Source_File.
fn run_pdf_forms(input: &EngineInput) -> Result<EngineOutput, EngineError> {
    let ToolOptions::PdfForms {
        field_values,
        added_fields,
    } = &input.options
    else {
        return Err(EngineError::Unsupported);
    };
    let source = single_source(input)?;
    let mut doc = pdf::parse(source.bytes)?;
    let total = pdf::page_count(&doc);

    pdf::apply_form(&mut doc, field_values, added_fields)?;
    let bytes = pdf::serialize(&mut doc)?;

    Ok(EngineOutput {
        files: vec![OutputFile {
            name: "forms.pdf".to_string(),
            bytes,
        }],
        source_page_counts: vec![total],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(tool: ToolId, options: ToolOptions) -> EngineInput<'static> {
        EngineInput {
            tool,
            sources: Vec::new(),
            options,
        }
    }

    #[test]
    fn client_capable_tool_returns_structured_error_without_panicking() {
        // Merge with no sources is invalid (it needs >= 2, Req 4.4): it must
        // return a structured error, never panic.
        let out = run(input(ToolId::Merge, ToolOptions::Merge { order: Vec::new() }));
        assert!(matches!(out, Err(EngineError::Unsupported)));
    }

    #[test]
    fn server_only_tool_is_unsupported_in_the_shared_engine() {
        let out = run(input(ToolId::WordToPdf, ToolOptions::WordToPdf {}));
        assert!(matches!(out, Err(EngineError::Unsupported)));
    }

    #[test]
    fn every_tool_id_dispatches_without_panicking() {
        // Each variant must route to a handler that returns a structured error,
        // never panics. We pair each ToolId with a matching options variant.
        let cases = [
            (ToolId::Merge, ToolOptions::Merge { order: vec![] }),
            (
                ToolId::Split,
                ToolOptions::Split {
                    split_points: vec![],
                    fixed_size: None,
                },
            ),
            (ToolId::RemovePages, ToolOptions::RemovePages { pages: vec![] }),
            (
                ToolId::ExtractPages,
                ToolOptions::ExtractPages { pages: vec![] },
            ),
            (
                ToolId::Organize,
                ToolOptions::Organize {
                    order: vec![],
                    rotations: vec![],
                    deletes: vec![],
                },
            ),
            (
                ToolId::OptimizePdf,
                ToolOptions::Optimize { level: Level::Low },
            ),
            (
                ToolId::CompressPdf,
                ToolOptions::Compress {
                    level: Level::Medium,
                },
            ),
            (
                ToolId::JpgToPdf,
                ToolOptions::JpgToPdf {
                    orientation: Orientation::Portrait,
                    margin: Margin::None,
                    order: vec![],
                },
            ),
            (ToolId::PdfToJpg, ToolOptions::PdfToJpg { dpi: 150 }),
            (
                ToolId::Rotate,
                ToolOptions::Rotate {
                    angle: Angle::D90,
                    pages: PageScope::All,
                },
            ),
            (
                ToolId::AddPageNumbers,
                ToolOptions::AddPageNumbers {
                    position: Position::BottomCenter,
                    start: 1,
                },
            ),
            (
                ToolId::AddWatermark,
                ToolOptions::AddWatermark {
                    text: Some("DRAFT".to_string()),
                    image: None,
                    opacity: 50,
                    rotation_deg: 45,
                },
            ),
            (
                ToolId::Crop,
                ToolOptions::Crop {
                    region: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    all_pages: true,
                },
            ),
            (
                ToolId::MarkdownToPdf,
                ToolOptions::MarkdownToPdf {
                    text: "# Title".to_string(),
                },
            ),
            (ToolId::PdfToMarkdown, ToolOptions::PdfToMarkdown {}),
            (ToolId::EditPdf, ToolOptions::EditPdf { elements: vec![] }),
            (
                ToolId::PdfForms,
                ToolOptions::PdfForms {
                    field_values: vec![],
                    added_fields: vec![],
                },
            ),
            (ToolId::WordToPdf, ToolOptions::WordToPdf {}),
            (ToolId::PptToPdf, ToolOptions::PptToPdf {}),
            (
                ToolId::ExcelToPdf,
                ToolOptions::ExcelToPdf {
                    orientation: Orientation::Landscape,
                },
            ),
            (
                ToolId::HtmlToPdf,
                ToolOptions::HtmlToPdf {
                    source: HtmlSource::Inline("<p>hi</p>".to_string()),
                    orientation: Orientation::Portrait,
                },
            ),
            (ToolId::PdfToWord, ToolOptions::PdfToWord {}),
            (ToolId::PdfToPptx, ToolOptions::PdfToPptx {}),
            (ToolId::PdfToExcel, ToolOptions::PdfToExcel {}),
            (
                ToolId::PdfToPdfA,
                ToolOptions::PdfToPdfA {
                    level: PdfALevel::A2b,
                },
            ),
            (
                ToolId::ScanToPdf,
                ToolOptions::ScanToPdf {
                    images: vec![],
                    ocr: false,
                },
            ),
        ];

        for (tool, options) in cases {
            // Every arm must route to a handler and return a structured Result
            // without panicking. Tools that need no sources and have valid
            // options (e.g. MarkdownToPdf) may legitimately succeed here; the
            // guarantee under test is "dispatch is total and panic-free", so we
            // accept either an Ok or a structured Err.
            let _result: Result<EngineOutput, EngineError> = run(input(tool, options));
        }
    }
}

#[cfg(test)]
mod property_tests {
    //! Property-based tests for the page-algebra tools (design Properties 1-6).
    //!
    //! Each test runs a minimum of 100 iterations (`ProptestConfig::cases`) over
    //! generated valid PDFs whose pages are individually tagged, so page identity
    //! and order can be asserted after each operation. Generators are constrained
    //! to the meaningful input space (small but non-trivial page counts, valid
    //! permutations/subsets) rather than sampling raw bytes.

    // Test code may unwrap/expect freely; the library-wide deny of these lints is
    // relaxed here so failing assertions surface as clear test panics.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::pdf::test_support::{make_jpg, make_pdf_with_tags, read_tags};
    use proptest::collection::vec;
    use proptest::prelude::*;

    /// A minimum of 100 iterations per property, as required by the design.
    const MIN_CASES: u32 = 100;

    fn cfg() -> ProptestConfig {
        ProptestConfig::with_cases(MIN_CASES)
    }

    /// Build a `FileBytes` that owns its buffer for the duration of a case.
    fn source(bytes: &[u8]) -> FileBytes<'_> {
        FileBytes {
            name: "in.pdf".to_string(),
            bytes,
        }
    }

    // Feature: pdf-tools-suite, Property 1: Merge preserves and concatenates pages in source order
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop1_merge_concatenates_pages_in_source_order(
            counts in vec(1u32..=5, 2..=4)
        ) {
            // Each source gets a disjoint tag block so identity is unambiguous:
            // source k pages are tagged base_k.., and the merged output must be
            // exactly those blocks concatenated in source order.
            let mut base = 0u32;
            let mut sources_bytes: Vec<Vec<u8>> = Vec::new();
            let mut expected: Vec<u32> = Vec::new();
            for &c in &counts {
                let tags: Vec<u32> = (0..c).map(|i| base + i + 1).collect();
                base += 100; // large gap so blocks never overlap
                expected.extend(tags.iter().copied());
                sources_bytes.push(make_pdf_with_tags(&tags));
            }

            let sources: Vec<FileBytes> = sources_bytes.iter().map(|b| source(b)).collect();
            let expected_total: u32 = counts.iter().sum();

            let out = run(EngineInput {
                tool: ToolId::Merge,
                sources,
                options: ToolOptions::Merge { order: Vec::new() },
            }).expect("merge should succeed for valid sources");

            // Exactly one Output_File (Req 4.1).
            prop_assert_eq!(out.files.len(), 1);
            // Page count equals the sum of the source page counts (Property 1).
            let produced = read_tags(&out.files[0].bytes);
            prop_assert_eq!(produced.len() as u32, expected_total);
            // Pages appear in source order (Req 4.2).
            prop_assert_eq!(produced, expected);
            // source_page_counts is populated (Req 30.3).
            prop_assert_eq!(out.source_page_counts, counts);
        }
    }

    // Feature: pdf-tools-suite, Property 2: Split partitions the document exactly
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop2_split_partitions_exactly(
            page_count in 1u32..=10,
            raw_points in vec(1u32..=10, 0..=4),
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);

            // Constrain split points to be strictly inside the document.
            let mut points: Vec<u32> =
                raw_points.into_iter().filter(|&p| p >= 1 && p < page_count).collect();
            points.sort_unstable();
            points.dedup();

            let out = run(EngineInput {
                tool: ToolId::Split,
                sources: vec![source(&bytes)],
                options: ToolOptions::Split {
                    split_points: points.clone(),
                    fixed_size: None,
                },
            }).expect("split should succeed for in-range points");

            // Concatenating the ranges reproduces the original sequence exactly,
            // with no page lost or duplicated (Property 2).
            let mut recombined: Vec<u32> = Vec::new();
            for f in &out.files {
                recombined.extend(read_tags(&f.bytes));
            }
            prop_assert_eq!(recombined, tags);

            // Number of outputs equals number of resulting ranges (Req 5.1).
            prop_assert_eq!(out.files.len(), points.len() + 1);
        }
    }

    // Feature: pdf-tools-suite, Property 2 (fixed-size variant): fixed-size ranges partition exactly
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop2_split_fixed_size_partitions_exactly(
            page_count in 1u32..=10,
            size in 1u32..=5,
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);

            let out = run(EngineInput {
                tool: ToolId::Split,
                sources: vec![source(&bytes)],
                options: ToolOptions::Split {
                    split_points: Vec::new(),
                    fixed_size: Some(size),
                },
            }).expect("fixed-size split should succeed");

            let mut recombined: Vec<u32> = Vec::new();
            for f in &out.files {
                let part = read_tags(&f.bytes);
                // Each range has at most `size` pages.
                prop_assert!(part.len() as u32 <= size);
                recombined.extend(part);
            }
            prop_assert_eq!(recombined, tags);
        }
    }

    // Feature: pdf-tools-suite, Property 3: Split rejects out-of-range split points
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop3_split_rejects_out_of_range(
            page_count in 1u32..=10,
            overshoot in 1u32..=20,
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);
            let bad_point = page_count + overshoot; // strictly greater than page count

            let err = run(EngineInput {
                tool: ToolId::Split,
                sources: vec![source(&bytes)],
                options: ToolOptions::Split {
                    split_points: vec![bad_point],
                    fixed_size: None,
                },
            }).expect_err("split must reject an out-of-range point");

            // The error carries the actual page count (Req 5.3).
            prop_assert_eq!(err, EngineError::SplitPointOutOfRange { page_count });
        }
    }

    // Feature: pdf-tools-suite, Property 4: Remove Pages yields the complement in original order
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop4_remove_pages_yields_complement(
            page_count in 2u32..=10,
            selector in vec(any::<bool>(), 2..=10),
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);

            // Choose pages to remove from `selector`, but always keep at least one.
            let mut remove: Vec<u32> = (1..=page_count)
                .filter(|&p| *selector.get((p - 1) as usize).unwrap_or(&false))
                .collect();
            if remove.len() as u32 == page_count {
                remove.pop(); // retain at least one page
            }
            let remove_set: std::collections::BTreeSet<u32> = remove.iter().copied().collect();
            let expected: Vec<u32> =
                (1..=page_count).filter(|p| !remove_set.contains(p)).collect();

            let out = run(EngineInput {
                tool: ToolId::RemovePages,
                sources: vec![source(&bytes)],
                options: ToolOptions::RemovePages { pages: remove },
            }).expect("remove should succeed while a page remains");

            prop_assert_eq!(out.files.len(), 1);
            // Output is exactly the complement, in original relative order (Req 6.1).
            prop_assert_eq!(read_tags(&out.files[0].bytes), expected);
        }
    }

    // Feature: pdf-tools-suite, Property 5: Extract Pages page count equals selection size
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop5_extract_pages_count_and_order(
            page_count in 1u32..=10,
            selector in vec(any::<bool>(), 1..=10),
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);

            // Choose a non-empty selection.
            let mut selection: Vec<u32> = (1..=page_count)
                .filter(|&p| *selector.get((p - 1) as usize).unwrap_or(&false))
                .collect();
            if selection.is_empty() {
                selection.push(1);
            }
            let selection_set: std::collections::BTreeSet<u32> =
                selection.iter().copied().collect();
            // Expected: the selected pages in original document order (Req 7.1).
            let expected: Vec<u32> =
                (1..=page_count).filter(|p| selection_set.contains(p)).collect();

            let out = run(EngineInput {
                tool: ToolId::ExtractPages,
                sources: vec![source(&bytes)],
                options: ToolOptions::ExtractPages { pages: selection },
            }).expect("extract should succeed for a non-empty selection");

            prop_assert_eq!(out.files.len(), 1);
            let produced = read_tags(&out.files[0].bytes);
            // Page count equals selection size (Req 7.1, 30.3).
            prop_assert_eq!(produced.len(), expected.len());
            // Pages appear in original order.
            prop_assert_eq!(produced, expected);
        }
    }

    // Feature: pdf-tools-suite, Property 6: Organize applies the requested permutation
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop6_organize_applies_permutation(
            page_count in 1usize..=8,
            seed in any::<u64>(),
        ) {
            let tags: Vec<u32> = (1..=page_count as u32).collect();
            let bytes = make_pdf_with_tags(&tags);

            // Build a deterministic permutation of 0..page_count from `seed`
            // (Fisher-Yates using a small LCG, no external rng dependency).
            let mut order: Vec<usize> = (0..page_count).collect();
            let mut state = seed | 1;
            for i in (1..page_count).rev() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let j = (state >> 33) as usize % (i + 1);
                order.swap(i, j);
            }

            // Expected output tags follow the requested order (source page = index + 1).
            let expected: Vec<u32> = order.iter().map(|&i| (i as u32) + 1).collect();

            let out = run(EngineInput {
                tool: ToolId::Organize,
                sources: vec![source(&bytes)],
                options: ToolOptions::Organize {
                    order,
                    rotations: Vec::new(),
                    deletes: Vec::new(),
                },
            }).expect("organize should succeed for a valid permutation");

            prop_assert_eq!(out.files.len(), 1);
            // Output page sequence matches the requested order (Req 8.2).
            prop_assert_eq!(read_tags(&out.files[0].bytes), expected);
        }
    }

    // Feature: pdf-tools-suite, Property 6 (rotate/delete variant): Organize honors deletes and rotations
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop6_organize_delete_and_rotate(
            page_count in 2u32..=8,
            delete_which in 1u32..=8,
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);
            let to_delete = ((delete_which - 1) % page_count) + 1; // in 1..=page_count

            // Natural order, delete one page, rotate the first source page 90 deg.
            let expected: Vec<u32> =
                (1..=page_count).filter(|&p| p != to_delete).collect();

            let out = run(EngineInput {
                tool: ToolId::Organize,
                sources: vec![source(&bytes)],
                options: ToolOptions::Organize {
                    order: Vec::new(),
                    rotations: vec![(1, Angle::D90)],
                    deletes: vec![to_delete],
                },
            }).expect("organize should succeed");

            // The deleted page is gone; remaining pages keep their order (Req 8.4).
            prop_assert_eq!(read_tags(&out.files[0].bytes), expected);
        }
    }

    // A small helper: run Optimize or Compress and return the single output.
    fn run_size_tool(tool: ToolId, level: Level, bytes: &[u8]) -> EngineOutput {
        let options = match tool {
            ToolId::OptimizePdf => ToolOptions::Optimize { level },
            ToolId::CompressPdf => ToolOptions::Compress { level },
            _ => unreachable!("only size tools use this helper"),
        };
        run(EngineInput {
            tool,
            sources: vec![source(bytes)],
            options,
        })
        .expect("optimize/compress should succeed for a valid PDF")
    }

    // Feature: pdf-tools-suite, Property 7: Optimize and Compress never grow the file and preserve page count
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop7_optimize_compress_never_grow_preserve_pages(
            page_count in 1u32..=12,
            level_sel in 0u8..3,
            use_compress in any::<bool>(),
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);
            let source_len = bytes.len();

            let level = match level_sel {
                0 => Level::Low,
                1 => Level::Medium,
                _ => Level::High,
            };
            let tool = if use_compress { ToolId::CompressPdf } else { ToolId::OptimizePdf };

            let out = run_size_tool(tool, level, &bytes);

            // Exactly one Output_File.
            prop_assert_eq!(out.files.len(), 1);
            let out_bytes = &out.files[0].bytes;

            // Output never grows beyond the source (Req 10.1, 11.1).
            prop_assert!(
                out_bytes.len() <= source_len,
                "output {} must be <= source {}",
                out_bytes.len(),
                source_len
            );

            // The reported source page count is preserved, and the produced PDF
            // parses back to the same page count (Req 10.3, 11.3).
            prop_assert_eq!(out.source_page_counts, vec![page_count]);
            let reparsed = crate::pdf::parse(out_bytes).expect("output must be a valid PDF");
            prop_assert_eq!(crate::pdf::page_count(&reparsed), page_count);
        }
    }

    // Feature: pdf-tools-suite, Property 8: JPG to PDF produces one page per image
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop8_jpg_to_pdf_one_page_per_image(
            n in 1usize..=6,
            orient_sel in any::<bool>(),
            margin_sel in 0u8..3,
        ) {
            // Distinct small dimensions so images are not all identical.
            let jpgs: Vec<Vec<u8>> = (0..n)
                .map(|i| make_jpg(16 + i as u16, 24 + i as u16))
                .collect();
            let sources: Vec<FileBytes> = jpgs.iter().map(|b| source(b)).collect();

            let orientation = if orient_sel { Orientation::Portrait } else { Orientation::Landscape };
            let margin = match margin_sel { 0 => Margin::None, 1 => Margin::Small, _ => Margin::Large };

            let out = run(EngineInput {
                tool: ToolId::JpgToPdf,
                sources,
                options: ToolOptions::JpgToPdf { orientation, margin, order: Vec::new() },
            }).expect("jpg to pdf should succeed for valid jpgs");

            // Exactly one Output_File whose page count equals the image count (Req 12.1).
            prop_assert_eq!(out.files.len(), 1);
            let doc = crate::pdf::parse(&out.files[0].bytes).expect("output must be a valid PDF");
            prop_assert_eq!(crate::pdf::page_count(&doc) as usize, n);
        }
    }

    // Feature: pdf-tools-suite, Property 9: PDF to JPG produces one image per page
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop9_pdf_to_jpg_one_image_per_page(
            page_count in 1u32..=8,
            dpi in 24u32..=200,
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);

            let out = run(EngineInput {
                tool: ToolId::PdfToJpg,
                sources: vec![source(&bytes)],
                options: ToolOptions::PdfToJpg { dpi },
            }).expect("pdf to jpg should succeed for a valid PDF");

            // One JPG Output_File per page (Req 18.1).
            prop_assert_eq!(out.files.len() as u32, page_count);
            // Names are pairwise distinct and each output is a decodable JPEG.
            let mut names = std::collections::BTreeSet::new();
            for f in &out.files {
                prop_assert!(names.insert(f.name.clone()), "output names must be unique");
                let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(&f.bytes));
                prop_assert!(dec.read_info().is_ok(), "each output must be a valid JPEG");
            }
        }
    }

    // Feature: pdf-tools-suite, Property 10: JPG round-trip preserves image count
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop10_jpg_round_trip_preserves_image_count(
            n in 1usize..=6,
        ) {
            let jpgs: Vec<Vec<u8>> = (0..n)
                .map(|i| make_jpg(20 + i as u16, 28 + i as u16))
                .collect();
            let sources: Vec<FileBytes> = jpgs.iter().map(|b| source(b)).collect();

            // JpgToPdf ...
            let pdf_out = run(EngineInput {
                tool: ToolId::JpgToPdf,
                sources,
                options: ToolOptions::JpgToPdf {
                    orientation: Orientation::Portrait,
                    margin: Margin::Small,
                    order: Vec::new(),
                },
            }).expect("jpg to pdf should succeed");
            prop_assert_eq!(pdf_out.files.len(), 1);

            // ... then PdfToJpg yields exactly one JPG per original image (Req 30.2).
            let jpg_out = run(EngineInput {
                tool: ToolId::PdfToJpg,
                sources: vec![source(&pdf_out.files[0].bytes)],
                options: ToolOptions::PdfToJpg { dpi: 96 },
            }).expect("pdf to jpg should succeed");

            prop_assert_eq!(jpg_out.files.len(), n);
        }
    }

    // Feature: pdf-tools-suite, Property 11: Markdown round-trip preserves structure
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop11_markdown_round_trip_preserves_structure(
            lines in vec(md_line_strategy(), 1..=12),
        ) {
            // Build source Markdown from the generated structural lines, one per
            // line. Constrained to the design-guaranteed subset: ATX headings,
            // simple unordered list items, and single-line paragraphs.
            let source_md = lines.join("\n\n");

            // MarkdownToPdf ...
            let pdf_out = run(EngineInput {
                tool: ToolId::MarkdownToPdf,
                sources: Vec::new(),
                options: ToolOptions::MarkdownToPdf { text: source_md.clone() },
            }).expect("markdown to pdf should succeed");
            prop_assert_eq!(pdf_out.files.len(), 1);

            // ... then PdfToMarkdown.
            let md_out = run(EngineInput {
                tool: ToolId::PdfToMarkdown,
                sources: vec![source(&pdf_out.files[0].bytes)],
                options: ToolOptions::PdfToMarkdown {},
            }).expect("pdf to markdown should succeed");

            let round_tripped = String::from_utf8(md_out.files[0].bytes.clone())
                .expect("markdown output is utf-8");

            // The structural skeleton (kind + text of each heading/list/paragraph)
            // must match the original (Property 11, Req 30.1).
            let expected = structural_skeleton(&source_md);
            let actual = structural_skeleton(&round_tripped);
            prop_assert_eq!(actual, expected);
        }
    }

    /// Generate one Markdown structural line from the guaranteed subset:
    /// heading (levels 1-3), unordered list item, or a plain paragraph. Text is
    /// restricted to lowercase words + spaces so it survives PDF text extraction
    /// unambiguously and carries no Markdown-significant characters.
    fn md_line_strategy() -> impl Strategy<Value = String> {
        let word = "[a-z]{1,6}";
        let phrase = proptest::collection::vec(word, 1..=4)
            .prop_map(|ws| ws.join(" "));
        prop_oneof![
            (1u8..=3, phrase.clone()).prop_map(|(lvl, text)| {
                format!("{} {}", "#".repeat(lvl as usize), text)
            }),
            phrase.clone().prop_map(|text| format!("- {text}")),
            phrase.prop_map(|text| text),
        ]
    }

    /// Reduce Markdown text to its structural skeleton: a list of
    /// `(kind, text)` tuples for every non-empty line, where kind is the
    /// heading level, list marker, or paragraph. This is exactly the subset
    /// Property 11 guarantees round-trips.
    fn structural_skeleton(md: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for line in md.split('\n') {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let hashes = t.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
                out.push((format!("h{hashes}"), t[hashes + 1..].trim().to_string()));
            } else if let Some(rest) = t.strip_prefix("- ") {
                out.push(("li".to_string(), rest.trim().to_string()));
            } else {
                out.push(("p".to_string(), t.to_string()));
            }
        }
        out
    }

    // Feature: pdf-tools-suite, Property 12: Rotate is angle-correct and four 90° turns are the identity
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop12_rotate_is_angle_correct(
            page_count in 1u32..=6,
            angle_sel in 0u8..3,
            rotate_all in any::<bool>(),
            which in 1u32..=6,
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);

            let angle = match angle_sel {
                0 => Angle::D90,
                1 => Angle::D180,
                _ => Angle::D270,
            };
            let expected_deg: i64 = match angle {
                Angle::D90 => 90,
                Angle::D180 => 180,
                Angle::D270 => 270,
            };

            // Rotate all pages, or a single individual page (Req 24.2, 24.3).
            let target_page = ((which - 1) % page_count) + 1;
            let scope = if rotate_all {
                PageScope::All
            } else {
                PageScope::Pages(vec![target_page])
            };
            let targeted: Vec<u32> = if rotate_all {
                (1..=page_count).collect()
            } else {
                vec![target_page]
            };

            let out = run(EngineInput {
                tool: ToolId::Rotate,
                sources: vec![source(&bytes)],
                options: ToolOptions::Rotate { angle, pages: scope },
            }).expect("rotate should succeed for a valid PDF");

            prop_assert_eq!(out.files.len(), 1);
            let doc = crate::pdf::parse(&out.files[0].bytes).expect("output is a valid PDF");
            // Page count is preserved (Req 24.1 does not change page count).
            prop_assert_eq!(crate::pdf::page_count(&doc), page_count);

            // Each targeted page is rotated by exactly the selected angle; every
            // other page keeps rotation 0 (Req 24.1, 24.2, 24.3).
            for p in 1..=page_count {
                let rot = crate::pdf::page_rotation(&doc, p);
                if targeted.contains(&p) {
                    prop_assert_eq!(rot, expected_deg, "page {} should be rotated {}", p, expected_deg);
                } else {
                    prop_assert_eq!(rot, 0, "untargeted page {} should not rotate", p);
                }
            }
        }
    }

    // Feature: pdf-tools-suite, Property 12 (identity): four 90° turns restore the original /Rotate
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop12_four_quarter_turns_are_identity(
            page_count in 1u32..=6,
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let mut bytes = make_pdf_with_tags(&tags);

            // Apply a 90° rotation to all pages four times; the /Rotate of every
            // page must return to its original value (0) (Property 12).
            for _ in 0..4 {
                let out = run(EngineInput {
                    tool: ToolId::Rotate,
                    sources: vec![source(&bytes)],
                    options: ToolOptions::Rotate {
                        angle: Angle::D90,
                        pages: PageScope::All,
                    },
                }).expect("rotate should succeed");
                bytes = out.files[0].bytes.clone();
            }

            let doc = crate::pdf::parse(&bytes).expect("output is a valid PDF");
            prop_assert_eq!(crate::pdf::page_count(&doc), page_count);
            for p in 1..=page_count {
                prop_assert_eq!(
                    crate::pdf::page_rotation(&doc, p),
                    0,
                    "four 90° turns must restore page {}'s original orientation",
                    p
                );
            }
        }
    }

    /// Which per-page annotation tool to exercise in Property 13.
    #[derive(Debug, Clone, Copy)]
    enum Annotation {
        PageNumbers,
        Watermark,
        Crop,
    }

    // Feature: pdf-tools-suite, Property 13: Per-page annotations preserve page count and mark every page
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop13_annotations_preserve_count_and_mark_every_page(
            page_count in 1u32..=6,
            annotation_sel in 0u8..3,
        ) {
            let tags: Vec<u32> = (1..=page_count).collect();
            let bytes = make_pdf_with_tags(&tags);
            // Baseline number of content streams per page (make_pdf_with_tags
            // gives each page exactly one), used to detect an appended overlay.
            let base_doc = crate::pdf::parse(&bytes).expect("baseline is a valid PDF");
            let base_streams: Vec<usize> = (1..=page_count)
                .map(|p| crate::pdf::page_content_stream_count(&base_doc, p))
                .collect();

            let annotation = match annotation_sel {
                0 => Annotation::PageNumbers,
                1 => Annotation::Watermark,
                _ => Annotation::Crop,
            };

            let options = match annotation {
                Annotation::PageNumbers => ToolOptions::AddPageNumbers {
                    position: Position::BottomCenter,
                    start: 1,
                },
                Annotation::Watermark => ToolOptions::AddWatermark {
                    text: Some("DRAFT".to_string()),
                    image: None,
                    opacity: 40,
                    rotation_deg: 45,
                },
                Annotation::Crop => ToolOptions::Crop {
                    region: Rect { x: 10.0, y: 10.0, width: 400.0, height: 500.0 },
                    all_pages: true,
                },
            };
            let tool = match annotation {
                Annotation::PageNumbers => ToolId::AddPageNumbers,
                Annotation::Watermark => ToolId::AddWatermark,
                Annotation::Crop => ToolId::Crop,
            };

            let out = run(EngineInput {
                tool,
                sources: vec![source(&bytes)],
                options,
            }).expect("annotation should succeed for a valid PDF");

            prop_assert_eq!(out.files.len(), 1);
            let doc = crate::pdf::parse(&out.files[0].bytes).expect("output is a valid PDF");

            // Page count is preserved (Property 13, Req 25.1/26.1/27.1).
            prop_assert_eq!(crate::pdf::page_count(&doc), page_count);

            // Every targeted page received the change.
            for p in 1..=page_count {
                match annotation {
                    Annotation::Crop => {
                        // Each page carries the requested CropBox (Req 27.1).
                        let cb = crate::pdf::page_crop_box(&doc, p)
                            .expect("every page must have a CropBox");
                        prop_assert!((cb[0] - 10.0).abs() < 0.01);
                        prop_assert!((cb[1] - 10.0).abs() < 0.01);
                        prop_assert!((cb[2] - 410.0).abs() < 0.01);
                        prop_assert!((cb[3] - 510.0).abs() < 0.01);
                    }
                    Annotation::PageNumbers | Annotation::Watermark => {
                        // Each page gained an overlay content stream (Req 25.1, 26.1).
                        let streams = crate::pdf::page_content_stream_count(&doc, p);
                        let base = base_streams[(p - 1) as usize];
                        prop_assert!(
                            streams > base,
                            "page {} should gain an overlay stream (base {}, now {})",
                            p, base, streams
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod edit_and_forms_tests {
    //! Unit tests for Task 4.4 (Edit PDF and PDF Forms element application).

    // Test code may unwrap/expect freely.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::pdf::test_support::make_pdf_with_tags;

    fn source(bytes: &[u8]) -> FileBytes<'_> {
        FileBytes {
            name: "in.pdf".to_string(),
            bytes,
        }
    }

    #[test]
    fn edit_pdf_places_text_element_on_the_target_page() {
        // A two-page document; add a text element to page 2 (Req 28.4).
        let bytes = make_pdf_with_tags(&[1, 2]);
        let base = crate::pdf::parse(&bytes).unwrap();
        let base_streams_p2 = crate::pdf::page_content_stream_count(&base, 2);

        let out = run(EngineInput {
            tool: ToolId::EditPdf,
            sources: vec![source(&bytes)],
            options: ToolOptions::EditPdf {
                elements: vec![Element::Text {
                    page: 2,
                    at: Rect { x: 100.0, y: 100.0, width: 200.0, height: 20.0 },
                    content: "annotation".to_string(),
                    font_size: 14.0,
                }],
            },
        })
        .expect("edit pdf should succeed");

        assert_eq!(out.files.len(), 1);
        let doc = crate::pdf::parse(&out.files[0].bytes).unwrap();
        // Existing pages preserved.
        assert_eq!(crate::pdf::page_count(&doc), 2);
        // The element was added as an overlay stream on page 2, and the
        // extracted text now contains the element content (Req 28.4).
        assert!(crate::pdf::page_content_stream_count(&doc, 2) > base_streams_p2);
        let text = doc.extract_text(&[2]).unwrap_or_default();
        assert!(
            text.contains("annotation"),
            "page 2 text should include the added element: {text:?}"
        );
    }

    #[test]
    fn edit_pdf_draws_shape_element() {
        let bytes = make_pdf_with_tags(&[1]);
        let out = run(EngineInput {
            tool: ToolId::EditPdf,
            sources: vec![source(&bytes)],
            options: ToolOptions::EditPdf {
                elements: vec![Element::Shape {
                    page: 1,
                    at: Rect { x: 50.0, y: 50.0, width: 100.0, height: 80.0 },
                    kind: ShapeKind::Rectangle,
                }],
            },
        })
        .expect("edit pdf should succeed");

        let doc = crate::pdf::parse(&out.files[0].bytes).unwrap();
        assert_eq!(crate::pdf::page_count(&doc), 1);
        // The shape overlay is present as an added content stream.
        assert!(crate::pdf::page_content_stream_count(&doc, 1) >= 2);
    }

    #[test]
    fn pdf_forms_sets_value_and_adds_interactive_field() {
        // Start from a plain document (no AcroForm). PDF Forms adds an
        // interactive field and, for a field we pre-name, sets its value.
        let bytes = make_pdf_with_tags(&[1]);

        let out = run(EngineInput {
            tool: ToolId::PdfForms,
            sources: vec![source(&bytes)],
            options: ToolOptions::PdfForms {
                // No matching existing field in the plain doc, but the added
                // field below is created, then its value is verified via the
                // added field's own default; we also add a second field to set.
                field_values: vec![("full_name".to_string(), "Ada Lovelace".to_string())],
                added_fields: vec![
                    FormField {
                        name: "full_name".to_string(),
                        kind: FormFieldKind::Text,
                        page: 1,
                        at: Rect { x: 100.0, y: 700.0, width: 200.0, height: 20.0 },
                    },
                    FormField {
                        name: "agree".to_string(),
                        kind: FormFieldKind::Checkbox,
                        page: 1,
                        at: Rect { x: 100.0, y: 660.0, width: 20.0, height: 20.0 },
                    },
                ],
            },
        })
        .expect("pdf forms should succeed");

        assert_eq!(out.files.len(), 1);
        let doc = crate::pdf::parse(&out.files[0].bytes).unwrap();
        assert_eq!(crate::pdf::page_count(&doc), 1);

        // Both added fields are interactive: registered in the AcroForm (Req 29.4).
        assert_eq!(
            crate::pdf::acroform_field_count(&doc),
            2,
            "both added fields must be interactive AcroForm entries"
        );

        // The set field value is present on the corresponding field (Req 29.2).
        // The value is applied to the field matched by name, whether it existed
        // originally or was just added.
        assert_eq!(
            crate::pdf::field_value(&doc, "full_name").as_deref(),
            Some("Ada Lovelace"),
            "the entered value must be written into the field"
        );
    }

    #[test]
    fn pdf_forms_sets_value_on_a_preexisting_field() {
        // Build a document that already contains a text field named "email",
        // then set its value via PDF Forms (Req 29.2 for existing fields).
        let bytes = crate::pdf::test_support::make_pdf_with_text_field("email");

        let out = run(EngineInput {
            tool: ToolId::PdfForms,
            sources: vec![source(&bytes)],
            options: ToolOptions::PdfForms {
                field_values: vec![("email".to_string(), "ada@example.com".to_string())],
                added_fields: vec![],
            },
        })
        .expect("pdf forms should succeed");

        let doc = crate::pdf::parse(&out.files[0].bytes).unwrap();
        assert_eq!(
            crate::pdf::field_value(&doc, "email").as_deref(),
            Some("ada@example.com"),
            "an existing field's value must be updated"
        );
    }
}

#[cfg(test)]
mod security_property_tests {
    //! Property-based tests for the shared security helpers (design Properties
    //! 14, 15, 16, 19, 20). Each runs a minimum of 100 iterations
    //! (`ProptestConfig::cases`) over generators constrained to the meaningful
    //! input space. Generators for the Content_Scanner inject active-content
    //! constructs into otherwise-valid PDFs.

    // Test code may unwrap/expect freely; the crate-wide deny of these lints is
    // relaxed here so failing assertions surface as clear test panics.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::pdf::test_support::make_pdf_with_tags;
    use crate::security::scanner::ActiveContent;
    use lopdf::{dictionary, Document, Object, Stream};
    use proptest::collection::vec;
    use proptest::prelude::*;
    use std::collections::HashSet;

    /// A minimum of 100 iterations per property, as required by the design.
    const MIN_CASES: u32 = 100;

    fn cfg() -> ProptestConfig {
        ProptestConfig::with_cases(MIN_CASES)
    }

    // ---------------------------------------------------------------------
    // Property 14: Multiple outputs receive unique names (Req 49.3).
    // ---------------------------------------------------------------------

    // Feature: pdf-tools-suite, Property 14: Multiple outputs receive unique names
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop14_multiple_outputs_receive_unique_names(
            // A batch of raw, possibly-colliding, possibly-hostile names.
            names in vec(raw_name_strategy(), 2..=12),
        ) {
            let assigned = assign_unique_names(&names);

            // Same cardinality (order preserved, one output name per input).
            prop_assert_eq!(assigned.len(), names.len());

            // All assigned names are pairwise distinct (Property 14).
            let unique: HashSet<&String> = assigned.iter().collect();
            prop_assert_eq!(unique.len(), assigned.len(), "names must be pairwise distinct");

            // Every assigned name is itself path-traversal-safe (Req 50.1).
            for n in &assigned {
                prop_assert!(!n.contains('/') && !n.contains('\\'));
                prop_assert!(n != "." && n != "..");
                prop_assert!(!n.is_empty());
            }
        }
    }

    // ---------------------------------------------------------------------
    // Property 15: Filenames cannot cause path traversal (Req 50.1).
    // ---------------------------------------------------------------------

    // Feature: pdf-tools-suite, Property 15: Filenames cannot cause path traversal
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop15_filenames_cannot_cause_path_traversal(
            raw in traversal_name_strategy(),
        ) {
            let safe = sanitize_filename(&raw);

            // No path separators survive (Req 50.1).
            prop_assert!(!safe.contains('/'), "sanitized name has a '/': {safe:?}");
            prop_assert!(!safe.contains('\\'), "sanitized name has a '\\': {safe:?}");

            // Not a relative path segment.
            prop_assert_ne!(safe.as_str(), ".");
            prop_assert_ne!(safe.as_str(), "..");

            // Never empty (always a usable single segment).
            prop_assert!(!safe.is_empty());

            // Joining onto a File_Store root resolves strictly inside it: the
            // sanitized name is exactly one path component.
            let base = std::path::Path::new("/srv/filestore");
            let joined = base.join(&safe);
            let components: Vec<_> = joined
                .strip_prefix(base)
                .expect("joined path must stay under the base")
                .components()
                .collect();
            prop_assert_eq!(components.len(), 1, "must add exactly one path segment");
        }
    }

    // ---------------------------------------------------------------------
    // Property 16: Active content is removed or the file is rejected
    // (Req 39.5, 39.6).
    // ---------------------------------------------------------------------

    /// Which active-content construct to inject into a base PDF.
    #[derive(Debug, Clone, Copy)]
    enum Inject {
        JavaScriptOpenAction,
        JavaScriptNameTree,
        LaunchAction,
        EmbeddedExecutable,
    }

    fn inject_strategy() -> impl Strategy<Value = Inject> {
        prop_oneof![
            Just(Inject::JavaScriptOpenAction),
            Just(Inject::JavaScriptNameTree),
            Just(Inject::LaunchAction),
            Just(Inject::EmbeddedExecutable),
        ]
    }

    /// Build a valid multi-page PDF and inject the requested active content,
    /// returning the serialized bytes.
    fn make_pdf_with_active_content(page_count: u32, inject: Inject) -> Vec<u8> {
        let tags: Vec<u32> = (1..=page_count).collect();
        let base = make_pdf_with_tags(&tags);
        // Re-parse so we can attach constructs onto the real object graph.
        let mut doc = Document::load_mem(&base).unwrap();
        let catalog_id = doc
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .unwrap();

        match inject {
            Inject::JavaScriptOpenAction => {
                let action = doc.add_object(dictionary! {
                    "Type" => "Action",
                    "S" => "JavaScript",
                    "JS" => Object::string_literal("app.alert('x');"),
                });
                if let Ok(cat) = doc.get_object_mut(catalog_id).and_then(|o| o.as_dict_mut()) {
                    cat.set("OpenAction", Object::Reference(action));
                }
            }
            Inject::JavaScriptNameTree => {
                let js = doc.add_object(dictionary! {
                    "Type" => "Action",
                    "S" => "JavaScript",
                    "JS" => Object::string_literal("this.print();"),
                });
                if let Ok(cat) = doc.get_object_mut(catalog_id).and_then(|o| o.as_dict_mut()) {
                    cat.set(
                        "Names",
                        dictionary! {
                            "JavaScript" => dictionary! {
                                "Names" => vec![
                                    Object::string_literal("doc_js"),
                                    Object::Reference(js),
                                ],
                            },
                        },
                    );
                }
            }
            Inject::LaunchAction => {
                let action = doc.add_object(dictionary! {
                    "Type" => "Action",
                    "S" => "Launch",
                    "F" => Object::string_literal("calc.exe"),
                });
                if let Ok(cat) = doc.get_object_mut(catalog_id).and_then(|o| o.as_dict_mut()) {
                    cat.set("OpenAction", Object::Reference(action));
                }
            }
            Inject::EmbeddedExecutable => {
                let ef = doc.add_object(Stream::new(
                    dictionary! { "Type" => "EmbeddedFile", "Subtype" => "x-msdownload" },
                    b"MZ\x90\x00payload".to_vec(),
                ));
                let filespec = doc.add_object(dictionary! {
                    "Type" => "Filespec",
                    "F" => Object::string_literal("dropper.exe"),
                    "EF" => dictionary! { "F" => ef },
                });
                if let Ok(cat) = doc.get_object_mut(catalog_id).and_then(|o| o.as_dict_mut()) {
                    cat.set(
                        "Names",
                        dictionary! {
                            "EmbeddedFiles" => dictionary! {
                                "Names" => vec![
                                    Object::string_literal("dropper.exe"),
                                    Object::Reference(filespec),
                                ],
                            },
                        },
                    );
                }
            }
        }

        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        buf
    }

    // Feature: pdf-tools-suite, Property 16: Active content is removed or the file is rejected
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop16_active_content_removed_or_rejected(
            page_count in 1u32..=4,
            inject in inject_strategy(),
        ) {
            let bytes = make_pdf_with_active_content(page_count, inject);

            // The scanner detects the injected active content on the source.
            let report = scan_pdf(&bytes).expect("augmented PDF must still parse");
            prop_assert!(
                report.has_active_content(),
                "injected active content must be detected: {inject:?}"
            );

            // Removal branch (Req 39.6): sanitizing yields bytes with no active
            // content, OR the scanner rejects (which our sanitizer does not, so
            // the cleaned file must be clean).
            let cleaned = sanitize_pdf(&bytes).expect("sanitize should produce a valid PDF");
            let after = scan_pdf(&cleaned).expect("cleaned PDF must parse");
            prop_assert!(
                !after.has_active_content(),
                "cleaned PDF still contains active content: {:?}",
                after.findings()
            );

            // The cleaned document is still a valid, processable PDF with the
            // same page count (cleaning does not destroy the document).
            let doc = crate::pdf::parse(&cleaned).expect("cleaned PDF parses");
            prop_assert_eq!(crate::pdf::page_count(&doc), page_count);

            // Sanity: the specific injected category is gone.
            let expected_gone = match inject {
                Inject::JavaScriptOpenAction | Inject::JavaScriptNameTree => ActiveContent::JavaScript,
                Inject::LaunchAction => ActiveContent::LaunchAction,
                Inject::EmbeddedExecutable => ActiveContent::EmbeddedExecutable,
            };
            prop_assert!(!after.contains(expected_gone));
        }
    }

    // ---------------------------------------------------------------------
    // Property 19: Job_Tokens are high-entropy and unique (Req 43.1, 43.2).
    // ---------------------------------------------------------------------

    // Feature: pdf-tools-suite, Property 19: Job_Tokens are high-entropy and unique
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop19_job_tokens_high_entropy_and_unique(
            // A large sample of freshly generated tokens per case.
            sample_size in 32usize..=128,
        ) {
            let mut seen: HashSet<String> = HashSet::with_capacity(sample_size);
            for _ in 0..sample_size {
                let token = generate_job_token().expect("token generation must succeed");

                // URL-safe: only unreserved base64url characters, no padding
                // (Req 43.1).
                prop_assert!(
                    token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                    "token is not URL-safe: {token:?}"
                );
                prop_assert!(!token.contains('='), "token must be unpadded");

                // At least 128 bits of entropy (Req 43.2): a base64url string of
                // length L encodes 6*L bits; length must clear the floor.
                let encoded_bits = token.len() * 6;
                prop_assert!(
                    encoded_bits >= MIN_TOKEN_ENTROPY_BITS,
                    "token encodes only {encoded_bits} bits"
                );

                // No collisions across the sample (Property 19).
                prop_assert!(seen.insert(token), "generated tokens must not collide");
            }
        }
    }

    // ---------------------------------------------------------------------
    // Property 20: File access requires the correct, unexpired Job_Token
    // (Req 43.3, 43.4, 32.4, 36.6).
    // ---------------------------------------------------------------------

    // Feature: pdf-tools-suite, Property 20: File access requires the correct, unexpired Job_Token
    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn prop20_access_requires_correct_unexpired_token(
            expiry in 1u64..=u64::MAX,
            now in any::<u64>(),
            present_correct in any::<bool>(),
            wrong_seed in any::<u64>(),
        ) {
            let expected = generate_job_token().expect("token generation must succeed");
            // A presented token that is either the correct one, or a different
            // valid-looking token guaranteed not to equal `expected`.
            let presented = if present_correct {
                expected.clone()
            } else {
                let mut other = generate_job_token().expect("token generation must succeed");
                // Astronomically unlikely to collide, but make it certain.
                while other == expected {
                    other = format!("{other}{wrong_seed}");
                }
                other
            };

            let granted = check_access(&expected, &presented, expiry, now);

            // The ground-truth predicate: correct token AND strictly before expiry.
            let should_grant = present_correct && now < expiry;
            prop_assert_eq!(
                granted, should_grant,
                "access must be granted iff correct token AND now < expiry (correct={}, now={}, expiry={})",
                present_correct, now, expiry
            );
        }
    }

    /// Raw output-name generator: a mix of plain names, colliding names,
    /// extensioned/extensionless names, and hostile traversal strings, so the
    /// unique-name assignment is exercised across realistic collisions.
    fn raw_name_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            // A small pool of frequently-colliding names.
            Just("report.pdf".to_string()),
            Just("report.pdf".to_string()),
            Just("page".to_string()),
            Just("output.jpg".to_string()),
            // Generated stems with an extension.
            ("[a-z]{1,6}", "[a-z]{1,3}").prop_map(|(s, e)| format!("{s}.{e}")),
            // Hostile inputs that must be sanitized before uniquing.
            traversal_name_strategy(),
        ]
    }

    /// Generator of hostile / traversal-oriented file-name strings, including
    /// separators, relative segments, control characters, and empties.
    fn traversal_name_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("../../etc/passwd".to_string()),
            Just("..\\..\\Windows\\system32\\cmd.exe".to_string()),
            Just(".".to_string()),
            Just("..".to_string()),
            Just("/".to_string()),
            Just(String::new()),
            Just("   ".to_string()),
            Just("a/b/c.txt".to_string()),
            // Arbitrary strings that may embed separators and dot segments.
            r"[a-zA-Z0-9_./\\ .-]{0,20}",
            // Any short unicode string, to stress control/odd characters.
            r".{0,12}",
        ]
    }
}

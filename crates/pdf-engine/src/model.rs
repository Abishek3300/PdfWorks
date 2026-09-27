//! Core data models for the shared engine.
//!
//! These mirror the design's "Data Models" section. The outer surface
//! ([`ToolId`], [`EngineInput`], [`EngineOutput`], [`EngineError`]) and the full
//! per-tool [`ToolOptions`] tagged union with its supporting enums are defined
//! here. Everything is pure data with no I/O and no native-only types, keeping
//! the crate WASM-compatible.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Identifies which Tool the engine should run.
///
/// The first block of variants are the `Client_Capable` tools implementable by
/// the shared engine (WASM and native). The `Server_Only` variants are declared
/// here for a complete, single source of truth over tool identity, but their
/// execution lives in the server layer (LibreOffice / OCR / pdfium subprocesses),
/// not in [`crate::run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolId {
    // --- Client_Capable tools (executed by the shared engine) ---
    Merge,
    Split,
    RemovePages,
    ExtractPages,
    Organize,
    OptimizePdf,
    CompressPdf,
    JpgToPdf,
    PdfToJpg,
    Rotate,
    AddPageNumbers,
    AddWatermark,
    Crop,
    MarkdownToPdf,
    PdfToMarkdown,
    EditPdf,
    PdfForms,

    // --- Server_Only tools (resolved by the server layer, not by `run`) ---
    /// Word (DOC/DOCX) to PDF.
    WordToPdf,
    /// PowerPoint (PPT/PPTX) to PDF.
    PptToPdf,
    /// Excel (XLS/XLSX) to PDF.
    ExcelToPdf,
    /// HTML (URL or inline) to PDF.
    HtmlToPdf,
    /// PDF to Word (DOCX).
    PdfToWord,
    /// PDF to PowerPoint (PPTX).
    PdfToPptx,
    /// PDF to Excel (XLSX).
    PdfToExcel,
    /// PDF to PDF/A archival format.
    PdfToPdfA,
    /// Images to PDF, optionally with an OCR text layer.
    ScanToPdf,
}

/// A borrowed slice of input file bytes (one Source_File).
#[derive(Debug, Clone)]
pub struct FileBytes<'a> {
    /// Original (untrusted) file name; sanitized before use as a display name.
    pub name: String,
    /// Raw file bytes. The engine never mutates the caller's buffer.
    pub bytes: &'a [u8],
}

/// Input to a single engine invocation.
#[derive(Debug)]
pub struct EngineInput<'a> {
    /// Which Tool to run.
    pub tool: ToolId,
    /// One or more Source_Files.
    pub sources: Vec<FileBytes<'a>>,
    /// Tool-specific options (tagged union).
    pub options: ToolOptions,
}

/// A single produced Output_File.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// Display name for the produced file (unique across a multi-output Job).
    pub name: String,
    /// Produced file bytes.
    pub bytes: Vec<u8>,
}

/// Result of a successful engine invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineOutput {
    /// One or more Output_Files.
    pub files: Vec<OutputFile>,
    /// Page counts of each Source_File, used to assert invariants
    /// (Req 30.3, 10.3, 11.3).
    pub source_page_counts: Vec<u32>,
}

/// Metadata about a stored file referenced by an option (e.g. a watermark image
/// or the images assembled by Scan to PDF).
///
/// Mirrors the design's `FileRef` data model. In the I/O-free engine this is a
/// pure descriptor; the server layer resolves it to actual bytes via the
/// File_Store, addressed only by an opaque `store_key` (Req 50.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    /// Opaque File_Store key; never a client-supplied path (Req 50.1).
    pub store_key: String,
    /// Sanitized, path-separator-free display name (Req 50.1).
    pub display_name: String,
    /// Size in bytes, shown before download (Req 3.4).
    pub size_bytes: u64,
    /// Non-executable content type used on download (Req 50.2).
    pub content_type: String,
    /// Page count when known, used for invariants (Req 30.3, 10.3, 11.3).
    pub page_count: Option<u32>,
}

/// Which bounded resource a Job exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    /// CPU time limit.
    Cpu,
    /// Memory limit.
    Memory,
    /// Wall-clock time limit.
    WallClock,
    /// Produced-output size limit.
    OutputSize,
}

/// Structured, non-panicking engine failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EngineError {
    /// The source file is password-protected or encrypted (Req 49.1).
    #[error("the source file is password-protected or encrypted")]
    Protected,
    /// The source file has zero bytes (Req 49.2).
    #[error("the source file is empty")]
    Empty,
    /// The source file is corrupt or unparseable (Req 33.2, 39.7).
    #[error("the source file is corrupt or malformed")]
    Corrupt,
    /// The requested operation is not supported for this input.
    #[error("the requested operation is not supported for this input")]
    Unsupported,
    /// A split point exceeds the document's page count (Req 5.3).
    #[error("split point is out of range for a document with {page_count} pages")]
    SplitPointOutOfRange {
        /// The actual page count of the source document.
        page_count: u32,
    },
    /// No extractable text was found (Req 19.3).
    #[error("no extractable text was found")]
    NoTextFound,
    /// No detectable table was found (Req 21.3).
    #[error("no table was found")]
    NoTableFound,
    /// The operation produced no Output_File (Req 49.6).
    #[error("the operation produced no output")]
    NoOutputProduced,
    /// A bounded resource was exceeded (Req 40.4).
    #[error("a resource limit was exceeded: {0:?}")]
    ResourceLimit(ResourceKind),
}

/// Compression / optimization aggressiveness (Req 10.2, 11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    /// Least aggressive; highest fidelity.
    Low,
    /// Balanced.
    Medium,
    /// Most aggressive; smallest output.
    High,
}

/// A clockwise rotation angle (Req 24).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Angle {
    /// 90 degrees clockwise.
    D90,
    /// 180 degrees.
    D180,
    /// 270 degrees clockwise (90 counter-clockwise).
    D270,
}

/// Page orientation (Req 12, 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    /// Taller than wide.
    Portrait,
    /// Wider than tall.
    Landscape,
}

/// Margin size applied around placed images (Req 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Margin {
    /// No margin.
    None,
    /// Small margin.
    Small,
    /// Large margin.
    Large,
}

/// PDF/A archival conformance level (Req 22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PdfALevel {
    /// PDF/A-1b.
    A1b,
    /// PDF/A-2b.
    A2b,
    /// PDF/A-3b.
    A3b,
}

/// Source of HTML content for HTML to PDF (Req 16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HtmlSource {
    /// A remote URL to fetch (SSRF-guarded by the server layer, Req 41).
    Url(String),
    /// Inline HTML markup.
    Inline(String),
}

/// Placement of a page-number element (Req 25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Position {
    /// Top-left corner.
    TopLeft,
    /// Top-center.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-center.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

/// Which pages an operation targets (Req 24).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageScope {
    /// Every page in the document.
    All,
    /// A specific set of 1-based page numbers.
    Pages(Vec<u32>),
}

/// A rectangular region in PDF user-space points, measured from the bottom-left
/// origin (Req 27).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Distance from the left edge, in points.
    pub x: f32,
    /// Distance from the bottom edge, in points.
    pub y: f32,
    /// Width, in points.
    pub width: f32,
    /// Height, in points.
    pub height: f32,
}

/// An editable content element placed on a page by Edit PDF (Req 28).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Element {
    /// A text run positioned at `at` with the given font size in points.
    Text {
        /// 1-based page the element belongs to.
        page: u32,
        /// Position of the element in page points.
        at: Rect,
        /// Text content.
        content: String,
        /// Font size in points.
        font_size: f32,
    },
    /// An image placed at `at`, referenced from the File_Store.
    Image {
        /// 1-based page the element belongs to.
        page: u32,
        /// Position and size of the element in page points.
        at: Rect,
        /// The image to place.
        image: FileRef,
    },
    /// A vector shape occupying `at`.
    Shape {
        /// 1-based page the element belongs to.
        page: u32,
        /// Bounding box of the shape in page points.
        at: Rect,
        /// Shape kind.
        kind: ShapeKind,
    },
}

/// Kinds of vector shape an [`Element::Shape`] can draw (Req 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    /// A rectangle filling the element's bounding box.
    Rectangle,
    /// An ellipse inscribed in the element's bounding box.
    Ellipse,
    /// A straight line across the element's bounding box.
    Line,
}

/// An interactive form field added to a PDF by PDF Forms (Req 29).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    /// Field name (key used when reading/writing values).
    pub name: String,
    /// The kind of interactive control.
    pub kind: FormFieldKind,
    /// 1-based page the field is placed on.
    pub page: u32,
    /// Position and size of the field in page points.
    pub at: Rect,
}

/// The interactive control type of a [`FormField`] (Req 29).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormFieldKind {
    /// Single-line or multi-line text entry.
    Text,
    /// A boolean checkbox.
    Checkbox,
    /// A radio-button group member.
    Radio,
    /// A dropdown selection.
    Dropdown,
}

/// Tool-specific options.
///
/// A tagged union so each Tool carries only its own settings, matching the
/// design's Data Models section. The first block covers the `Client_Capable`
/// tools executed by [`crate::run`]; the trailing block carries the
/// `Server_Only` tools' options, which are resolved by the server layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolOptions {
    /// Concatenate sources in the given source order (Req 4.2).
    Merge {
        /// Permutation of source indices defining output order.
        order: Vec<usize>,
    },
    /// Split a document by explicit split points and/or fixed-size ranges
    /// (Req 5.1, 5.2).
    Split {
        /// 1-based page numbers after which to split.
        split_points: Vec<u32>,
        /// Optional fixed range size (pages per output).
        fixed_size: Option<u32>,
    },
    /// Remove the listed 1-based pages, keeping the complement (Req 6.1).
    RemovePages {
        /// 1-based pages to remove.
        pages: Vec<u32>,
    },
    /// Extract the listed 1-based pages in order (Req 7.1).
    ExtractPages {
        /// 1-based pages to keep.
        pages: Vec<u32>,
    },
    /// Reorder, rotate, and delete pages in a single pass (Req 8).
    Organize {
        /// Permutation of page indices defining output order.
        order: Vec<usize>,
        /// Per-page rotations, keyed by 1-based page number.
        rotations: Vec<(u32, Angle)>,
        /// 1-based pages to delete.
        deletes: Vec<u32>,
    },
    /// Reduce structural overhead at the chosen level (Req 10.2).
    Optimize {
        /// Aggressiveness level.
        level: Level,
    },
    /// Reduce file size at the chosen level (Req 11.2).
    Compress {
        /// Aggressiveness level.
        level: Level,
    },
    /// Build a PDF from JPG images, one page per image (Req 12).
    JpgToPdf {
        /// Page orientation.
        orientation: Orientation,
        /// Margin around each placed image.
        margin: Margin,
        /// Permutation of source indices defining page order.
        order: Vec<usize>,
    },
    /// Rasterize each page to a JPG at the chosen DPI (Req 18.2).
    PdfToJpg {
        /// Output resolution in dots per inch.
        dpi: u32,
    },
    /// Rotate the scoped pages by the chosen angle (Req 24).
    Rotate {
        /// Rotation angle.
        angle: Angle,
        /// Which pages to rotate.
        pages: PageScope,
    },
    /// Stamp page numbers starting at `start` (Req 25).
    AddPageNumbers {
        /// Where the number is placed.
        position: Position,
        /// The number assigned to the first page.
        start: u32,
    },
    /// Overlay a text and/or image watermark (Req 26).
    AddWatermark {
        /// Optional watermark text.
        text: Option<String>,
        /// Optional watermark image.
        image: Option<FileRef>,
        /// Opacity from 0 (transparent) to 100 (opaque).
        opacity: u8,
        /// Rotation of the watermark in degrees.
        rotation_deg: i16,
    },
    /// Crop pages to a region (Req 27).
    Crop {
        /// Crop region in page points.
        region: Rect,
        /// Apply to every page when true, else the first page only.
        all_pages: bool,
    },
    /// Render Markdown text to a PDF (Req 17).
    MarkdownToPdf {
        /// Source Markdown.
        text: String,
    },
    /// Extract a PDF's text as Markdown (Req 23).
    PdfToMarkdown {},
    /// Apply added text/image/shape elements (Req 28).
    EditPdf {
        /// Elements to place.
        elements: Vec<Element>,
    },
    /// Write field values and add interactive fields (Req 29).
    PdfForms {
        /// Existing field values to set, as `(name, value)` pairs.
        field_values: Vec<(String, String)>,
        /// New interactive fields to add.
        added_fields: Vec<FormField>,
    },

    // --- Server_Only options (resolved in the server layer) ---
    /// Word (DOC/DOCX) to PDF.
    WordToPdf {},
    /// PowerPoint (PPT/PPTX) to PDF.
    PptToPdf {},
    /// Excel (XLS/XLSX) to PDF (Req 15).
    ExcelToPdf {
        /// Page orientation.
        orientation: Orientation,
    },
    /// HTML to PDF from a URL or inline markup (Req 16).
    HtmlToPdf {
        /// The HTML source.
        source: HtmlSource,
        /// Page orientation.
        orientation: Orientation,
    },
    /// PDF to Word (DOCX).
    PdfToWord {},
    /// PDF to PowerPoint (PPTX).
    PdfToPptx {},
    /// PDF to Excel (XLSX).
    PdfToExcel {},
    /// PDF to PDF/A archival format (Req 22).
    PdfToPdfA {
        /// Target conformance level.
        level: PdfALevel,
    },
    /// Assemble images into a PDF, optionally with an OCR text layer (Req 9).
    ScanToPdf {
        /// Source images, one page per image.
        images: Vec<FileRef>,
        /// When true the Job becomes `Server_Only` (OCR layer requested).
        ocr: bool,
    },
}

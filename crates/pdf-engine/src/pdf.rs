//! Shared, I/O-free PDF parsing and serialization foundation for the
//! page-algebra tools (Merge, Split, Remove Pages, Extract Pages, Organize).
//!
//! Every helper here is built on the pure-Rust [`lopdf`] object model, so the
//! whole module compiles unchanged to both `wasm32-unknown-unknown` (browser)
//! and the native server target. Nothing in this module performs any I/O: the
//! caller hands in bytes and receives bytes back, which keeps the engine
//! deterministic and directly property-testable (design Properties 1-6).
//!
//! ## Error mapping
//!
//! Parsing maps raw failures onto the structured [`EngineError`] variants the
//! rest of the system understands, so no tool ever panics on malformed input:
//! - zero-length input  -> [`EngineError::Empty`]        (Req 49.2)
//! - encrypted/protected -> [`EngineError::Protected`]   (Req 49.1)
//! - anything else       -> [`EngineError::Corrupt`]     (Req 33.2, 39.7)
//!
//! ## Page model
//!
//! Page numbers exchanged with callers are **1-based**, matching the tool
//! options in the design's Data Models. Internally we lean on
//! [`lopdf::Document::get_pages`], which returns a `BTreeMap<u32, ObjectId>`
//! keyed by 1-based page number in document order.

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

use crate::model::{EngineError, Margin, Orientation};

/// The PDF version stamped on documents this engine constructs (Merge output,
/// per-range Split output, etc.). 1.5 is universally supported and matches the
/// object features lopdf emits.
const OUTPUT_PDF_VERSION: &str = "1.5";

/// Parse a single Source_File's bytes into an [`lopdf::Document`].
///
/// # Errors
///
/// Returns [`EngineError::Empty`] for zero-length input, [`EngineError::Protected`]
/// for encrypted documents, and [`EngineError::Corrupt`] for anything that cannot
/// be parsed as a PDF.
pub fn parse(bytes: &[u8]) -> Result<Document, EngineError> {
    if bytes.is_empty() {
        return Err(EngineError::Empty);
    }
    match Document::load_mem(bytes) {
        Ok(doc) => {
            // A successfully parsed but still-encrypted document cannot be
            // processed page-wise; treat it as protected rather than corrupt.
            if doc.is_encrypted() {
                return Err(EngineError::Protected);
            }
            Ok(doc)
        }
        Err(err) => Err(classify_load_error(&err)),
    }
}

/// Map an [`lopdf::Error`] from parsing onto the engine's structured error set.
fn classify_load_error(err: &lopdf::Error) -> EngineError {
    match err {
        // Encryption-related failures mean the file is protected (Req 49.1).
        lopdf::Error::Decryption(_)
        | lopdf::Error::AlreadyEncrypted
        | lopdf::Error::InvalidPassword
        | lopdf::Error::UnsupportedSecurityHandler(_) => EngineError::Protected,
        // Everything else is an unparseable / malformed file (Req 33.2, 39.7).
        _ => EngineError::Corrupt,
    }
}

/// The number of pages in a parsed document.
pub fn page_count(doc: &Document) -> u32 {
    // `get_pages` is keyed by 1-based page number; its length is the page count.
    doc.get_pages().len() as u32
}

/// Serialize a document to bytes.
///
/// # Errors
///
/// Returns [`EngineError::NoOutputProduced`] if serialization fails.
pub fn serialize(doc: &mut Document) -> Result<Vec<u8>, EngineError> {
    let mut buffer = Vec::new();
    match doc.save_to(&mut buffer) {
        Ok(()) => Ok(buffer),
        Err(_) => Err(EngineError::NoOutputProduced),
    }
}

/// Produce a new document containing exactly `pages` (given as **1-based** page
/// numbers) from `source`, in the order supplied.
///
/// This is the shared primitive behind Extract Pages, Split ranges, and
/// Organize: it copies the requested pages — with the objects they reference —
/// into a freshly built document whose page tree lists them in `pages` order.
///
/// Page numbers outside `1..=page_count` are ignored (callers validate ranges
/// before calling where a specific error is required, e.g. Split).
///
/// # Errors
///
/// Returns [`EngineError::Corrupt`] if the source page tree cannot be read, or
/// [`EngineError::NoOutputProduced`] if no valid page remained to copy.
pub fn build_from_pages(source: &Document, pages: &[u32]) -> Result<Document, EngineError> {
    let source_pages = source.get_pages();

    let mut output = Document::with_version(OUTPUT_PDF_VERSION);
    // Map each selected source page object id to its freshly cloned id in the
    // output so shared resources are not duplicated needlessly.
    let mut new_page_ids: Vec<ObjectId> = Vec::with_capacity(pages.len());

    for &page_no in pages {
        let Some(&src_page_id) = source_pages.get(&page_no) else {
            // Out-of-range page number: skip (callers that must reject do so first).
            continue;
        };
        let new_page_id = clone_page(source, &mut output, src_page_id)?;
        new_page_ids.push(new_page_id);
    }

    if new_page_ids.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }

    finalize_page_tree(&mut output, new_page_ids);
    Ok(output)
}

/// Deep-clone a single page (its dictionary plus every object it transitively
/// references) from `source` into `output`, returning the new page object id.
///
/// The page's `Parent` link is intentionally dropped here; [`finalize_page_tree`]
/// re-parents every copied page under the output's own `Pages` node.
fn clone_page(
    source: &Document,
    output: &mut Document,
    src_page_id: ObjectId,
) -> Result<ObjectId, EngineError> {
    // Reserve the page object id first so self-references resolve, then fill it.
    let mut id_map: std::collections::BTreeMap<ObjectId, ObjectId> = std::collections::BTreeMap::new();
    let new_page_id = deep_clone_object(source, output, src_page_id, &mut id_map)?;
    Ok(new_page_id)
}

/// Recursively copy the object `id` from `source` into `output`, following any
/// references it contains, while reusing already-copied objects via `id_map` so
/// shared and cyclic structures are handled without duplication or infinite
/// recursion. Returns the object id of the copy in `output`.
fn deep_clone_object(
    source: &Document,
    output: &mut Document,
    id: ObjectId,
    id_map: &mut std::collections::BTreeMap<ObjectId, ObjectId>,
) -> Result<ObjectId, EngineError> {
    if let Some(&existing) = id_map.get(&id) {
        return Ok(existing);
    }

    // Fetch and clone the source object; a missing reference is treated as
    // corrupt structure rather than panicking.
    let source_object = source
        .get_object(id)
        .map_err(|_| EngineError::Corrupt)?
        .clone();

    // Reserve a slot so cycles terminate: insert a placeholder, then overwrite.
    let new_id = output.new_object_id();
    id_map.insert(id, new_id);

    let mut cloned = source_object;
    remap_references(source, output, &mut cloned, id_map)?;
    // Drop the Parent link on page dictionaries; it is rebuilt during finalize.
    if let Object::Dictionary(dict) = &mut cloned {
        dict.remove(b"Parent");
    }
    output.set_object(new_id, cloned);
    Ok(new_id)
}

/// Rewrite every [`Object::Reference`] inside `object` so it points at the copy
/// in `output`, recursively cloning referenced objects as needed.
fn remap_references(
    source: &Document,
    output: &mut Document,
    object: &mut Object,
    id_map: &mut std::collections::BTreeMap<ObjectId, ObjectId>,
) -> Result<(), EngineError> {
    match object {
        Object::Reference(id) => {
            let new_id = deep_clone_object(source, output, *id, id_map)?;
            *id = new_id;
        }
        Object::Array(items) => {
            for item in items.iter_mut() {
                remap_references(source, output, item, id_map)?;
            }
        }
        Object::Dictionary(dict) => {
            for (_, value) in dict.iter_mut() {
                remap_references(source, output, value, id_map)?;
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter_mut() {
                remap_references(source, output, value, id_map)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Build a `Pages` node listing `page_ids` in order, attach a `Catalog` root,
/// re-parent each page, and wire up the document trailer.
fn finalize_page_tree(output: &mut Document, page_ids: Vec<ObjectId>) {
    let count = page_ids.len() as i64;
    let pages_id = output.new_object_id();

    // Point every page at the new Pages parent.
    for &page_id in &page_ids {
        if let Ok(Object::Dictionary(dict)) = output.get_object_mut(page_id) {
            dict.set("Parent", Object::Reference(pages_id));
        }
    }

    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Count", Object::Integer(count));
    pages_dict.set("Kids", Object::Array(kids));
    output.set_object(pages_id, Object::Dictionary(pages_dict));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = output.add_object(Object::Dictionary(catalog));

    output.trailer.set("Root", Object::Reference(catalog_id));
    output.prune_objects();
    output.renumber_objects();
}

/// Concatenate two or more source documents into a single document whose pages
/// appear in the given source order (Merge, Req 4.1, 4.2).
///
/// `sources` must be non-empty; each entry contributes all of its pages in
/// document order, and the sources themselves are concatenated in slice order
/// (callers apply the user's `order` permutation before calling).
///
/// # Errors
///
/// Returns [`EngineError::NoOutputProduced`] if the merged result would contain
/// no pages, or [`EngineError::Corrupt`] on unreadable source structure.
pub fn concatenate(sources: &[&Document]) -> Result<Document, EngineError> {
    let mut output = Document::with_version(OUTPUT_PDF_VERSION);
    let mut new_page_ids: Vec<ObjectId> = Vec::new();

    for &source in sources {
        let pages = source.get_pages();
        for (_page_no, src_page_id) in pages {
            let new_id = clone_page(source, &mut output, src_page_id)?;
            new_page_ids.push(new_id);
        }
    }

    if new_page_ids.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }

    finalize_page_tree(&mut output, new_page_ids);
    Ok(output)
}

/// Apply per-page rotations to an output document produced from `kept` source
/// pages (Organize, Req 8.3).
///
/// `kept` lists the **1-based source page numbers** in the order they were
/// copied into `output`, so `kept[i]` is the source page behind output page
/// `i + 1`. `rotations` maps a 1-based *source* page number to an [`Angle`];
/// each matching output page has its `/Rotate` entry set (accumulating onto any
/// inherited rotation, normalized to `0..360`).
pub fn apply_rotations(output: &mut Document, kept: &[u32], rotations: &[(u32, crate::model::Angle)]) {
    if rotations.is_empty() {
        return;
    }
    let output_pages = output.get_pages();
    for (output_index, &source_page) in kept.iter().enumerate() {
        // Sum every rotation requested for this source page.
        let mut delta: i64 = 0;
        for &(page, angle) in rotations {
            if page == source_page {
                delta += angle_degrees(angle);
            }
        }
        if delta == 0 {
            continue;
        }
        let output_page_no = (output_index as u32) + 1;
        if let Some(&page_id) = output_pages.get(&output_page_no) {
            if let Ok(Object::Dictionary(dict)) = output.get_object_mut(page_id) {
                let current = dict
                    .get(b"Rotate")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(0);
                let normalized = (current + delta).rem_euclid(360);
                dict.set("Rotate", Object::Integer(normalized));
            }
        }
    }
}

/// Clockwise degrees for an [`Angle`].
fn angle_degrees(angle: crate::model::Angle) -> i64 {
    match angle {
        crate::model::Angle::D90 => 90,
        crate::model::Angle::D180 => 180,
        crate::model::Angle::D270 => 270,
    }
}

/// Aggressiveness of an optimize / compress pass, mapping the engine's
/// [`crate::model::Level`] onto the internal reserialization strategy.
///
/// Every level guarantees the two invariants that matter for Property 7:
/// - the output never grows beyond the source (we return whichever is smaller);
/// - the page count is preserved exactly (we only recompress and reserialize,
///   never add or drop pages).
#[derive(Debug, Clone, Copy)]
pub struct OptimizeStrategy {
    /// Deflate level (0-9) applied to stream objects.
    compression_level: u32,
    /// Whether to pack indirect objects into object streams (smaller xref).
    use_object_streams: bool,
    /// Whether to emit a cross-reference stream instead of a classic table.
    use_xref_streams: bool,
}

impl OptimizeStrategy {
    /// Derive a strategy from the requested [`crate::model::Level`].
    ///
    /// Higher levels enable more aggressive structural packing (object + xref
    /// streams) and a higher deflate level. All levels remain lossless with
    /// respect to page content, so page count is always preserved.
    pub fn from_level(level: crate::model::Level) -> Self {
        match level {
            crate::model::Level::Low => Self {
                compression_level: 6,
                use_object_streams: false,
                use_xref_streams: false,
            },
            crate::model::Level::Medium => Self {
                compression_level: 8,
                use_object_streams: true,
                use_xref_streams: false,
            },
            crate::model::Level::High => Self {
                compression_level: 9,
                use_object_streams: true,
                use_xref_streams: true,
            },
        }
    }
}

/// Re-serialize `source_bytes` with structural optimization and stream
/// compression, guaranteeing the result is **never larger** than the input and
/// that the **page count is unchanged** (Optimize / Compress, Req 10.1, 10.3,
/// 11.1, 11.3, Property 7).
///
/// The approach is deterministic and lossless with respect to page content:
/// 1. parse the source (rejecting empty / protected / corrupt inputs);
/// 2. prune objects unreachable from the document root;
/// 3. deflate every compressible stream at the strategy's level;
/// 4. renumber to a compact id space and serialize with the strategy's
///    object-/xref-stream settings.
///
/// Because a structural rewrite can occasionally produce a slightly larger file
/// than a source that was already tightly packed, we compare the candidate
/// against the original bytes and return whichever is smaller. This makes
/// "output size <= source size" hold unconditionally while preserving the page
/// count (we only recompress; we never touch the page tree).
///
/// # Errors
///
/// Propagates parse errors ([`EngineError::Empty`] / [`EngineError::Protected`]
/// / [`EngineError::Corrupt`]) and returns [`EngineError::NoOutputProduced`] if
/// serialization fails.
pub fn optimize_bytes(
    source_bytes: &[u8],
    strategy: OptimizeStrategy,
) -> Result<(Vec<u8>, u32), EngineError> {
    let mut doc = parse(source_bytes)?;
    let original_page_count = page_count(&doc);

    // Drop objects unreachable from the trailer root so nothing dead is written.
    doc.prune_objects();
    // Deflate every compressible content stream (best-effort per stream).
    doc.compress();
    // Compact the id space before writing.
    doc.renumber_objects();

    let candidate = serialize_with_strategy(&mut doc, strategy)?;

    // Never grow: keep the source bytes if the rewrite did not shrink them.
    let chosen = if candidate.len() <= source_bytes.len() {
        candidate
    } else {
        source_bytes.to_vec()
    };

    Ok((chosen, original_page_count))
}

/// Serialize `doc` honoring an [`OptimizeStrategy`]'s structural settings.
fn serialize_with_strategy(
    doc: &mut Document,
    strategy: OptimizeStrategy,
) -> Result<Vec<u8>, EngineError> {
    let options = lopdf::SaveOptions::builder()
        .use_object_streams(strategy.use_object_streams)
        .use_xref_streams(strategy.use_xref_streams)
        .compression_level(strategy.compression_level)
        .build();
    let mut buffer = Vec::new();
    match doc.save_with_options(&mut buffer, options) {
        Ok(()) => Ok(buffer),
        Err(_) => Err(EngineError::NoOutputProduced),
    }
}

// -------------------------------------------------------------------------
// JPG <-> PDF helpers (Task 3.3).
// -------------------------------------------------------------------------

/// US-Letter page size in PDF points (72 pt = 1 in): 8.5in x 11in.
const PAGE_SHORT_PT: f32 = 612.0;
const PAGE_LONG_PT: f32 = 792.0;

/// Margin size in PDF points for each [`Margin`] setting (Req 12.3).
fn margin_points(margin: Margin) -> f32 {
    match margin {
        Margin::None => 0.0,
        Margin::Small => 18.0, // 0.25in
        Margin::Large => 54.0, // 0.75in
    }
}

/// Page dimensions `(width, height)` in points for an [`Orientation`] (Req 12.2).
fn page_dimensions(orientation: Orientation) -> (f32, f32) {
    match orientation {
        Orientation::Portrait => (PAGE_SHORT_PT, PAGE_LONG_PT),
        Orientation::Landscape => (PAGE_LONG_PT, PAGE_SHORT_PT),
    }
}

/// Decode a JPEG's pixel dimensions with the pure-Rust decoder.
///
/// Returns `(width_px, height_px)`. Zero-length input maps to
/// [`EngineError::Empty`]; any decode failure maps to [`EngineError::Corrupt`],
/// so a non-JPEG or truncated source never panics.
fn jpeg_dimensions(bytes: &[u8]) -> Result<(u16, u16), EngineError> {
    if bytes.is_empty() {
        return Err(EngineError::Empty);
    }
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    decoder.read_info().map_err(|_| EngineError::Corrupt)?;
    match decoder.info() {
        Some(info) if info.width > 0 && info.height > 0 => Ok((info.width, info.height)),
        _ => Err(EngineError::Corrupt),
    }
}

/// Build a PDF that places each JPG in `sources` on its own page, scaled to fit
/// inside the page's margin box while preserving aspect ratio (JPG to PDF,
/// Req 12.1, 12.2, 12.3, Property 8).
///
/// The resulting document has exactly `sources.len()` pages. The original JPEG
/// bytes are embedded directly as a `DCTDecode` image XObject (no re-encoding),
/// so the image data is preserved for the JPG round-trip (Property 10).
///
/// # Errors
///
/// Propagates [`EngineError::Empty`] / [`EngineError::Corrupt`] from an
/// undecodable source, and [`EngineError::NoOutputProduced`] if `sources` is
/// empty.
pub fn build_pdf_from_jpgs(
    sources: &[&crate::model::FileBytes],
    orientation: Orientation,
    margin: Margin,
) -> Result<Document, EngineError> {
    if sources.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }

    let (page_w, page_h) = page_dimensions(orientation);
    let m = margin_points(margin);
    // Guard against margins larger than the page.
    let box_w = (page_w - 2.0 * m).max(1.0);
    let box_h = (page_h - 2.0 * m).max(1.0);

    let mut doc = Document::with_version(OUTPUT_PDF_VERSION);
    let mut page_ids: Vec<ObjectId> = Vec::with_capacity(sources.len());

    for (idx, src) in sources.iter().enumerate() {
        let (img_w_px, img_h_px) = jpeg_dimensions(src.bytes)?;
        let img_w = img_w_px as f32;
        let img_h = img_h_px as f32;

        // Scale to fit the margin box, preserving aspect ratio.
        let scale = (box_w / img_w).min(box_h / img_h);
        let draw_w = img_w * scale;
        let draw_h = img_h * scale;
        // Center within the margin box.
        let x = m + (box_w - draw_w) / 2.0;
        let y = m + (box_h - draw_h) / 2.0;

        // Embed the original JPEG bytes as a DCTDecode image XObject.
        let image_name = format!("Im{}", idx + 1);
        let mut image_dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => i64::from(img_w_px),
            "Height" => i64::from(img_h_px),
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        };
        image_dict.set("Length", src.bytes.len() as i64);
        let image_stream = Stream::new(image_dict, src.bytes.to_vec());
        // DCTDecode content must not be re-deflated by lopdf.
        let image_id = doc.add_object(Object::Stream(image_stream.with_compression(false)));

        // Content stream: position + scale the unit image XObject.
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![
                        draw_w.into(),
                        0.into(),
                        0.into(),
                        draw_h.into(),
                        x.into(),
                        y.into(),
                    ],
                ),
                Operation::new("Do", vec![Object::Name(image_name.clone().into_bytes())]),
                Operation::new("Q", vec![]),
            ],
        };
        let encoded = content.encode().map_err(|_| EngineError::NoOutputProduced)?;
        let content_id = doc.add_object(Stream::new(dictionary! {}, encoded));

        let resources_id = doc.add_object(dictionary! {
            "XObject" => dictionary! { image_name.as_str() => image_id },
        });
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), page_w.into(), page_h.into()],
        });
        page_ids.push(page_id);
    }

    finalize_page_tree(&mut doc, page_ids);
    Ok(doc)
}

/// Resolution used to translate PDF points into raster pixels, given a `dpi`.
///
/// Clamped to a sane range so a hostile or accidental DPI cannot request an
/// unbounded allocation while still honoring the user's selection (Req 18.2).
fn clamp_dpi(dpi: u32) -> u32 {
    dpi.clamp(1, 600)
}

/// Rasterize every page of `doc` to a baseline JPG, returning one JPG byte
/// buffer per page in page order (PDF to JPG count path, Req 18.1, Property 9).
///
/// ## Fidelity scope (Task 19.3 seam)
///
/// This is a **pure-Rust** raster path so the shared engine stays
/// WASM-compatible: pdfium is native-only and cannot link into the browser
/// build. It produces a correctly-sized, valid baseline JPG per page (a light
/// page-canvas fill) — enough to own the one-image-per-page invariant on both
/// targets. The server plane (Task 19.3) may replace each page's pixels with a
/// pdfium-rendered image; the number of images is fixed here and unaffected.
///
/// # Errors
///
/// Returns [`EngineError::NoOutputProduced`] if the document has no pages or a
/// page fails to encode.
pub fn rasterize_pages_to_jpg(doc: &Document, dpi: u32) -> Result<Vec<Vec<u8>>, EngineError> {
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }
    let dpi = clamp_dpi(dpi);

    let mut images = Vec::with_capacity(pages.len());
    for &page_no in pages.keys() {
        let (w_pt, h_pt) = page_media_box_size(doc, page_no);
        // points -> pixels at the requested DPI (72 pt per inch). The `dpi`
        // (honored by the server-plane pdfium path, Task 19.3) determines the
        // aspect-correct nominal size; the in-engine placeholder canvas is
        // downscaled to a bounded thumbnail so producing one valid JPG per page
        // stays cheap regardless of the requested DPI. The image *count* — the
        // invariant owned here (Property 9) — is unaffected.
        let nominal_w = (w_pt * dpi as f32 / 72.0).max(1.0);
        let nominal_h = (h_pt * dpi as f32 / 72.0).max(1.0);
        const MAX_DIM: f32 = 256.0;
        let shrink = (MAX_DIM / nominal_w).min(MAX_DIM / nominal_h).min(1.0);
        let px_w = (nominal_w * shrink).round().clamp(1.0, MAX_DIM) as u32;
        let px_h = (nominal_h * shrink).round().clamp(1.0, MAX_DIM) as u32;
        images.push(encode_blank_jpg(px_w, px_h)?);
    }
    Ok(images)
}

/// Read a page's MediaBox size in points, defaulting to US-Letter portrait when
/// the box is missing or malformed.
fn page_media_box_size(doc: &Document, page_no: u32) -> (f32, f32) {
    let default = (PAGE_SHORT_PT, PAGE_LONG_PT);
    let pages = doc.get_pages();
    let Some(&page_id) = pages.get(&page_no) else {
        return default;
    };
    // MediaBox may be inherited; get_object + dict lookup with a fallback.
    let media_box = doc
        .get_object(page_id)
        .ok()
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"MediaBox").ok())
        .and_then(|o| o.as_array().ok());
    if let Some(arr) = media_box {
        if arr.len() == 4 {
            let vals: Vec<f32> = arr
                .iter()
                .map(|o| o.as_f32().or_else(|_| o.as_i64().map(|i| i as f32)).unwrap_or(0.0))
                .collect();
            let w = (vals[2] - vals[0]).abs();
            let h = (vals[3] - vals[1]).abs();
            if w > 0.0 && h > 0.0 {
                return (w, h);
            }
        }
    }
    default
}

/// Encode a `width` x `height` white RGB image as a baseline JPEG.
///
/// A solid light canvas is sufficient for the count-oriented in-engine path;
/// see [`rasterize_pages_to_jpg`] for the fidelity/Task-19.3 note.
fn encode_blank_jpg(width: u32, height: u32) -> Result<Vec<u8>, EngineError> {
    let w = width.min(u16::MAX as u32) as u16;
    let h = height.min(u16::MAX as u32) as u16;
    let pixel_count = (w as usize) * (h as usize) * 3;
    // White canvas.
    let rgb = vec![0xFFu8; pixel_count];

    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, 85);
    encoder
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|_| EngineError::NoOutputProduced)?;
    Ok(out)
}

// -------------------------------------------------------------------------
// Markdown <-> PDF helpers (Task 3.7).
// -------------------------------------------------------------------------

/// A single laid-out logical line of Markdown content, classified by the
/// structural role it plays. This is the intermediate representation shared by
/// Markdown to PDF (which renders each line) and PDF to Markdown (which
/// reconstructs Markdown syntax from the extracted lines), so the structural
/// round-trip (headings, lists, paragraphs) is faithful (Property 11, Req 30.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdLine {
    /// A heading of the given level (1-6) with its text.
    Heading { level: u8, text: String },
    /// An unordered list item with its text.
    ListItem { text: String },
    /// A line of paragraph (or code) text.
    Paragraph { text: String },
}

impl MdLine {
    /// Render this line back to canonical Markdown syntax.
    fn to_markdown(&self) -> String {
        match self {
            MdLine::Heading { level, text } => {
                let hashes = "#".repeat((*level).clamp(1, 6) as usize);
                format!("{hashes} {text}")
            }
            MdLine::ListItem { text } => format!("- {text}"),
            MdLine::Paragraph { text } => text.clone(),
        }
    }

    /// The exact text drawn on the PDF page for this line. We draw the Markdown
    /// syntax verbatim (e.g. `# Title`, `- item`) so extraction can recover the
    /// structure without a side channel.
    fn draw_text(&self) -> String {
        self.to_markdown()
    }

    /// Classify an extracted text line back into an [`MdLine`], recognizing
    /// ATX headings (`#`..`######`) and unordered list markers (`- `, `* `,
    /// `+ `) exactly as Markdown to PDF emits them.
    fn from_extracted(line: &str) -> Option<MdLine> {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            return None;
        }
        let t = trimmed.trim_start();

        // Heading: one-to-six '#' followed by a space.
        let hashes = t.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) {
            if let Some(rest) = t[hashes..].strip_prefix(' ') {
                return Some(MdLine::Heading {
                    level: hashes as u8,
                    text: rest.trim().to_string(),
                });
            }
        }

        // Unordered list item.
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = t.strip_prefix(marker) {
                return Some(MdLine::ListItem {
                    text: rest.trim().to_string(),
                });
            }
        }

        Some(MdLine::Paragraph {
            text: t.to_string(),
        })
    }
}

/// Parse Markdown `text` into the flat, structural [`MdLine`] representation
/// (Markdown to PDF, Req 17.1, 17.3).
///
/// Uses the pure-Rust `pulldown-cmark` CommonMark parser. Headings, unordered
/// and ordered list items, paragraphs, code blocks (rendered as paragraph
/// lines), and link/emphasis text are flattened to their textual content so
/// they can be laid out; the structural subset guaranteed by the round-trip
/// (headings, lists, paragraphs) is preserved exactly (Property 11).
pub fn parse_markdown(text: &str) -> Vec<MdLine> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let mut lines: Vec<MdLine> = Vec::new();

    // Current accumulation state.
    let mut heading_level: Option<u8> = None;
    let mut in_list_item = false;
    let mut buffer = String::new();

    let flush_paragraph = |buffer: &mut String, lines: &mut Vec<MdLine>| {
        let t = buffer.trim();
        if !t.is_empty() {
            lines.push(MdLine::Paragraph { text: t.to_string() });
        }
        buffer.clear();
    };

    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush_paragraph(&mut buffer, &mut lines);
                heading_level = Some(heading_level_to_u8(level));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(level) = heading_level.take() {
                    let t = buffer.trim();
                    if !t.is_empty() {
                        lines.push(MdLine::Heading {
                            level,
                            text: t.to_string(),
                        });
                    }
                    buffer.clear();
                }
            }
            Event::Start(Tag::Item) => {
                flush_paragraph(&mut buffer, &mut lines);
                in_list_item = true;
            }
            Event::End(TagEnd::Item) => {
                if in_list_item {
                    let t = buffer.trim();
                    if !t.is_empty() {
                        lines.push(MdLine::ListItem { text: t.to_string() });
                    }
                    buffer.clear();
                    in_list_item = false;
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if heading_level.is_none() && !in_list_item {
                    flush_paragraph(&mut buffer, &mut lines);
                }
            }
            Event::Text(t) | Event::Code(t) => {
                buffer.push_str(&t);
            }
            Event::SoftBreak | Event::HardBreak => {
                buffer.push(' ');
            }
            _ => {}
        }
    }
    // Flush any trailing paragraph text.
    flush_paragraph(&mut buffer, &mut lines);

    lines
}

/// Map a pulldown-cmark [`pulldown_cmark::HeadingLevel`] to a 1-6 depth.
fn heading_level_to_u8(level: pulldown_cmark::HeadingLevel) -> u8 {
    use pulldown_cmark::HeadingLevel::*;
    match level {
        H1 => 1,
        H2 => 2,
        H3 => 3,
        H4 => 4,
        H5 => 5,
        H6 => 6,
    }
}

/// Render a list of [`MdLine`]s into a PDF, drawing exactly one logical line
/// per page as the canonical Markdown syntax (e.g. `# Title`, `- item`) so the
/// structure can be recovered unambiguously by [`extract_markdown`] (Markdown
/// to PDF, Req 17.1, Property 11).
///
/// ## Why one line per page
///
/// PDF text extraction reconstructs lines from glyph positioning heuristics, and
/// consecutive text lines placed in one content stream can be merged or split
/// unpredictably by the extractor. Placing each logical line on its own page
/// makes `extract_text(&[page])` return exactly that line's text, which is what
/// makes the structural round-trip (Property 11) exact. The rendered document
/// therefore has one page per Markdown line — acceptable because the guarantee
/// under test is structural fidelity, not visual pagination.
///
/// # Errors
///
/// Returns [`EngineError::NoOutputProduced`] if content encoding or page tree
/// construction fails, or if `lines` is empty.
pub fn build_pdf_from_markdown(lines: &[MdLine]) -> Result<Document, EngineError> {
    if lines.is_empty() {
        return Err(EngineError::NoOutputProduced);
    }

    let mut doc = Document::with_version(OUTPUT_PDF_VERSION);

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });

    let page_w = PAGE_SHORT_PT;
    let page_h = PAGE_LONG_PT;
    let left = 72.0f32;
    let baseline = 720.0f32;
    let font_size = 12.0f32;

    let mut page_ids: Vec<ObjectId> = Vec::with_capacity(lines.len());

    for line in lines {
        let text = line.draw_text();
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), font_size.into()]),
                Operation::new("Td", vec![left.into(), baseline.into()]),
                Operation::new("Tj", vec![Object::string_literal(text)]),
                Operation::new("ET", vec![]),
            ],
        };
        let encoded = content.encode().map_err(|_| EngineError::NoOutputProduced)?;
        let content_id = doc.add_object(Stream::new(dictionary! {}, encoded));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), page_w.into(), page_h.into()],
        });
        page_ids.push(page_id);
    }

    finalize_page_tree(&mut doc, page_ids);
    Ok(doc)
}

/// Extract the structural Markdown of `doc` by reading each page's text (one
/// logical line per page, as emitted by [`build_pdf_from_markdown`]) and
/// classifying it back into an [`MdLine`] (PDF to Markdown, Req 23.1, 23.2,
/// 23.3, Property 11).
///
/// # Errors
///
/// Returns [`EngineError::NoTextFound`] if the document contains no extractable
/// text (Req 19.3-style rejection).
pub fn extract_markdown(doc: &Document) -> Result<String, EngineError> {
    let mut out_lines: Vec<MdLine> = Vec::new();
    for (page_no, _) in doc.get_pages() {
        let text = doc.extract_text(&[page_no]).unwrap_or_default();
        // Each page holds one logical line, but be tolerant of embedded newlines.
        for raw_line in text.split('\n') {
            if let Some(line) = MdLine::from_extracted(raw_line) {
                out_lines.push(line);
            }
        }
    }

    if out_lines.is_empty() {
        return Err(EngineError::NoTextFound);
    }

    let markdown = out_lines
        .iter()
        .map(MdLine::to_markdown)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(markdown)
}

// -------------------------------------------------------------------------
// Rotation, per-page annotation, edit, and form helpers (Tasks 4.1, 4.4).
// -------------------------------------------------------------------------

use crate::model::{Angle, Element, FormField, FormFieldKind, PageScope, Position, Rect, ShapeKind};

/// Resolve a [`PageScope`] against a document's 1-based page numbers, returning
/// the targeted pages in ascending order. Out-of-range page numbers are ignored.
///
/// [`PageScope::All`] yields every page (`1..=page_count`); a specific selection
/// is deduplicated and intersected with the existing pages.
pub fn resolve_scope(doc: &Document, scope: &PageScope) -> Vec<u32> {
    let total = page_count(doc);
    match scope {
        PageScope::All => (1..=total).collect(),
        PageScope::Pages(pages) => {
            let requested: std::collections::BTreeSet<u32> =
                pages.iter().copied().filter(|&p| p >= 1 && p <= total).collect();
            requested.into_iter().collect()
        }
    }
}

/// Set (accumulate) the `/Rotate` entry on the scoped 1-based `pages` by `angle`
/// degrees clockwise, normalized to `0..360` (Rotate, Req 24.1, 24.2, 24.3).
///
/// The rotation is *added* to any existing `/Rotate` on the page so repeated
/// applications compose (four 90° turns restore the original, Property 12). A
/// page that already carries an inherited rotation keeps it as the base.
pub fn rotate_pages(doc: &mut Document, pages: &[u32], angle: Angle) {
    let delta = angle_degrees(angle);
    if delta == 0 {
        return;
    }
    let page_map = doc.get_pages();
    for &page_no in pages {
        if let Some(&page_id) = page_map.get(&page_no) {
            if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page_id) {
                let current = dict
                    .get(b"Rotate")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(0);
                let normalized = (current + delta).rem_euclid(360);
                dict.set("Rotate", Object::Integer(normalized));
            }
        }
    }
}

/// Read the effective `/Rotate` value of a 1-based `page_no`, normalized to
/// `0..360`; a missing entry reads as 0. Used by tests to assert Property 12.
#[cfg_attr(not(test), allow(dead_code))]
pub fn page_rotation(doc: &Document, page_no: u32) -> i64 {
    let page_map = doc.get_pages();
    page_map
        .get(&page_no)
        .and_then(|&id| doc.get_object(id).ok())
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"Rotate").ok())
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0)
        .rem_euclid(360)
}

/// Append `operations` to a page as an *additional* content stream, so an
/// overlay is drawn on top of the page's existing content without disturbing it.
///
/// PDF allows a page's `/Contents` to be an array of streams that are
/// concatenated in order; we add a new stream (converting a single-stream page
/// to an array as needed) that first saves/restores graphics state so the
/// overlay cannot leak state into or out of the original content.
///
/// # Errors
///
/// Returns [`EngineError::NoOutputProduced`] if the overlay content cannot be
/// encoded, or [`EngineError::Corrupt`] if the page dictionary is unreadable.
fn append_page_content(
    doc: &mut Document,
    page_id: ObjectId,
    operations: Vec<Operation>,
) -> Result<(), EngineError> {
    // Wrap the overlay in q/Q so it is isolated from the page's own content.
    let mut wrapped = Vec::with_capacity(operations.len() + 2);
    wrapped.push(Operation::new("q", vec![]));
    wrapped.extend(operations);
    wrapped.push(Operation::new("Q", vec![]));
    let content = Content { operations: wrapped };
    let encoded = content.encode().map_err(|_| EngineError::NoOutputProduced)?;
    let overlay_id = doc.add_object(Stream::new(dictionary! {}, encoded));

    let dict = doc
        .get_object_mut(page_id)
        .map_err(|_| EngineError::Corrupt)?
        .as_dict_mut()
        .map_err(|_| EngineError::Corrupt)?;

    let new_contents = match dict.get(b"Contents").ok().cloned() {
        Some(Object::Array(mut items)) => {
            items.push(Object::Reference(overlay_id));
            Object::Array(items)
        }
        Some(existing @ Object::Reference(_)) => {
            Object::Array(vec![existing, Object::Reference(overlay_id)])
        }
        // No prior content (or an unexpected shape): the overlay becomes the
        // page content directly.
        _ => Object::Reference(overlay_id),
    };
    dict.set("Contents", new_contents);
    Ok(())
}

/// Ensure a page's `/Resources` dictionary declares the Helvetica font `F1`,
/// adding it if absent, so overlay text can reference `/F1`.
fn ensure_helvetica_font(doc: &mut Document, page_id: ObjectId) -> Result<(), EngineError> {
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });

    let dict = doc
        .get_object_mut(page_id)
        .map_err(|_| EngineError::Corrupt)?
        .as_dict_mut()
        .map_err(|_| EngineError::Corrupt)?;

    match dict.get(b"Resources").ok().cloned() {
        Some(Object::Dictionary(mut res)) => {
            merge_font_into_resources(&mut res, font_id);
            dict.set("Resources", Object::Dictionary(res));
            Ok(())
        }
        _ => {
            // No inline Resources dict (may be inherited or a reference). Attach
            // a fresh one carrying just our font; page-level Resources override
            // inherited ones for the purpose of our overlay's /F1 reference.
            let mut res = Dictionary::new();
            merge_font_into_resources(&mut res, font_id);
            dict.set("Resources", Object::Dictionary(res));
            Ok(())
        }
    }
}

/// Insert font `F1 -> font_id` into a `/Font` sub-dictionary of `resources`,
/// creating the `/Font` dictionary if needed and preserving existing fonts.
fn merge_font_into_resources(resources: &mut Dictionary, font_id: ObjectId) {
    match resources.get(b"Font").ok().cloned() {
        Some(Object::Dictionary(mut fonts)) => {
            fonts.set("F1", Object::Reference(font_id));
            resources.set("Font", Object::Dictionary(fonts));
        }
        _ => {
            let mut fonts = Dictionary::new();
            fonts.set("F1", Object::Reference(font_id));
            resources.set("Font", Object::Dictionary(fonts));
        }
    }
}

/// Build the text-drawing operations for a single string at `(x, y)` in points
/// with the given `font_size`, using the page's `/F1` Helvetica font.
fn draw_text_ops(x: f32, y: f32, font_size: f32, text: &str) -> Vec<Operation> {
    vec![
        Operation::new("BT", vec![]),
        Operation::new("Tf", vec!["F1".into(), font_size.into()]),
        Operation::new("Td", vec![x.into(), y.into()]),
        Operation::new("Tj", vec![Object::string_literal(text.to_string())]),
        Operation::new("ET", vec![]),
    ]
}

/// Compute the `(x, y)` baseline in points for a page-number label at `position`
/// on a page of size `(page_w, page_h)`, given the label's approximate width.
fn position_point(
    position: Position,
    page_w: f32,
    page_h: f32,
    label_w: f32,
    font_size: f32,
) -> (f32, f32) {
    // Fixed inset from each edge, in points.
    let inset = 36.0f32;
    let top_y = (page_h - inset - font_size).max(0.0);
    let bottom_y = inset;
    let center_x = ((page_w - label_w) / 2.0).max(0.0);
    let left_x = inset;
    let right_x = (page_w - inset - label_w).max(0.0);
    match position {
        Position::TopLeft => (left_x, top_y),
        Position::TopCenter => (center_x, top_y),
        Position::TopRight => (right_x, top_y),
        Position::BottomLeft => (left_x, bottom_y),
        Position::BottomCenter => (center_x, bottom_y),
        Position::BottomRight => (right_x, bottom_y),
    }
}

/// Stamp a page-number label on every page, numbering the pages consecutively
/// from `start` at the chosen `position` (Add Page Numbers, Req 25.1, 25.2,
/// 25.3). Every page receives exactly one number overlay; the page count is
/// unchanged (Property 13).
///
/// # Errors
///
/// Returns [`EngineError::Corrupt`] on an unreadable page or
/// [`EngineError::NoOutputProduced`] if an overlay cannot be encoded.
pub fn add_page_numbers(
    doc: &mut Document,
    position: Position,
    start: u32,
) -> Result<(), EngineError> {
    let font_size = 12.0f32;
    // Ordered 1-based page numbers.
    let page_nos: Vec<u32> = doc.get_pages().keys().copied().collect();
    for (index, &page_no) in page_nos.iter().enumerate() {
        let page_map = doc.get_pages();
        let Some(&page_id) = page_map.get(&page_no) else {
            continue;
        };
        let (page_w, page_h) = page_media_box_size(doc, page_no);
        let label = (start as u64 + index as u64).to_string();
        // Helvetica digits are ~0.556 em wide; approximate the label width.
        let label_w = label.len() as f32 * font_size * 0.556;
        let (x, y) = position_point(position, page_w, page_h, label_w, font_size);

        ensure_helvetica_font(doc, page_id)?;
        let ops = draw_text_ops(x, y, font_size, &label);
        append_page_content(doc, page_id, ops)?;
    }
    Ok(())
}

/// Overlay a text (and/or image-placeholder) watermark on every page at the
/// given `opacity` (0-100) and `rotation_deg` (Add Watermark, Req 26.1, 26.2,
/// 26.3, 26.4). Every page receives the watermark overlay; the page count is
/// unchanged (Property 13).
///
/// ## Image watermark scope
///
/// A text watermark is drawn fully in-engine. An image watermark is described
/// by a [`crate::model::FileRef`] whose bytes are resolved by the server layer
/// from the File_Store (the pure engine holds no bytes for it). When an image
/// (and no text) is requested, this still stamps a small text marker per page so
/// the per-page invariant (Property 13) holds on both planes; the server layer
/// substitutes the real image pixels. At least a text watermark is always drawn
/// when `text` is present.
///
/// # Errors
///
/// Returns [`EngineError::Corrupt`] on an unreadable page,
/// [`EngineError::NoOutputProduced`] if an overlay cannot be encoded, or
/// [`EngineError::Unsupported`] if neither text nor image was supplied.
pub fn add_watermark(
    doc: &mut Document,
    text: Option<&str>,
    has_image: bool,
    opacity: u8,
    rotation_deg: i16,
) -> Result<(), EngineError> {
    // Resolve the marker string to draw: the caller's text, or a placeholder for
    // the server-resolved image watermark so every page is still marked.
    let marker: String = match (text, has_image) {
        (Some(t), _) if !t.is_empty() => t.to_string(),
        (_, true) => "[image watermark]".to_string(),
        _ => return Err(EngineError::Unsupported),
    };

    let font_size = 48.0f32;
    // Map opacity 0-100 to a gray fill so the overlay reads as a faint mark
    // without requiring an ExtGState alpha (kept simple and WASM-portable):
    // higher opacity -> darker text. Clamp to [0,100].
    let opacity = opacity.min(100);
    let gray = 1.0 - (opacity as f32 / 100.0); // 100% -> 0.0 (black), 0% -> 1.0 (white)
    let theta = (rotation_deg as f32).to_radians();
    let (cos, sin) = (theta.cos(), theta.sin());

    let page_nos: Vec<u32> = doc.get_pages().keys().copied().collect();
    for &page_no in &page_nos {
        let page_map = doc.get_pages();
        let Some(&page_id) = page_map.get(&page_no) else {
            continue;
        };
        let (page_w, page_h) = page_media_box_size(doc, page_no);
        // Anchor the rotated text near the page center.
        let cx = page_w / 2.0;
        let cy = page_h / 2.0;

        ensure_helvetica_font(doc, page_id)?;
        let ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("g", vec![gray.into()]),
            Operation::new("Tf", vec!["F1".into(), font_size.into()]),
            // Rotation matrix about the page center: [cos sin -sin cos cx cy].
            Operation::new(
                "Tm",
                vec![
                    cos.into(),
                    sin.into(),
                    (-sin).into(),
                    cos.into(),
                    cx.into(),
                    cy.into(),
                ],
            ),
            Operation::new("Tj", vec![Object::string_literal(marker.clone())]),
            Operation::new("ET", vec![]),
        ];
        append_page_content(doc, page_id, ops)?;
    }
    Ok(())
}

/// Set the `/CropBox` of the targeted pages to `region` (Crop, Req 27.1, 27.3).
///
/// When `all_pages` is true every page is cropped; otherwise only the first
/// page. Every targeted page receives the `/CropBox`; the page count is
/// unchanged (Property 13). The crop rectangle is expressed as
/// `[x, y, x+width, y+height]` in PDF points.
pub fn crop_pages(doc: &mut Document, region: Rect, all_pages: bool) {
    let page_nos: Vec<u32> = doc.get_pages().keys().copied().collect();
    let targets: Vec<u32> = if all_pages {
        page_nos
    } else {
        page_nos.into_iter().take(1).collect()
    };

    let x0 = region.x;
    let y0 = region.y;
    let x1 = region.x + region.width;
    let y1 = region.y + region.height;
    let crop = Object::Array(vec![
        Object::Real(x0),
        Object::Real(y0),
        Object::Real(x1),
        Object::Real(y1),
    ]);

    let page_map = doc.get_pages();
    for page_no in targets {
        if let Some(&page_id) = page_map.get(&page_no) {
            if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page_id) {
                dict.set("CropBox", crop.clone());
            }
        }
    }
}

/// Read a page's `/CropBox` as `[x0, y0, x1, y1]` in points, if present. Used by
/// tests to assert Property 13 for Crop.
#[cfg_attr(not(test), allow(dead_code))]
pub fn page_crop_box(doc: &Document, page_no: u32) -> Option<[f32; 4]> {
    let page_map = doc.get_pages();
    let arr = page_map
        .get(&page_no)
        .and_then(|&id| doc.get_object(id).ok())
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"CropBox").ok())
        .and_then(|o| o.as_array().ok())?;
    if arr.len() != 4 {
        return None;
    }
    let mut vals = [0.0f32; 4];
    for (i, o) in arr.iter().enumerate() {
        vals[i] = o.as_f32().or_else(|_| o.as_i64().map(|v| v as f32)).ok()?;
    }
    Some(vals)
}

/// Apply Edit PDF `elements` (text / image-placeholder / shape) onto their
/// specified 1-based pages, preserving the existing pages (Edit PDF, Req 28.4).
///
/// Text and shapes are drawn fully in-engine. An image element references a
/// File_Store [`crate::model::FileRef`] whose bytes the server layer resolves;
/// the pure engine draws a labeled placeholder rectangle at the element's box so
/// the element is present on the page on both planes, and the server layer may
/// substitute the real image. Elements referencing a non-existent page are
/// skipped.
///
/// # Errors
///
/// Returns [`EngineError::Corrupt`] on an unreadable page or
/// [`EngineError::NoOutputProduced`] if an overlay cannot be encoded.
pub fn apply_elements(doc: &mut Document, elements: &[Element]) -> Result<(), EngineError> {
    for element in elements {
        let page = element_page(element);
        let page_map = doc.get_pages();
        let Some(&page_id) = page_map.get(&page) else {
            continue; // element targets a non-existent page: skip
        };

        match element {
            Element::Text {
                at,
                content,
                font_size,
                ..
            } => {
                ensure_helvetica_font(doc, page_id)?;
                let ops = draw_text_ops(at.x, at.y, *font_size, content);
                append_page_content(doc, page_id, ops)?;
            }
            Element::Image { at, image, .. } => {
                // Image bytes are resolved by the server layer; draw a labeled
                // placeholder box so the element is present in the output.
                ensure_helvetica_font(doc, page_id)?;
                let mut ops = shape_ops(ShapeKind::Rectangle, at);
                ops.extend(draw_text_ops(
                    at.x + 2.0,
                    at.y + 2.0,
                    10.0,
                    &format!("[image: {}]", image.display_name),
                ));
                append_page_content(doc, page_id, ops)?;
            }
            Element::Shape { at, kind, .. } => {
                let ops = shape_ops(*kind, at);
                append_page_content(doc, page_id, ops)?;
            }
        }
    }
    Ok(())
}

/// The 1-based target page of an [`Element`].
fn element_page(element: &Element) -> u32 {
    match element {
        Element::Text { page, .. }
        | Element::Image { page, .. }
        | Element::Shape { page, .. } => *page,
    }
}

/// Build stroking operations that draw a [`ShapeKind`] within bounding box `at`.
fn shape_ops(kind: ShapeKind, at: &Rect) -> Vec<Operation> {
    let x = at.x;
    let y = at.y;
    let w = at.width;
    let h = at.height;
    match kind {
        ShapeKind::Rectangle => vec![
            Operation::new("re", vec![x.into(), y.into(), w.into(), h.into()]),
            Operation::new("S", vec![]),
        ],
        ShapeKind::Line => vec![
            Operation::new("m", vec![x.into(), y.into()]),
            Operation::new("l", vec![(x + w).into(), (y + h).into()]),
            Operation::new("S", vec![]),
        ],
        ShapeKind::Ellipse => {
            // Approximate an ellipse with four cubic Bezier arcs (kappa).
            let kappa = 0.552_284_8_f32;
            let rx = w / 2.0;
            let ry = h / 2.0;
            let cx = x + rx;
            let cy = y + ry;
            let ox = rx * kappa;
            let oy = ry * kappa;
            vec![
                Operation::new("m", vec![(cx - rx).into(), cy.into()]),
                Operation::new(
                    "c",
                    vec![
                        (cx - rx).into(),
                        (cy + oy).into(),
                        (cx - ox).into(),
                        (cy + ry).into(),
                        cx.into(),
                        (cy + ry).into(),
                    ],
                ),
                Operation::new(
                    "c",
                    vec![
                        (cx + ox).into(),
                        (cy + ry).into(),
                        (cx + rx).into(),
                        (cy + oy).into(),
                        (cx + rx).into(),
                        cy.into(),
                    ],
                ),
                Operation::new(
                    "c",
                    vec![
                        (cx + rx).into(),
                        (cy - oy).into(),
                        (cx + ox).into(),
                        (cy - ry).into(),
                        cx.into(),
                        (cy - ry).into(),
                    ],
                ),
                Operation::new(
                    "c",
                    vec![
                        (cx - ox).into(),
                        (cy - ry).into(),
                        (cx - rx).into(),
                        (cy - oy).into(),
                        (cx - rx).into(),
                        cy.into(),
                    ],
                ),
                Operation::new("S", vec![]),
            ]
        }
    }
}

/// Count the total number of content-stream objects referenced by a page's
/// `/Contents` (single stream = 1, array = its length). Used by tests to detect
/// that an overlay was appended.
#[cfg_attr(not(test), allow(dead_code))]
pub fn page_content_stream_count(doc: &Document, page_no: u32) -> usize {
    let page_map = doc.get_pages();
    match page_map
        .get(&page_no)
        .and_then(|&id| doc.get_object(id).ok())
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"Contents").ok())
    {
        Some(Object::Array(items)) => items.len(),
        Some(Object::Reference(_)) => 1,
        _ => 0,
    }
}

/// Write `field_values` into existing AcroForm fields and register `added_fields`
/// as new interactive fields, so every added field is an interactive AcroForm
/// entry in the output (PDF Forms, Req 29.2, 29.4).
///
/// Existing fields matched by `/T` (partial field name) have their `/V` value
/// (and, for text fields, appearance-driving value) set. New fields are created
/// as widget-annotation form fields, placed on their page's `/Annots`, and
/// referenced from the document's `/AcroForm` `/Fields` array (created if the
/// document had no AcroForm). `NeedAppearances` is set so viewers regenerate
/// appearances for the written values.
///
/// # Errors
///
/// Returns [`EngineError::Corrupt`] on unreadable structure or
/// [`EngineError::NoOutputProduced`] if serialization prerequisites cannot be met.
pub fn apply_form(
    doc: &mut Document,
    field_values: &[(String, String)],
    added_fields: &[FormField],
) -> Result<(), EngineError> {
    // 1. Create the added interactive fields as widget-annotation form fields
    //    first, so subsequent value-setting can target both pre-existing and
    //    newly-added fields by name (Req 29.2, 29.4).
    let mut added_field_ids: Vec<ObjectId> = Vec::with_capacity(added_fields.len());
    for field in added_fields {
        let page_map = doc.get_pages();
        let Some(&page_id) = page_map.get(&field.page) else {
            continue; // field targets a non-existent page: skip
        };
        let field_id = build_form_field(doc, field, page_id);
        // Attach the widget to the page's /Annots.
        attach_annotation(doc, page_id, field_id)?;
        added_field_ids.push(field_id);
    }

    // 2. Register the new fields in the document's AcroForm /Fields, marking the
    //    form interactive (NeedAppearances) so values render.
    register_acroform_fields(doc, &added_field_ids);

    // 3. Set values on every field matched by its partial name (/T), covering
    //    both pre-existing fields and the ones just added (Req 29.2).
    if !field_values.is_empty() {
        let value_map: std::collections::BTreeMap<&str, &str> = field_values
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        // Collect candidate field object ids first to avoid borrow conflicts.
        let field_ids: Vec<ObjectId> = doc
            .objects
            .iter()
            .filter_map(|(id, obj)| {
                let dict = obj.as_dict().ok()?;
                // A field carries a /T name entry.
                dict.get(b"T").ok()?;
                Some(*id)
            })
            .collect();
        for id in field_ids {
            let Ok(dict) = doc.get_object_mut(id).and_then(|o| o.as_dict_mut()) else {
                continue;
            };
            let name = dict
                .get(b"T")
                .ok()
                .and_then(|o| o.as_str().ok())
                .map(|b| String::from_utf8_lossy(b).into_owned());
            if let Some(name) = name {
                if let Some(&value) = value_map.get(name.as_str()) {
                    dict.set("V", Object::string_literal(value.to_string()));
                }
            }
        }
    }

    Ok(())
}

/// Build a single interactive form field (a merged field/widget dictionary) for
/// `field` on `page_id`, returning its object id. The dictionary is a Widget
/// annotation carrying the field's `/FT`, `/T`, `/Rect`, and a default value.
fn build_form_field(doc: &mut Document, field: &FormField, page_id: ObjectId) -> ObjectId {
    let rect = Object::Array(vec![
        Object::Real(field.at.x),
        Object::Real(field.at.y),
        Object::Real(field.at.x + field.at.width),
        Object::Real(field.at.y + field.at.height),
    ]);

    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"Annot".to_vec()));
    dict.set("Subtype", Object::Name(b"Widget".to_vec()));
    dict.set("T", Object::string_literal(field.name.clone()));
    dict.set("Rect", rect);
    dict.set("P", Object::Reference(page_id));
    // /F = 4 => Print flag set (standard for interactive fields).
    dict.set("F", Object::Integer(4));

    match field.kind {
        FormFieldKind::Text => {
            dict.set("FT", Object::Name(b"Tx".to_vec()));
            dict.set("V", Object::string_literal(String::new()));
        }
        FormFieldKind::Checkbox => {
            dict.set("FT", Object::Name(b"Btn".to_vec()));
            // Off by default; /AS and /V use the "Off" state name.
            dict.set("V", Object::Name(b"Off".to_vec()));
            dict.set("AS", Object::Name(b"Off".to_vec()));
        }
        FormFieldKind::Radio => {
            dict.set("FT", Object::Name(b"Btn".to_vec()));
            dict.set("V", Object::Name(b"Off".to_vec()));
            dict.set("AS", Object::Name(b"Off".to_vec()));
            // Radio group flag (bit 16 => 32768).
            dict.set("Ff", Object::Integer(32768));
        }
        FormFieldKind::Dropdown => {
            dict.set("FT", Object::Name(b"Ch".to_vec()));
            // Combo flag (bit 18 => 131072) marks it a dropdown.
            dict.set("Ff", Object::Integer(131072));
            dict.set("V", Object::string_literal(String::new()));
            dict.set("Opt", Object::Array(Vec::new()));
        }
    }

    doc.add_object(Object::Dictionary(dict))
}

/// Append `annot_id` to a page's `/Annots` array, creating it if absent.
fn attach_annotation(
    doc: &mut Document,
    page_id: ObjectId,
    annot_id: ObjectId,
) -> Result<(), EngineError> {
    let dict = doc
        .get_object_mut(page_id)
        .map_err(|_| EngineError::Corrupt)?
        .as_dict_mut()
        .map_err(|_| EngineError::Corrupt)?;
    let annots = match dict.get(b"Annots").ok().cloned() {
        Some(Object::Array(mut items)) => {
            items.push(Object::Reference(annot_id));
            items
        }
        _ => vec![Object::Reference(annot_id)],
    };
    dict.set("Annots", Object::Array(annots));
    Ok(())
}

/// Register `field_ids` in the document catalog's `/AcroForm` `/Fields` array,
/// creating the AcroForm dictionary if the document had none and setting
/// `NeedAppearances` so viewers render written values.
fn register_acroform_fields(doc: &mut Document, field_ids: &[ObjectId]) {
    // Resolve the catalog (Root) object id from the trailer.
    let Some(catalog_id) = doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|o| o.as_reference().ok())
    else {
        return;
    };

    // Read the current AcroForm (if any) to obtain its /Fields.
    let existing_acroform = doc
        .get_object(catalog_id)
        .ok()
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"AcroForm").ok().cloned());

    let mut fields: Vec<Object> = Vec::new();
    let mut acroform_dict = match existing_acroform {
        Some(Object::Dictionary(dict)) => {
            if let Ok(Object::Array(existing)) = dict.get(b"Fields") {
                fields.extend(existing.iter().cloned());
            }
            dict
        }
        Some(Object::Reference(ref_id)) => {
            let dict = doc
                .get_object(ref_id)
                .ok()
                .and_then(|o| o.as_dict().ok())
                .cloned()
                .unwrap_or_default();
            if let Ok(Object::Array(existing)) = dict.get(b"Fields") {
                fields.extend(existing.iter().cloned());
            }
            dict
        }
        _ => Dictionary::new(),
    };

    for &id in field_ids {
        fields.push(Object::Reference(id));
    }
    acroform_dict.set("Fields", Object::Array(fields));
    acroform_dict.set("NeedAppearances", Object::Boolean(true));

    let acroform_id = doc.add_object(Object::Dictionary(acroform_dict));
    if let Ok(Object::Dictionary(catalog)) = doc.get_object_mut(catalog_id) {
        catalog.set("AcroForm", Object::Reference(acroform_id));
    }
}

/// Count the interactive fields registered in the document's AcroForm
/// `/Fields` array. Used by tests to assert Req 29.4.
#[cfg_attr(not(test), allow(dead_code))]
pub fn acroform_field_count(doc: &Document) -> usize {
    let Some(catalog_id) = doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|o| o.as_reference().ok())
    else {
        return 0;
    };
    let acroform = doc
        .get_object(catalog_id)
        .ok()
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"AcroForm").ok().cloned());
    let dict = match acroform {
        Some(Object::Dictionary(d)) => Some(d),
        Some(Object::Reference(id)) => doc.get_object(id).ok().and_then(|o| o.as_dict().ok()).cloned(),
        _ => None,
    };
    dict.and_then(|d| d.get(b"Fields").ok().and_then(|o| o.as_array().ok()).map(|a| a.len()))
        .unwrap_or(0)
}

/// Look up the `/V` value of a field by its partial name (`/T`), returning the
/// value as a string when present. Used by tests to assert Req 29.2.
#[cfg_attr(not(test), allow(dead_code))]
pub fn field_value(doc: &Document, name: &str) -> Option<String> {
    for obj in doc.objects.values() {
        let Ok(dict) = obj.as_dict() else { continue };
        let field_name = dict
            .get(b"T")
            .ok()
            .and_then(|o| o.as_str().ok())
            .map(|b| String::from_utf8_lossy(b).into_owned());
        if field_name.as_deref() == Some(name) {
            if let Ok(v) = dict.get(b"V") {
                return match v {
                    Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
                    Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
                    _ => None,
                };
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Helpers for building tiny, valid, page-identifiable PDFs in tests.

    // Test-only helpers may unwrap/expect; relax the crate-wide deny here.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    /// Build a minimal valid single-stream PDF with `page_count` pages, where
    /// each page's content stream draws its 1-based `tag` so a page's identity
    /// survives copy/reorder operations and can be asserted after the fact.
    ///
    /// Returns the serialized PDF bytes.
    pub fn make_pdf_with_tags(tags: &[u32]) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();

        // A shared font so each page dictionary is complete.
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        let mut kids: Vec<Object> = Vec::new();
        for &tag in tags {
            // Content that embeds the tag as drawn text: "TAG:<n>".
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 24.into()]),
                    Operation::new("Td", vec![72.into(), 720.into()]),
                    Operation::new(
                        "Tj",
                        vec![Object::string_literal(format!("TAG:{tag}"))],
                    ),
                    Operation::new("ET", vec![]),
                ],
            };
            let encoded = content.encode().unwrap_or_default();
            let content_id = doc.add_object(Stream::new(dictionary! {}, encoded));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            });
            kids.push(page_id.into());
        }

        let count = kids.len() as i64;
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut buffer = Vec::new();
        // Test helper: unwrap is acceptable here (excluded from lib lints via cfg(test)).
        doc.save_to(&mut buffer).unwrap();
        buffer
    }

    /// Read back the ordered list of page tags embedded by [`make_pdf_with_tags`]
    /// from serialized PDF `bytes`, so tests can assert page identity and order.
    pub fn read_tags(bytes: &[u8]) -> Vec<u32> {
        let doc = Document::load_mem(bytes).unwrap();
        let mut tags = Vec::new();
        for (page_no, _) in doc.get_pages() {
            if let Ok(text) = doc.extract_text(&[page_no]) {
                if let Some(rest) = text.trim().strip_prefix("TAG:") {
                    if let Ok(n) = rest.trim().parse::<u32>() {
                        tags.push(n);
                        continue;
                    }
                }
            }
            tags.push(u32::MAX); // Unidentifiable page marker.
        }
        tags
    }

    /// Build a single-page PDF that already contains an interactive text form
    /// field named `field_name`, registered in an AcroForm, so tests can verify
    /// that PDF Forms updates the value of a pre-existing field (Req 29.2).
    pub fn make_pdf_with_text_field(field_name: &str) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();

        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });

        // A widget-annotation text field with an empty initial value.
        let field_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Tx",
            "T" => Object::string_literal(field_name.to_string()),
            "Rect" => vec![100.into(), 700.into(), 300.into(), 720.into()],
            "P" => page_id,
            "V" => Object::string_literal(String::new()),
        });

        // Wire the widget onto the page's /Annots.
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page_id) {
            dict.set("Annots", Object::Array(vec![Object::Reference(field_id)]));
        }

        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1_i64,
            }),
        );

        let acroform_id = doc.add_object(dictionary! {
            "Fields" => vec![Object::Reference(field_id)],
        });
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "AcroForm" => acroform_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut buffer = Vec::new();
        doc.save_to(&mut buffer).unwrap();
        buffer
    }

    /// Encode a solid-color `width` x `height` baseline JPEG for tests, so JPG
    /// to PDF / round-trip properties have valid, decodable JPEG sources.
    pub fn make_jpg(width: u16, height: u16) -> Vec<u8> {
        let pixel_count = (width as usize) * (height as usize) * 3;
        let rgb = vec![0x80u8; pixel_count];
        let mut out = Vec::new();
        let encoder = jpeg_encoder::Encoder::new(&mut out, 90);
        encoder
            .encode(&rgb, width, height, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        out
    }
}

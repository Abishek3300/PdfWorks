//! Validator: content-signature typing, size/batch limits, and zip-bomb guards
//! (Task 15.2, Req 33.1, 33.2, 39.1-39.4, 39.7, 42.5, 49.2, 50.4).
//!
//! Validation runs on the Backend *before* a Job is enqueued (Req 39.4), so a
//! rejected Source_File never reaches processing. Every check is a pure
//! function of the file bytes + the selected Tool, which keeps it directly
//! unit- and property-testable.
//!
//! The file type is determined from the **content signature** (magic bytes),
//! not the client-declared name or MIME type (Req 39.1). For archive-based
//! Office formats (DOCX/XLSX/PPTX, which are ZIP containers) the ZIP central
//! directory is parsed *without decompressing* to bound the total uncompressed
//! size, the expansion ratio, and the nesting depth — this rejects zip bombs
//! (Property 23, Req 50.4, 42.5) using only the declared sizes in the archive
//! metadata.

use crate::config::{ArchiveLimits, MAX_BATCH_COUNT, MAX_FILE_SIZE_BYTES};

/// A detected file type, keyed to the Supported_Formats a Tool documents
/// (Req 2.8, 33.1, 39.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedType {
    /// `%PDF-` header.
    Pdf,
    /// JFIF/Exif JPEG (`FF D8 FF`).
    Jpeg,
    /// PNG (`89 50 4E 47`).
    Png,
    /// A ZIP-container Office document (DOCX/XLSX/PPTX share the ZIP signature).
    OfficeZip,
    /// Plain UTF-8 text / Markdown (no binary signature).
    Text,
    /// Legacy OLE compound file (DOC/XLS/PPT: `D0 CF 11 E0`).
    OleLegacy,
}

/// Why a Source_File was rejected. Each variant maps to a spec message and
/// never carries file content (Req 44.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// Zero-byte Source_File (Req 49.2).
    Empty,
    /// Content signature does not match a Supported_Format for the Tool
    /// (Req 39.1, 33.1).
    UnsupportedType,
    /// Corrupt / unparseable Source_File (Req 33.2, 39.7).
    Corrupt,
    /// Source_File exceeds `Max_File_Size` (Req 39.2).
    TooLarge {
        /// Configured maximum per-file size.
        max_bytes: u64,
    },
    /// Job exceeds `Max_Batch_Count` (Req 39.3).
    BatchTooLarge {
        /// Configured maximum batch count.
        max_count: usize,
    },
    /// Archive uncompressed size exceeded the limit (Req 50.4).
    ArchiveTooLarge,
    /// Archive expansion ratio exceeded the limit (Req 42.5).
    ArchiveExpansionTooHigh,
    /// Archive nesting depth exceeded the limit (Req 50.4).
    ArchiveTooDeep,
}

impl ValidationError {
    /// User-facing message (Req 33.2, 39.2, 42.5, 49.2).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            ValidationError::Empty => "the source file is empty".to_string(),
            ValidationError::UnsupportedType => {
                "the source file is not a supported format for this tool".to_string()
            }
            ValidationError::Corrupt => {
                "the source file cannot be processed".to_string()
            }
            ValidationError::TooLarge { max_bytes } => {
                format!("the source file exceeds the maximum size of {max_bytes} bytes")
            }
            ValidationError::BatchTooLarge { max_count } => {
                format!("a job may contain at most {max_count} files")
            }
            ValidationError::ArchiveTooLarge
            | ValidationError::ArchiveExpansionTooHigh
            | ValidationError::ArchiveTooDeep => {
                "the source file cannot be processed".to_string()
            }
        }
    }
}

/// Detect a file's type from its leading content signature (Req 39.1).
///
/// Returns `None` when no known binary signature matches and the bytes are not
/// valid UTF-8 text.
#[must_use]
pub fn detect_type(bytes: &[u8]) -> Option<DetectedType> {
    if bytes.starts_with(b"%PDF-") {
        return Some(DetectedType::Pdf);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(DetectedType::Jpeg);
    }
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(DetectedType::Png);
    }
    // ZIP local-file-header ("PK\x03\x04") or empty-archive/central markers.
    if bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04])
        || bytes.starts_with(&[0x50, 0x4B, 0x05, 0x06])
        || bytes.starts_with(&[0x50, 0x4B, 0x07, 0x08])
    {
        return Some(DetectedType::OfficeZip);
    }
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Some(DetectedType::OleLegacy);
    }
    // Fall back to text if the bytes decode as UTF-8 (Markdown / plain text).
    if std::str::from_utf8(bytes).is_ok() {
        return Some(DetectedType::Text);
    }
    None
}

/// Whether a detected type is one of the accepted `accepted` set.
fn type_accepted(detected: DetectedType, accepted: &[DetectedType]) -> bool {
    accepted.contains(&detected)
}

/// Validate a single Source_File against the accepted types + limits.
///
/// Checks, in order: non-empty (Req 49.2), size (Req 39.2), content-signature
/// type (Req 39.1), and — for ZIP-container archives — the zip-bomb limits
/// (Req 50.4, 42.5).
///
/// # Errors
///
/// Returns the first [`ValidationError`] that applies.
pub fn validate_source(
    bytes: &[u8],
    accepted: &[DetectedType],
    limits: &ArchiveLimits,
) -> Result<DetectedType, ValidationError> {
    if bytes.is_empty() {
        return Err(ValidationError::Empty);
    }
    if bytes.len() as u64 > MAX_FILE_SIZE_BYTES {
        return Err(ValidationError::TooLarge {
            max_bytes: MAX_FILE_SIZE_BYTES,
        });
    }

    let detected = detect_type(bytes).ok_or(ValidationError::UnsupportedType)?;
    if !type_accepted(detected, accepted) {
        return Err(ValidationError::UnsupportedType);
    }

    if detected == DetectedType::OfficeZip {
        inspect_archive(bytes, limits, 0)?;
    }

    Ok(detected)
}

/// Enforce `Max_Batch_Count` for a whole Job (Req 39.3).
///
/// # Errors
///
/// Returns [`ValidationError::BatchTooLarge`] if more than `Max_Batch_Count`
/// files are present.
pub fn validate_batch_count(count: usize) -> Result<(), ValidationError> {
    if count > MAX_BATCH_COUNT {
        return Err(ValidationError::BatchTooLarge {
            max_count: MAX_BATCH_COUNT,
        });
    }
    Ok(())
}

// -------------------------------------------------------------------------
// Minimal, decompression-free ZIP central-directory inspection.
//
// A ZIP archive ends with an End Of Central Directory (EOCD) record whose
// signature is `PK\x05\x06`. The central directory lists each entry with its
// compressed and uncompressed sizes. We read only those declared sizes (never
// decompressing) to compute the total expanded size, the expansion ratio, and —
// by recursing into nested ZIP entries via their local headers — the nesting
// depth. This is exactly what is needed to reject a zip bomb (Property 23).
// -------------------------------------------------------------------------

/// EOCD record signature.
const EOCD_SIG: [u8; 4] = [0x50, 0x4B, 0x05, 0x06];
/// Central directory file-header signature.
const CDH_SIG: [u8; 4] = [0x50, 0x4B, 0x01, 0x02];
/// Local file-header signature.
const LFH_SIG: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];

/// Read a little-endian u16 at `off`, or `None` if out of range.
fn read_u16(buf: &[u8], off: usize) -> Option<u16> {
    let b = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// Read a little-endian u32 at `off`, or `None` if out of range.
fn read_u32(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Inspect a ZIP archive's central directory and enforce the bomb limits.
///
/// `depth` is the current nesting level (0 = the top-level upload). Recurses
/// into entries that are themselves ZIP archives to enforce the nesting-depth
/// limit (Req 50.4).
fn inspect_archive(
    bytes: &[u8],
    limits: &ArchiveLimits,
    depth: u32,
) -> Result<(), ValidationError> {
    if depth > limits.max_nesting_depth {
        return Err(ValidationError::ArchiveTooDeep);
    }

    let eocd = find_eocd(bytes).ok_or(ValidationError::Corrupt)?;
    let cd_entries = read_u16(bytes, eocd + 10).ok_or(ValidationError::Corrupt)?;
    let cd_offset = read_u32(bytes, eocd + 16).ok_or(ValidationError::Corrupt)? as usize;

    let mut total_uncompressed: u64 = 0;
    let mut total_compressed: u64 = 0;

    let mut cursor = cd_offset;
    for _ in 0..cd_entries {
        // Each central-directory header is at least 46 bytes before its
        // variable-length name/extra/comment fields.
        let sig = bytes
            .get(cursor..cursor + 4)
            .ok_or(ValidationError::Corrupt)?;
        if sig != CDH_SIG {
            return Err(ValidationError::Corrupt);
        }
        let comp = read_u32(bytes, cursor + 20).ok_or(ValidationError::Corrupt)? as u64;
        let uncomp = read_u32(bytes, cursor + 24).ok_or(ValidationError::Corrupt)? as u64;
        let name_len = read_u16(bytes, cursor + 28).ok_or(ValidationError::Corrupt)? as usize;
        let extra_len = read_u16(bytes, cursor + 30).ok_or(ValidationError::Corrupt)? as usize;
        let comment_len =
            read_u16(bytes, cursor + 32).ok_or(ValidationError::Corrupt)? as usize;
        let local_offset =
            read_u32(bytes, cursor + 42).ok_or(ValidationError::Corrupt)? as usize;
        let name = bytes
            .get(cursor + 46..cursor + 46 + name_len)
            .ok_or(ValidationError::Corrupt)?;

        total_uncompressed = total_uncompressed.saturating_add(uncomp);
        total_compressed = total_compressed.saturating_add(comp);

        if total_uncompressed > limits.max_total_uncompressed {
            return Err(ValidationError::ArchiveTooLarge);
        }

        // A nested archive is detected by a `.zip`/office extension on the
        // entry name; recurse into its stored bytes via the local header.
        if is_archive_name(name) {
            if let Some(inner) = stored_entry_bytes(bytes, local_offset) {
                inspect_archive(inner, limits, depth + 1)?;
            }
        }

        cursor = cursor
            .checked_add(46 + name_len + extra_len + comment_len)
            .ok_or(ValidationError::Corrupt)?;
    }

    // Expansion-ratio guard (Req 42.5). Guard against divide-by-zero: an
    // archive that declares zero compressed bytes but non-zero output is itself
    // suspicious.
    if total_compressed == 0 {
        if total_uncompressed > 0 {
            return Err(ValidationError::ArchiveExpansionTooHigh);
        }
    } else if total_uncompressed / total_compressed > limits.max_expansion_ratio {
        return Err(ValidationError::ArchiveExpansionTooHigh);
    }

    Ok(())
}

/// Find the EOCD record by scanning backwards for its signature.
fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    // The EOCD is within the last (22 + 65535) bytes; scan from the latest
    // possible position backwards.
    let min = bytes.len().saturating_sub(22 + 0xFFFF);
    for i in (min..=bytes.len() - 22).rev() {
        if bytes[i..i + 4] == EOCD_SIG {
            return Some(i);
        }
    }
    None
}

/// Whether an entry name denotes a nested archive.
fn is_archive_name(name: &[u8]) -> bool {
    let lower = name.to_ascii_lowercase();
    const EXTS: &[&[u8]] = &[b".zip", b".docx", b".xlsx", b".pptx", b".jar"];
    EXTS.iter().any(|e| lower.ends_with(e))
}

/// Return the stored (compressed) bytes of an entry given its local-header
/// offset, when the entry appears to itself be a ZIP archive we can recurse.
///
/// Only stored (uncompressed) nested archives can be recursed without a
/// decompressor; that is sufficient for the nesting-depth guard because a
/// nested *compressed* archive's declared uncompressed size is already counted
/// toward the total-size and ratio limits.
fn stored_entry_bytes(bytes: &[u8], local_offset: usize) -> Option<&[u8]> {
    let sig = bytes.get(local_offset..local_offset + 4)?;
    if sig != LFH_SIG {
        return None;
    }
    let method = read_u16(bytes, local_offset + 8)?;
    // 0 == stored (no compression). Only these are safe to recurse into.
    if method != 0 {
        return None;
    }
    let comp = read_u32(bytes, local_offset + 18)? as usize;
    let name_len = read_u16(bytes, local_offset + 26)? as usize;
    let extra_len = read_u16(bytes, local_offset + 28)? as usize;
    let data_start = local_offset + 30 + name_len + extra_len;
    let data = bytes.get(data_start..data_start + comp)?;
    // Only recurse if the stored payload is itself a ZIP.
    if data.starts_with(&LFH_SIG) || data.starts_with(&EOCD_SIG) {
        Some(data)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn pdf_bytes() -> Vec<u8> {
        b"%PDF-1.5\n...".to_vec()
    }

    #[test]
    fn detects_pdf_jpeg_png_text() {
        assert_eq!(detect_type(&pdf_bytes()), Some(DetectedType::Pdf));
        assert_eq!(detect_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(DetectedType::Jpeg));
        assert_eq!(
            detect_type(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            Some(DetectedType::Png)
        );
        assert_eq!(detect_type(b"# hello markdown"), Some(DetectedType::Text));
    }

    #[test]
    fn rejects_empty() {
        let err = validate_source(&[], &[DetectedType::Pdf], &ArchiveLimits::default());
        assert_eq!(err, Err(ValidationError::Empty));
    }

    #[test]
    fn rejects_type_mismatch() {
        // JPEG bytes offered to a PDF-only tool.
        let err = validate_source(
            &[0xFF, 0xD8, 0xFF, 0xE0],
            &[DetectedType::Pdf],
            &ArchiveLimits::default(),
        );
        assert_eq!(err, Err(ValidationError::UnsupportedType));
    }

    #[test]
    fn accepts_matching_type() {
        let ok = validate_source(&pdf_bytes(), &[DetectedType::Pdf], &ArchiveLimits::default());
        assert_eq!(ok, Ok(DetectedType::Pdf));
    }

    #[test]
    fn enforces_batch_count() {
        assert!(validate_batch_count(MAX_BATCH_COUNT).is_ok());
        assert_eq!(
            validate_batch_count(MAX_BATCH_COUNT + 1),
            Err(ValidationError::BatchTooLarge {
                max_count: MAX_BATCH_COUNT
            })
        );
    }

    /// Build a minimal single-entry ZIP with the given declared compressed and
    /// uncompressed sizes and an optional stored payload. The declared sizes
    /// intentionally may not match the payload — the bomb guard reads the
    /// *declared* sizes, exactly the attack surface a zip bomb exploits.
    pub(super) fn build_zip(entry_name: &[u8], comp: u32, uncomp: u32, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        // --- Local file header ---
        let lfh_offset = buf.len() as u32;
        buf.extend_from_slice(&LFH_SIG);
        buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
        buf.extend_from_slice(&0u16.to_le_bytes()); // flags
        buf.extend_from_slice(&0u16.to_le_bytes()); // method = stored
        buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
        buf.extend_from_slice(&0u16.to_le_bytes()); // mod date
        buf.extend_from_slice(&0u32.to_le_bytes()); // crc32
        buf.extend_from_slice(&comp.to_le_bytes());
        buf.extend_from_slice(&uncomp.to_le_bytes());
        buf.extend_from_slice(&(entry_name.len() as u16).to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // extra len
        buf.extend_from_slice(entry_name);
        buf.extend_from_slice(payload);

        // --- Central directory header ---
        let cd_offset = buf.len() as u32;
        buf.extend_from_slice(&CDH_SIG);
        buf.extend_from_slice(&20u16.to_le_bytes()); // version made by
        buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
        buf.extend_from_slice(&0u16.to_le_bytes()); // flags
        buf.extend_from_slice(&0u16.to_le_bytes()); // method
        buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
        buf.extend_from_slice(&0u16.to_le_bytes()); // mod date
        buf.extend_from_slice(&0u32.to_le_bytes()); // crc32
        buf.extend_from_slice(&comp.to_le_bytes());
        buf.extend_from_slice(&uncomp.to_le_bytes());
        buf.extend_from_slice(&(entry_name.len() as u16).to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // extra len
        buf.extend_from_slice(&0u16.to_le_bytes()); // comment len
        buf.extend_from_slice(&0u16.to_le_bytes()); // disk number
        buf.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        buf.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        buf.extend_from_slice(&lfh_offset.to_le_bytes()); // local header offset
        buf.extend_from_slice(entry_name);

        // --- End of central directory ---
        let cd_size = buf.len() as u32 - cd_offset;
        buf.extend_from_slice(&EOCD_SIG);
        buf.extend_from_slice(&0u16.to_le_bytes()); // disk number
        buf.extend_from_slice(&0u16.to_le_bytes()); // cd start disk
        buf.extend_from_slice(&1u16.to_le_bytes()); // entries on disk
        buf.extend_from_slice(&1u16.to_le_bytes()); // total entries
        buf.extend_from_slice(&cd_size.to_le_bytes());
        buf.extend_from_slice(&cd_offset.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // comment len
        buf
    }

    #[test]
    fn accepts_benign_archive() {
        // 1000 bytes compressed -> 2000 uncompressed: ratio 2, well under limit.
        let zip = build_zip(b"word/document.xml", 1000, 2000, &[0u8; 1000]);
        let ok = validate_source(&zip, &[DetectedType::OfficeZip], &ArchiveLimits::default());
        assert_eq!(ok, Ok(DetectedType::OfficeZip));
    }

    #[test]
    fn rejects_expansion_bomb() {
        // 10 compressed bytes claiming 10 MB uncompressed: ratio ~1e6.
        let zip = build_zip(b"bomb.xml", 10, 10_000_000, &[0u8; 10]);
        let err = validate_source(&zip, &[DetectedType::OfficeZip], &ArchiveLimits::default());
        assert_eq!(err, Err(ValidationError::ArchiveExpansionTooHigh));
    }

    #[test]
    fn rejects_oversized_archive() {
        let limits = ArchiveLimits {
            max_total_uncompressed: 1000,
            ..ArchiveLimits::default()
        };
        let zip = build_zip(b"big.xml", 900, 5000, &[0u8; 900]);
        let err = validate_source(&zip, &[DetectedType::OfficeZip], &limits);
        assert_eq!(err, Err(ValidationError::ArchiveTooLarge));
    }
}

// -------------------------------------------------------------------------
// Property 23: Archive-bomb inputs are rejected.
// -------------------------------------------------------------------------
#[cfg(test)]
mod property_tests {
    #![allow(clippy::unwrap_used)]

    use super::tests_support::build_zip_public;
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        // Feature: pdf-tools-suite, Property 23: Archive-bomb inputs are rejected.
        //
        // For all archive-based Source_Files whose declared uncompressed size
        // exceeds the configured limit OR whose expansion ratio exceeds the
        // configured ratio, the Validator rejects the Source_File.
        // Validates: Requirements 50.4, 42.5.
        #[test]
        fn archive_bombs_are_rejected(
            comp in 1u32..2_000u32,
            multiplier in 200u64..5_000u64,
        ) {
            let limits = ArchiveLimits::default();
            // Construct an entry whose declared uncompressed size is
            // `comp * multiplier`, guaranteeing the expansion ratio exceeds the
            // 100x limit (multiplier >= 200) — a classic zip bomb.
            let uncomp = (comp as u64).saturating_mul(multiplier);
            let uncomp_u32 = uncomp.min(u64::from(u32::MAX)) as u32;
            let zip = build_zip_public(b"bomb.xml", comp, uncomp_u32, comp as usize);

            let result = validate_source(&zip, &[DetectedType::OfficeZip], &limits);
            prop_assert!(
                matches!(
                    result,
                    Err(ValidationError::ArchiveExpansionTooHigh)
                        | Err(ValidationError::ArchiveTooLarge)
                ),
                "expected a bomb rejection, got {result:?}"
            );
        }
    }
}

/// Test-only helper re-exported so both the unit and property test modules can
/// build synthetic ZIPs.
#[cfg(test)]
mod tests_support {
    use super::tests;

    pub(super) fn build_zip_public(
        name: &[u8],
        comp: u32,
        uncomp: u32,
        payload_len: usize,
    ) -> Vec<u8> {
        tests::build_zip(name, comp, uncomp, &vec![0u8; payload_len])
    }
}

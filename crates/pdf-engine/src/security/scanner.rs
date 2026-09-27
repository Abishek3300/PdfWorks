//! PDF Content_Scanner: active-content detection and removal (Task 6.4).
//!
//! A malicious PDF can carry code that runs when the document is opened or a
//! link is clicked. Before a PDF is processed, the Content_Scanner must inspect
//! it for such active content and either strip that content or reject the file
//! (Req 39.5, 39.6, Property 16). Because this guarantee must hold identically
//! whether a PDF is processed in the browser (WASM) or on the server (native),
//! the scanner lives in the shared, I/O-free engine and walks the document with
//! the pure-Rust [`lopdf`] object model.
//!
//! The scanner recognizes:
//! - **Embedded JavaScript** — `/JavaScript` actions, the `/JS` action entry,
//!   an `/OpenAction` whose action is JavaScript, and the document-level
//!   `/Names /JavaScript` name tree.
//! - **Launch actions** — `/Launch` actions that run an external program.
//! - **Additional actions** — `/AA` action dictionaries (page/document event
//!   actions), a common carrier for the above.
//! - **Embedded executables** — `/EmbeddedFile` streams whose declared type is
//!   an executable (e.g. `application/x-msdownload`, `.exe`, `.bat`, ...).
//!
//! Two entry points are exposed:
//! - [`scan_pdf`] parses and reports what active content was found, without
//!   modifying the document.
//! - [`sanitize_pdf`] parses, strips all detected active content, and returns
//!   the cleaned PDF bytes (the "remove" branch of Req 39.6). Callers that
//!   prefer the "reject" branch can call [`scan_pdf`] first and refuse when
//!   [`ScanReport::has_active_content`] is true.

use lopdf::{Document, Object};

use crate::model::EngineError;
use crate::pdf;

/// A category of active content the scanner looks for (Req 39.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActiveContent {
    /// Embedded JavaScript (a `/JavaScript` action, a `/JS` entry, a JavaScript
    /// `/OpenAction`, or the `/Names /JavaScript` tree).
    JavaScript,
    /// A `/Launch` action that would run an external program.
    LaunchAction,
    /// An `/EmbeddedFile` stream whose declared type is executable.
    EmbeddedExecutable,
}

/// The result of scanning a PDF for active content (Req 39.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Distinct categories of active content that were detected.
    findings: Vec<ActiveContent>,
}

impl ScanReport {
    fn record(&mut self, kind: ActiveContent) {
        if !self.findings.contains(&kind) {
            self.findings.push(kind);
        }
    }

    /// The distinct categories of active content found.
    #[must_use]
    pub fn findings(&self) -> &[ActiveContent] {
        &self.findings
    }

    /// Whether any active content was detected.
    #[must_use]
    pub fn has_active_content(&self) -> bool {
        !self.findings.is_empty()
    }

    /// Whether a specific category was detected.
    #[must_use]
    pub fn contains(&self, kind: ActiveContent) -> bool {
        self.findings.contains(&kind)
    }
}

/// Dictionary keys that carry or reference active content and must be removed
/// from every object when sanitizing.
const ACTIVE_CONTENT_KEYS: &[&[u8]] = &[
    b"JavaScript", // JavaScript action / name-tree entry
    b"JS",         // JavaScript action code entry
    b"Launch",     // launch-action reference
    b"OpenAction", // document-open action (may be JS or Launch)
    b"AA",         // additional (event) actions dictionary
];

/// Scan `bytes` for active content without modifying the document (Req 39.5).
///
/// # Errors
///
/// Returns the same structured parse errors as [`pdf::parse`]
/// ([`EngineError::Empty`], [`EngineError::Protected`], [`EngineError::Corrupt`]).
pub fn scan_pdf(bytes: &[u8]) -> Result<ScanReport, EngineError> {
    let doc = pdf::parse(bytes)?;
    Ok(scan_document(&doc))
}

/// Borrow an object's dictionary, whether the object is a plain `Dictionary` or
/// a `Stream` (whose descriptor dictionary carries the same keys, e.g. an
/// `/EmbeddedFile` stream's `/Subtype`).
fn object_dict(obj: &Object) -> Option<&lopdf::Dictionary> {
    match obj {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

/// Mutable counterpart of [`object_dict`].
fn object_dict_mut(obj: &mut Object) -> Option<&mut lopdf::Dictionary> {
    match obj {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(stream) => Some(&mut stream.dict),
        _ => None,
    }
}

/// Detect the active content present in an already-parsed document.
fn scan_document(doc: &Document) -> ScanReport {
    let mut report = ScanReport::default();
    for obj in doc.objects.values() {
        let Some(dict) = object_dict(obj) else { continue };

        // Any JavaScript-bearing key.
        if dict.get(b"JS").is_ok() {
            report.record(ActiveContent::JavaScript);
        }
        if is_action_of_type(dict, b"JavaScript") {
            report.record(ActiveContent::JavaScript);
        }
        if dict.get(b"JavaScript").is_ok() && !is_action_of_type(dict, b"JavaScript") {
            // A /Names -> /JavaScript name tree entry in the catalog.
            report.record(ActiveContent::JavaScript);
        }

        // Launch actions.
        if is_action_of_type(dict, b"Launch") || dict.get(b"Launch").is_ok() {
            report.record(ActiveContent::LaunchAction);
        }

        // Embedded executables.
        if is_executable_embedded_file(dict) {
            report.record(ActiveContent::EmbeddedExecutable);
        }
    }
    report
}

/// Remove all active content from `bytes` and return the cleaned PDF bytes
/// (Req 39.6, "remove" branch).
///
/// Every object dictionary has its active-content keys deleted, executable
/// `/EmbeddedFile` streams are neutralized, and the resulting document is
/// re-serialized. Re-scanning the returned bytes reports no active content
/// (Property 16).
///
/// # Errors
///
/// Returns the parse errors of [`pdf::parse`], or [`EngineError::NoOutputProduced`]
/// if the cleaned document cannot be serialized.
pub fn sanitize_pdf(bytes: &[u8]) -> Result<Vec<u8>, EngineError> {
    let mut doc = pdf::parse(bytes)?;
    strip_active_content(&mut doc);
    pdf::serialize(&mut doc)
}

/// Strip every category of active content from a parsed document in place.
fn strip_active_content(doc: &mut Document) {
    // Collect ids first to avoid holding an immutable borrow while mutating.
    let ids: Vec<_> = doc.objects.keys().copied().collect();
    for id in ids {
        let Some(dict) = doc.get_object_mut(id).ok().and_then(object_dict_mut) else {
            continue;
        };

        // Neutralize an executable embedded file by clearing its subtype/type
        // hints so it is no longer a launchable payload.
        let is_exec = is_executable_embedded_file(dict);

        // If this object is itself a JavaScript or Launch action dictionary
        // (`/S /JavaScript` or `/S /Launch`), strip its action type so it is no
        // longer recognized or executed as active content.
        if is_action_of_type(dict, b"JavaScript") || is_action_of_type(dict, b"Launch") {
            dict.remove(b"S");
        }

        // Drop every active-content-bearing key outright.
        for key in ACTIVE_CONTENT_KEYS {
            dict.remove(key);
        }

        if is_exec {
            // Neutralize the embedded file so nothing treats it as an executable
            // payload: drop the executable mimetype hint, the embedded-file
            // reference (/EF), and the executable file name (/F). Removing the
            // object entirely could leave dangling references, so we strip only
            // the markers that make it active, leaving an inert descriptor.
            dict.remove(b"Subtype");
            dict.remove(b"EF");
            dict.remove(b"F");
        }
    }
}

/// Whether `dict` is an action dictionary (`/S <type>`) of the given action
/// type, e.g. `/S /JavaScript` or `/S /Launch`.
fn is_action_of_type(dict: &lopdf::Dictionary, action: &[u8]) -> bool {
    dict.get(b"S")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|name| name == action)
        .unwrap_or(false)
}

/// Whether `dict` describes an embedded file whose declared type is executable.
///
/// Recognizes an `/EmbeddedFile` stream (or a `/Filespec` referencing one) whose
/// `/Subtype` mimetype or `/F` file name indicates an executable payload.
fn is_executable_embedded_file(dict: &lopdf::Dictionary) -> bool {
    // The object must be an embedded-file stream or a file specification.
    let type_name = dict.get(b"Type").ok().and_then(|o| o.as_name().ok());
    let subtype = dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok());

    let is_embedded_file = matches!(type_name, Some(t) if t == b"EmbeddedFile")
        || matches!(type_name, Some(t) if t == b"Filespec")
        || dict.get(b"EF").is_ok();
    if !is_embedded_file {
        return false;
    }

    // Executable by declared mimetype (/Subtype is a PDF name of the mimetype
    // with '/' escaped as '#2F', so match on the trailing token).
    if let Some(sub) = subtype {
        if mimetype_is_executable(sub) {
            return true;
        }
    }

    // Executable by file-name extension on the /F entry.
    if let Some(name) = dict.get(b"F").ok().and_then(|o| o.as_str().ok()) {
        if filename_is_executable(name) {
            return true;
        }
    }

    false
}

/// Whether a PDF-name mimetype token denotes an executable payload.
fn mimetype_is_executable(name: &[u8]) -> bool {
    let lower = name.to_ascii_lowercase();
    // Match common executable mimetypes; the PDF name may carry the full
    // `application#2Fx-msdownload` form, so test on suffixes/substrings.
    const EXECUTABLE_MIMES: &[&[u8]] = &[
        b"x-msdownload",
        b"x-msdos-program",
        b"octet-stream",
        b"x-executable",
        b"x-sh",
        b"x-bat",
        b"vnd.microsoft.portable-executable",
    ];
    EXECUTABLE_MIMES.iter().any(|m| window_contains(&lower, m))
}

/// Whether a file name ends in an executable extension.
fn filename_is_executable(name: &[u8]) -> bool {
    let lower = name.to_ascii_lowercase();
    const EXECUTABLE_EXTS: &[&[u8]] = &[
        b".exe", b".bat", b".cmd", b".com", b".scr", b".msi", b".dll", b".sh", b".js", b".jar",
        b".ps1", b".vbs",
    ];
    EXECUTABLE_EXTS.iter().any(|ext| lower.ends_with(ext))
}

/// Substring test over byte slices.
fn window_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return needle.is_empty();
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    // Test code may unwrap/expect freely; the crate-wide deny is relaxed here.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use lopdf::{dictionary, Document, Object, Stream};

    /// Build a minimal valid one-page PDF, returning the doc and its catalog id.
    fn base_doc() -> (Document, lopdf::ObjectId) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1_i64,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        (doc, catalog_id)
    }

    fn serialize(mut doc: Document) -> Vec<u8> {
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        buf
    }

    fn pdf_with_javascript_open_action() -> Vec<u8> {
        let (mut doc, catalog_id) = base_doc();
        let action = doc.add_object(dictionary! {
            "Type" => "Action",
            "S" => "JavaScript",
            "JS" => Object::string_literal("app.alert('hi');"),
        });
        if let Ok(cat) = doc.get_object_mut(catalog_id).and_then(|o| o.as_dict_mut()) {
            cat.set("OpenAction", Object::Reference(action));
        }
        serialize(doc)
    }

    fn pdf_with_launch_action() -> Vec<u8> {
        let (mut doc, catalog_id) = base_doc();
        let action = doc.add_object(dictionary! {
            "Type" => "Action",
            "S" => "Launch",
            "F" => Object::string_literal("cmd.exe"),
        });
        if let Ok(cat) = doc.get_object_mut(catalog_id).and_then(|o| o.as_dict_mut()) {
            cat.set("OpenAction", Object::Reference(action));
        }
        serialize(doc)
    }

    fn pdf_with_embedded_executable() -> Vec<u8> {
        let (mut doc, catalog_id) = base_doc();
        let ef = doc.add_object(Stream::new(
            dictionary! { "Type" => "EmbeddedFile", "Subtype" => "x-msdownload" },
            b"MZ\x90\x00".to_vec(),
        ));
        let filespec = doc.add_object(dictionary! {
            "Type" => "Filespec",
            "F" => Object::string_literal("payload.exe"),
            "EF" => dictionary! { "F" => ef },
        });
        if let Ok(cat) = doc.get_object_mut(catalog_id).and_then(|o| o.as_dict_mut()) {
            cat.set(
                "Names",
                dictionary! {
                    "EmbeddedFiles" => dictionary! {
                        "Names" => vec![
                            Object::string_literal("payload.exe"),
                            Object::Reference(filespec),
                        ],
                    },
                },
            );
        }
        serialize(doc)
    }

    #[test]
    fn detects_javascript() {
        let report = scan_pdf(&pdf_with_javascript_open_action()).unwrap();
        assert!(report.contains(ActiveContent::JavaScript));
        assert!(report.has_active_content());
    }

    #[test]
    fn detects_launch_action() {
        let report = scan_pdf(&pdf_with_launch_action()).unwrap();
        assert!(report.contains(ActiveContent::LaunchAction));
    }

    #[test]
    fn detects_embedded_executable() {
        let report = scan_pdf(&pdf_with_embedded_executable()).unwrap();
        assert!(report.contains(ActiveContent::EmbeddedExecutable));
    }

    #[test]
    fn sanitize_removes_all_active_content() {
        for bytes in [
            pdf_with_javascript_open_action(),
            pdf_with_launch_action(),
            pdf_with_embedded_executable(),
        ] {
            let cleaned = sanitize_pdf(&bytes).unwrap();
            let report = scan_pdf(&cleaned).unwrap();
            assert!(
                !report.has_active_content(),
                "cleaned PDF still had active content: {report:?}"
            );
        }
    }

    #[test]
    fn clean_pdf_has_no_findings() {
        let (doc, _) = base_doc();
        let report = scan_pdf(&serialize(doc)).unwrap();
        assert!(!report.has_active_content());
    }
}

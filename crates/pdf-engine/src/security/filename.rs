//! Filename sanitization and unique-name assignment (Task 6.1).
//!
//! These helpers back two backend guarantees that must hold identically on the
//! WASM and native builds, so they live in the shared, I/O-free engine:
//!
//! - **Path-traversal safety (Req 50.1, Property 15):** an arbitrary, untrusted
//!   file-name string is reduced to a single safe path segment that carries no
//!   path separators (`/`, `\`) and no relative segments (`.`, `..`), so joining
//!   it to the File_Store directory can never escape that directory.
//! - **Unique output names (Req 49.3, Property 14):** when a Job produces
//!   multiple Output_Files that would otherwise share a name, each is assigned a
//!   distinct display name by inserting a numeric suffix before the extension.
//!
//! Both are pure string transforms with no I/O.

/// Fallback stem used when an input name reduces to nothing usable.
const DEFAULT_NAME: &str = "file";

/// Sanitize an untrusted file-name string into a single safe path segment.
///
/// The result is guaranteed to:
/// - contain no path separators (`/` or `\`),
/// - not be a relative path segment (`.` or `..`),
/// - be non-empty (falling back to [`DEFAULT_NAME`] when the input reduces to
///   empty, e.g. `""`, `"/"`, `"../.."`, or all-whitespace),
///
/// so that joining it onto the File_Store directory resolves strictly inside
/// that directory (Req 50.1, Property 15).
///
/// The strategy is deliberately conservative: rather than trying to "clean" a
/// path in place, it takes only the final component and rewrites every byte
/// that is a separator, control character, or otherwise unsafe for a display
/// name into `_`, then rejects the reserved `.`/`..` results.
#[must_use]
pub fn sanitize_filename(input: &str) -> String {
    // Take the last component after any `/` or `\`, mirroring how a traversal
    // string like "../../etc/passwd" collapses to its final segment.
    let last_component = input.rsplit(['/', '\\']).next().unwrap_or(input);

    // Rewrite unsafe bytes. We keep a conservative allowlist-by-rejection:
    // separators, path/control characters, and NUL become `_`. Everything else
    // (letters, digits, spaces, dots that are not the whole name, common
    // punctuation) is preserved so names stay recognizable.
    let mut cleaned = String::with_capacity(last_component.len());
    for ch in last_component.chars() {
        let safe = match ch {
            // Path separators must never survive (Req 50.1).
            '/' | '\\' => false,
            // Control characters (incl. NUL) are unsafe in a display name.
            c if c.is_control() => false,
            // Windows-reserved path characters, kept out to stay portable.
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => false,
            _ => true,
        };
        cleaned.push(if safe { ch } else { '_' });
    }

    // Trim leading/trailing whitespace and dots so results like ".", "..", or
    // "  " cannot become a relative segment or a hidden/dotted name.
    let trimmed = cleaned.trim().trim_matches('.').trim();

    if trimmed.is_empty() {
        return DEFAULT_NAME.to_string();
    }

    // Defense in depth: the two relative segments must never be returned.
    if trimmed == "." || trimmed == ".." {
        return DEFAULT_NAME.to_string();
    }

    trimmed.to_string()
}

/// Assign a unique display name to every entry of `names`, preserving order.
///
/// The first occurrence of a name is kept as-is; each subsequent collision (case
/// sensitive, exact match) gets a numeric suffix inserted before the file
/// extension, e.g. `report.pdf`, `report (2).pdf`, `report (3).pdf`. Names
/// without an extension are suffixed at the end: `page`, `page (2)`.
///
/// Every returned name is first passed through [`sanitize_filename`], so the
/// output is simultaneously path-traversal-safe (Req 50.1) and pairwise distinct
/// (Req 49.3, Property 14).
#[must_use]
pub fn assign_unique_names(names: &[String]) -> Vec<String> {
    use std::collections::HashSet;

    let mut used: HashSet<String> = HashSet::with_capacity(names.len());
    let mut out: Vec<String> = Vec::with_capacity(names.len());

    for name in names {
        let base = sanitize_filename(name);
        if used.insert(base.clone()) {
            out.push(base);
            continue;
        }

        // The base name is taken; find the smallest counter that is free.
        let (stem, ext) = split_extension(&base);
        let mut counter: u32 = 2;
        let candidate = loop {
            let candidate = match &ext {
                Some(ext) => format!("{stem} ({counter}).{ext}"),
                None => format!("{stem} ({counter})"),
            };
            if !used.contains(&candidate) {
                break candidate;
            }
            // Saturate rather than wrap so this always terminates.
            counter = counter.saturating_add(1);
            // With saturation the counter can stall at u32::MAX; break the tie
            // by appending an ever-growing marker so uniqueness still holds.
            if counter == u32::MAX {
                let mut forced = match &ext {
                    Some(ext) => format!("{stem} ({counter}).{ext}"),
                    None => format!("{stem} ({counter})"),
                };
                while used.contains(&forced) {
                    forced.push('_');
                }
                break forced;
            }
        };

        used.insert(candidate.clone());
        out.push(candidate);
    }

    out
}

/// Split a sanitized single-segment name into `(stem, extension)`.
///
/// The extension is the text after the final `.` when that `.` is neither the
/// first character (so dotfiles like `.env` keep their whole name as the stem)
/// nor produces an empty extension. Returns `None` for the extension when the
/// name has no usable extension.
fn split_extension(name: &str) -> (String, Option<String>) {
    match name.rfind('.') {
        Some(idx) if idx > 0 && idx + 1 < name.len() => {
            (name[..idx].to_string(), Some(name[idx + 1..].to_string()))
        }
        _ => (name.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    // Test code may unwrap/expect freely; the crate-wide deny is relaxed here.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn strips_forward_slash_traversal() {
        // "../../etc/passwd" collapses to its final component with no separators.
        let out = sanitize_filename("../../etc/passwd");
        assert_eq!(out, "passwd");
        assert!(!out.contains('/') && !out.contains('\\'));
    }

    #[test]
    fn strips_backslash_traversal() {
        let out = sanitize_filename("..\\..\\Windows\\system32\\cmd.exe");
        assert_eq!(out, "cmd.exe");
        assert!(!out.contains('/') && !out.contains('\\'));
    }

    #[test]
    fn dot_and_dotdot_fall_back_to_default() {
        assert_eq!(sanitize_filename("."), DEFAULT_NAME);
        assert_eq!(sanitize_filename(".."), DEFAULT_NAME);
        assert_eq!(sanitize_filename("../.."), DEFAULT_NAME);
        assert_eq!(sanitize_filename("/"), DEFAULT_NAME);
        assert_eq!(sanitize_filename(""), DEFAULT_NAME);
        assert_eq!(sanitize_filename("   "), DEFAULT_NAME);
    }

    #[test]
    fn preserves_a_normal_name() {
        assert_eq!(sanitize_filename("report.pdf"), "report.pdf");
    }

    #[test]
    fn rewrites_reserved_and_control_characters() {
        let out = sanitize_filename("a:b*c?.pdf");
        assert!(!out.contains(':') && !out.contains('*') && !out.contains('?'));
    }

    #[test]
    fn assigns_unique_names_to_collisions() {
        let names = vec![
            "report.pdf".to_string(),
            "report.pdf".to_string(),
            "report.pdf".to_string(),
        ];
        let out = assign_unique_names(&names);
        assert_eq!(out, vec!["report.pdf", "report (2).pdf", "report (3).pdf"]);
    }

    #[test]
    fn assigns_unique_names_without_extension() {
        let names = vec!["page".to_string(), "page".to_string()];
        let out = assign_unique_names(&names);
        assert_eq!(out, vec!["page", "page (2)"]);
    }

    #[test]
    fn distinct_names_are_left_untouched() {
        let names = vec!["a.pdf".to_string(), "b.pdf".to_string()];
        assert_eq!(assign_unique_names(&names), names);
    }
}

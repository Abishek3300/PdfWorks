//! WASM binding layer (Task 7.1, Req 37.5, 47.1).
//!
//! This module is compiled **only** for the `wasm32` target (it is gated behind
//! `#[cfg(target_family = "wasm")]` in `lib.rs`), so the native build and the
//! native `cargo test -p pdf-engine` never see `wasm-bindgen` and are completely
//! unaffected.
//!
//! It exposes a single JS-callable entry point that delegates to the pure,
//! I/O-free [`crate::run`] function. The binding is intentionally **thin**: it
//! only marshals JavaScript values into an [`EngineInput`] and marshals the
//! [`EngineOutput`] (or a mapped [`EngineError`]) back out. The tool id and
//! options arrive as a JSON string (parsed with `serde_json`) and the file
//! bytes cross as `Uint8Array` values, so no PDF bytes ever pass through a JSON
//! string. All PDF logic lives
//! in the shared engine so the WASM (Client_Side_Processing) and native
//! (Server_Side_Processing) planes run byte-for-byte identical code.
//!
//! ## JS contract
//!
//! ```js
//! import init, { run_tool } from "./pdf_engine.js";
//! await init();
//! // request: { tool: "Merge", options: { Merge: { order: [] } }, sourceNames: ["a.pdf","b.pdf"] }
//! // sources: Uint8Array[] (one per source, same order as sourceNames)
//! const outputs = run_tool(JSON.stringify(request), sources);
//! // outputs: Array<{ name: string, bytes: Uint8Array }>
//! ```
//!
//! On failure `run_tool` throws a JS `Error` whose message is the mapped,
//! human-readable [`EngineError`] text (never a panic — the engine is panic-free
//! and this layer surfaces structured errors as JS exceptions).

use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::model::{EngineError, EngineInput, FileBytes, ToolId, ToolOptions};

/// The JSON request payload accepted by [`run_tool`].
///
/// The source **bytes** are passed separately as a `Uint8Array[]` (so large
/// buffers never round-trip through a JSON string); this struct carries only the
/// tool selection, its options, and the per-source display names.
#[derive(Deserialize)]
struct ToolRequest {
    /// Which Tool to run.
    tool: ToolId,
    /// Tool-specific options (the serde-tagged [`ToolOptions`] union).
    options: ToolOptions,
    /// Original (untrusted) file names, one per source, in source order.
    /// Optional: when omitted, positional placeholder names are used.
    #[serde(default)]
    source_names: Vec<String>,
}

/// Run a Tool entirely on-device and return its Output_File(s).
///
/// `request_json` is a JSON string matching [`ToolRequest`]; `sources` is the
/// list of Source_File byte buffers (one `Uint8Array` per source, in the same
/// order as `source_names`).
///
/// Returns a JS array of `{ name: string, bytes: Uint8Array }` on success.
///
/// # Errors
///
/// Throws a JS `Error` when the request JSON is malformed or when the engine
/// rejects the Job; the message is the mapped [`EngineError`] text.
#[wasm_bindgen]
pub fn run_tool(request_json: &str, sources: Vec<js_sys::Uint8Array>) -> Result<JsValue, JsValue> {
    // Parse the request envelope (tool + options + names). A malformed payload
    // becomes a JS error rather than a panic.
    let request: ToolRequest = serde_json::from_str(request_json)
        .map_err(|e| js_error(&format!("invalid request payload: {e}")))?;

    // Copy each Uint8Array into an owned Vec<u8>; the engine borrows these.
    let owned: Vec<Vec<u8>> = sources.iter().map(js_sys::Uint8Array::to_vec).collect();

    // Build the borrowed FileBytes slice list the engine expects. Names default
    // to positional placeholders when the caller supplied fewer than sources.
    let files: Vec<FileBytes<'_>> = owned
        .iter()
        .enumerate()
        .map(|(i, bytes)| FileBytes {
            name: request
                .source_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("source_{}", i + 1)),
            bytes: bytes.as_slice(),
        })
        .collect();

    let input = EngineInput {
        tool: request.tool,
        sources: files,
        options: request.options,
    };

    // Delegate to the pure engine and map its result to JS values.
    match crate::run(input) {
        Ok(output) => outputs_to_js(&output.files),
        Err(err) => Err(map_engine_error(&err)),
    }
}

/// Convert the produced Output_Files into a JS array of `{ name, bytes }`.
fn outputs_to_js(files: &[crate::model::OutputFile]) -> Result<JsValue, JsValue> {
    let array = js_sys::Array::new();
    for file in files {
        let obj = js_sys::Object::new();
        set_prop(&obj, "name", &JsValue::from_str(&file.name))?;
        // Copy the bytes into a fresh Uint8Array owned by JS.
        let bytes = js_sys::Uint8Array::new_with_length(file.bytes.len() as u32);
        bytes.copy_from(&file.bytes);
        set_prop(&obj, "bytes", &bytes.into())?;
        array.push(&obj);
    }
    Ok(array.into())
}

/// Set an own property on a JS object, surfacing any reflect failure as an error.
fn set_prop(obj: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), JsValue> {
    js_sys::Reflect::set(obj, &JsValue::from_str(key), value)?;
    Ok(())
}

/// Map a structured [`EngineError`] to a JS `Error`. The message is the engine's
/// own `Display` text (via `thiserror`), so the UI can surface a clear,
/// tool-agnostic reason without leaking internals.
fn map_engine_error(err: &EngineError) -> JsValue {
    js_error(&err.to_string())
}

/// Build a JS `Error` value carrying `message`.
fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

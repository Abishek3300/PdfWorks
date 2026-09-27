// Feature: pdf-tools-suite (Task 7.2, Req 47.1, 47.2, 32.6)
//
// TypeScript mirror of the shared Rust engine's public option/result shapes
// (`crates/pdf-engine/src/model.rs`). These types describe the JSON payload the
// WASM entry point `run_tool` expects and the Output_File shape it returns, so
// the UI can build a request and consume results in a type-safe way without
// depending on the generated wasm-bindgen glue for typing.
//
// serde serializes the Rust `ToolOptions` enum "externally tagged": each variant
// is an object with a single key equal to the variant name whose value holds the
// variant's fields (e.g. `{ "Merge": { "order": [] } }`). The `ToolOptions`
// union below matches that wire format exactly.

/** Stable tool identifiers, mirroring `pdf-engine::ToolId`. */
export type EngineToolId =
	| 'Merge'
	| 'Split'
	| 'RemovePages'
	| 'ExtractPages'
	| 'Organize'
	| 'OptimizePdf'
	| 'CompressPdf'
	| 'JpgToPdf'
	| 'PdfToJpg'
	| 'Rotate'
	| 'AddPageNumbers'
	| 'AddWatermark'
	| 'Crop'
	| 'MarkdownToPdf'
	| 'PdfToMarkdown'
	| 'EditPdf'
	| 'PdfForms'
	// Server_Only tools are declared for completeness but are never dispatched
	// through the client-side WASM path (`run_tool` returns Unsupported for them).
	| 'WordToPdf'
	| 'PptToPdf'
	| 'ExcelToPdf'
	| 'HtmlToPdf'
	| 'PdfToWord'
	| 'PdfToPptx'
	| 'PdfToExcel'
	| 'PdfToPdfA'
	| 'ScanToPdf';

export type Level = 'Low' | 'Medium' | 'High';
export type Angle = 'D90' | 'D180' | 'D270';
export type Orientation = 'Portrait' | 'Landscape';
export type Margin = 'None' | 'Small' | 'Large';
export type Position =
	| 'TopLeft'
	| 'TopCenter'
	| 'TopRight'
	| 'BottomLeft'
	| 'BottomCenter'
	| 'BottomRight';

/** `PageScope` mirrors the Rust externally-tagged enum. */
export type PageScope = 'All' | { Pages: number[] };

export interface Rect {
	x: number;
	y: number;
	width: number;
	height: number;
}

/**
 * Tool-specific options. Externally-tagged union matching the serde wire format
 * of `pdf-engine::ToolOptions`. Only the `Client_Capable` variants are meant to
 * flow through the client-side path.
 */
export type ToolOptions =
	| { Merge: { order: number[] } }
	| { Split: { split_points: number[]; fixed_size: number | null } }
	| { RemovePages: { pages: number[] } }
	| { ExtractPages: { pages: number[] } }
	| { Organize: { order: number[]; rotations: [number, Angle][]; deletes: number[] } }
	| { Optimize: { level: Level } }
	| { Compress: { level: Level } }
	| { JpgToPdf: { orientation: Orientation; margin: Margin; order: number[] } }
	| { PdfToJpg: { dpi: number } }
	| { Rotate: { angle: Angle; pages: PageScope } }
	| { AddPageNumbers: { position: Position; start: number } }
	| {
			AddWatermark: {
				text: string | null;
				image: null;
				opacity: number;
				rotation_deg: number;
			};
	  }
	| { Crop: { region: Rect; all_pages: boolean } }
	| { MarkdownToPdf: { text: string } }
	| { PdfToMarkdown: Record<string, never> }
	| { EditPdf: { elements: unknown[] } }
	| { PdfForms: { field_values: [string, string][]; added_fields: unknown[] } };

/** The JSON request envelope accepted by the WASM `run_tool` entry point. */
export interface ToolRequest {
	tool: EngineToolId;
	options: ToolOptions;
	/** Original file names, one per source, in source order. */
	source_names?: string[];
}

/** A produced Output_File as returned by the engine. */
export interface OutputFile {
	name: string;
	bytes: Uint8Array;
}

// ---- Web Worker message protocol (main thread <-> worker) ----

/** Request posted to the Web Worker to run a Tool on-device. */
export interface WorkerRequest {
	/** Correlates the response with this request. */
	id: number;
	toolId: EngineToolId;
	options: ToolOptions;
	/** Source_File bytes, one buffer per source. Transferred, never copied. */
	sources: ArrayBuffer[];
	/** Original file names, one per source, in source order. */
	sourceNames?: string[];
}

/** Successful worker response carrying the Output_File bytes. */
export interface WorkerSuccess {
	id: number;
	ok: true;
	outputs: OutputFile[];
}

/** Failed worker response carrying a human-readable reason. */
export interface WorkerFailure {
	id: number;
	ok: false;
	error: string;
}

export type WorkerResponse = WorkerSuccess | WorkerFailure;

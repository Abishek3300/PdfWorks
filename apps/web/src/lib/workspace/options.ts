// Feature: pdf-tools-suite (Task 10.3, 11.1) — per-tool option UI state.
//
// `ToolOptionState` is a permissive bag holding whatever a tool's option panel
// needs. `buildEngineOptions` translates that UI state into the engine's
// externally-tagged `ToolOptions` wire format for the Client_Side path, and
// `toEngineToolId` maps the registry ToolId to the engine's ToolId.

import type { ToolId } from '../tools/registry';
import type {
	EngineToolId,
	ToolOptions,
	OutputFile,
	Level,
	Angle,
	Orientation,
	Margin,
	Position
} from '../engine/types';
import { fileExtension } from '../util/format';
import { sanitizeFileName } from '../util/sanitize';

/** Loose per-tool option state maintained by the option panels. */
export interface ToolOptionState {
	// Ordering (Merge, JPG to PDF, Organize, Scan)
	order?: number[];
	// Split
	splitMode?: 'points' | 'fixed';
	splitPoints?: number[];
	fixedSize?: number;
	// Page selection (Remove/Extract)
	pages?: number[];
	// Organize
	rotations?: [number, Angle][];
	deletes?: number[];
	// Optimize / Compress
	level?: Level;
	// JPG to PDF
	orientation?: Orientation;
	margin?: Margin;
	// PDF to JPG
	dpi?: number;
	// Rotate
	angle?: Angle;
	rotateScope?: 'all' | 'selected';
	// Add Page Numbers
	position?: Position;
	startNumber?: number;
	// Watermark
	watermarkText?: string;
	opacity?: number;
	rotationDeg?: number;
	// Crop
	crop?: { x: number; y: number; width: number; height: number };
	cropAllPages?: boolean;
	// Markdown
	markdownText?: string;
	// HTML (server-only)
	htmlUrl?: string;
	// PDF/A (server-only)
	pdfaLevel?: 'A1b' | 'A2b' | 'A3b';
	// Edit / Forms
	elements?: unknown[];
	fieldValues?: [string, string][];
	addedFields?: unknown[];
	// Scan (server-only)
	ocr?: boolean;
}

/** Sensible defaults for a tool's option state. */
export function defaultOptions(toolId: ToolId, sourceCount = 0): ToolOptionState {
	const order = Array.from({ length: sourceCount }, (_, i) => i);
	switch (toolId) {
		case 'Merge':
		case 'JpgToPdf':
		case 'Organize':
		case 'ScanToPdf':
			return { order, orientation: 'Portrait', margin: 'Small', rotations: [], deletes: [], ocr: false };
		case 'Split':
			return { splitMode: 'points', splitPoints: [], fixedSize: 1 };
		case 'RemovePages':
		case 'ExtractPages':
			return { pages: [] };
		case 'OptimizePdf':
		case 'CompressPdf':
			return { level: 'Medium' };
		case 'PdfToJpg':
			return { dpi: 150 };
		case 'Rotate':
			return { angle: 'D90', rotateScope: 'all', pages: [] };
		case 'AddPageNumbers':
			return { position: 'BottomCenter', startNumber: 1 };
		case 'AddWatermark':
			return { watermarkText: 'CONFIDENTIAL', opacity: 30, rotationDeg: 45 };
		case 'Crop':
			return { crop: { x: 0, y: 0, width: 100, height: 100 }, cropAllPages: true };
		case 'MarkdownToPdf':
			return { markdownText: '' };
		case 'HtmlToPdf':
			return { htmlUrl: '', orientation: 'Portrait' };
		case 'ExcelToPdf':
			return { orientation: 'Portrait' };
		case 'PdfToPdfA':
			return { pdfaLevel: 'A2b' };
		case 'EditPdf':
			return { elements: [] };
		case 'PdfForms':
			return { fieldValues: [], addedFields: [] };
		default:
			return {};
	}
}

/** Map a registry ToolId to the engine's ToolId (client-capable subset). */
export function toEngineToolId(toolId: ToolId): EngineToolId {
	// The registry and engine ids match for every client-capable tool.
	return toolId as EngineToolId;
}

/**
 * Build the engine wire `ToolOptions` for a Client_Capable tool from UI state.
 * Throws for tools that never run client-side.
 */
export function buildEngineOptions(toolId: ToolId, state: ToolOptionState, sourceCount: number): ToolOptions {
	const order = state.order && state.order.length === sourceCount ? state.order : Array.from({ length: sourceCount }, (_, i) => i);
	switch (toolId) {
		case 'Merge':
			return { Merge: { order } };
		case 'Split':
			return {
				Split: {
					split_points: state.splitMode === 'points' ? [...(state.splitPoints ?? [])] : [],
					fixed_size: state.splitMode === 'fixed' ? state.fixedSize ?? null : null
				}
			};
		case 'RemovePages':
			return { RemovePages: { pages: [...(state.pages ?? [])] } };
		case 'ExtractPages':
			return { ExtractPages: { pages: [...(state.pages ?? [])] } };
		case 'Organize':
			return {
				Organize: {
					order,
					rotations: [...(state.rotations ?? [])],
					deletes: [...(state.deletes ?? [])]
				}
			};
		case 'OptimizePdf':
			return { Optimize: { level: state.level ?? 'Medium' } };
		case 'CompressPdf':
			return { Compress: { level: state.level ?? 'Medium' } };
		case 'JpgToPdf':
			return {
				JpgToPdf: {
					orientation: state.orientation ?? 'Portrait',
					margin: state.margin ?? 'Small',
					order
				}
			};
		case 'PdfToJpg':
			return { PdfToJpg: { dpi: state.dpi ?? 150 } };
		case 'Rotate':
			return {
				Rotate: {
					angle: state.angle ?? 'D90',
					pages:
						state.rotateScope === 'selected'
							? { Pages: [...(state.pages ?? [])] }
							: 'All'
				}
			};
		case 'AddPageNumbers':
			return {
				AddPageNumbers: {
					position: state.position ?? 'BottomCenter',
					start: state.startNumber ?? 1
				}
			};
		case 'AddWatermark':
			return {
				AddWatermark: {
					text: state.watermarkText && state.watermarkText.length > 0 ? state.watermarkText : null,
					image: null,
					opacity: clampInt(state.opacity ?? 30, 0, 100),
					rotation_deg: clampInt(state.rotationDeg ?? 0, -360, 360)
				}
			};
		case 'Crop': {
			const c = state.crop ?? { x: 0, y: 0, width: 100, height: 100 };
			return { Crop: { region: { ...c }, all_pages: state.cropAllPages ?? true } };
		}
		case 'MarkdownToPdf':
			return { MarkdownToPdf: { text: state.markdownText ?? '' } };
		case 'PdfToMarkdown':
			return { PdfToMarkdown: {} };
		case 'EditPdf':
			return { EditPdf: { elements: [...(state.elements ?? [])] } };
		case 'PdfForms':
			return {
				PdfForms: {
					field_values: [...(state.fieldValues ?? [])],
					added_fields: [...(state.addedFields ?? [])]
				}
			};
		default:
			throw new Error(`${toolId} does not run on-device.`);
	}
}

function clampInt(value: number, min: number, max: number): number {
	return Math.max(min, Math.min(max, Math.round(value)));
}

/**
 * Per-tool operation suffix applied to renamed Output_Files (Improvement 1).
 * The word describes what the tool did, so a source `report.pdf` becomes e.g.
 * `report_removed.pdf` through Remove Pages.
 */
const OPERATION_SUFFIX: Record<ToolId, string> = {
	// Organize
	Merge: 'merged',
	Split: 'split',
	RemovePages: 'removed',
	ExtractPages: 'extracted',
	Organize: 'organized',
	// Scan & Optimize
	ScanToPdf: 'scanned',
	OptimizePdf: 'optimized',
	CompressPdf: 'compressed',
	// Convert to PDF
	JpgToPdf: 'converted',
	MarkdownToPdf: 'document',
	WordToPdf: 'converted',
	PptToPdf: 'converted',
	ExcelToPdf: 'converted',
	HtmlToPdf: 'converted',
	// Convert from PDF
	PdfToJpg: 'page',
	PdfToMarkdown: 'markdown',
	PdfToWord: 'converted',
	PdfToPptx: 'converted',
	PdfToExcel: 'converted',
	PdfToPdfA: 'pdfa',
	// Edit
	Rotate: 'rotated',
	AddPageNumbers: 'numbered',
	AddWatermark: 'watermarked',
	Crop: 'cropped',
	EditPdf: 'edited',
	PdfForms: 'form'
};

/**
 * Fallback base names for tools that have no uploaded Source_File to derive a
 * name from (e.g. Markdown to PDF types content, HTML to PDF fetches a URL).
 */
const FALLBACK_BASE: Partial<Record<ToolId, string>> = {
	MarkdownToPdf: 'document',
	HtmlToPdf: 'webpage'
};

/** Strip the trailing `.<ext>` from a file name, yielding just the base. */
function baseNameOf(name: string): string {
	const ext = fileExtension(name);
	if (ext === '') return name;
	return name.slice(0, name.length - (ext.length + 1));
}

/**
 * Rename engine Output_Files to `<originalBaseName>_<operation><ext>`
 * (Improvement 1). The base name comes from the FIRST Source_File; tools with
 * no source fall back to a sensible name (or the engine output's own base).
 * Each output keeps its ORIGINAL extension. Multiple outputs get a `_<n>`
 * index (starting at 1) so they stay distinguishable and unique. Final names
 * are sanitized to stay path-safe. Pure.
 */
export function renameOutputs(
	toolId: ToolId,
	sourceNames: string[],
	outputs: OutputFile[]
): OutputFile[] {
	const suffix = OPERATION_SUFFIX[toolId] ?? 'output';
	const firstSource = sourceNames.find((n) => n && n.trim() !== '');
	const multiple = outputs.length > 1;

	return outputs.map((output, i) => {
		const ext = fileExtension(output.name);
		const dotExt = ext === '' ? '' : `.${ext}`;
		// Base: from the first source, else a per-tool fallback, else the
		// engine output's own base name.
		const base = firstSource
			? baseNameOf(firstSource)
			: FALLBACK_BASE[toolId] ?? baseNameOf(output.name) ?? 'file';

		const stem = multiple ? `${base}_${suffix}_${i + 1}` : `${base}_${suffix}`;
		return { ...output, name: sanitizeFileName(`${stem}${dotExt}`) };
	});
}

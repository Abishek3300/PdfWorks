// Feature: pdf-tools-suite
//
// Declarative tool registry shared in shape between the TypeScript UI and the
// Rust backend. It is the single source of truth for tool catalog rendering
// (Req 1.1, 1.2, 1.6), format validation (Req 2.8, 33.1, 39.1), and Privacy
// Mode / Processing_Mode routing (Req 37.3, 37.4, 37.7). See design.md
// "Components and Interfaces — 2. Tool registry".

/**
 * Stable tool identifiers. The Client_Capable subset mirrors the Rust
 * `pdf-engine::ToolId` enum; the Server_Only tools (Office <-> PDF, PDF/A,
 * OCR Scan-to-PDF) are executed by the server layer via LibreOffice/OCR/pdfium
 * and are not part of the shared engine's execution path.
 */
export type ToolId =
	// Organize
	| 'Merge'
	| 'Split'
	| 'RemovePages'
	| 'ExtractPages'
	| 'Organize'
	// Scan & Optimize
	| 'ScanToPdf'
	| 'OptimizePdf'
	| 'CompressPdf'
	// Convert to PDF
	| 'JpgToPdf'
	| 'MarkdownToPdf'
	| 'WordToPdf'
	| 'PptToPdf'
	| 'ExcelToPdf'
	| 'HtmlToPdf'
	// Convert from PDF
	| 'PdfToJpg'
	| 'PdfToMarkdown'
	| 'PdfToWord'
	| 'PdfToPptx'
	| 'PdfToExcel'
	| 'PdfToPdfA'
	// Edit
	| 'Rotate'
	| 'AddPageNumbers'
	| 'AddWatermark'
	| 'Crop'
	| 'EditPdf'
	| 'PdfForms';

/** The five Tool_Categories (Req 1.1). */
export type ToolCategory = 'Organize' | 'ScanOptimize' | 'ConvertTo' | 'ConvertFrom' | 'Edit';

/**
 * Whether a tool can run entirely in the browser (`Client_Capable`) or must be
 * processed by the server (`Server_Only`). Drives Privacy Mode routing
 * (Req 37.4 / 37.7).
 */
export type Capability = 'Client_Capable' | 'Server_Only';

/** The execution location for a Job, derived from Privacy_Mode + capability. */
export type ProcessingMode = 'Client_Side' | 'Server_Side';

export interface ToolDescriptor {
	id: ToolId;
	category: ToolCategory;
	/** Descriptive, human-friendly label stating what the tool does (Req 1.6). */
	label: string;
	/** Routing capability (Req 37.4 / 37.7). */
	capability: Capability;
	/** Accepted input formats for validation (Req 2.8 / 33.1 / 39.1). */
	supportedFormats: string[];
	/** Minimum number of Source_Files, where relevant (e.g. Merge >= 2, Req 4.4). */
	minSources?: number;
	/** True when the tool can produce more than one Output_File (Req 3.2, 18.3). */
	producesMultiple?: boolean;
}

/**
 * The declarative registry table. This is the single source of truth for the
 * tool catalog. `Server_Only` tools are exactly, per the glossary: Word to PDF,
 * PowerPoint to PDF, Excel to PDF, PDF to Word, PDF to PowerPoint, PDF to Excel,
 * PDF to PDF/A, and Scan to PDF when OCR text is requested. All others are
 * `Client_Capable`.
 */
export const TOOL_REGISTRY: readonly ToolDescriptor[] = [
	// ----- Organize PDF (all Client_Capable) -----
	{
		id: 'Merge',
		category: 'Organize',
		label: 'Merge PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf'],
		minSources: 2
	},
	{
		id: 'Split',
		category: 'Organize',
		label: 'Split PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf'],
		producesMultiple: true
	},
	{
		id: 'RemovePages',
		category: 'Organize',
		label: 'Remove Pages',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'ExtractPages',
		category: 'Organize',
		label: 'Extract Pages',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'Organize',
		category: 'Organize',
		label: 'Organize PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},

	// ----- Scan & Optimize -----
	{
		// Scan to PDF is Server_Only when OCR text is requested (per glossary).
		id: 'ScanToPdf',
		category: 'ScanOptimize',
		label: 'Scan to PDF',
		capability: 'Server_Only',
		supportedFormats: ['jpg', 'jpeg', 'png']
	},
	{
		id: 'OptimizePdf',
		category: 'ScanOptimize',
		label: 'Optimize PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'CompressPdf',
		category: 'ScanOptimize',
		label: 'Compress PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},

	// ----- Convert to PDF -----
	{
		id: 'JpgToPdf',
		category: 'ConvertTo',
		label: 'JPG to PDF',
		capability: 'Client_Capable',
		supportedFormats: ['jpg', 'jpeg']
	},
	{
		id: 'MarkdownToPdf',
		category: 'ConvertTo',
		label: 'Markdown to PDF',
		capability: 'Client_Capable',
		supportedFormats: ['md', 'markdown']
	},
	{
		id: 'WordToPdf',
		category: 'ConvertTo',
		label: 'Word to PDF',
		capability: 'Server_Only',
		supportedFormats: ['doc', 'docx']
	},
	{
		id: 'PptToPdf',
		category: 'ConvertTo',
		label: 'PowerPoint to PDF',
		capability: 'Server_Only',
		supportedFormats: ['ppt', 'pptx']
	},
	{
		id: 'ExcelToPdf',
		category: 'ConvertTo',
		label: 'Excel to PDF',
		capability: 'Server_Only',
		supportedFormats: ['xls', 'xlsx']
	},
	{
		id: 'HtmlToPdf',
		category: 'ConvertTo',
		label: 'HTML to PDF',
		capability: 'Server_Only',
		supportedFormats: ['html', 'htm']
	},

	// ----- Convert from PDF -----
	{
		id: 'PdfToJpg',
		category: 'ConvertFrom',
		label: 'PDF to JPG',
		capability: 'Client_Capable',
		supportedFormats: ['pdf'],
		producesMultiple: true
	},
	{
		id: 'PdfToMarkdown',
		category: 'ConvertFrom',
		label: 'PDF to Markdown',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'PdfToWord',
		category: 'ConvertFrom',
		label: 'PDF to Word',
		capability: 'Server_Only',
		supportedFormats: ['pdf']
	},
	{
		id: 'PdfToPptx',
		category: 'ConvertFrom',
		label: 'PDF to PowerPoint',
		capability: 'Server_Only',
		supportedFormats: ['pdf']
	},
	{
		id: 'PdfToExcel',
		category: 'ConvertFrom',
		label: 'PDF to Excel',
		capability: 'Server_Only',
		supportedFormats: ['pdf']
	},
	{
		id: 'PdfToPdfA',
		category: 'ConvertFrom',
		label: 'PDF to PDF/A',
		capability: 'Server_Only',
		supportedFormats: ['pdf']
	},

	// ----- Edit PDF (all Client_Capable) -----
	{
		id: 'Rotate',
		category: 'Edit',
		label: 'Rotate PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'AddPageNumbers',
		category: 'Edit',
		label: 'Add Page Numbers',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'AddWatermark',
		category: 'Edit',
		label: 'Add Watermark',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'Crop',
		category: 'Edit',
		label: 'Crop PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'EditPdf',
		category: 'Edit',
		label: 'Edit PDF',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	},
	{
		id: 'PdfForms',
		category: 'Edit',
		label: 'PDF Forms',
		capability: 'Client_Capable',
		supportedFormats: ['pdf']
	}
];

/** Display order and human-friendly names for the five Tool_Categories (Req 1.1). */
export const CATEGORY_LABELS: Readonly<Record<ToolCategory, string>> = {
	Organize: 'Organize PDF',
	ScanOptimize: 'Scan & Optimize',
	ConvertTo: 'Convert to PDF',
	ConvertFrom: 'Convert from PDF',
	Edit: 'Edit PDF'
};

const CATEGORY_ORDER: readonly ToolCategory[] = [
	'Organize',
	'ScanOptimize',
	'ConvertTo',
	'ConvertFrom',
	'Edit'
];

export interface ToolCategoryGroup {
	category: ToolCategory;
	label: string;
	tools: ToolDescriptor[];
}

/**
 * Return every tool grouped by its category, in canonical category order
 * (Req 1.1, 1.2). Each category includes every tool that belongs to it.
 */
export function getToolsByCategory(): ToolCategoryGroup[] {
	return CATEGORY_ORDER.map((category) => ({
		category,
		label: CATEGORY_LABELS[category],
		tools: TOOL_REGISTRY.filter((tool) => tool.category === category)
	}));
}

/** Look up a single descriptor by id. */
export function getTool(id: ToolId): ToolDescriptor | undefined {
	return TOOL_REGISTRY.find((tool) => tool.id === id);
}

/**
 * Derive the Processing_Mode from the Privacy_Mode setting and a tool's
 * capability (Req 37.3, 37.4, 37.7). A `Server_Only` tool is always
 * `Server_Side`; a `Client_Capable` tool is `Client_Side` only when Privacy
 * Mode is enabled.
 */
export function deriveProcessingMode(
	privacyMode: boolean,
	capability: Capability
): ProcessingMode {
	return privacyMode && capability === 'Client_Capable' ? 'Client_Side' : 'Server_Side';
}

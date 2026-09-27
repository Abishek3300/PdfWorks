// Feature: pdf-tools-suite — shared UI constants and descriptive tool copy.
//
// Central place for the configured limits from the requirements glossary and
// the human-friendly descriptions the catalog shows (Req 1.6).

import type { ToolId } from './tools/registry';

/** Max_File_Size: 100 MB per Source_File (Req 2.3, glossary). */
export const MAX_FILE_SIZE = 100 * 1024 * 1024;

/** Max_Batch_Count: 50 Source_Files per Job (Req 2.4, glossary). */
export const MAX_BATCH_COUNT = 50;

/** Retention_Period: 60 minutes (Req 32.5, glossary). */
export const RETENTION_MINUTES = 60;

/**
 * Client-side memory budget. Files larger than this trigger the confirmed
 * Server_Side fallback prompt under Privacy Mode (Req 37.8). A conservative
 * value keeps large documents off the WASM heap.
 */
export const CLIENT_MEMORY_BUDGET = 75 * 1024 * 1024;

/**
 * Descriptive labels stating what each Tool does (Req 1.6). Keyed by ToolId so
 * the catalog can render a helpful one-liner under each tool name.
 */
export const TOOL_DESCRIPTIONS: Readonly<Record<ToolId, string>> = {
	Merge: 'Combine several PDFs into one document, in the order you choose.',
	Split: 'Break one PDF into multiple files by page ranges or fixed sizes.',
	RemovePages: 'Delete unwanted pages and keep the rest in order.',
	ExtractPages: 'Pull selected pages into a brand-new PDF.',
	Organize: 'Reorder, rotate, and remove pages on a thumbnail board.',
	ScanToPdf: 'Turn camera captures or images into a searchable PDF.',
	OptimizePdf: 'Shrink a PDF while keeping acceptable quality.',
	CompressPdf: 'Compress a PDF to a smaller, easier-to-share size.',
	JpgToPdf: 'Convert JPG images into a single PDF, one page per image.',
	MarkdownToPdf: 'Render Markdown text into a formatted PDF.',
	WordToPdf: 'Convert Word documents (DOC, DOCX) to fixed-layout PDF.',
	PptToPdf: 'Convert PowerPoint slides (PPT, PPTX) to PDF.',
	ExcelToPdf: 'Convert Excel spreadsheets (XLS, XLSX) to PDF.',
	HtmlToPdf: 'Capture a web page or HTML file as a PDF.',
	PdfToJpg: 'Export each PDF page as a JPG image.',
	PdfToMarkdown: 'Extract PDF text into clean Markdown.',
	PdfToWord: 'Convert a PDF into an editable Word document.',
	PdfToPptx: 'Turn each PDF page into a PowerPoint slide.',
	PdfToExcel: 'Pull tables from a PDF into an Excel spreadsheet.',
	PdfToPdfA: 'Convert a PDF to the archival PDF/A format.',
	Rotate: 'Rotate pages by 90, 180, or 270 degrees.',
	AddPageNumbers: 'Stamp page numbers in the position you pick.',
	AddWatermark: 'Overlay text or image watermarks on every page.',
	Crop: 'Trim page margins to a region you define.',
	EditPdf: 'Add text, images, and shapes anywhere on a page.',
	PdfForms: 'Fill existing form fields or add new interactive fields.'
};

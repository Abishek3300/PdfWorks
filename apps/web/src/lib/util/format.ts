// Feature: pdf-tools-suite — small pure formatting/validation helpers.

import type { ToolDescriptor } from '../tools/registry';

/** Human-readable byte size (e.g. "2.4 MB"). Pure. */
export function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes < 0) return '0 B';
	if (bytes < 1024) return `${bytes} B`;
	const units = ['KB', 'MB', 'GB', 'TB'];
	let value = bytes / 1024;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit++;
	}
	return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** Percentage reduction from source to output size, clamped to [0, 100]. Pure. */
export function percentReduction(sourceBytes: number, outputBytes: number): number {
	if (sourceBytes <= 0) return 0;
	const pct = ((sourceBytes - outputBytes) / sourceBytes) * 100;
	return Math.max(0, Math.min(100, Math.round(pct)));
}

/** Lower-cased file extension without the dot, or '' when none. Pure. */
export function fileExtension(name: string): string {
	const dot = name.lastIndexOf('.');
	if (dot < 0 || dot === name.length - 1) return '';
	return name.slice(dot + 1).toLowerCase();
}

/**
 * Whether a file name's extension is one of a tool's Supported_Formats
 * (Req 2.8 / 33.1). Client-side pre-check; the Backend re-validates by content
 * signature (Req 39.1).
 */
export function isSupportedFormat(name: string, tool: ToolDescriptor): boolean {
	return tool.supportedFormats.includes(fileExtension(name));
}

/**
 * A decorative emoji glyph representing a file's type, chosen from its
 * extension. Purely visual — callers should render it with `aria-hidden`. Pure.
 */
export function fileIcon(name: string): string {
	switch (fileExtension(name)) {
		case 'pdf':
			return '📕';
		case 'doc':
		case 'docx':
			return '📘';
		case 'xls':
		case 'xlsx':
			return '📗';
		case 'ppt':
		case 'pptx':
			return '📙';
		case 'jpg':
		case 'jpeg':
		case 'png':
			return '🖼️';
		case 'md':
		case 'markdown':
			return '📝';
		case 'zip':
			return '🗜️';
		case 'html':
		case 'htm':
			return '🌐';
		default:
			return '📄';
	}
}

/**
 * A real logo URL (served from /formats) for file types that have one, else
 * null so callers can fall back to the emoji `fileIcon`. Pure.
 */
export function fileIconSrc(name: string): string | null {
	switch (fileExtension(name)) {
		case 'pdf':
			return '/formats/pdf.png';
		case 'xls':
		case 'xlsx':
			return '/formats/excel.png';
		case 'ppt':
		case 'pptx':
			return '/formats/powerpoint.png';
		case 'html':
		case 'htm':
			return '/formats/html.png';
		case 'md':
		case 'markdown':
			return '/formats/markdown.png';
		default:
			return null;
	}
}

/** Uppercased file-type label from the extension, or 'FILE' when none. Pure. */
export function fileTypeLabel(name: string): string {
	const ext = fileExtension(name);
	return ext ? ext.toUpperCase() : 'FILE';
}

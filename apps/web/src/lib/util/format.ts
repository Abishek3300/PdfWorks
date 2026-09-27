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

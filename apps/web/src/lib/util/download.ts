// Feature: pdf-tools-suite (Task 10.2, Req 3.1-3.4, 18.3, 48.4)
//
// Download helpers for Output_Files produced on-device. For a single output we
// trigger a direct download; for multiple outputs we also offer a ZIP built
// with fflate (pinned exact). All bytes stay in the browser for client-side
// jobs (no network, Req 32.6).

import { zipSync } from 'fflate';
import type { OutputFile } from '../engine/types';
import { sanitizeFileName } from './sanitize';

/**
 * Trigger a browser download of raw bytes under a sanitized file name. Uses an
 * object URL with a non-executable type and revokes it after the click.
 */
export function downloadBytes(name: string, bytes: Uint8Array, contentType = 'application/octet-stream'): void {
	const safeName = sanitizeFileName(name);
	// Copy into a fresh ArrayBuffer so the Blob owns exactly these bytes.
	const copy = bytes.slice();
	const blob = new Blob([copy], { type: contentType });
	const url = URL.createObjectURL(blob);
	try {
		const a = document.createElement('a');
		a.href = url;
		a.download = safeName;
		a.rel = 'noopener';
		document.body.appendChild(a);
		a.click();
		a.remove();
	} finally {
		// Defer revoke so the download has time to start.
		setTimeout(() => URL.revokeObjectURL(url), 1000);
	}
}

/** Download a single Output_File directly (Req 3.1). */
export function downloadOutput(file: OutputFile): void {
	downloadBytes(file.name, file.bytes, contentTypeFor(file.name));
}

/**
 * Build a ZIP archive of all Output_Files (Req 3.2, 18.3). Names are sanitized
 * and de-duplicated so colliding names remain distinct in the archive (Req 49.3).
 */
export function buildZip(files: OutputFile[]): Uint8Array {
	const seen = new Map<string, number>();
	const entries: Record<string, Uint8Array> = {};
	for (const file of files) {
		let name = sanitizeFileName(file.name);
		const count = seen.get(name) ?? 0;
		seen.set(name, count + 1);
		if (count > 0) {
			const dot = name.lastIndexOf('.');
			name = dot > 0 ? `${name.slice(0, dot)}-${count}${name.slice(dot)}` : `${name}-${count}`;
		}
		entries[name] = file.bytes;
	}
	return zipSync(entries, { level: 6 });
}

/** Download all Output_Files as a single ZIP (Req 3.2). */
export function downloadZip(files: OutputFile[], zipName = 'pdf-tools-output.zip'): void {
	const bytes = buildZip(files);
	downloadBytes(zipName, bytes, 'application/zip');
}

/** Map a file name to a safe, non-executable Content-Type for the Blob. */
export function contentTypeFor(name: string): string {
	const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
	switch (ext) {
		case 'pdf':
			return 'application/pdf';
		case 'jpg':
		case 'jpeg':
			return 'image/jpeg';
		case 'png':
			return 'image/png';
		case 'md':
		case 'markdown':
			return 'text/plain';
		case 'zip':
			return 'application/zip';
		default:
			return 'application/octet-stream';
	}
}

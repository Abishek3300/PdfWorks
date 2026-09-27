// Feature: pdf-tools-suite — client-side sanitization helpers (Req 47.3).
//
// The UI renders content derived from Source_Files (file names, extracted
// Markdown/text) and from untrusted input (URLs). All of it must be treated as
// data, never as markup: we never inject file-derived strings as raw HTML.
// These helpers make that explicit and safe.

/**
 * Escape a string for safe insertion as HTML text content. Svelte's `{value}`
 * interpolation already escapes, so components should prefer that; this helper
 * exists for the rare place that must build a string (e.g. a document title in
 * a data URL) without risking injected script.
 */
export function escapeHtml(value: string): string {
	return value
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;')
		.replace(/'/g, '&#39;');
}

/**
 * Sanitize a display file name so it carries no path separators or relative
 * segments (Req 50.1). Mirrors the Backend sanitizer's intent on the client so
 * carried/chained names shown in the UI are already clean.
 */
export function sanitizeFileName(name: string): string {
	// Strip any directory portion (handles both separators) and '..' segments.
	const base = name.replace(/^.*[\\/]/, '').replace(/\.\.+/g, '.');
	const cleaned = base.replace(/[\u0000-\u001f]/g, '').trim();
	return cleaned === '' ? 'file' : cleaned;
}

/**
 * Only http/https URLs are acceptable for the HTML-to-PDF URL input (Req 41.1).
 * This is a client-side convenience check; the Backend URL_Fetcher enforces the
 * real SSRF guards (Req 41.2–41.4).
 */
export function isHttpUrl(value: string): boolean {
	try {
		const url = new URL(value.trim());
		return url.protocol === 'http:' || url.protocol === 'https:';
	} catch {
		return false;
	}
}

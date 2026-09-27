// Feature: pdf-tools-suite (Task 10.1, 11.2) — Source_File model + validation.
//
// A `SourceItem` wraps either an uploaded File or bytes carried from a previous
// Job's Output_File (a Chained_Operation, Req 36.2). Carried items never touch
// the network on the client-side path (Req 50.5).

import type { ToolDescriptor } from '../tools/registry';
import { MAX_FILE_SIZE, MAX_BATCH_COUNT } from '../config';
import { fileExtension, formatBytes } from '../util/format';
import { sanitizeFileName } from '../util/sanitize';

export interface SourceItem {
	id: string;
	name: string;
	size: number;
	/** Present for uploaded files (used for the Server_Side upload path). */
	file?: File;
	/** Present for carried outputs and read files (Client_Side path). */
	bytes?: Uint8Array;
	/** True when carried from a previous Job (Req 36.2). */
	carried: boolean;
}

let seq = 0;
function nextId(): string {
	seq += 1;
	return `src-${Date.now().toString(36)}-${seq}`;
}

/** Wrap a browser File as a SourceItem with a sanitized display name. */
export function itemFromFile(file: File): SourceItem {
	return {
		id: nextId(),
		name: sanitizeFileName(file.name),
		size: file.size,
		file,
		carried: false
	};
}

/** Wrap carried bytes (a prior Output_File) as a SourceItem (Req 36.2). */
export function itemFromBytes(name: string, bytes: Uint8Array): SourceItem {
	return {
		id: nextId(),
		name: sanitizeFileName(name),
		size: bytes.byteLength,
		bytes,
		carried: true
	};
}

export interface ValidationResult {
	accepted: SourceItem[];
	/** Human-readable rejection messages to surface to the User. */
	errors: string[];
}

/**
 * Validate a set of candidate items against a tool's Supported_Format,
 * Max_File_Size, and Max_Batch_Count (Req 2.3, 2.4, 2.8). The same checks apply
 * to carried files in a Chained_Operation (Req 36.3).
 *
 * @param existing Items already accepted for this tool (counts toward batch).
 * @param candidates New items being added.
 * @param tool The selected tool descriptor.
 */
export function validateAdditions(
	existing: SourceItem[],
	candidates: SourceItem[],
	tool: ToolDescriptor
): ValidationResult {
	const errors: string[] = [];
	const accepted: SourceItem[] = [];
	let count = existing.length;

	for (const item of candidates) {
		const ext = fileExtension(item.name);
		if (!tool.supportedFormats.includes(ext)) {
			errors.push(
				`“${item.name}” isn’t a supported format. ${tool.label} accepts: ${tool.supportedFormats
					.map((f) => `.${f}`)
					.join(', ')}.`
			);
			continue;
		}
		if (item.size > MAX_FILE_SIZE) {
			errors.push(
				`“${item.name}” is ${formatBytes(item.size)}, over the ${formatBytes(MAX_FILE_SIZE)} limit per file.`
			);
			continue;
		}
		if (count >= MAX_BATCH_COUNT) {
			errors.push(`You can add up to ${MAX_BATCH_COUNT} files per job. Extra files were not added.`);
			break;
		}
		accepted.push(item);
		count += 1;
	}

	return { accepted, errors };
}

/** Read an item's bytes, loading from the wrapped File when needed (Client_Side). */
export async function readItemBytes(item: SourceItem): Promise<Uint8Array> {
	if (item.bytes) return item.bytes;
	if (item.file) {
		const buffer = await item.file.arrayBuffer();
		return new Uint8Array(buffer);
	}
	throw new Error(`“${item.name}” has no readable content.`);
}

/** The largest item size in bytes (drives the client-memory-budget check). */
export function largestSize(items: SourceItem[]): number {
	return items.reduce((max, item) => Math.max(max, item.size), 0);
}

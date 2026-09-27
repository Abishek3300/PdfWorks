// Feature: pdf-tools-suite (Task 10.1/10.2 server path; wired end-to-end in Task 21)
//
// Documented client contract for Server_Side_Processing. The Backend is not yet
// built (Tasks 14-21), so every call is *gated* behind `isBackendConfigured()`.
// With no backend configured the UI stays fully navigable and shows a clear
// "server processing" state instead of making a doomed network request.
//
// ---------------------------------------------------------------------------
// Endpoint contract (fulfilled by Task 21 — gateway -> API -> queue -> store):
//
//   POST   {base}/api/jobs                multipart: tool, options(JSON), files[]
//          -> 201 { jobId, jobToken, expiresAt }        (Req 43.1, 43.2, 32.5)
//   GET    {base}/api/jobs/{jobId}        header: X-Job-Token
//          -> 200 { phase, progress?, outputs?: OutputRef[], error? }
//   GET    {base}/api/jobs/{jobId}/outputs/{index}   header: X-Job-Token
//          -> 200 file bytes; Content-Disposition: attachment (Req 50.2, 3.3)
//   DELETE {base}/api/jobs/{jobId}        header: X-Job-Token
//          -> 204  (immediate secure deletion, Req 32.3)
//
// All server file access is gated by the Job_Token (Req 43.3). Bytes travel
// only over the encrypted connection (Req 32.1). No token is ever logged.
// ---------------------------------------------------------------------------

import type { ProcessingMode } from '../tools/registry';

/** Metadata about one server-stored Output_File (shown before download, Req 3.4). */
export interface OutputRef {
	index: number;
	name: string;
	sizeBytes: number;
	contentType: string;
}

export interface JobCreation {
	jobId: string;
	jobToken: string;
	/** ISO timestamp when the Job's files expire (Req 32.5, 43.4). */
	expiresAt: string;
}

export interface JobStatus {
	phase: 'queued' | 'running' | 'succeeded' | 'failed';
	progress?: number;
	outputs?: OutputRef[];
	error?: string;
}

/**
 * The configured Backend base URL, read from the static build-time public env
 * (`PUBLIC_API_BASE_URL`). Empty when no backend is wired yet.
 */
function backendBaseUrl(): string {
	// Vite exposes statically-replaced public vars on import.meta.env.
	const env = (import.meta as unknown as { env?: Record<string, string | undefined> }).env;
	return (env?.PUBLIC_API_BASE_URL ?? '').trim();
}

/**
 * Whether a Backend is configured for this build. When false, the UI must not
 * attempt Server_Side network calls; it shows a "server processing" placeholder
 * state instead (real wiring lands in Task 21). This keeps the app fully
 * navigable with no live backend.
 */
export function isBackendConfigured(): boolean {
	return backendBaseUrl() !== '';
}

/** Thrown when a server call is attempted with no Backend configured. */
export class BackendUnavailableError extends Error {
	constructor() {
		super('Server processing is not available in this preview build yet.');
		this.name = 'BackendUnavailableError';
	}
}

function requireBase(): string {
	const base = backendBaseUrl();
	if (base === '') throw new BackendUnavailableError();
	return base.replace(/\/$/, '');
}

/**
 * Create a Server_Side Job by uploading the Source_Files (Req 2.5, 32.1). Uses
 * XMLHttpRequest so byte-percentage upload progress is observable (Req 2.6).
 * Rejects with a retryable error on connection interruption (Req 2.7).
 */
export function createJob(params: {
	tool: string;
	options: unknown;
	files: File[];
	onProgress?: (percent: number) => void;
	signal?: AbortSignal;
}): Promise<JobCreation> {
	const base = requireBase();
	return new Promise<JobCreation>((resolve, reject) => {
		const form = new FormData();
		form.set('tool', params.tool);
		form.set('options', JSON.stringify(params.options));
		for (const file of params.files) {
			form.append('files', file, file.name);
		}

		const xhr = new XMLHttpRequest();
		xhr.open('POST', `${base}/api/jobs`);
		xhr.responseType = 'json';

		xhr.upload.addEventListener('progress', (event) => {
			if (event.lengthComputable && params.onProgress) {
				params.onProgress(Math.round((event.loaded / event.total) * 100));
			}
		});
		xhr.addEventListener('load', () => {
			if (xhr.status >= 200 && xhr.status < 300) {
				resolve(xhr.response as JobCreation);
			} else {
				reject(new Error(`Upload failed (status ${xhr.status}).`));
			}
		});
		xhr.addEventListener('error', () =>
			reject(new Error('The connection was interrupted during upload. You can retry.'))
		);
		xhr.addEventListener('abort', () => reject(new Error('Upload cancelled.')));

		if (params.signal) {
			params.signal.addEventListener('abort', () => xhr.abort(), { once: true });
		}
		xhr.send(form);
	});
}

/** Poll a Job's status (Req 31.3 progress; Req 33.3 error reason). */
export async function getJobStatus(jobId: string, jobToken: string): Promise<JobStatus> {
	const base = requireBase();
	const res = await fetch(`${base}/api/jobs/${encodeURIComponent(jobId)}`, {
		headers: { 'X-Job-Token': jobToken }
	});
	if (!res.ok) throw new Error(`Could not read job status (status ${res.status}).`);
	return (await res.json()) as JobStatus;
}

/** Absolute, token-scoped download URL for one Output_File (Req 3.3, 43.3). */
export function outputDownloadUrl(jobId: string, index: number): string {
	const base = requireBase();
	return `${base}/api/jobs/${encodeURIComponent(jobId)}/outputs/${index}`;
}

/**
 * Fetch one Output_File's bytes into memory using the token-scoped download URL
 * (Req 3.3, 43.3). The Job_Token is sent in the X-Job-Token header so server
 * file access stays gated; bytes travel only over the encrypted connection
 * (Req 32.1). Rejects with a retryable error on connection interruption (Req 2.7).
 */
export async function fetchOutput(
	jobId: string,
	index: number,
	jobToken: string
): Promise<Uint8Array> {
	let res: Response;
	try {
		res = await fetch(outputDownloadUrl(jobId, index), {
			headers: { 'X-Job-Token': jobToken }
		});
	} catch {
		throw new Error('The connection was interrupted during download. You can retry.');
	}
	if (!res.ok) throw new Error(`Could not download output (status ${res.status}).`);
	return new Uint8Array(await res.arrayBuffer());
}

/** Request immediate secure deletion of a Job's files (Req 32.3). */
export async function deleteJob(jobId: string, jobToken: string): Promise<void> {
	const base = requireBase();
	await fetch(`${base}/api/jobs/${encodeURIComponent(jobId)}`, {
		method: 'DELETE',
		headers: { 'X-Job-Token': jobToken }
	});
}

/** Convenience: is a mode server-side and, if so, is the backend actually wired? */
export function serverPathReady(mode: ProcessingMode): boolean {
	return mode === 'Server_Side' && isBackendConfigured();
}

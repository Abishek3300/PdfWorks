// Feature: pdf-tools-suite (Task 7.2, Req 47.1, 47.2, 32.6)
//
// Typed client wrapper the UI calls to run a `Client_Capable` Tool on-device.
// `runClientSide` hands the Source_File bytes to the Web Worker that hosts the
// WASM engine and resolves with the Output_File bytes. No network request is
// ever made from this path: the bytes go only to the worker, never to the
// Backend (Req 47.2 / 32.6). Property 21 (Task 7.3) asserts that at this
// boundary.

import type { EngineToolId, ToolOptions, OutputFile, WorkerRequest, WorkerResponse } from './types';

/** Minimal structural type of the Worker surface this client depends on. */
export interface EngineWorkerLike {
	postMessage(message: WorkerRequest, transfer?: Transferable[]): void;
	addEventListener(
		type: 'message',
		listener: (event: MessageEvent<WorkerResponse>) => void
	): void;
	addEventListener(type: 'error', listener: (event: ErrorEvent) => void): void;
	removeEventListener(type: 'message', listener: (event: MessageEvent<WorkerResponse>) => void): void;
	removeEventListener(type: 'error', listener: (event: ErrorEvent) => void): void;
	terminate(): void;
}

/** Factory that constructs the engine Web Worker. Injectable for testing. */
export type WorkerFactory = () => EngineWorkerLike;

/**
 * Default worker factory: construct the module Worker from `worker.ts`.
 *
 * `import.meta.url`-relative `new Worker(new URL(...))` is the Vite/SvelteKit
 * idiom for bundling a Web Worker. Kept behind a factory so tests inject a stub
 * instead of a real browser Worker.
 */
export const defaultWorkerFactory: WorkerFactory = () =>
	new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }) as EngineWorkerLike;

let sharedWorker: EngineWorkerLike | null = null;
let activeFactory: WorkerFactory = defaultWorkerFactory;
let nextRequestId = 1;

/**
 * Override the worker factory (used by tests to inject a deterministic stub).
 * Passing no argument restores the default and disposes any shared worker.
 */
export function setWorkerFactory(factory?: WorkerFactory): void {
	disposeClientWorker();
	activeFactory = factory ?? defaultWorkerFactory;
}

/** Terminate and clear the shared worker, if any. */
export function disposeClientWorker(): void {
	if (sharedWorker) {
		sharedWorker.terminate();
		sharedWorker = null;
	}
}

function getWorker(): EngineWorkerLike {
	if (sharedWorker === null) {
		sharedWorker = activeFactory();
	}
	return sharedWorker;
}

/**
 * Extract the transferable `ArrayBuffer` backing each Source_File. When a file's
 * bytes are only a view over a larger buffer, its own slice is copied so the
 * exact bytes (and nothing else) are transferred.
 */
function toArrayBuffers(files: Uint8Array[]): ArrayBuffer[] {
	return files.map((view) => {
		if (view.byteOffset === 0 && view.byteLength === view.buffer.byteLength) {
			return view.buffer;
		}
		return view.slice().buffer;
	});
}

/**
 * Run a `Client_Capable` Tool entirely on the User's device.
 *
 * @param toolId Which Tool to run.
 * @param options Tool-specific options (matching the engine's wire format).
 * @param files Source_File byte buffers, in source order.
 * @param sourceNames Optional original file names, in source order.
 * @returns The produced Output_File(s).
 * @throws When the engine rejects the Job or the worker errors; the message
 *   identifies the reason.
 */
export function runClientSide(
	toolId: EngineToolId,
	options: ToolOptions,
	files: Uint8Array[],
	sourceNames?: string[]
): Promise<OutputFile[]> {
	const worker = getWorker();
	const id = nextRequestId++;
	const buffers = toArrayBuffers(files);

	return new Promise<OutputFile[]>((resolve, reject) => {
		const onMessage = (event: MessageEvent<WorkerResponse>) => {
			const response = event.data;
			if (response.id !== id) {
				return; // Not our request; leave it for its own listener.
			}
			cleanup();
			if (response.ok) {
				resolve(response.outputs);
			} else {
				reject(new Error(response.error));
			}
		};
		const onError = (event: ErrorEvent) => {
			cleanup();
			reject(new Error(event.message || 'engine worker error'));
		};
		const cleanup = () => {
			worker.removeEventListener('message', onMessage);
			worker.removeEventListener('error', onError);
		};

		worker.addEventListener('message', onMessage);
		worker.addEventListener('error', onError);

		const request: WorkerRequest = {
			id,
			toolId,
			options,
			sources: buffers,
			sourceNames
		};
		// Transfer the source buffers to the worker (no copy). The bytes go only
		// to the worker — never to the network (Req 47.2 / 32.6).
		worker.postMessage(request, buffers);
	});
}

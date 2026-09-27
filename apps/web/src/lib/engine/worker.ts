/// <reference lib="webworker" />
// Feature: pdf-tools-suite (Task 7.2, Req 47.1, 47.2, 32.6)
//
// Web Worker host for on-device processing. It loads the Rust->WASM PDF engine
// and processes `postMessage({ toolId, options, sources })` requests by invoking
// the engine's `run_tool` entry point, returning the Output_File bytes to the
// main thread.
//
// Privacy guarantee (Req 47.2 / 32.6): this worker performs NO network I/O. It
// only initializes the WASM module (a same-origin static asset) and runs pure,
// I/O-free engine code. Source_File and Output_File bytes never leave the
// device. Property 21 (Task 7.3) asserts this at the client boundary.

import init, { run_tool } from '../wasm/pdf_engine.js';
import type { ToolRequest, WorkerRequest, WorkerResponse, OutputFile } from './types';

/**
 * Lazily initialize the WASM module exactly once. `init()` instantiates the
 * `.wasm` (fetched as a same-origin static asset by the wasm-bindgen `web`
 * target loader); subsequent calls reuse the same promise.
 */
let ready: Promise<unknown> | null = null;
function ensureReady(): Promise<unknown> {
	if (ready === null) {
		ready = init();
	}
	return ready;
}

/**
 * Run a single request through the engine. Kept separate from the message
 * handler so the logic is unit-testable without a real Worker/`self` context.
 */
export async function handleRequest(request: WorkerRequest): Promise<WorkerResponse> {
	try {
		await ensureReady();

		const payload: ToolRequest = {
			tool: request.toolId,
			options: request.options,
			source_names: request.sourceNames
		};

		// Each ArrayBuffer becomes a Uint8Array view the engine reads from.
		const sources = request.sources.map((buffer) => new Uint8Array(buffer));

		const outputs = run_tool(JSON.stringify(payload), sources) as OutputFile[];
		return { id: request.id, ok: true, outputs };
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err);
		return { id: request.id, ok: false, error: message };
	}
}

/**
 * Wire the message handler onto a worker-scope target. Exported so tests can
 * drive it against a stub scope; the module also self-registers below when it is
 * actually running inside a Worker.
 */
export function registerWorker(scope: {
	addEventListener: (type: 'message', listener: (event: MessageEvent<WorkerRequest>) => void) => void;
	postMessage: (message: WorkerResponse, transfer?: Transferable[]) => void;
}): void {
	scope.addEventListener('message', (event) => {
		void handleRequest(event.data).then((response) => {
			// Transfer output buffers back to the main thread to avoid a copy.
			const transfer =
				response.ok && response.outputs.length > 0
					? response.outputs.map((file: OutputFile) => file.bytes.buffer)
					: undefined;
			scope.postMessage(response, transfer);
		});
	});
}

// Self-register only when executing inside an actual worker global scope.
// Guarded so importing this module in a test (Node/jsdom) does not throw.
const maybeScope = globalThis as { postMessage?: unknown; addEventListener?: unknown };
if (
	typeof maybeScope.postMessage === 'function' &&
	typeof maybeScope.addEventListener === 'function'
) {
	registerWorker(globalThis as unknown as Parameters<typeof registerWorker>[0]);
}

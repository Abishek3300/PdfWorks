import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import fc from 'fast-check';
import {
	runClientSide,
	setWorkerFactory,
	disposeClientWorker,
	type EngineWorkerLike
} from './client';
import type {
	EngineToolId,
	ToolOptions,
	WorkerRequest,
	WorkerResponse,
	OutputFile
} from './types';

// Feature: pdf-tools-suite, Property 21: Client_Side_Processing transmits no source bytes
//
// For all Client_Capable_Tools executed under Privacy_Mode (including chained
// client-to-client operations), no network request carrying the bytes of the
// Source_File or Output_File is issued to the Backend.
// Validates: Requirements 32.6, 47.2, 50.5
//
// The client-side path is `runClientSide` -> Web Worker -> WASM engine. To keep
// the test deterministic and browser-free, the real Worker is replaced with an
// in-process stub that emulates on-device processing: it reads the transferred
// Source_File bytes and returns Output_File bytes WITHOUT touching the network,
// exactly as the WASM engine does. The property is asserted at the client
// boundary by spying on every network primitive (`fetch`, `XMLHttpRequest`,
// `navigator.sendBeacon`, `WebSocket`, `EventSource`) and requiring that none is
// invoked during a client run — even when Output_Files are chained back in as
// the next Source_Files (Req 50.5).

/**
 * An in-process stand-in for the engine Web Worker. It performs the same shape
 * of work the real worker does (consume source bytes, produce output bytes) and,
 * crucially, makes no network call — mirroring the pure WASM engine.
 */
class StubEngineWorker implements EngineWorkerLike {
	private messageListeners = new Set<(event: MessageEvent<WorkerResponse>) => void>();
	private errorListeners = new Set<(event: ErrorEvent) => void>();
	terminated = false;

	postMessage(message: WorkerRequest): void {
		// Emulate on-device processing: derive one Output_File per Source_File by
		// transforming the received bytes purely in memory. No network I/O.
		const outputs: OutputFile[] = message.sources.map((buffer, index) => {
			const input = new Uint8Array(buffer);
			const transformed = new Uint8Array(input.length + 1);
			transformed.set(input, 0);
			transformed[input.length] = input.length & 0xff; // deterministic marker
			return { name: `out_${index + 1}.bin`, bytes: transformed };
		});

		const response: WorkerResponse = { id: message.id, ok: true, outputs };
		// Deliver asynchronously, like a real Worker message.
		queueMicrotask(() => {
			const event = { data: response } as MessageEvent<WorkerResponse>;
			for (const listener of this.messageListeners) {
				listener(event);
			}
		});
	}

	addEventListener(type: 'message' | 'error', listener: unknown): void {
		if (type === 'message') {
			this.messageListeners.add(listener as (event: MessageEvent<WorkerResponse>) => void);
		} else {
			this.errorListeners.add(listener as (event: ErrorEvent) => void);
		}
	}

	removeEventListener(type: 'message' | 'error', listener: unknown): void {
		if (type === 'message') {
			this.messageListeners.delete(listener as (event: MessageEvent<WorkerResponse>) => void);
		} else {
			this.errorListeners.delete(listener as (event: ErrorEvent) => void);
		}
	}

	terminate(): void {
		this.terminated = true;
		this.messageListeners.clear();
		this.errorListeners.clear();
	}
}

// --- Network spies. Any invocation means source/output bytes could leave the
// device, which would violate Property 21. ---
type Spies = {
	fetch: ReturnType<typeof vi.fn>;
	send: ReturnType<typeof vi.fn>;
	open: ReturnType<typeof vi.fn>;
	sendBeacon: ReturnType<typeof vi.fn>;
	webSocket: ReturnType<typeof vi.fn>;
	eventSource: ReturnType<typeof vi.fn>;
};

let spies: Spies;
const g = globalThis as Record<string, unknown>;
const originals: Record<string, unknown> = {};

beforeEach(() => {
	setWorkerFactory(() => new StubEngineWorker());

	spies = {
		fetch: vi.fn(() => Promise.reject(new Error('network blocked in client-side test'))),
		send: vi.fn(),
		open: vi.fn(),
		sendBeacon: vi.fn(() => true),
		webSocket: vi.fn(),
		eventSource: vi.fn()
	};

	// Preserve and replace every network primitive with a spy.
	originals.fetch = g.fetch;
	g.fetch = spies.fetch;

	originals.XMLHttpRequest = g.XMLHttpRequest;
	class SpyXHR {
		open(...args: unknown[]) {
			spies.open(...args);
		}
		send(...args: unknown[]) {
			spies.send(...args);
		}
		setRequestHeader() {}
		addEventListener() {}
		abort() {}
	}
	g.XMLHttpRequest = SpyXHR as unknown;

	originals.navigator = g.navigator;
	g.navigator = { ...(g.navigator as object), sendBeacon: spies.sendBeacon };

	originals.WebSocket = g.WebSocket;
	g.WebSocket = spies.webSocket;

	originals.EventSource = g.EventSource;
	g.EventSource = spies.eventSource;
});

afterEach(() => {
	disposeClientWorker();
	setWorkerFactory();
	for (const [key, value] of Object.entries(originals)) {
		if (value === undefined) {
			delete g[key];
		} else {
			g[key] = value;
		}
	}
});

function assertNoNetwork(): void {
	expect(spies.fetch).not.toHaveBeenCalled();
	expect(spies.send).not.toHaveBeenCalled();
	expect(spies.open).not.toHaveBeenCalled();
	expect(spies.sendBeacon).not.toHaveBeenCalled();
	expect(spies.webSocket).not.toHaveBeenCalled();
	expect(spies.eventSource).not.toHaveBeenCalled();
}

// The Client_Capable tools that may run on-device under Privacy Mode.
const CLIENT_TOOLS: EngineToolId[] = [
	'Merge',
	'Split',
	'RemovePages',
	'ExtractPages',
	'Organize',
	'OptimizePdf',
	'CompressPdf',
	'JpgToPdf',
	'PdfToJpg',
	'Rotate',
	'AddPageNumbers',
	'AddWatermark',
	'Crop',
	'MarkdownToPdf',
	'PdfToMarkdown',
	'EditPdf',
	'PdfForms'
];

/** A representative options value for a tool (shape is irrelevant to the property). */
function optionsFor(tool: EngineToolId): ToolOptions {
	switch (tool) {
		case 'Merge':
			return { Merge: { order: [] } };
		case 'Split':
			return { Split: { split_points: [], fixed_size: null } };
		case 'Rotate':
			return { Rotate: { angle: 'D90', pages: 'All' } };
		case 'PdfToJpg':
			return { PdfToJpg: { dpi: 150 } };
		default:
			// Any well-formed variant works; the property does not depend on it.
			return { Optimize: { level: 'Medium' } };
	}
}

describe('Client_Side_Processing network isolation', () => {
	// Feature: pdf-tools-suite, Property 21: Client_Side_Processing transmits no source bytes
	it('Property 21: a single client-side run issues no network request carrying bytes', async () => {
		await fc.assert(
			fc.asyncProperty(
				fc.constantFrom(...CLIENT_TOOLS),
				fc.array(fc.uint8Array({ minLength: 0, maxLength: 64 }), {
					minLength: 1,
					maxLength: 4
				}),
				async (tool, files) => {
					const outputs = await runClientSide(tool, optionsFor(tool), files, files.map((_, i) => `src_${i}.pdf`));
					// The engine ran (produced output) and nothing hit the network.
					expect(outputs.length).toBe(files.length);
					assertNoNetwork();
				}
			),
			{ numRuns: 150 }
		);
	});

	// Feature: pdf-tools-suite, Property 21: Client_Side_Processing transmits no source bytes
	it('Property 21: chained client-to-client operations keep bytes on-device (Req 50.5)', async () => {
		await fc.assert(
			fc.asyncProperty(
				fc.array(fc.constantFrom(...CLIENT_TOOLS), { minLength: 2, maxLength: 5 }),
				fc.array(fc.uint8Array({ minLength: 1, maxLength: 48 }), {
					minLength: 1,
					maxLength: 3
				}),
				async (toolChain, initialFiles) => {
					// Feed the outputs of each step in as the sources of the next,
					// simulating a Chained_Operation that stays client-side.
					let carried: Uint8Array[] = initialFiles;
					for (const tool of toolChain) {
						const outputs = await runClientSide(tool, optionsFor(tool), carried);
						carried = outputs.map((file) => file.bytes);
					}
					// Across the entire chain, no bytes were ever sent to the Backend.
					assertNoNetwork();
					expect(carried.length).toBeGreaterThan(0);
				}
			),
			{ numRuns: 150 }
		);
	});
});

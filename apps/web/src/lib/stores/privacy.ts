// Feature: pdf-tools-suite (Task 9.2, Req 37.1-37.8, 48.2)
//
// The Privacy Mode state machine. `privacyMode` is the single User-controlled
// setting (default OFF => Server_Side, Req 37.2/37.3). Everything else about
// how a given Tool + file will run is *derived* — the User never picks a
// Processing_Mode directly (Req 37.3/37.4).

import { writable, derived, type Readable } from 'svelte/store';
import {
	deriveProcessingMode,
	type Capability,
	type ProcessingMode,
	type ToolDescriptor
} from '../tools/registry';
import { CLIENT_MEMORY_BUDGET } from '../config';

/** Whether this browser can run the WASM engine (Req 48.2). */
export function hasWebAssembly(): boolean {
	return typeof WebAssembly === 'object' && typeof WebAssembly.instantiate === 'function';
}

/** The User-controlled Privacy_Mode toggle. Defaults to disabled (Req 37.2). */
export const privacyMode = writable<boolean>(false);

/**
 * Set the toggle, but never allow it on when the browser lacks WebAssembly
 * (Req 48.2). Callers use this instead of a raw `.set(true)`.
 */
export function setPrivacyMode(enabled: boolean): void {
	privacyMode.set(enabled && hasWebAssembly());
}

/** A resolved description of how the current Tool + files will be processed. */
export interface PrivacyResolution {
	/** The derived Processing_Mode (Req 37.3, 37.4). */
	mode: ProcessingMode;
	/** Whether the toggle can be interacted with at all. */
	toggleEnabled: boolean;
	/** Whether the on-device indicator should show (Req 37.6). */
	onDevice: boolean;
	/**
	 * A plain-language explanation shown near the toggle: why it is disabled, or
	 * that files stay on-device. `null` when no note is needed.
	 */
	note: string | null;
	/**
	 * True when Privacy Mode is on and a client-capable tool has a file that
	 * exceeds the client memory budget: the UI must confirm a Server_Side
	 * fallback before transmitting bytes (Req 37.8).
	 */
	requiresServerFallbackConfirm: boolean;
}

/**
 * Pure resolver for the Privacy Mode state machine (design "3. Privacy Mode
 * logic"). Given the toggle, WebAssembly availability, the selected tool, and
 * the largest source file size, produce the full resolution the UI renders.
 */
export function resolvePrivacy(
	privacyOn: boolean,
	wasmAvailable: boolean,
	tool: ToolDescriptor | undefined,
	largestFileBytes: number
): PrivacyResolution {
	// No tool selected yet: report the base state.
	if (!tool) {
		return {
			mode: deriveProcessingMode(privacyOn && wasmAvailable, 'Client_Capable'),
			toggleEnabled: wasmAvailable,
			onDevice: false,
			note: wasmAvailable
				? null
				: 'On-device processing is unavailable in this browser, so files are processed securely on the server.',
			requiresServerFallbackConfirm: false
		};
	}

	const capability: Capability = tool.capability;

	// Server_Only tools always use the server and lock the toggle (Req 37.7).
	if (capability === 'Server_Only') {
		return {
			mode: 'Server_Side',
			toggleEnabled: false,
			onDevice: false,
			note: `${tool.label} requires secure server processing, so Privacy Mode can’t keep it on your device.`,
			requiresServerFallbackConfirm: false
		};
	}

	// Client_Capable, but the browser can't run WASM: disable Privacy Mode and
	// explain (Req 48.2).
	if (!wasmAvailable) {
		return {
			mode: 'Server_Side',
			toggleEnabled: false,
			onDevice: false,
			note: 'On-device processing is unavailable in this browser, so files are processed securely on the server.',
			requiresServerFallbackConfirm: false
		};
	}

	const mode = deriveProcessingMode(privacyOn, capability);

	// Privacy Mode on + file too big for the client memory budget => confirmed
	// Server_Side fallback (Req 37.8).
	if (privacyOn && largestFileBytes > CLIENT_MEMORY_BUDGET) {
		return {
			mode: 'Server_Side',
			toggleEnabled: true,
			onDevice: false,
			note: 'This file is too large to process on your device. It will be processed securely on the server once you confirm.',
			requiresServerFallbackConfirm: true
		};
	}

	return {
		mode,
		toggleEnabled: true,
		onDevice: mode === 'Client_Side',
		note:
			mode === 'Client_Side'
				? 'Privacy Mode is on: this file is processed on your device and never uploaded.'
				: null,
		requiresServerFallbackConfirm: false
	};
}

/**
 * A store factory that reactively resolves the Privacy Mode state for a given
 * selected tool + largest-file-size pair. Components pass reactive stores for
 * the tool and file size and get a live `PrivacyResolution`.
 */
export function createPrivacyResolution(
	tool: Readable<ToolDescriptor | undefined>,
	largestFileBytes: Readable<number>
): Readable<PrivacyResolution> {
	const wasm = hasWebAssembly();
	return derived([privacyMode, tool, largestFileBytes], ([$privacy, $tool, $bytes]) =>
		resolvePrivacy($privacy, wasm, $tool, $bytes)
	);
}

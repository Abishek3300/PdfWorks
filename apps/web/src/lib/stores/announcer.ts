// Feature: pdf-tools-suite (Task 12.2, Req 35.5)
//
// A single global aria-live message store. Status changes anywhere in the app
// call `announce(...)`; the layout renders one visually-hidden live region that
// screen readers announce (Req 35.5).

import { writable, type Readable } from 'svelte/store';

const store = writable<string>('');

export const announcement: Readable<string> = { subscribe: store.subscribe };

/**
 * Announce a status change to assistive technology. Setting the same string
 * twice in a row still re-announces by briefly clearing first.
 */
export function announce(message: string): void {
	store.set('');
	// Microtask so the DOM registers the change and re-announces identical text.
	queueMicrotask(() => store.set(message));
}

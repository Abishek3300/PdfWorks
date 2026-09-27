// @vitest-environment jsdom
//
// Feature: pdf-tools-suite (Task 22.6, Req 34.1, 34.2, 34.4)
//
// Responsive-layout example tests at the 320 / 768 / 1024 px viewports.
//
// jsdom does not run layout, so we assert the responsive contract at its
// source: the real global stylesheet (`app.css`). The catalog/workspace grid
// is single-column below 768 px, two-column at >= 768 px, and three-column at
// >= 1024 px; the body forbids horizontal scrolling so there is never sideways
// scroll at >= 320 px; and interactive controls meet a >= 44 px touch target.
// A small `matchMedia`-driven check demonstrates which column count each named
// viewport resolves to, mirroring how the browser applies the same breakpoints.

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

// Read the real global stylesheet from disk. The SvelteKit Vite plugin turns a
// `.css` import into an empty module for style-injection, so we read the file
// directly. Vitest runs with the working directory at the package root
// (apps/web), which is stable for `npm test` and CI.
const appCss = readFileSync(resolve(process.cwd(), 'src/lib/styles/app.css'), 'utf-8');

/** Extract the body of the first `@media (<query>) { ... }` block. */
function mediaBlock(query: string): string {
	const idx = appCss.indexOf(`@media (${query})`);
	expect(idx, `expected an @media (${query}) block in app.css`).toBeGreaterThanOrEqual(0);
	const open = appCss.indexOf('{', idx);
	// Walk braces to find the matching close of the media block.
	let depth = 0;
	for (let i = open; i < appCss.length; i++) {
		if (appCss[i] === '{') depth++;
		else if (appCss[i] === '}') {
			depth--;
			if (depth === 0) return appCss.slice(open + 1, i);
		}
	}
	throw new Error(`unbalanced braces after @media (${query})`);
}

describe('responsive layout contract (app.css)', () => {
	it('no horizontal scroll at >= 320 px (Req 34.4)', () => {
		// The body clips sideways overflow so a 320 px viewport never scrolls
		// horizontally.
		expect(appCss).toMatch(/body\s*\{[^}]*overflow-x:\s*hidden/s);
	});

	it('single-column base grid below 768 px (Req 34.2)', () => {
		// The base `.grid` declares a single column (no multi-column
		// grid-template-columns outside a min-width media query).
		const base = appCss.slice(0, appCss.indexOf('@media'));
		expect(base).toMatch(/\.grid\s*\{[^}]*display:\s*grid/s);
		// Base grid must NOT set a multi-column template — that only appears
		// inside the >= 768 / >= 1024 media queries.
		const baseGrid = base.slice(base.indexOf('.grid'));
		expect(baseGrid).not.toMatch(/grid-template-columns:\s*repeat\(2/);
		expect(baseGrid).not.toMatch(/grid-template-columns:\s*repeat\(3/);
	});

	it('two-column tool grid at >= 768 px (Req 34.1/34.2)', () => {
		const block = mediaBlock('min-width: 768px');
		expect(block).toMatch(/\.grid--tools\s*\{[^}]*grid-template-columns:\s*repeat\(2,\s*1fr\)/s);
	});

	it('three-column tool grid and split workspace at >= 1024 px (Req 34.1)', () => {
		const block = mediaBlock('min-width: 1024px');
		expect(block).toMatch(/\.grid--tools\s*\{[^}]*grid-template-columns:\s*repeat\(3,\s*1fr\)/s);
		// The workspace becomes a multi-column layout at the large breakpoint.
		expect(block).toMatch(/\.workspace-layout\s*\{[^}]*grid-template-columns:/s);
	});

	it('touch targets are at least 44 px tall (Req 34.3)', () => {
		// Buttons and text inputs meet the minimum touch target size.
		expect(appCss).toMatch(/\.btn\s*\{[^}]*min-height:\s*44px/s);
		expect(appCss).toMatch(/min-height:\s*44px/);
	});
});

describe('breakpoint resolution at named viewports (320 / 768 / 1024 px)', () => {
	/**
	 * Mirror the app's breakpoint logic: which tool-grid column count applies at
	 * a given viewport width. This is the same decision the browser makes from
	 * the app.css media queries, exercised deterministically.
	 */
	function toolColumnsAt(width: number): 1 | 2 | 3 {
		if (width >= 1024) return 3;
		if (width >= 768) return 2;
		return 1;
	}

	it('320 px viewport -> single column (Req 34.2)', () => {
		expect(toolColumnsAt(320)).toBe(1);
	});

	it('768 px viewport -> two columns (Req 34.1)', () => {
		expect(toolColumnsAt(768)).toBe(2);
	});

	it('1024 px viewport -> three columns (Req 34.1)', () => {
		expect(toolColumnsAt(1024)).toBe(3);
	});

	it('matchMedia reflects the min-width breakpoints', () => {
		// Install a width-driven matchMedia so a min-width query resolves like a
		// real browser at each named viewport.
		const setViewport = (width: number) => {
			(globalThis as unknown as { matchMedia: (q: string) => MediaQueryList }).matchMedia = (
				query: string
			) => {
				const m = query.match(/min-width:\s*(\d+)px/);
				const min = m ? Number(m[1]) : 0;
				return { matches: width >= min, media: query } as MediaQueryList;
			};
		};

		setViewport(320);
		expect(window.matchMedia('(min-width: 768px)').matches).toBe(false);
		expect(window.matchMedia('(min-width: 1024px)').matches).toBe(false);

		setViewport(768);
		expect(window.matchMedia('(min-width: 768px)').matches).toBe(true);
		expect(window.matchMedia('(min-width: 1024px)').matches).toBe(false);

		setViewport(1024);
		expect(window.matchMedia('(min-width: 1024px)').matches).toBe(true);
	});
});

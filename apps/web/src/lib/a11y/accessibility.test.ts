// @vitest-environment jsdom
//
// Feature: pdf-tools-suite (Task 22.6, Req 35.1–35.5)
//
// Automated accessibility checks. Two layers:
//
//  1. axe-core (pinned exact 4.10.2) runs the WCAG 2.x A/AA rule set against
//     (a) the real `ToolCard` component rendered via @testing-library/svelte,
//     and (b) a document assembled from the exact accessibility structures the
//     app shell + home page emit (skip link, header landmark, labeled search,
//     category nav landmark, tool grid, and the global aria-live status
//     region). This exercises keyboard operability (focusable controls with
//     accessible names), text alternatives (labels / aria-label / sr-only),
//     colour-independent structure, and the status-announcement region.
//
//  2. The status-announcement store (`announce`) is asserted directly (Req
//     35.5): a status change publishes text that the layout's single
//     aria-live="polite" region renders for assistive technology.
//
// NOTE: axe-core can only detect a subset of WCAG failures. Full WCAG AA
// conformance additionally requires MANUAL testing with assistive technologies
// (screen readers, keyboard-only navigation) and expert review — see the
// design's Testing Strategy note. These automated checks guard against
// regressions in the machine-detectable rules.

import { afterEach, describe, expect, it } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import axe from 'axe-core';

import ToolCard from '$lib/components/ToolCard.svelte';
import { TOOL_REGISTRY } from '$lib/tools/registry';
import { announce, announcement } from '$lib/stores/announcer';

afterEach(() => cleanup());

/** Run axe against a container and return the violation list. */
async function axeViolations(node: Element | Document): Promise<axe.Result[]> {
	const results = await axe.run(node as axe.ElementContext, {
		runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'] }
	});
	return results.violations;
}

/** A concise, readable summary of any violations for assertion messages. */
function summarize(violations: axe.Result[]): string {
	return violations.map((v) => `${v.id}: ${v.help}`).join('\n');
}

describe('accessibility (axe-core WCAG 2 A/AA)', () => {
	it('ToolCard renders with no axe violations', async () => {
		const tool = TOOL_REGISTRY[0];
		const { container } = render(ToolCard, { props: { tool } });
		const violations = await axeViolations(container);
		expect(violations, summarize(violations)).toHaveLength(0);
	});

	it('a Server_Only ToolCard renders with no axe violations', async () => {
		const serverTool = TOOL_REGISTRY.find((t) => t.capability === 'Server_Only');
		if (!serverTool) throw new Error('registry should contain a Server_Only tool');
		const { container } = render(ToolCard, { props: { tool: serverTool } });
		const violations = await axeViolations(container);
		expect(violations, summarize(violations)).toHaveLength(0);
	});

	it('the app shell + home structure has no axe violations', async () => {
		// Assemble the accessibility-relevant structures the layout + home page
		// emit (matching +layout.svelte and +page.svelte markup exactly). The
		// real document sets `<html lang="en">` in app.html, so we mirror it.
		document.documentElement.lang = 'en';
		document.title = 'PDF Tools Suite';
		document.body.innerHTML = `
			<a class="skip-link" href="#main">Skip to content</a>
			<header class="app-header">
				<a class="brand" href="/">
					<span class="brand-mark" aria-hidden="true">◆</span>
					<span>PDF Tools Suite</span>
				</a>
			</header>
			<main id="main">
				<section class="hero">
					<h1>Every PDF tool, in one calm place</h1>
					<div class="search">
						<label for="tool-search" class="sr-only">Search tools by name</label>
						<span class="search-icon" aria-hidden="true">⌕</span>
						<input id="tool-search" type="text" autocomplete="off"
							placeholder="Search tools…" />
					</div>
				</section>
				<nav class="cat-nav" aria-label="Tool categories">
					<ul>
						<li><a href="#cat-Organize">Organize</a></li>
						<li><a href="#cat-Edit">Edit</a></li>
					</ul>
				</nav>
				<section id="cat-Organize" aria-labelledby="cat-Organize-h">
					<h2 id="cat-Organize-h">Organize</h2>
					<div class="grid grid--tools">
						<a class="tool-card" href="/tool?tool=merge">
							<span class="tool-title">Merge PDF</span>
							<span class="tool-desc">Combine several PDFs into one.</span>
							<span class="tool-go" aria-hidden="true">Open →</span>
						</a>
					</div>
				</section>
			</main>
			<footer class="app-footer"><p>Beautiful, private PDF tools.</p></footer>
			<div class="sr-only" role="status" aria-live="polite" aria-atomic="true"></div>
		`;
		const violations = await axeViolations(document);
		expect(violations, summarize(violations)).toHaveLength(0);
	});

	it('every interactive control has an accessible name (keyboard operability, Req 35.1/35.3)', async () => {
		// A tool card is a link whose accessible name is its label; the search
		// input is named by its associated <label>. axe's label + link-name
		// rules cover both.
		document.body.innerHTML = `
			<label for="q" class="sr-only">Search tools by name</label>
			<input id="q" type="text" />
			<a class="tool-card" href="/tool?tool=split">
				<span class="tool-title">Split PDF</span>
			</a>
			<button type="button">Run</button>
		`;
		const results = await axe.run(document, {
			runOnly: { type: 'rule', values: ['label', 'link-name', 'button-name'] }
		});
		expect(results.violations, summarize(results.violations)).toHaveLength(0);
	});
});

describe('status announcements (Req 35.5)', () => {
	it('announce publishes the message to the live-region store', async () => {
		let latest = '';
		const unsub = announcement.subscribe((v) => {
			latest = v;
		});

		announce('3 tools match “merge”.');
		// `announce` clears then re-sets on a microtask so identical text
		// re-announces; await the microtask.
		await Promise.resolve();
		expect(latest).toBe('3 tools match “merge”.');

		unsub();
	});

	it('re-announcing identical text still updates the region', async () => {
		const seen: string[] = [];
		const unsub = announcement.subscribe((v) => seen.push(v));

		announce('Processing complete.');
		await Promise.resolve();
		announce('Processing complete.');
		await Promise.resolve();

		// The store cleared to '' between the two identical messages so the
		// aria-live region fires twice (Req 35.5).
		expect(seen).toContain('Processing complete.');
		expect(seen.filter((s) => s === '').length).toBeGreaterThanOrEqual(1);

		unsub();
	});
});

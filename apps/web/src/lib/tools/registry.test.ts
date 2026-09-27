import { describe, expect, it } from 'vitest';
import { toolsAcceptingExtensions, type ToolCategoryGroup } from './registry';

/** Flatten grouped results to a flat list of tool ids for easy assertions. */
const flatIds = (groups: ToolCategoryGroup[]): string[] =>
	groups.flatMap((group) => group.tools.map((tool) => tool.id));

describe('toolsAcceptingExtensions', () => {
	// Feature: pdf-tools-suite (Req 36.1, 36.3)
	it("['pdf'] includes PDF-consuming tools and excludes image-only tools", () => {
		const ids = flatIds(toolsAcceptingExtensions(['pdf']));
		expect(ids).toContain('RemovePages');
		expect(ids).toContain('Merge');
		expect(ids).not.toContain('JpgToPdf');
		expect(ids).not.toContain('ScanToPdf');
	});

	it("['jpg'] includes JpgToPdf and excludes PDF-only tools", () => {
		const ids = flatIds(toolsAcceptingExtensions(['jpg']));
		expect(ids).toContain('JpgToPdf');
		expect(ids).not.toContain('RemovePages');
	});

	it('empty and blank extensions are ignored, yielding no groups', () => {
		expect(toolsAcceptingExtensions([])).toEqual([]);
		expect(toolsAcceptingExtensions([''])).toEqual([]);
		expect(toolsAcceptingExtensions(['   '])).toEqual([]);
	});

	it("['pdf','jpg'] yields nothing because no single tool accepts both", () => {
		expect(toolsAcceptingExtensions(['pdf', 'jpg'])).toEqual([]);
	});
});

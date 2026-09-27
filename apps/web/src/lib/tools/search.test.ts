import { describe, expect, it } from 'vitest';
import fc from 'fast-check';
import { searchTools } from './search';
import { TOOL_REGISTRY, type ToolDescriptor } from './registry';

/**
 * Reference predicate used to independently derive the expected result set.
 * A tool matches when its label contains the query as a case-insensitive
 * substring; an empty/whitespace-only query matches every tool (Req 1.4, 1.5).
 */
function expectedMatches(query: string): ToolDescriptor[] {
	const needle = query.trim().toLowerCase();
	return TOOL_REGISTRY.filter((tool) => tool.label.toLowerCase().includes(needle));
}

const ids = (tools: readonly ToolDescriptor[]): string[] => tools.map((tool) => tool.id);

describe('searchTools', () => {
	// Feature: pdf-tools-suite, Property 24: Tool search filters by name substring
	//
	// For all query strings, the filtered result set is exactly the tools whose
	// labels contain the query text (case-insensitive). Validates: Requirements 1.5
	it('Property 24: returns exactly the tools whose labels contain the query (case-insensitive)', () => {
		fc.assert(
			fc.property(fc.string(), (query) => {
				const result = searchTools(query);
				const expected = expectedMatches(query);

				// Same tools, in registry order, with no extras or omissions.
				expect(ids(result)).toEqual(ids(expected));

				// Every returned tool genuinely contains the query as a substring.
				const needle = query.trim().toLowerCase();
				for (const tool of result) {
					expect(tool.label.toLowerCase().includes(needle)).toBe(true);
				}
				// No matching tool is dropped from the result.
				for (const tool of TOOL_REGISTRY) {
					const matches = tool.label.toLowerCase().includes(needle);
					expect(ids(result).includes(tool.id)).toBe(matches);
				}
			}),
			{ numRuns: 200 }
		);
	});

	// Feature: pdf-tools-suite, Property 24: biased generator over real label fragments
	// Exercises queries drawn from actual tool-label substrings so non-empty
	// matches are frequently hit, not just random misses.
	it('Property 24: matches when the query is a real substring of a tool label', () => {
		const labelFragment = fc
			.constantFrom(...TOOL_REGISTRY.map((tool) => tool.label))
			.chain((label) => {
				const upper = Math.max(1, label.length);
				return fc
					.tuple(
						fc.integer({ min: 0, max: upper - 1 }),
						fc.integer({ min: 1, max: upper })
					)
					.map(([start, len]) => label.slice(start, start + len));
			});

		fc.assert(
			fc.property(labelFragment, (query) => {
				const result = searchTools(query);
				const expected = expectedMatches(query);
				expect(ids(result)).toEqual(ids(expected));
			}),
			{ numRuns: 200 }
		);
	});

	it('empty and whitespace-only queries return every tool (Req 1.4)', () => {
		expect(ids(searchTools(''))).toEqual(ids(TOOL_REGISTRY));
		expect(ids(searchTools('   '))).toEqual(ids(TOOL_REGISTRY));
	});
});

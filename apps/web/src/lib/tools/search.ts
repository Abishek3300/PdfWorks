// Feature: pdf-tools-suite
//
// Tool search filtering over the registry (Req 1.4, 1.5). The search control
// filters the displayed tools by name: a tool matches when its label contains
// the query text as a case-insensitive substring. An empty or whitespace-only
// query returns every tool.

import { TOOL_REGISTRY, type ToolDescriptor } from './registry';

/**
 * Return the tools whose `label` contains `query` as a case-insensitive
 * substring (Req 1.5). An empty or whitespace-only query returns all tools
 * (Req 1.4 — the search control simply displays the full catalog).
 *
 * Pure function: it reads only its arguments and the static registry.
 *
 * @param query The search text entered by the User.
 * @param tools The tool set to filter; defaults to the full registry.
 */
export function searchTools(
	query: string,
	tools: readonly ToolDescriptor[] = TOOL_REGISTRY
): ToolDescriptor[] {
	const needle = query.trim().toLowerCase();
	if (needle === '') {
		return [...tools];
	}
	return tools.filter((tool) => tool.label.toLowerCase().includes(needle));
}

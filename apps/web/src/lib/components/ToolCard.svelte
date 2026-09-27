<script lang="ts">
	// Feature: pdf-tools-suite (Task 9.1, Req 1.2, 1.3, 1.6)
	//
	// One tool in the catalog: descriptive label + one-line description, and a
	// link into its workspace. Uses SvelteKit client-side navigation for a fast
	// open (Req 1.3 — <= 300 ms).

	import type { ToolDescriptor } from '$lib/tools/registry';
	import { TOOL_DESCRIPTIONS } from '$lib/config';

	export let tool: ToolDescriptor;
</script>

<a class="tool-card" href={`/tool?tool=${tool.id}`} data-sveltekit-preload-data="hover">
	<span class="tool-title">
		{tool.label}
		{#if tool.capability === 'Server_Only'}
			<span class="badge badge--server">Server</span>
		{/if}
	</span>
	<span class="tool-desc">{TOOL_DESCRIPTIONS[tool.id]}</span>
	<span class="tool-go" aria-hidden="true">Open →</span>
</a>

<style>
	.tool-card {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		padding: 1.1rem 1.15rem;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		text-decoration: none;
		color: var(--ink);
		box-shadow: var(--shadow);
		transition: transform 0.1s ease, border-color 0.12s ease;
		height: 100%;
	}
	.tool-card:hover {
		transform: translateY(-2px);
		border-color: var(--accent);
	}
	.tool-title {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-weight: 650;
		font-size: 1.05rem;
	}
	.tool-desc {
		color: var(--ink-soft);
		font-size: 0.92rem;
		flex: 1;
	}
	.tool-go {
		color: var(--accent);
		font-weight: 600;
		font-size: 0.9rem;
		margin-top: 0.25rem;
	}
</style>

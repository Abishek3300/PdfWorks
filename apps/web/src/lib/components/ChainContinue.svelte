<script lang="ts">
	// Feature: pdf-tools-suite (Task 11.2, Req 36.1-36.4, 50.5)
	//
	// After a successful Job, offer to continue with the Output_Files as input to
	// another Tool (Req 36.1). The User picks which outputs to carry (Req 36.4)
	// and which tool to continue into; the workspace loads them without re-upload
	// (Req 36.2) and re-validates them (Req 36.3). Client-to-client chains stay
	// on-device (Req 50.5).

	import { createEventDispatcher } from 'svelte';
	import type { OutputFile } from '$lib/engine/types';
	import { getToolsByCategory, type ToolId } from '$lib/tools/registry';
	import { fileExtension } from '$lib/util/format';

	export let outputs: OutputFile[] = [];

	const dispatch = createEventDispatcher<{ continue: { toolId: ToolId; indices: number[] } }>();

	const groups = getToolsByCategory();

	// Default: carry every output.
	let selected = new Set<number>(outputs.map((_, i) => i));
	let targetTool: ToolId | '' = '';

	function toggle(i: number) {
		if (selected.has(i)) selected.delete(i);
		else selected.add(i);
		selected = new Set(selected);
	}

	// Only offer tools that accept the carried outputs' formats.
	$: carriedExts = new Set([...selected].map((i) => fileExtension(outputs[i]?.name ?? '')));
	$: compatible = groups
		.map((g) => ({
			...g,
			tools: g.tools.filter((t) => [...carriedExts].every((ext) => t.supportedFormats.includes(ext)))
		}))
		.filter((g) => g.tools.length > 0);

	function go() {
		if (targetTool === '' || selected.size === 0) return;
		dispatch('continue', { toolId: targetTool, indices: [...selected].sort((a, b) => a - b) });
	}
</script>

<section class="chain card" aria-label="Continue to another tool">
	<h2>Keep going</h2>
	<p class="hint">Use these results in another tool — no re-uploading.</p>

	{#if outputs.length > 1}
		<fieldset class="carry">
			<legend>Files to carry forward</legend>
			{#each outputs as file, i (i)}
				<label class="carry-item">
					<input type="checkbox" checked={selected.has(i)} on:change={() => toggle(i)} />
					{file.name}
				</label>
			{/each}
		</fieldset>
	{/if}

	<div class="chain-go">
		<label for="chain-tool" class="sr-only">Choose the next tool</label>
		<select id="chain-tool" bind:value={targetTool}>
			<option value="" disabled>Choose the next tool…</option>
			{#each compatible as group (group.category)}
				<optgroup label={group.label}>
					{#each group.tools as tool (tool.id)}
						<option value={tool.id}>{tool.label}</option>
					{/each}
				</optgroup>
			{/each}
		</select>
		<button class="btn btn--primary" on:click={go} disabled={targetTool === '' || selected.size === 0}>
			Continue
		</button>
	</div>
	{#if compatible.length === 0}
		<p class="hint">No other tool accepts these output formats.</p>
	{/if}
</section>

<style>
	.chain {
		padding: 1.25rem 1.35rem;
		margin-top: 1.5rem;
	}
	.chain h2 {
		font-size: 1.15rem;
	}
	.carry {
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		padding: 0.6rem 0.9rem;
		margin: 0.75rem 0;
	}
	.carry legend {
		font-weight: 600;
		padding: 0 0.35rem;
	}
	.carry-item {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		min-height: 40px;
	}
	.chain-go {
		display: flex;
		gap: 0.6rem;
		flex-wrap: wrap;
		margin-top: 0.75rem;
	}
	.chain-go select {
		flex: 1;
		min-width: 12rem;
	}
</style>

<script lang="ts">
	// Feature: pdf-tools-suite (Task 10.1, Req 2.1-2.8)
	//
	// Drag-and-drop + file-dialog intake. The parent (workspace) owns the
	// SourceItem list and runs validation (Req 2.3, 2.4, 2.8); this component
	// raises an `add` event with the picked Files, renders the accepted list, and
	// for Server_Side mode shows byte-percentage upload progress with retry
	// (Req 2.6, 2.7). For Client_Side mode nothing is transmitted (Req 32.6).

	import { createEventDispatcher } from 'svelte';
	import type { SourceItem } from '$lib/workspace/files';
	import type { ProcessingMode, ToolDescriptor } from '$lib/tools/registry';
	import { formatBytes } from '$lib/util/format';
	import FileIcon from '$lib/components/FileIcon.svelte';
	import { MAX_FILE_SIZE, MAX_BATCH_COUNT } from '$lib/config';

	export let tool: ToolDescriptor;
	export let items: SourceItem[] = [];
	export let mode: ProcessingMode = 'Server_Side';
	/** Upload progress 0-100 when a Server_Side upload is in flight, else null. */
	export let uploadProgress: number | null = null;
	/** A retryable upload error message, if any (Req 2.7). */
	export let uploadError: string | null = null;
	export let errors: string[] = [];

	const dispatch = createEventDispatcher<{
		add: { files: File[] };
		remove: { id: string };
		retry: void;
	}>();

	let dragging = false;
	let inputEl: HTMLInputElement;

	const accept = tool.supportedFormats.map((f) => `.${f}`).join(',');

	// Only Merge, JPG to PDF, and Scan to PDF accept a batch (Req 2.x).
	$: allowMultiple = tool.multiFile === true;

	function pick() {
		inputEl?.click();
	}

	function onInputChange(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		if (input.files && input.files.length > 0) {
			dispatch('add', { files: Array.from(input.files) });
		}
		input.value = ''; // allow re-selecting the same file
	}

	function onDrop(event: DragEvent) {
		event.preventDefault();
		dragging = false;
		const files = event.dataTransfer?.files;
		if (files && files.length > 0) {
			dispatch('add', { files: Array.from(files) });
		}
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			pick();
		}
	}
</script>

<div class="upload">
	<!-- Drop zone doubles as a button: click, Enter, or Space open the dialog. -->
	<div
		class="dropzone"
		class:dragging
		role="button"
		tabindex="0"
		aria-label={`Add files. Drag and drop here or activate to browse. Accepted: ${accept || 'any'}.`}
		on:click={pick}
		on:keydown={onKeydown}
		on:dragover|preventDefault={() => (dragging = true)}
		on:dragleave={() => (dragging = false)}
		on:drop={onDrop}
	>
		<span class="dz-icon" aria-hidden="true">⬆</span>
		<p class="dz-title">Drag &amp; drop files here</p>
		<p class="dz-sub">or <span class="dz-link">browse your device</span></p>
		<p class="dz-formats">
			Accepts {tool.supportedFormats.map((f) => f.toUpperCase()).join(', ')} · up to
			{formatBytes(MAX_FILE_SIZE)} each · {allowMultiple ? `${MAX_BATCH_COUNT} files max` : 'one file at a time'}
		</p>
		<input
			bind:this={inputEl}
			class="sr-only"
			type="file"
			multiple={allowMultiple}
			{accept}
			on:change={onInputChange}
			tabindex="-1"
			aria-hidden="true"
		/>
	</div>

	{#if errors.length > 0}
		<ul class="notice notice--error msgs" role="alert">
			{#each errors as message}
				<li>{message}</li>
			{/each}
		</ul>
	{/if}

	{#if items.length > 0}
		<ul class="file-list" aria-label="Added files">
			{#each items as item (item.id)}
				<li class="file-row">
					<FileIcon name={item.name} />
					<span class="file-name" title={item.name}>{item.name}</span>
					<span class="file-size">{formatBytes(item.size)}</span>
					{#if item.carried}
						<span class="badge">carried</span>
					{/if}
					<button
						class="btn btn--ghost remove"
						on:click={() => dispatch('remove', { id: item.id })}
					>
						Remove<span class="sr-only"> {item.name}</span>
					</button>
				</li>
			{/each}
		</ul>
	{/if}

	{#if mode === 'Server_Side' && uploadProgress != null}
		<div class="progress" aria-label="Upload progress">
			<span
				class="bar"
				role="progressbar"
				aria-valuemin="0"
				aria-valuemax="100"
				aria-valuenow={uploadProgress}
			>
				<span class="bar-fill" style="width:{uploadProgress}%"></span>
			</span>
			<span class="progress-label">{uploadProgress}% uploaded</span>
		</div>
	{/if}

	{#if uploadError}
		<div class="notice notice--error" role="alert">
			<p>{uploadError}</p>
			<button class="btn" on:click={() => dispatch('retry')}>Retry upload</button>
		</div>
	{/if}
</div>

<style>
	.dropzone {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.35rem;
		padding: 2rem 1rem;
		border: 2px dashed var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		text-align: center;
		cursor: pointer;
		transition: border-color 0.12s ease, background 0.12s ease;
	}
	.dropzone:hover,
	.dropzone.dragging {
		border-color: var(--accent);
		background: var(--accent-soft);
	}
	.dz-icon {
		font-size: 1.8rem;
		color: var(--accent);
	}
	.dz-title {
		margin: 0;
		font-weight: 650;
		font-size: 1.05rem;
	}
	.dz-sub {
		margin: 0;
		color: var(--ink-soft);
	}
	.dz-link {
		color: var(--accent);
		font-weight: 600;
		text-decoration: underline;
	}
	.dz-formats {
		margin: 0.4rem 0 0;
		font-size: 0.82rem;
		color: var(--ink-soft);
	}
	.msgs {
		margin: 1rem 0 0;
		padding-left: 2rem;
	}
	.file-list {
		list-style: none;
		margin: 1rem 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}
	.file-row {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		padding: 0.55rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface);
	}
	.file-name {
		flex: 1;
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.file-size {
		color: var(--ink-soft);
		font-size: 0.88rem;
		flex: none;
	}
	.remove {
		min-height: 36px;
		padding: 0.3rem 0.7rem;
		font-size: 0.85rem;
		color: var(--danger);
	}
	.progress {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		margin-top: 1rem;
	}
	.bar {
		flex: 1;
		height: 10px;
		border-radius: 999px;
		background: var(--border);
		overflow: hidden;
	}
	.bar-fill {
		display: block;
		height: 100%;
		background: var(--accent);
		transition: width 0.2s ease;
	}
	.progress-label {
		font-size: 0.85rem;
		color: var(--ink-soft);
		flex: none;
	}
	.notice button {
		margin-top: 0.6rem;
	}
</style>

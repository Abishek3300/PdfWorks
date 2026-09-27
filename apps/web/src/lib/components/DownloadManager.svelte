<script lang="ts">
	// Feature: pdf-tools-suite (Task 10.2, Req 3.1-3.4, 18.3, 32.5, 48.4)
	//
	// Presents each Output_File's name + size before download (Req 3.4). A single
	// output downloads directly (Req 3.1); multiple outputs offer a ZIP of all
	// plus a per-file control (Req 3.2, 18.3). Downloads are retryable (Req 48.4)
	// — re-clicking simply re-triggers the download from the in-memory bytes. For
	// Server_Side jobs the Retention_Period note is shown (Req 32.5).

	import type { OutputFile } from '$lib/engine/types';
	import type { ProcessingMode } from '$lib/tools/registry';
	import { formatBytes, fileIcon } from '$lib/util/format';
	import { downloadOutput, downloadZip } from '$lib/util/download';
	import { RETENTION_MINUTES } from '$lib/config';
	import { announce } from '$lib/stores/announcer';

	export let outputs: OutputFile[] = [];
	export let mode: ProcessingMode = 'Client_Side';
	/** Optional size/reduction summary shown above the list (Req 10.4, 11.4). */
	export let summary: string | null = null;

	let downloaded = false;

	function one(file: OutputFile) {
		downloadOutput(file);
		announce(`Downloading ${file.name}.`);
		downloaded = true;
	}
	function all() {
		downloadZip(outputs);
		announce(`Downloading a ZIP of ${outputs.length} files.`);
		downloaded = true;
	}
</script>

{#if outputs.length > 0}
	<section class="downloads card" aria-label="Results">
		<h2>Your files are ready</h2>

		{#if summary}
			<p class="summary notice notice--info">{summary}</p>
		{/if}

		{#if outputs.length === 1}
			<!-- Single output: a large, centered green download control (Req 3.1). -->
			<div class="single">
				<p class="single-meta">
					<span class="file-ico" aria-hidden="true">{fileIcon(outputs[0].name)}</span>
					<span class="single-name" title={outputs[0].name}>{outputs[0].name}</span>
					<span class="single-size">{formatBytes(outputs[0].bytes.byteLength)}</span>
				</p>
				<button class="btn btn--download" on:click={() => one(outputs[0])}>
					Download {outputs[0].name}
				</button>
			</div>
		{:else}
			<div class="all-row">
				<span>{outputs.length} files produced</span>
				<button class="btn btn--download" on:click={all}>Download all (ZIP)</button>
			</div>

			<ul class="out-list">
				{#each outputs as file, i (i)}
					<li class="out-row">
						<span class="file-ico" aria-hidden="true">{fileIcon(file.name)}</span>
						<span class="out-name" title={file.name}>{file.name}</span>
						<span class="out-size">{formatBytes(file.bytes.byteLength)}</span>
						<button class="btn" on:click={() => one(file)}>
							Download<span class="sr-only"> {file.name}</span>
						</button>
					</li>
				{/each}
			</ul>
		{/if}

		{#if downloaded}
			<p class="retry-hint">Downloaded. Need it again? Just press Download.</p>
		{/if}

		{#if mode === 'Server_Side'}
			<p class="retention">
				Files are stored securely and automatically deleted after {RETENTION_MINUTES} minutes.
			</p>
		{/if}
	</section>
{/if}

<style>
	.downloads {
		padding: 1.25rem 1.35rem;
		margin-top: 1.5rem;
	}
	.downloads h2 {
		font-size: 1.2rem;
	}
	.summary {
		margin: 0 0 1rem;
	}
	.all-row {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 0.75rem;
		flex-wrap: wrap;
		text-align: center;
		padding-bottom: 1rem;
		margin-bottom: 1rem;
		border-bottom: 1px solid var(--border);
	}
	.single {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 0.75rem;
		padding: 0.5rem 0;
	}
	.single-meta {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		margin: 0;
		flex-wrap: wrap;
	}
	.single-name {
		font-weight: 600;
		word-break: break-all;
	}
	.single-size {
		color: var(--ink-soft);
		font-size: 0.88rem;
	}
	.file-ico {
		flex: none;
		font-size: 1.1rem;
		line-height: 1;
	}
	.btn--download {
		display: block;
		margin-inline: auto;
		background: var(--accent);
		color: var(--accent-ink);
		border-color: var(--accent);
		min-height: 52px;
		font-size: 1.05rem;
		padding: 0.8rem 2rem;
	}
	.out-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}
	.out-row {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 0.6rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface-2);
	}
	.out-name {
		flex: 1;
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.out-size {
		color: var(--ink-soft);
		font-size: 0.88rem;
		flex: none;
	}
	.retry-hint,
	.retention {
		font-size: 0.85rem;
		color: var(--ink-soft);
		margin: 1rem 0 0;
	}
</style>

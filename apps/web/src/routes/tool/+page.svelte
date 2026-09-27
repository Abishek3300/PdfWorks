<script lang="ts">
	// Feature: pdf-tools-suite (Tasks 9.1, 10.1-10.3, 11.1-11.3)
	//
	// The tool workspace. It owns the Source_File list, resolves Privacy Mode for
	// the selected tool + files, renders the option panel and upload/download
	// managers, and runs the Job:
	//   - Client_Side: through runClientSide (the WASM Web Worker), on-device only
	//     (Req 47.2/32.6), with a progress indicator (Req 31.3).
	//   - Server_Side: through the gated API client (real wiring in Task 21); with
	//     no backend configured it shows a clear "server processing" state.
	// It also handles empty state (Req 48.1), chained operations (Req 36), and
	// multi-job status via the jobs store (Req 48.5).

	import { onMount, onDestroy } from 'svelte';
	import { get, writable } from 'svelte/store';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';

	import { getTool, type ToolDescriptor, type ToolId } from '$lib/tools/registry';
	import { TOOL_DESCRIPTIONS } from '$lib/config';
	import { createPrivacyResolution } from '$lib/stores/privacy';
	import { announce } from '$lib/stores/announcer';
	import { startJob, updateJob } from '$lib/stores/jobs';
	import {
		itemFromFile,
		itemFromBytes,
		validateAdditions,
		readItemBytes,
		largestSize,
		type SourceItem
	} from '$lib/workspace/files';
	import { defaultOptions, buildEngineOptions, toEngineToolId, type ToolOptionState } from '$lib/workspace/options';
	import { runClientSide } from '$lib/engine';
	import type { OutputFile } from '$lib/engine/types';
	import { formatBytes, percentReduction } from '$lib/util/format';
	import { isBackendConfigured, createJob, getJobStatus, fetchOutput } from '$lib/api/client';

	import UploadManager from '$lib/components/UploadManager.svelte';
	import DownloadManager from '$lib/components/DownloadManager.svelte';
	import OptionsPanel from '$lib/components/OptionsPanel.svelte';
	import ScanCapture from '$lib/components/ScanCapture.svelte';
	import ChainContinue from '$lib/components/ChainContinue.svelte';
	import JobsPanel from '$lib/components/JobsPanel.svelte';

	// --- Selected tool from the URL ---
	let tool: ToolDescriptor | undefined;
	let toolId: ToolId | undefined;

	// --- Workspace state ---
	let items: SourceItem[] = [];
	let options: ToolOptionState = {};
	let uploadErrors: string[] = [];
	let runError: string | null = null;
	let phase: 'idle' | 'running' | 'done' | 'server-pending' = 'idle';
	let progress: number | null = null;
	let outputs: OutputFile[] = [];
	let sourceTotalBytes = 0;
	let fallbackConfirmed = false;

	// Privacy resolution reacts to the selected tool + largest file size.
	const toolStore = writable<ToolDescriptor | undefined>(undefined);
	const largestFile = writable<number>(0);
	const resolution = createPrivacyResolution(toolStore, largestFile);

	// React to URL changes: (re)load the tool and reset workspace state.
	$: {
		const id = $page.url.searchParams.get('tool') as ToolId | null;
		if (id && id !== toolId) {
			loadTool(id);
		} else if (!id) {
			tool = undefined;
			toolId = undefined;
			toolStore.set(undefined);
		}
	}

	function loadTool(id: ToolId) {
		const descriptor = getTool(id);
		toolId = id;
		tool = descriptor;
		toolStore.set(descriptor);
		// Reset per-tool state on switch.
		items = [];
		options = defaultOptions(id, 0);
		uploadErrors = [];
		runError = null;
		outputs = [];
		phase = 'idle';
		progress = null;
		fallbackConfirmed = false;
		refreshDerived();
	}

	function refreshDerived() {
		sourceTotalBytes = items.reduce((n, i) => n + i.size, 0);
		largestFile.set(largestSize(items));
		// Keep option ordering in sync with the current item count.
		if (options.order && options.order.length !== items.length) {
			options = { ...options, order: items.map((_, i) => i) };
		}
	}

	// --- Add / remove files ---
	function addFiles(files: File[]) {
		if (!tool) return;
		const candidates = files.map(itemFromFile);
		const { accepted, errors } = validateAdditions(items, candidates, tool);
		items = [...items, ...accepted];
		uploadErrors = errors;
		if (accepted.length > 0) {
			announce(`${accepted.length} file${accepted.length === 1 ? '' : 's'} added.`);
		}
		fallbackConfirmed = false;
		refreshDerived();
	}

	function addCaptured(file: File) {
		addFiles([file]);
	}

	function removeItem(id: string) {
		items = items.filter((i) => i.id !== id);
		fallbackConfirmed = false;
		refreshDerived();
	}

	// --- Guard checks for enabling Run ---
	$: minSources = tool?.minSources ?? 1;
	$: hasEnoughFiles = items.length >= minSources;
	$: needsFallbackConfirm = $resolution.requiresServerFallbackConfirm && !fallbackConfirmed;
	$: canRun = !!tool && hasEnoughFiles && phase !== 'running' && !needsFallbackConfirm;

	// Merge requires >= 2 (Req 4.4); Remove Pages requires >= 1 retained (Req 6.2).
	$: guardMessage = (() => {
		if (!tool) return null;
		if (tool.id === 'Merge' && items.length < 2) return 'Add at least two PDF files to merge.';
		if (tool.id === 'RemovePages' && items.length > 0 && (options.pages?.length ?? 0) > 0)
			return 'Make sure at least one page remains after removal.';
		return null;
	})();

	// --- Run the Job ---
	async function run() {
		if (!tool || !toolId || !canRun) return;
		runError = null;
		outputs = [];
		const mode = $resolution.mode;

		const jobId = startJob({ toolId, toolLabel: tool.label, mode, phase: 'running', progress: 0 });

		if (mode === 'Client_Side') {
			phase = 'running';
			progress = null; // engine work is indeterminate
			announce(`Processing ${tool.label} on your device.`);
			try {
				const engineOptions = buildEngineOptions(toolId, options, items.length);
				const bytes = await Promise.all(items.map(readItemBytes));
				const names = items.map((i) => i.name);
				const result = await runClientSide(toEngineToolId(toolId), engineOptions, bytes, names);
				outputs = result;
				phase = 'done';
				updateJob(jobId, { phase: 'succeeded', progress: 100 });
				announce(`Done. ${result.length} file${result.length === 1 ? '' : 's'} ready to download.`);
			} catch (err) {
				runError = err instanceof Error ? err.message : String(err);
				phase = 'idle';
				updateJob(jobId, { phase: 'failed', error: runError });
				announce(`${tool.label} failed: ${runError}`);
			}
			return;
		}

		// Server_Side path. When no backend is configured, show a clear pending
		// state instead of a doomed request (keeps the app navigable in preview).
		if (!isBackendConfigured()) {
			phase = 'server-pending';
			updateJob(jobId, { phase: 'queued' });
			announce(`${tool.label} will be processed on the server. Server processing is coming soon.`);
			return;
		}

		// A backend IS configured: run the real upload -> poll -> download flow
		// (Req 2.5, 2.6, 2.7, 31.3, 32.1, 33.3, 43.x). Bytes travel only over the
		// encrypted connection; the Job_Token gates every server file access.
		phase = 'running';
		progress = 0;
		announce(`Uploading ${tool.label} to secure server processing.`);
		try {
			const engineOptions = buildEngineOptions(toolId, options, items.length);
			// Build File[] from the current sources (uploaded or carried bytes).
			const uploadFiles = await Promise.all(
				items.map(async (item) => {
					if (item.file) return item.file;
					const bytes = await readItemBytes(item);
					return new File([bytes], item.name, { type: 'application/octet-stream' });
				})
			);

			const { jobId: remoteJobId, jobToken } = await createJob({
				tool: toEngineToolId(toolId),
				options: engineOptions,
				files: uploadFiles,
				onProgress: (percent) => {
					progress = percent;
					updateJob(jobId, { phase: 'running', progress: percent });
				}
			});

			updateJob(jobId, { phase: 'running', progress: 100 });

			// Poll until the Job succeeds or fails, with a sane overall timeout.
			const POLL_INTERVAL_MS = 800;
			const OVERALL_TIMEOUT_MS = 120_000;
			const deadline = Date.now() + OVERALL_TIMEOUT_MS;
			const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

			// eslint-disable-next-line no-constant-condition
			while (true) {
				if (Date.now() > deadline) {
					throw new Error(
						'Server processing timed out. The connection may have been interrupted — you can retry.'
					);
				}

				const status = await getJobStatus(remoteJobId, jobToken);

				if (status.phase === 'queued' || status.phase === 'running') {
					if (typeof status.progress === 'number') {
						progress = status.progress;
						updateJob(jobId, { phase: status.phase, progress: status.progress });
					} else {
						updateJob(jobId, { phase: status.phase });
					}
					await sleep(POLL_INTERVAL_MS);
					continue;
				}

				if (status.phase === 'failed') {
					throw new Error(status.error ?? 'Server processing failed.');
				}

				// 'succeeded': download each Output_File's bytes into memory.
				const refs = status.outputs ?? [];
				const downloaded = await Promise.all(
					refs.map(async (ref): Promise<OutputFile> => {
						const bytes = await fetchOutput(remoteJobId, ref.index, jobToken);
						return { name: ref.name, bytes };
					})
				);
				outputs = downloaded;
				phase = 'done';
				progress = 100;
				updateJob(jobId, { phase: 'succeeded', progress: 100 });
				announce(
					`Done. ${downloaded.length} file${downloaded.length === 1 ? '' : 's'} ready to download.`
				);
				return;
			}
		} catch (err) {
			runError = err instanceof Error ? err.message : String(err);
			phase = 'idle';
			progress = null;
			updateJob(jobId, { phase: 'failed', error: runError });
			announce(`${tool.label} failed: ${runError}`);
		}
	}

	function confirmFallback() {
		fallbackConfirmed = true;
		announce('Confirmed. This file will be processed securely on the server.');
	}

	// --- Chained operations (Req 36) ---
	function continueTo(nextTool: ToolId, indices: number[]) {
		const carried = indices.map((i) => outputs[i]).filter(Boolean);
		const next = getTool(nextTool);
		if (!next) return;
		// Load carried outputs as sources for the next tool (Req 36.2), re-running
		// the same validation as uploads (Req 36.3). Bytes stay in memory — no
		// re-upload, and no transmission on the client path (Req 50.5).
		const candidates = carried.map((o) => itemFromBytes(o.name, o.bytes));
		void goto(`/tool?tool=${nextTool}`).then(() => {
			// loadTool has reset state via the reactive URL block; add carried files.
			const { accepted, errors } = validateAdditions([], candidates, next);
			items = accepted;
			uploadErrors = errors;
			refreshDerived();
			announce(`Carried ${accepted.length} file${accepted.length === 1 ? '' : 's'} into ${next.label}.`);
		});
	}

	// --- Result summary for size-aware tools (Req 10.4, 11.4) ---
	$: resultSummary = (() => {
		if (phase !== 'done' || outputs.length === 0 || !toolId) return null;
		if (toolId === 'CompressPdf') {
			const out = outputs[0].bytes.byteLength;
			return `Reduced from ${formatBytes(sourceTotalBytes)} to ${formatBytes(out)} — ${percentReduction(sourceTotalBytes, out)}% smaller.`;
		}
		if (toolId === 'OptimizePdf') {
			const out = outputs[0].bytes.byteLength;
			return `Original ${formatBytes(sourceTotalBytes)} → optimized ${formatBytes(out)}.`;
		}
		return null;
	})();

	onMount(() => {
		const id = get(page).url.searchParams.get('tool') as ToolId | null;
		if (id) loadTool(id);
	});
	onDestroy(() => {
		// Client worker is shared and reused; nothing to tear down per-page.
	});
</script>

<svelte:head>
	<title>{tool ? `${tool.label} — PDF Tools Suite` : 'Tool — PDF Tools Suite'}</title>
</svelte:head>

{#if !tool}
	<div class="notice notice--warn" role="alert">
		<p>That tool isn’t available. <a href="/">Back to all tools</a>.</p>
	</div>
{:else}
	<nav class="crumbs" aria-label="Breadcrumb">
		<a href="/">All tools</a> <span aria-hidden="true">/</span> <span>{tool.label}</span>
	</nav>

	<header class="ws-head">
		<h1>{tool.label}</h1>
		<p class="ws-desc">{TOOL_DESCRIPTIONS[tool.id]}</p>
		<p class="ws-mode">
			{#if $resolution.mode === 'Client_Side'}
				<span class="badge badge--device">On your device</span>
			{:else}
				<span class="badge badge--server">Server processing</span>
			{/if}
		</p>
	</header>

	<div class="grid workspace-layout">
		<div class="ws-main">
			{#if toolId === 'ScanToPdf'}
				<ScanCapture on:capture={(e) => addCaptured(e.detail.file)} />
			{/if}

			{#if toolId !== 'HtmlToPdf' && toolId !== 'MarkdownToPdf'}
				<UploadManager
					{tool}
					{items}
					mode={$resolution.mode}
					errors={uploadErrors}
					on:add={(e) => addFiles(e.detail.files)}
					on:remove={(e) => removeItem(e.detail.id)}
				/>
			{/if}

			{#if items.length === 0 && toolId !== 'HtmlToPdf' && toolId !== 'MarkdownToPdf'}
				<!-- Empty_State (Req 48.1) -->
				<div class="empty" role="status">
					<p class="empty-title">No files yet</p>
					<p class="empty-sub">
						Drag files onto the box above or browse your device to get started.
					</p>
				</div>
			{/if}
		</div>

		<aside class="ws-side">
			<section class="card options-card" aria-label="Options">
				<h2>Options</h2>
				{#if toolId}
					<OptionsPanel {toolId} bind:state={options} {items} />
				{/if}
			</section>

			{#if needsFallbackConfirm}
				<div class="notice notice--warn" role="alert">
					<p>{$resolution.note}</p>
					<button class="btn" on:click={confirmFallback}>Confirm server processing</button>
				</div>
			{/if}

			{#if guardMessage}
				<p class="notice notice--info" role="status">{guardMessage}</p>
			{/if}

			<button class="btn btn--primary run-btn" on:click={run} disabled={!canRun}>
				{phase === 'running' ? 'Processing…' : `Run ${tool.label}`}
			</button>
			{#if !hasEnoughFiles && (toolId === 'HtmlToPdf' || toolId === 'MarkdownToPdf')}
				<p class="hint">Provide input in the options to run.</p>
			{/if}
		</aside>
	</div>

	{#if phase === 'running'}
		<div class="processing card" role="status" aria-live="polite">
			<span class="spinner" aria-hidden="true"></span>
			<span
				>{$resolution.mode === 'Client_Side'
					? 'Processing on your device…'
					: 'Processing securely on the server…'}</span
			>
			{#if progress != null}
				<span
					class="bar"
					role="progressbar"
					aria-valuemin="0"
					aria-valuemax="100"
					aria-valuenow={progress}
				>
					<span class="bar-fill" style="width:{progress}%"></span>
				</span>
			{/if}
		</div>
	{/if}

	{#if phase === 'server-pending'}
		<div class="notice notice--info server-pending" role="status">
			<p>
				<strong>{tool.label}</strong> runs with secure server processing. Server processing is
				being connected — your files stay on this page in the meantime, nothing was uploaded.
			</p>
		</div>
	{/if}

	{#if runError}
		<div class="notice notice--error" role="alert">
			<p>{tool.label} couldn’t finish: {runError}</p>
		</div>
	{/if}

	{#if phase === 'done'}
		<DownloadManager {outputs} mode={$resolution.mode} summary={resultSummary} />
		<ChainContinue {outputs} on:continue={(e) => continueTo(e.detail.toolId, e.detail.indices)} />
	{/if}

	<JobsPanel />
{/if}

<style>
	.crumbs {
		font-size: 0.9rem;
		color: var(--ink-soft);
		margin-bottom: 1rem;
	}
	.crumbs a {
		color: var(--accent);
	}
	.ws-head {
		margin-bottom: 1.5rem;
	}
	.ws-head h1 {
		font-size: clamp(1.6rem, 4vw, 2.2rem);
		margin-bottom: 0.35rem;
	}
	.ws-desc {
		color: var(--ink-soft);
		margin: 0 0 0.6rem;
		max-width: 40rem;
	}
	.ws-mode {
		margin: 0;
	}
	.workspace-layout {
		display: grid;
		gap: 1.5rem;
	}
	.ws-main {
		min-width: 0;
	}
	.ws-side {
		display: flex;
		flex-direction: column;
		gap: 1rem;
	}
	.options-card {
		padding: 1.1rem 1.2rem;
	}
	.options-card h2 {
		font-size: 1.1rem;
		margin-bottom: 0.75rem;
	}
	.run-btn {
		width: 100%;
		min-height: 52px;
		font-size: 1.05rem;
	}
	.empty {
		text-align: center;
		padding: 1.5rem 1rem;
		color: var(--ink-soft);
	}
	.empty-title {
		font-weight: 650;
		margin: 0 0 0.25rem;
		color: var(--ink);
	}
	.empty-sub {
		margin: 0;
	}
	.processing {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 1rem 1.2rem;
		margin-top: 1.5rem;
	}
	.spinner {
		width: 20px;
		height: 20px;
		border: 3px solid var(--border);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
		flex: none;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.bar {
		flex: 1;
		height: 10px;
		border-radius: 999px;
		background: var(--border);
		overflow: hidden;
		max-width: 16rem;
	}
	.bar-fill {
		display: block;
		height: 100%;
		background: var(--accent);
	}
	.server-pending {
		margin-top: 1.5rem;
	}
	.hint {
		color: var(--ink-soft);
		font-size: 0.85rem;
		margin: 0;
	}
	@media (prefers-reduced-motion: reduce) {
		.spinner {
			animation: none;
		}
	}
</style>

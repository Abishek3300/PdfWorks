<script lang="ts">
	// Feature: pdf-tools-suite (Tasks 9.1, 11.3, 12.1, 12.2)
	//
	// App shell: header with brand + Privacy Mode toggle, skip link, a global
	// aria-live status region (Req 35.5), and a beforeunload guard that warns
	// while a Server_Side Job is running (Req 48.3).

	import '$lib/styles/app.css';
	import { onMount } from 'svelte';
	import { writable } from 'svelte/store';
	import { page } from '$app/stores';
	import PrivacyToggle from '$lib/components/PrivacyToggle.svelte';
	import { announcement } from '$lib/stores/announcer';
	import { hasRunningServerJob } from '$lib/stores/jobs';
	import { createPrivacyResolution } from '$lib/stores/privacy';
	import { getTool, type ToolDescriptor, type ToolId } from '$lib/tools/registry';

	// The header toggle resolves against the tool named in the URL (if any) so
	// its enabled/disabled state and note match the active workspace.
	const activeTool = writable<ToolDescriptor | undefined>(undefined);
	const largestFile = writable<number>(0);
	const resolution = createPrivacyResolution(activeTool, largestFile);

	$: {
		const id = $page.url.searchParams.get('tool') as ToolId | null;
		activeTool.set(id ? getTool(id) : undefined);
	}

	onMount(() => {
		const beforeUnload = (event: BeforeUnloadEvent) => {
			if ($hasRunningServerJob) {
				// Warn the User that an in-progress Server_Side Job may be lost.
				event.preventDefault();
				event.returnValue = '';
			}
		};
		window.addEventListener('beforeunload', beforeUnload);
		return () => window.removeEventListener('beforeunload', beforeUnload);
	});
</script>

<a class="skip-link" href="#main">Skip to content</a>

<header class="app-header">
	<div class="container header-inner">
		<a class="brand" href="/">
			<span class="brand-mark" aria-hidden="true">◆</span>
			<span>PDF Tools Suite</span>
		</a>
		<div class="header-privacy">
			<PrivacyToggle resolution={$resolution} />
		</div>
	</div>
</header>

<main id="main" class="container app-main">
	<slot />
</main>

<footer class="app-footer">
	<div class="container">
		<p>Beautiful, private PDF tools. Enable Privacy Mode to process supported tools on your device.</p>
	</div>
</footer>

<!-- Global assertive live region: status changes are announced here (Req 35.5). -->
<div class="sr-only" role="status" aria-live="polite" aria-atomic="true">{$announcement}</div>

<style>
	.app-header {
		background: var(--surface);
		border-bottom: 1px solid var(--border);
		position: sticky;
		top: 0;
		z-index: 20;
	}
	.header-inner {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 0.75rem 1.5rem;
		padding-top: 0.75rem;
		padding-bottom: 0.75rem;
	}
	.brand {
		display: inline-flex;
		align-items: center;
		gap: 0.55rem;
		font-weight: 700;
		font-size: 1.15rem;
		color: var(--ink);
		text-decoration: none;
	}
	.brand-mark {
		color: var(--accent);
		font-size: 1.1rem;
	}
	.header-privacy {
		flex: 1 1 auto;
		display: flex;
		justify-content: flex-end;
		min-width: 15rem;
	}
	.app-main {
		padding-top: 2rem;
		padding-bottom: 3rem;
		min-height: 60vh;
	}
	.app-footer {
		border-top: 1px solid var(--border);
		background: var(--surface);
		color: var(--ink-soft);
		font-size: 0.88rem;
		padding: 1.5rem 0;
	}
	.app-footer p {
		margin: 0;
	}
</style>

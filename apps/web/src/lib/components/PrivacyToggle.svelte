<script lang="ts">
	// Feature: pdf-tools-suite (Task 9.2, Req 37.1-37.8, 48.2)
	//
	// The global Privacy Mode toggle plus its plain-language note. The toggle is
	// disabled and explained for Server_Only tools (Req 37.7) and when the
	// browser lacks WebAssembly (Req 48.2). A resolution object (from the privacy
	// store) drives the disabled state, note, and on-device indicator.

	import { privacyMode, setPrivacyMode, type PrivacyResolution } from '$lib/stores/privacy';
	import { announce } from '$lib/stores/announcer';

	export let resolution: PrivacyResolution;

	function onToggle(event: Event) {
		const checked = (event.currentTarget as HTMLInputElement).checked;
		setPrivacyMode(checked);
		announce(
			checked
				? 'Privacy Mode enabled. Supported tools run on your device.'
				: 'Privacy Mode disabled. Files are processed securely on the server.'
		);
	}
</script>

<div class="privacy">
	<label class="switch" class:disabled={!resolution.toggleEnabled}>
		<input
			type="checkbox"
			role="switch"
			checked={$privacyMode}
			disabled={!resolution.toggleEnabled}
			on:change={onToggle}
			aria-describedby="privacy-note"
		/>
		<span class="track" aria-hidden="true"><span class="thumb"></span></span>
		<span class="switch-label">Privacy Mode</span>
	</label>

	{#if resolution.onDevice}
		<span class="badge badge--device" data-testid="on-device-indicator">
			<span aria-hidden="true">●</span> Processed on your device
		</span>
	{/if}

	{#if resolution.note}
		<p id="privacy-note" class="note">{resolution.note}</p>
	{:else}
		<p id="privacy-note" class="sr-only">
			When off, files are processed securely on the server.
		</p>
	{/if}
</div>

<style>
	.privacy {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.6rem 0.75rem;
	}
	.switch {
		display: inline-flex;
		align-items: center;
		gap: 0.55rem;
		cursor: pointer;
		font-weight: 600;
		min-height: 44px;
	}
	.switch.disabled {
		cursor: not-allowed;
		opacity: 0.7;
	}
	/* Hide the native checkbox visually but keep it operable/focusable. */
	.switch input {
		position: absolute;
		opacity: 0;
		width: 1px;
		height: 1px;
	}
	.track {
		width: 46px;
		height: 26px;
		border-radius: 999px;
		background: var(--border);
		position: relative;
		transition: background 0.15s ease;
		flex: none;
	}
	.thumb {
		position: absolute;
		top: 3px;
		left: 3px;
		width: 20px;
		height: 20px;
		border-radius: 50%;
		background: #fff;
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
		transition: transform 0.15s ease;
	}
	.switch input:checked + .track {
		background: var(--accent);
	}
	.switch input:checked + .track .thumb {
		transform: translateX(20px);
	}
	.switch input:focus-visible + .track {
		outline: 3px solid var(--focus);
		outline-offset: 2px;
	}
	.note {
		flex-basis: 100%;
		margin: 0;
		font-size: 0.85rem;
		color: var(--ink-soft);
	}
</style>

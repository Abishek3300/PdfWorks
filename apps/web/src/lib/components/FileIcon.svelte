<script lang="ts">
	// Feature: pdf-tools-suite — decorative file-type icon.
	//
	// Renders a real format logo (from /formats) when one exists, otherwise falls
	// back to the emoji glyph from `fileIcon`. Purely decorative: alt="" and
	// aria-hidden so screen readers read the adjacent visible file name instead.

	import { fileIconSrc, fileIcon } from '$lib/util/format';

	export let name: string;
	export let size = 20; // px

	$: src = fileIconSrc(name);
</script>

{#if src}
	<img
		class="file-ico-img"
		{src}
		alt=""
		aria-hidden="true"
		width={size}
		height={size}
		loading="lazy"
	/>
{:else}
	<span class="file-ico" aria-hidden="true" style="font-size:{size * 0.9}px">{fileIcon(name)}</span>
{/if}

<style>
	.file-ico-img {
		flex: none;
		object-fit: contain;
		display: inline-block;
		vertical-align: middle;
	}
	.file-ico {
		flex: none;
		line-height: 1;
	}
</style>

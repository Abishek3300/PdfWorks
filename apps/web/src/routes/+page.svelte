<script lang="ts">
	// Feature: pdf-tools-suite (Task 9.1, Req 1.1-1.6)
	//
	// Home: the five Tool_Categories with their tools and descriptive labels
	// (Req 1.1, 1.2, 1.6), plus a search control that filters via searchTools
	// (Req 1.4, 1.5). Selecting a tool opens its workspace route.

	import { getToolsByCategory, searchTools, type ToolCategory } from '$lib/tools';
	import ToolCard from '$lib/components/ToolCard.svelte';
	import { announce } from '$lib/stores/announcer';

	const categories = getToolsByCategory();

	let query = '';

	// Live filtered view: for each category, keep only tools matching the query.
	$: matches = new Set(searchTools(query).map((t) => t.id));
	$: filtered = categories
		.map((group) => ({ ...group, tools: group.tools.filter((t) => matches.has(t.id)) }))
		.filter((group) => group.tools.length > 0);
	$: totalMatches = filtered.reduce((n, g) => n + g.tools.length, 0);

	let debounce: ReturnType<typeof setTimeout> | undefined;
	function onSearch() {
		clearTimeout(debounce);
		debounce = setTimeout(() => {
			announce(
				query.trim() === ''
					? 'Showing all tools.'
					: `${totalMatches} tool${totalMatches === 1 ? '' : 's'} match “${query.trim()}”.`
			);
		}, 250);
	}

	function anchorId(category: ToolCategory): string {
		return `cat-${category}`;
	}
</script>

<svelte:head>
	<title>PDF Tools Suite — fast, private PDF tools</title>
	<meta
		name="description"
		content="Merge, split, convert, edit, and optimize PDFs. Enable Privacy Mode to process supported tools entirely on your device."
	/>
</svelte:head>

<section class="hero">
	<h1>Every PDF tool, in one calm place</h1>
	<p class="lede">
		Organize, convert, and edit PDFs in seconds. Flip on Privacy Mode and supported tools run
		entirely on your device.
	</p>

	<div class="search">
		<label for="tool-search" class="sr-only">Search tools by name</label>
		<span class="search-icon" aria-hidden="true">⌕</span>
		<input
			id="tool-search"
			type="text"
			placeholder="Search tools (e.g. merge, compress, rotate)…"
			bind:value={query}
			on:input={onSearch}
			autocomplete="off"
		/>
	</div>
</section>

<nav class="cat-nav" aria-label="Tool categories">
	<ul>
		{#each categories as group (group.category)}
			<li><a href={`#${anchorId(group.category)}`}>{group.label}</a></li>
		{/each}
	</ul>
</nav>

{#if filtered.length === 0}
	<p class="notice notice--info no-results" role="status">
		No tools match “{query.trim()}”. Try a different word, like “pdf” or “convert”.
	</p>
{:else}
	{#each filtered as group (group.category)}
		<section class="category" id={anchorId(group.category)} aria-labelledby={`${anchorId(group.category)}-h`}>
			<h2 id={`${anchorId(group.category)}-h`}>{group.label}</h2>
			<div class="grid grid--tools">
				{#each group.tools as tool (tool.id)}
					<ToolCard {tool} />
				{/each}
			</div>
		</section>
	{/each}
{/if}

<style>
	.hero {
		text-align: center;
		max-width: 44rem;
		margin: 1rem auto 2.5rem;
	}
	.hero h1 {
		font-size: clamp(1.9rem, 5vw, 2.8rem);
	}
	.lede {
		color: var(--ink-soft);
		font-size: 1.1rem;
		margin: 0 auto 1.75rem;
		max-width: 34rem;
	}
	.search {
		position: relative;
		max-width: 32rem;
		margin: 0 auto;
	}
	.search-icon {
		position: absolute;
		left: 0.9rem;
		top: 50%;
		transform: translateY(-50%);
		font-size: 1.2rem;
		color: var(--ink-soft);
	}
	.search input {
		padding-left: 2.5rem;
		min-height: 52px;
		font-size: 1.05rem;
		box-shadow: var(--shadow);
	}
	.cat-nav {
		margin: 0 0 2rem;
	}
	.cat-nav ul {
		list-style: none;
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		padding: 0;
		margin: 0;
		justify-content: center;
	}
	.cat-nav a {
		display: inline-block;
		padding: 0.4rem 0.9rem;
		border-radius: 999px;
		background: var(--surface);
		border: 1px solid var(--border);
		color: var(--ink);
		text-decoration: none;
		font-weight: 600;
		font-size: 0.9rem;
	}
	.cat-nav a:hover {
		border-color: var(--accent);
		color: var(--accent);
	}
	.category {
		margin-bottom: 2.5rem;
		scroll-margin-top: 6rem;
	}
	.category h2 {
		font-size: 1.4rem;
		margin-bottom: 1rem;
	}
	.no-results {
		text-align: center;
	}
</style>

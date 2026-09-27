<script lang="ts">
	// Feature: pdf-tools-suite (Task 10.3) — per-tool option UIs.
	//
	// One componentized panel per tool, selected by tool id. Each renders a clean
	// control per option and mutates the bound `state` (ToolOptionState). Page
	// counts / thumbnails come from `pageCount` (0 when unknown, e.g. before the
	// engine reports it). Requirement clauses noted inline.

	import type { ToolId } from '$lib/tools/registry';
	import type { ToolOptionState } from '$lib/workspace/options';
	import type { SourceItem } from '$lib/workspace/files';
	import { isHttpUrl } from '$lib/util/sanitize';

	export let toolId: ToolId;
	export let state: ToolOptionState;
	export let items: SourceItem[] = [];
	/** Known page count of the (first) source, or 0 when not yet known. */
	export let pageCount = 0;

	// --- Reorder helpers (Merge / JPG to PDF ordering: Req 4.3, 12.4) ---
	function move(index: number, delta: number) {
		const order = state.order ?? items.map((_, i) => i);
		const to = index + delta;
		if (to < 0 || to >= order.length) return;
		const next = [...order];
		[next[index], next[to]] = [next[to], next[index]];
		state = { ...state, order: next };
	}

	// --- Page selection parsing (Remove/Extract: Req 6.1, 7.1, 7.2) ---
	let pagesText = '';
	function parsePages(text: string): number[] {
		const out = new Set<number>();
		for (const part of text.split(',')) {
			const token = part.trim();
			if (token === '') continue;
			const range = token.match(/^(\d+)\s*-\s*(\d+)$/);
			if (range) {
				const start = parseInt(range[1], 10);
				const end = parseInt(range[2], 10);
				for (let p = Math.min(start, end); p <= Math.max(start, end); p++) out.add(p);
			} else if (/^\d+$/.test(token)) {
				out.add(parseInt(token, 10));
			}
		}
		return [...out].sort((a, b) => a - b);
	}
	function onPagesInput() {
		state = { ...state, pages: parsePages(pagesText) };
	}

	// --- Split points parsing (Req 5.1, 5.2) ---
	let splitText = '';
	function onSplitInput() {
		state = { ...state, splitPoints: parsePages(splitText) };
	}

	// --- Organize: rotate/delete per page (Req 8.1, 8.3, 8.4) ---
	function organizePages(): number[] {
		return state.order && state.order.length > 0
			? state.order
			: Array.from({ length: Math.max(pageCount, items.length) }, (_, i) => i);
	}
	function rotatePage(page: number) {
		const rotations = [...(state.rotations ?? [])];
		const existing = rotations.findIndex(([p]) => p === page);
		// Cycle 90 -> 180 -> 270 -> remove.
		const order = ['D90', 'D180', 'D270'] as const;
		if (existing < 0) {
			rotations.push([page, 'D90']);
		} else {
			const cur = order.indexOf(rotations[existing][1]);
			if (cur >= order.length - 1) rotations.splice(existing, 1);
			else rotations[existing] = [page, order[cur + 1]];
		}
		state = { ...state, rotations };
	}
	function rotationOf(page: number): string {
		const r = (state.rotations ?? []).find(([p]) => p === page);
		return r ? `${r[1].slice(1)}°` : '0°';
	}
	function toggleDelete(page: number) {
		const deletes = new Set(state.deletes ?? []);
		if (deletes.has(page)) deletes.delete(page);
		else deletes.add(page);
		state = { ...state, deletes: [...deletes] };
	}

	// --- Forms: add/remove field-value rows (Req 29.1, 29.2) ---
	function inputValue(event: Event): string {
		return (event.currentTarget as HTMLInputElement).value;
	}
	function addFieldRow() {
		state = { ...state, fieldValues: [...(state.fieldValues ?? []), ['', '']] };
	}
	function updateFieldRow(i: number, key: string, value: string) {
		const rows = [...(state.fieldValues ?? [])];
		rows[i] = [key, value];
		state = { ...state, fieldValues: rows };
	}
	function removeFieldRow(i: number) {
		const rows = [...(state.fieldValues ?? [])];
		rows.splice(i, 1);
		state = { ...state, fieldValues: rows };
	}

	// --- Edit PDF: add/move/delete elements (Req 28.1-28.6) ---
	interface EditElement {
		id: number;
		kind: 'text' | 'image' | 'shape';
		text: string;
		x: number;
		y: number;
	}
	let editSeq = 0;
	$: editElements = (state.elements ?? []) as unknown as EditElement[];
	function addElement(kind: EditElement['kind']) {
		editSeq += 1;
		const el: EditElement = { id: editSeq, kind, text: kind === 'text' ? 'Text' : '', x: 50, y: 50 };
		state = { ...state, elements: [...editElements, el] as unknown[] };
	}
	function updateElement(id: number, patch: Partial<EditElement>) {
		state = {
			...state,
			elements: editElements.map((e) => (e.id === id ? { ...e, ...patch } : e)) as unknown[]
		};
	}
	function deleteElement(id: number) {
		state = { ...state, elements: editElements.filter((e) => e.id !== id) as unknown[] };
	}

	$: orderedItems = (state.order ?? items.map((_, i) => i)).map((idx) => ({ idx, item: items[idx] }));
	$: urlInvalid = toolId === 'HtmlToPdf' && (state.htmlUrl ?? '') !== '' && !isHttpUrl(state.htmlUrl ?? '');

	// Crop region: work on a guaranteed object and write it back on each change.
	function crop() {
		return state.crop ?? { x: 0, y: 0, width: 100, height: 100 };
	}
	function setCrop(part: Partial<{ x: number; y: number; width: number; height: number }>) {
		state = { ...state, crop: { ...crop(), ...part } };
	}
</script>

<div class="options">
	{#if toolId === 'Merge' || toolId === 'JpgToPdf'}
		<!-- Reorder sources (Req 4.3, 12.4). JPG to PDF also has orientation/margin. -->
		{#if items.length > 0}
			<div class="field">
				<span class="opt-label">Order</span>
				<p class="hint">Arrange the files in the order they should appear.</p>
				<ol class="reorder">
					{#each orderedItems as entry, i (entry.idx)}
						<li>
							<span class="reorder-name">{entry.item?.name ?? `File ${entry.idx + 1}`}</span>
							<span class="reorder-btns">
								<button class="btn btn--ghost" on:click={() => move(i, -1)} disabled={i === 0} aria-label={`Move ${entry.item?.name ?? 'file'} up`}>↑</button>
								<button class="btn btn--ghost" on:click={() => move(i, 1)} disabled={i === orderedItems.length - 1} aria-label={`Move ${entry.item?.name ?? 'file'} down`}>↓</button>
							</span>
						</li>
					{/each}
				</ol>
			</div>
		{/if}
	{/if}

	{#if toolId === 'JpgToPdf'}
		<div class="field">
			<label for="jpg-orientation">Page orientation</label>
			<select id="jpg-orientation" bind:value={state.orientation}>
				<option value="Portrait">Portrait</option>
				<option value="Landscape">Landscape</option>
			</select>
		</div>
		<div class="field">
			<label for="jpg-margin">Page margin</label>
			<select id="jpg-margin" bind:value={state.margin}>
				<option value="None">None</option>
				<option value="Small">Small</option>
				<option value="Large">Large</option>
			</select>
		</div>
	{/if}

	{#if toolId === 'Split'}
		<fieldset class="field">
			<legend>Split method</legend>
			<label class="radio"><input type="radio" bind:group={state.splitMode} value="points" /> By split points</label>
			<label class="radio"><input type="radio" bind:group={state.splitMode} value="fixed" /> Fixed-size ranges</label>
		</fieldset>
		{#if state.splitMode === 'points'}
			<div class="field">
				<label for="split-points">Split after these page numbers</label>
				<input id="split-points" type="text" placeholder="e.g. 3, 7, 10" bind:value={splitText} on:input={onSplitInput} />
				<p class="hint">Each split point starts a new file. {#if pageCount}This PDF has {pageCount} pages.{/if}</p>
			</div>
		{:else}
			<div class="field">
				<label for="fixed-size">Pages per file</label>
				<input id="fixed-size" type="number" min="1" bind:value={state.fixedSize} />
			</div>
		{/if}
	{/if}

	{#if toolId === 'RemovePages' || toolId === 'ExtractPages'}
		<div class="field">
			<label for="pages">
				{toolId === 'RemovePages' ? 'Pages to remove' : 'Pages to extract'}
			</label>
			<input id="pages" type="text" placeholder="e.g. 1, 3, 5-8" bind:value={pagesText} on:input={onPagesInput} />
			{#if toolId === 'RemovePages' && (state.pages?.length ?? 0) === 0}
				<p class="hint hint--note">Make sure at least one page remains after removal.</p>
			{/if}
			<p class="hint">Select individual pages and ranges. {#if pageCount}Pages 1–{pageCount}.{/if}</p>
		</div>
	{/if}

	{#if toolId === 'Organize'}
		<div class="field">
			<span class="opt-label">Pages</span>
			<p class="hint">Reorder, rotate, or remove pages. Rotate cycles 90° → 180° → 270°.</p>
			<ol class="thumbs">
				{#each organizePages() as page, i (page)}
					<li class="thumb-card" class:deleted={(state.deletes ?? []).includes(page)}>
						<span class="thumb-preview" aria-hidden="true">{page + 1}</span>
						<span class="thumb-meta">Page {page + 1} · {rotationOf(page)}</span>
						<span class="thumb-actions">
							<button class="btn btn--ghost" on:click={() => move(i, -1)} disabled={i === 0} aria-label={`Move page ${page + 1} earlier`}>↑</button>
							<button class="btn btn--ghost" on:click={() => move(i, 1)} disabled={i === organizePages().length - 1} aria-label={`Move page ${page + 1} later`}>↓</button>
							<button class="btn btn--ghost" on:click={() => rotatePage(page)} aria-label={`Rotate page ${page + 1}`}>⟳</button>
							<button class="btn btn--ghost" on:click={() => toggleDelete(page)} aria-label={`Toggle delete page ${page + 1}`}>🗑</button>
						</span>
					</li>
				{/each}
			</ol>
			{#if organizePages().length === 0}
				<p class="hint">Add a PDF to see its pages here.</p>
			{/if}
		</div>
	{/if}

	{#if toolId === 'OptimizePdf' || toolId === 'CompressPdf'}
		<div class="field">
			<label for="level">{toolId === 'OptimizePdf' ? 'Optimization' : 'Compression'} level</label>
			<select id="level" bind:value={state.level}>
				<option value="Low">Low — best quality</option>
				<option value="Medium">Medium — balanced</option>
				<option value="High">High — smallest size</option>
			</select>
			<p class="hint">You’ll see the size {toolId === 'CompressPdf' ? 'reduction' : 'before and after'} once it finishes.</p>
		</div>
	{/if}

	{#if toolId === 'PdfToJpg'}
		<div class="field">
			<label for="dpi">Image resolution (DPI)</label>
			<select id="dpi" bind:value={state.dpi}>
				<option value={72}>72 — screen</option>
				<option value={150}>150 — standard</option>
				<option value={300}>300 — print</option>
			</select>
		</div>
	{/if}

	{#if toolId === 'Rotate'}
		<div class="field">
			<label for="angle">Rotation angle</label>
			<select id="angle" bind:value={state.angle}>
				<option value="D90">90° clockwise</option>
				<option value="D180">180°</option>
				<option value="D270">270° clockwise</option>
			</select>
		</div>
		<fieldset class="field">
			<legend>Apply to</legend>
			<label class="radio"><input type="radio" bind:group={state.rotateScope} value="all" /> All pages</label>
			<label class="radio"><input type="radio" bind:group={state.rotateScope} value="selected" /> Selected pages</label>
		</fieldset>
		{#if state.rotateScope === 'selected'}
			<div class="field">
				<label for="rotate-pages">Pages to rotate</label>
				<input id="rotate-pages" type="text" placeholder="e.g. 1, 4-6" bind:value={pagesText} on:input={onPagesInput} />
			</div>
		{/if}
	{/if}

	{#if toolId === 'AddPageNumbers'}
		<div class="field">
			<label for="position">Position</label>
			<select id="position" bind:value={state.position}>
				<option value="TopLeft">Top left</option>
				<option value="TopCenter">Top center</option>
				<option value="TopRight">Top right</option>
				<option value="BottomLeft">Bottom left</option>
				<option value="BottomCenter">Bottom center</option>
				<option value="BottomRight">Bottom right</option>
			</select>
		</div>
		<div class="field">
			<label for="start-number">Starting number</label>
			<input id="start-number" type="number" min="0" bind:value={state.startNumber} />
		</div>
	{/if}

	{#if toolId === 'AddWatermark'}
		<div class="field">
			<label for="wm-text">Watermark text</label>
			<input id="wm-text" type="text" bind:value={state.watermarkText} placeholder="e.g. DRAFT" />
			<p class="hint">Image watermarks can be added on the server; text works everywhere.</p>
		</div>
		<div class="field">
			<label for="wm-opacity">Opacity: {state.opacity}%</label>
			<input id="wm-opacity" type="range" min="0" max="100" bind:value={state.opacity} />
		</div>
		<div class="field">
			<label for="wm-rotation">Rotation (degrees)</label>
			<input id="wm-rotation" type="number" min="-360" max="360" bind:value={state.rotationDeg} />
		</div>
	{/if}

	{#if toolId === 'Crop'}
		<div class="field">
			<span class="opt-label">Crop region</span>
			<p class="hint">Set the region to keep (percent of the page). A visual selector appears on the page preview.</p>
			<div class="crop-grid">
				<label>X% <input type="number" min="0" max="100" value={crop().x} on:input={(e) => setCrop({ x: +inputValue(e) })} /></label>
				<label>Y% <input type="number" min="0" max="100" value={crop().y} on:input={(e) => setCrop({ y: +inputValue(e) })} /></label>
				<label>Width% <input type="number" min="1" max="100" value={crop().width} on:input={(e) => setCrop({ width: +inputValue(e) })} /></label>
				<label>Height% <input type="number" min="1" max="100" value={crop().height} on:input={(e) => setCrop({ height: +inputValue(e) })} /></label>
			</div>
		</div>
		<label class="checkbox"><input type="checkbox" bind:checked={state.cropAllPages} /> Apply to all pages</label>
	{/if}

	{#if toolId === 'MarkdownToPdf'}
		<div class="field">
			<label for="md">Markdown</label>
			<textarea id="md" bind:value={state.markdownText} placeholder="# Heading&#10;&#10;- list item&#10;- another"></textarea>
			<p class="hint">Type Markdown directly, or add a .md file above.</p>
		</div>
	{/if}

	{#if toolId === 'HtmlToPdf'}
		<div class="field">
			<label for="html-url">Web page URL</label>
			<input id="html-url" type="url" inputmode="url" bind:value={state.htmlUrl} placeholder="https://example.com" aria-invalid={urlInvalid} />
			{#if urlInvalid}
				<p class="hint hint--error">Enter a valid http(s) URL.</p>
			{:else}
				<p class="hint">Only http and https URLs are allowed.</p>
			{/if}
		</div>
		<div class="field">
			<label for="html-orientation">Orientation</label>
			<select id="html-orientation" bind:value={state.orientation}>
				<option value="Portrait">Portrait</option>
				<option value="Landscape">Landscape</option>
			</select>
		</div>
	{/if}

	{#if toolId === 'ExcelToPdf'}
		<div class="field">
			<label for="xls-orientation">Orientation</label>
			<select id="xls-orientation" bind:value={state.orientation}>
				<option value="Portrait">Portrait</option>
				<option value="Landscape">Landscape</option>
			</select>
		</div>
	{/if}

	{#if toolId === 'PdfToPdfA'}
		<div class="field">
			<label for="pdfa-level">PDF/A conformance level</label>
			<select id="pdfa-level" bind:value={state.pdfaLevel}>
				<option value="A1b">PDF/A-1b</option>
				<option value="A2b">PDF/A-2b</option>
				<option value="A3b">PDF/A-3b</option>
			</select>
		</div>
	{/if}

	{#if toolId === 'ScanToPdf'}
		<!-- Camera capture with upload fallback handled by ScanCapture (Req 9.1, 9.4). -->
		{#if items.length > 0}
			<div class="field">
				<span class="opt-label">Image order</span>
				<ol class="reorder">
					{#each orderedItems as entry, i (entry.idx)}
						<li>
							<span class="reorder-name">{entry.item?.name ?? `Image ${entry.idx + 1}`}</span>
							<span class="reorder-btns">
								<button class="btn btn--ghost" on:click={() => move(i, -1)} disabled={i === 0} aria-label="Move image up">↑</button>
								<button class="btn btn--ghost" on:click={() => move(i, 1)} disabled={i === orderedItems.length - 1} aria-label="Move image down">↓</button>
							</span>
						</li>
					{/each}
				</ol>
			</div>
		{/if}
		<label class="checkbox"><input type="checkbox" bind:checked={state.ocr} /> Add searchable text (OCR)</label>
		<p class="hint">OCR runs securely on the server.</p>
	{/if}

	{#if toolId === 'PdfForms'}
		<div class="field">
			<span class="opt-label">Form fields</span>
			<p class="hint">Existing fields load here. Add text, checkbox, or dropdown fields below.</p>
			{#each state.fieldValues ?? [] as row, i (i)}
				<div class="kv-row">
					<input type="text" placeholder="Field name" value={row[0]} on:input={(e) => updateFieldRow(i, inputValue(e), row[1])} aria-label="Field name" />
					<input type="text" placeholder="Value" value={row[1]} on:input={(e) => updateFieldRow(i, row[0], inputValue(e))} aria-label="Field value" />
					<button class="btn btn--ghost" on:click={() => removeFieldRow(i)} aria-label="Remove field">✕</button>
				</div>
			{/each}
			<button class="btn" on:click={addFieldRow}>+ Add field</button>
		</div>
	{/if}

	{#if toolId === 'EditPdf'}
		<div class="field">
			<span class="opt-label">Elements</span>
			<p class="hint">Add text, image, or shape elements, then position or delete them.</p>
			<div class="add-elements">
				<button class="btn" on:click={() => addElement('text')}>+ Text</button>
				<button class="btn" on:click={() => addElement('image')}>+ Image</button>
				<button class="btn" on:click={() => addElement('shape')}>+ Shape</button>
			</div>
			{#each editElements as el (el.id)}
				<div class="element-row">
					<span class="element-kind">{el.kind}</span>
					{#if el.kind === 'text'}
						<input type="text" value={el.text} on:input={(e) => updateElement(el.id, { text: inputValue(e) })} aria-label="Element text" />
					{/if}
					<label class="pos">x% <input type="number" min="0" max="100" value={el.x} on:input={(e) => updateElement(el.id, { x: +inputValue(e) })} /></label>
					<label class="pos">y% <input type="number" min="0" max="100" value={el.y} on:input={(e) => updateElement(el.id, { y: +inputValue(e) })} /></label>
					<button class="btn btn--ghost" on:click={() => deleteElement(el.id)} aria-label="Delete element">✕</button>
				</div>
			{/each}
		</div>
	{/if}
</div>

<style>
	.options {
		display: flex;
		flex-direction: column;
	}
	.opt-label {
		font-weight: 600;
		font-size: 0.95rem;
	}
	.hint--error {
		color: var(--danger);
	}
	.hint--note {
		font-weight: 500;
		color: var(--ink);
	}
	.radio,
	.checkbox {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		min-height: 40px;
		font-weight: 500;
	}
	fieldset.field {
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		padding: 0.6rem 0.9rem;
	}
	fieldset.field legend {
		font-weight: 600;
		padding: 0 0.35rem;
	}
	.reorder,
	.thumbs {
		list-style: none;
		margin: 0.5rem 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
		counter-reset: item;
	}
	.reorder li {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.6rem;
		padding: 0.5rem 0.7rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface);
	}
	.reorder-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.reorder-btns .btn,
	.thumb-actions .btn {
		min-height: 36px;
		padding: 0.25rem 0.55rem;
	}
	.thumbs {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr));
		gap: 0.75rem;
	}
	.thumb-card {
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface);
		padding: 0.6rem;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.4rem;
	}
	.thumb-card.deleted {
		opacity: 0.45;
		outline: 2px dashed var(--danger);
	}
	.thumb-preview {
		display: grid;
		place-items: center;
		width: 100%;
		aspect-ratio: 3 / 4;
		background: var(--surface-2);
		border-radius: 6px;
		font-weight: 700;
		color: var(--ink-soft);
		font-size: 1.3rem;
	}
	.thumb-meta {
		font-size: 0.8rem;
		color: var(--ink-soft);
	}
	.thumb-actions {
		display: flex;
		gap: 0.2rem;
	}
	.crop-grid {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: 0.6rem;
	}
	.crop-grid label {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		font-size: 0.85rem;
		font-weight: 600;
	}
	.kv-row,
	.element-row {
		display: flex;
		gap: 0.5rem;
		align-items: center;
		margin-bottom: 0.5rem;
		flex-wrap: wrap;
	}
	.kv-row input {
		flex: 1;
		min-width: 8rem;
	}
	.add-elements {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 0.75rem;
		flex-wrap: wrap;
	}
	.element-kind {
		text-transform: capitalize;
		font-weight: 600;
		min-width: 3.5rem;
	}
	.pos {
		display: flex;
		align-items: center;
		gap: 0.3rem;
		font-size: 0.85rem;
	}
	.pos input {
		width: 5rem;
	}
</style>

import { describe, expect, it } from 'vitest';
import fc from 'fast-check';
import { renameOutputs } from './options';
import type { OutputFile } from '../engine/types';
import { fileExtension } from '../util/format';

const out = (name: string): OutputFile => ({ name, bytes: new Uint8Array() });

describe('renameOutputs', () => {
	it('names a single output <base>_<suffix><ext> from the first source', () => {
		const result = renameOutputs('RemovePages', ['report.pdf'], [out('removed.pdf')]);
		expect(result.map((o) => o.name)).toEqual(['report_removed.pdf']);
	});

	it('keeps the engine output extension, not the source extension', () => {
		// PDF to JPG: source is a .pdf, outputs are .jpg pages. A single output
		// gets no _<n> index.
		const result = renameOutputs('PdfToJpg', ['scan.pdf'], [out('page-1.jpg')]);
		expect(result[0].name).toBe('scan_page.jpg');
	});

	it('indexes multiple .jpg pages for PdfToJpg', () => {
		const result = renameOutputs('PdfToJpg', ['scan.pdf'], [out('p1.jpg'), out('p2.jpg')]);
		expect(result.map((o) => o.name)).toEqual(['scan_page_1.jpg', 'scan_page_2.jpg']);
	});

	it('indexes multiple outputs from 1 and preserves order', () => {
		const result = renameOutputs('Split', ['report.pdf'], [
			out('a.pdf'),
			out('b.pdf'),
			out('c.pdf')
		]);
		expect(result.map((o) => o.name)).toEqual([
			'report_split_1.pdf',
			'report_split_2.pdf',
			'report_split_3.pdf'
		]);
	});

	it('uses the first source name when several are provided (Merge)', () => {
		const result = renameOutputs('Merge', ['first.pdf', 'second.pdf'], [out('merged.pdf')]);
		expect(result[0].name).toBe('first_merged.pdf');
	});

	it('falls back to a per-tool base when there is no source (MarkdownToPdf)', () => {
		const result = renameOutputs('MarkdownToPdf', [], [out('document.pdf')]);
		expect(result[0].name).toBe('document_document.pdf');
	});

	it('falls back to webpage for HtmlToPdf with no source', () => {
		const result = renameOutputs('HtmlToPdf', [], [out('page.pdf')]);
		expect(result[0].name).toBe('webpage_converted.pdf');
	});

	it('uses the engine output base when there is no source and no per-tool fallback', () => {
		// A single-source client tool given an empty source list falls back to the
		// output's own base name.
		const result = renameOutputs('Rotate', [], [out('rotated.pdf')]);
		expect(result[0].name).toBe('rotated_rotated.pdf');
	});

	it('sanitizes names so they carry no path separators', () => {
		const result = renameOutputs('RemovePages', ['../etc/passwd.pdf'], [out('x.pdf')]);
		expect(result[0].name).not.toContain('/');
		expect(result[0].name).not.toContain('\\');
	});

	it('preserves extensionless output names', () => {
		const result = renameOutputs('RemovePages', ['report.pdf'], [out('noext')]);
		expect(result[0].name).toBe('report_removed');
	});

	// Property: every renamed output keeps the original extension of its engine
	// output, and multi-output names are all unique. Validates: Improvement 1.
	it('Property: preserves each output extension and keeps multi-output names unique', () => {
		const extArb = fc.constantFrom('pdf', 'jpg', 'md', 'zip');
		const outputArb = fc
			.tuple(fc.string({ minLength: 1, maxLength: 8 }), extArb)
			.map(([stem, ext]) => out(`${stem}.${ext}`));

		fc.assert(
			fc.property(
				fc.constantFrom(
					'RemovePages',
					'Split',
					'PdfToJpg',
					'Merge',
					'Rotate'
				) as fc.Arbitrary<'RemovePages' | 'Split' | 'PdfToJpg' | 'Merge' | 'Rotate'>,
				fc.array(fc.string({ minLength: 1, maxLength: 10 }).map((s) => `${s}.pdf`), {
					maxLength: 3
				}),
				fc.array(outputArb, { minLength: 1, maxLength: 5 }),
				(toolId, sources, outputs) => {
					const result = renameOutputs(toolId, sources, outputs);

					// Same count and order length.
					expect(result).toHaveLength(outputs.length);

					// Each keeps its original extension.
					for (let i = 0; i < outputs.length; i++) {
						expect(fileExtension(result[i].name)).toBe(fileExtension(outputs[i].name));
					}

					// Multi-output names are unique.
					const names = result.map((o) => o.name);
					expect(new Set(names).size).toBe(names.length);
				}
			)
		);
	});
});

/// <reference types="vitest" />
import { sveltekit } from '@sveltejs/kit/vite';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	// `svelteTesting` adds `browser` to resolve.conditions and an automatic
	// component cleanup fixture so `@testing-library/svelte` can render real
	// components under jsdom (Task 22.6). The default test environment stays
	// `node` so the fast pure-logic suites (search, no-network) are unaffected;
	// the accessibility/responsive suites opt into jsdom per-file via a
	// `// @vitest-environment jsdom` directive.
	plugins: [sveltekit(), svelteTesting()],
	test: {
		include: ['src/**/*.{test,spec}.{js,ts}'],
		environment: 'node'
	}
});

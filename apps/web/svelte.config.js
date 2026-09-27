import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/**
 * Static adapter: the SvelteKit app and the WASM engine are served as static
 * assets (Cloudflare Pages per the design). Privacy Mode tools run entirely
 * client-side.
 * @type {import('@sveltejs/kit').Config}
 */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		adapter: adapter({
			fallback: 'index.html'
		})
	}
};

export default config;

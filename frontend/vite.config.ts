import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	resolve: {
		alias: {
			'svelte-i18n': '/src/lib/i18n/svelte-i18n.ts'
		}
	}
});

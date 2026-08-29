import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	resolve: {
		alias: {
			'svelte-i18n': '/src/lib/i18n/svelte-i18n.ts',
			// `loro-crdt`'s conditional `browser` export map resolves `bundler/index.js` (an ESM
			// wasm-bindgen import Vite's dev pre-transform cannot handle) under `vite dev` and
			// `browser/index.js` (the synchronous-XHR entry, no top-level await) under `vite build`.
			// Forcing the browser entry in both modes keeps engine wiring identical dev vs prod.
			// See spikes/bundle-loro/vite.config.ts and contracts/ui-surface-v1.md item 1/2.
			'loro-crdt': 'loro-crdt/browser'
		}
	},
	build: {
		// contracts/ui-surface-v1.md item 2: the browser matrix this app targets supports
		// top-level await, and loro-crdt's browser entry deliberately avoids TLA anyway.
		target: 'es2022'
	},
	optimizeDeps: {
		// Never let Vite's dev-server esbuild pre-bundling pass re-process the wasm-bindgen glue
		// or the loro-prosemirror binding (contracts/ui-surface-v1.md item 3).
		exclude: ['loro-crdt', 'loro-prosemirror']
	}
});

import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

/**
 * ADR-0015 R4 `engine_runtime_ready` dedicated build for the `yrs-yjs` candidate's adapter-only
 * probe entry. See `vite.config.loro.mjs` for the full rationale -- identical shape, other
 * candidate. `yjs` has no WASM and no export-condition aliasing quirk (see
 * `src/yrs-yjs-engine-only-entry.ts`'s header), so this config is simpler: just an absolute-path
 * alias into `bundle-yrs-yjs`'s own already-installed `node_modules/yjs`.
 */
export default defineConfig({
  resolve: {
    alias: {
      yjs: resolve(HERE, "..", "..", "bundle-yrs-yjs", "node_modules", "yjs"),
    },
  },
  build: {
    target: "es2022",
    outDir: resolve(HERE, "dist-yrs-yjs"),
    emptyOutDir: true,
    // See vite.config.loro.mjs's header/inline comments: NOT build.lib (forces base64-inlined
    // assets, doesn't apply to this WASM-free candidate but kept symmetric for the same reasons),
    // and preserveEntrySignatures is required or Vite's default non-lib tree-shaking removes this
    // entry's exports entirely (confirmed empirically on the loro config; this one is analogous).
    rollupOptions: {
      input: { "yrs-yjs-engine-only-entry": resolve(HERE, "src", "yrs-yjs-engine-only-entry.ts") },
      preserveEntrySignatures: "exports-only",
      output: {
        entryFileNames: "[name].js",
        chunkFileNames: "[name]-[hash].js",
        assetFileNames: "[name]-[hash][extname]",
      },
    },
  },
});

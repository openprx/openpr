import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

/**
 * ADR-0015 R4 `engine_runtime_ready` dedicated build for the `loro` candidate's adapter-only probe
 * entry (coordinator directive 2026-08-29 item 2: "给 engine_runtime_ready 用专用 entry").
 *
 * Deliberately does NOT reuse `../../bundle-loro/vite.config.ts` -- that config's `manualChunks`
 * exists only to split out `prosemirror-model`/`state`/`view`, none of which this build's entry
 * (`src/loro-engine-only-entry.ts`) imports at all, so there is nothing for that logic to do here.
 * Keeping a separate, minimal config makes it mechanically checkable (see
 * `check-module-graph.mjs`) that this build's own config contains no ProseMirror reference either.
 *
 * `resolve.alias` mirrors `bundle-loro/vite.config.ts`'s item exactly, but points at an ABSOLUTE
 * path into `bundle-loro`'s own already-installed `node_modules/loro-crdt` rather than adding a
 * second copy of the dependency under this spike -- same package, same version, same bytes,
 * same install (verifiable: this file's `loro-crdt` resolves to the identical inode as the one
 * `packageLockfileHash("bundle-loro")` covers), not a separately-pinned/potentially-drifted copy.
 * `bundle-loro/` is read-only from this delivery's file scope; nothing under it is modified by
 * this config or by running this build.
 */
export default defineConfig({
  resolve: {
    alias: {
      "loro-crdt": resolve(HERE, "..", "..", "bundle-loro", "node_modules", "loro-crdt", "browser"),
    },
  },
  build: {
    target: "es2022",
    outDir: resolve(HERE, "dist-loro"),
    emptyOutDir: true,
    assetsInlineLimit: 0, // keep the .wasm as a real fetched asset, matching bundle-loro/vite.config.ts.
    // Deliberately NOT `build.lib`: Vite's library-mode asset handling force-inlines any
    // `new URL(asset, import.meta.url)` reference (such as loro-crdt's own internal wasm URL) as a
    // base64 data: URI regardless of `assetsInlineLimit` -- confirmed empirically here (a first
    // attempt using `build.lib` produced a single 4.4MB chunk with the .wasm embedded as base64,
    // NOT the real bundle-loro/dist shape of a separate ~3.1MB .wasm asset file). Using a plain
    // `rollupOptions.input` entry (the same "app build" mode bundle-loro/vite.config.ts itself
    // uses) instead reproduces the real candidate's asset-splitting behavior faithfully, which
    // matters here because ADR-0015 3.3's "候选专属资源的原始bytes计时前已在本地内存中可用" +
    // "分段耗时" requirements are specifically about a REAL separate WASM fetch/compile/instantiate
    // step, not an inlined base64 blob parsed as part of JS source text.
    rollupOptions: {
      input: { "loro-engine-only-entry": resolve(HERE, "src", "loro-engine-only-entry.ts") },
      // Vite's non-lib build defaults `preserveEntrySignatures: false` (correct for an HTML page's
      // script, which has no meaningful "exports"), which silently tree-shook away this entry's
      // exported functions entirely (confirmed empirically: the first attempt without this
      // produced a chunk with zero `export` statements and no trace of `initEngineDoc` et al. in
      // the output). Since the Node-side runner needs to call these exports via `import()`, force
      // Rollup to keep them.
      preserveEntrySignatures: "exports-only",
      output: {
        entryFileNames: "[name].js",
        chunkFileNames: "[name]-[hash].js",
        assetFileNames: "[name]-[hash][extname]",
      },
    },
  },
  optimizeDeps: {
    exclude: ["loro-crdt"],
  },
});

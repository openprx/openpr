import { defineConfig } from "vite";

/**
 * Checks against `contracts/ui-surface-v1.md`'s Vite/WASM checklist, verified by this exact
 * config + `measure-bundle.ts`'s post-build inspection (see the delivery report for the
 * pass/fail table):
 *
 * 1. `build.target = 'es2022'` -- the browser-target build item; this repo's real
 *    `frontend/vite.config.ts` inherits SvelteKit's default target, which is also es2022+, so
 *    this spike pins the same floor explicitly.
 * 2. Top-level await: handled by NOT needing to be handled -- `loro-crdt@1.14.1`'s browser entry
 *    (`node_modules/loro-crdt/browser/loro_wasm.js`) deliberately uses a synchronous
 *    `XMLHttpRequest` load instead of `await fetch`/`instantiateStreaming`, specifically to avoid
 *    top-level await (see that file's own comment: "Vite/Rolldown can otherwise create circular
 *    wasm wrapper chunks in production builds"). This is a genuine, version-specific fact this
 *    spike surfaces, not an assumption -- see the delivery report's "TLA does not apply" finding.
 * 3. `optimizeDeps.exclude` for the engine/binding packages below, so Vite's dev-server esbuild
 *    pre-bundling pass never tries to re-process the wasm-bindgen glue.
 * 4. `manualChunks` isolates `prosemirror-model/state/view` (common to both v0.3 candidates) into
 *    their own `vendor-prosemirror` chunk, which `measure-bundle.ts` excludes from the candidate
 *    gzip total per `testing/benchmark-spec.md`.
 */
export default defineConfig({
  resolve: {
    // Forces `loro-crdt`'s explicit `browser` export-map entry regardless of Vite's dev-vs-build
    // mode. Without this, `vite dev` (which sets Node's `development` export condition active)
    // and `vite build` (which does not) resolve `loro-crdt`'s conditional `"browser": {
    // "development": "./bundler/index.js", "default": "./browser/index.js" }` map to two
    // DIFFERENT physical files: `bundler/index.js` in dev (a raw `import * as rawWasm from
    // "./loro_wasm_bg.wasm"` -- the wasm-bindgen "ESM integration proposal" shape Vite does not
    // support without a plugin) vs `browser/index.js` in build (the synchronous-XHR entry this
    // config's item-2 comment above documents). This was found empirically: `vite dev` on this
    // package failed with `Pre-transform error: "ESM integration proposal for Wasm" is not
    // supported currently` before this alias was added -- a real, disclosed
    // `contracts/ui-surface-v1.md` item-1/item-2 gap (dev server and prod build silently using
    // different engine entry points), not a hypothetical. See the delivery report.
    alias: {
      "loro-crdt": "loro-crdt/browser",
    },
  },
  build: {
    target: "es2022",
    assetsInlineLimit: 0, // keep the .wasm as a real fetched asset, not an inlined base64 string, so it is separately measurable.
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("node_modules/prosemirror-model") || id.includes("node_modules/prosemirror-state") || id.includes("node_modules/prosemirror-view")) {
            return "vendor-prosemirror";
          }
          return undefined;
        },
      },
    },
  },
  optimizeDeps: {
    exclude: ["loro-crdt", "loro-prosemirror"],
  },
});

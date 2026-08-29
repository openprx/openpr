import { defineConfig } from "vite";

/**
 * See `bundle-loro/vite.config.ts` for the full `ui-surface-v1.md` checklist rationale. This
 * candidate has no WASM at all (`yjs`/`y-prosemirror` are pure JS -- see
 * `testing/benchmark-spec.md`'s explicit note that the Yjs browser side is pure JS), so items 2
 * (top-level await) and the WASM half of item 4 (asset URL loading) do not apply here; this
 * config still sets the same `build.target`/`optimizeDeps.exclude`/`manualChunks` shape as the
 * loro candidate so the two builds are genuinely comparable.
 */
export default defineConfig({
  build: {
    target: "es2022",
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
    exclude: ["yjs", "y-prosemirror", "y-protocols"],
  },
});

/**
 * Minimal client-side router standing in for `frontend`'s adapter-static + `fallback:
 * 'index.html'` SPA shell (see `frontend/svelte.config.js`): a static host serves this same
 * `index.html` for any unknown deep path, and THIS script -- not the static host -- decides what
 * to render for that path.
 *
 * The `/flow/*` branch is the only one that ever imports the CRDT engine, and it does so via a
 * dynamic `import()` -- exactly the `ui-surface-v1.md` requirement that engine code is not
 * statically bundled into the app shell and that non-Flow routes never pull the engine chunk (see
 * `home-entry.ts`, which has no engine import at all).
 */
const app = document.getElementById("app");

function mount(): void {
  if (app === null) {
    throw new Error("bundle-loro-spike: #app element missing from index.html");
  }
  if (location.pathname.startsWith("/flow/")) {
    void import("./flow-entry").then(({ mountFlowEditor }) => mountFlowEditor(app));
  } else {
    void import("./home-entry").then(({ mountHome }) => mountHome(app));
  }
}

mount();

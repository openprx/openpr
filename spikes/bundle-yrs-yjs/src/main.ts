/**
 * See `bundle-loro/src/main.ts` for the full rationale -- identical shell, other candidate.
 */
const app = document.getElementById("app");

function mount(): void {
  if (app === null) {
    throw new Error("bundle-yrs-yjs-spike: #app element missing from index.html");
  }
  if (location.pathname.startsWith("/flow/")) {
    void import("./flow-entry").then(({ mountFlowEditor }) => mountFlowEditor(app));
  } else {
    void import("./home-entry").then(({ mountHome }) => mountHome(app));
  }
}

mount();

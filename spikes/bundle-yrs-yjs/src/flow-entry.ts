/**
 * The one and only module in this package that touches the `yrs-yjs` CRDT candidate: real
 * `yjs@13.6.32` + `y-prosemirror@1.3.7` + ProseMirror, wired up the way `ADR-0006`'s editor-stack
 * candidate actually would be. Reached only via the dynamic `import("./flow-entry")` in
 * `main.ts` -- see `bundle-loro/src/flow-entry.ts` for the full rationale.
 */
import { Schema } from "prosemirror-model";
import { EditorState } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { ySyncPlugin } from "y-prosemirror";
import * as Y from "yjs";

const schema = new Schema({
  nodes: {
    doc: { content: "block+" },
    paragraph: {
      group: "block",
      content: "text*",
      toDOM: () => ["p", 0] as const,
      parseDOM: [{ tag: "p" }],
    },
    text: {},
  },
});

export function mountFlowEditor(target: HTMLElement): EditorView {
  target.textContent = "";
  target.dataset.route = "flow";

  const ydoc = new Y.Doc();
  const yXmlFragment = ydoc.getXmlFragment("prosemirror");
  const state = EditorState.create({
    schema,
    plugins: [ySyncPlugin(yXmlFragment)],
  });

  // `y-prosemirror`'s `ySyncPlugin` view-plugin forces a *synchronous* re-render as part of
  // `EditorView`'s own constructor (`updatePluginViews` -> the plugin's `view()` init ->
  // `_forceRerender` -> `view.dispatch(...)`, all before `new EditorView(...)` has returned) --
  // confirmed by reproducing this against the unminified `vite dev` server, which reported the
  // real (non-minified) name in the crash: `ReferenceError: Cannot access 'view' before
  // initialization at dispatchTransaction`. A plain `const view = new EditorView(...)` closure
  // that reads `view` unconditionally therefore hits the TDZ the moment construction begins, not
  // a minifier bug. `let view: EditorView | undefined` + a no-op guard for that
  // during-construction call (EditorView's own initial render from `state` already reflects that
  // transaction; nothing needs re-applying before `view` exists) is the fix. Real, disclosed
  // ADR-0006-relevant finding: `loro-prosemirror`'s `LoroSyncPlugin` (see `bundle-loro/src/
  // flow-entry.ts`) did not reproduce this even with the plain unguarded pattern, i.e. it does
  // not force a synchronous initial rerender the same way -- this package still applies the same
  // guard defensively.
  let view: EditorView | undefined;
  view = new EditorView(target, {
    state,
    dispatchTransaction(transaction) {
      if (view === undefined) return;
      view.updateState(view.state.apply(transaction));
    },
  });

  // Cold-start marker for `spikes/coldstart-measure`'s p95 measurement -- see
  // `bundle-loro/src/flow-entry.ts`'s identical marker for the full rationale. `yjs`/
  // `y-prosemirror` are pure JS (no WASM instantiate step), so this marks "chunk fetched +
  // evaluated + ProseMirror EditorView mounted".
  performance.mark("flow-ready");

  return view;
}

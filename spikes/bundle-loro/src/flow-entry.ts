/**
 * The one and only module in this package that touches the `loro` CRDT candidate: real
 * `loro-crdt@1.14.1` + `loro-prosemirror@0.4.3` + ProseMirror, wired up the way `ADR-0006`'s
 * editor-stack candidate actually would be, not a placeholder import. Reached only via the
 * dynamic `import("./flow-entry")` in `main.ts` -- Vite/Rollup therefore always places this
 * module (and everything it imports) in a chunk separate from the main entry graph, which is
 * exactly what `measure-bundle.ts` treats as "the candidate engine bundle".
 */
import { LoroDoc, type LoroMap } from "loro-crdt";
import { LoroSyncPlugin, LoroUndoPlugin, redo, undo, type LoroNodeContainerType } from "loro-prosemirror";
import { Schema } from "prosemirror-model";
import { EditorState } from "prosemirror-state";
import { EditorView } from "prosemirror-view";

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

  const doc = new LoroDoc<{ doc: LoroMap<LoroNodeContainerType>; data: LoroMap }>();
  const state = EditorState.create({
    schema,
    plugins: [LoroSyncPlugin({ doc }), LoroUndoPlugin({ doc })],
  });

  // `let ... | undefined` + guard defensively, matching `bundle-yrs-yjs/src/flow-entry.ts` --
  // that package's `y-prosemirror` `ySyncPlugin` was found (empirically, via an unminified
  // `vite dev` repro) to force a synchronous re-render during `EditorView`'s own constructor,
  // which crashes a plain `const view = new EditorView(...)` closure with a TDZ error the moment
  // construction begins. `loro-prosemirror`'s `LoroSyncPlugin` does NOT reproduce that here (this
  // candidate's cold-start samples ran clean with a plain unguarded closure first), but the guard
  // is kept for both candidates so this measurement compares two editors with identical
  // dispatch-wiring robustness, not one lucky and one defensively coded.
  let view: EditorView | undefined;
  view = new EditorView(target, {
    state,
    dispatchTransaction(transaction) {
      if (view === undefined) return;
      view.updateState(view.state.apply(transaction));
    },
  });

  // Exercises the real undo/redo command path too (not merely imported-and-dead), so
  // `loro-prosemirror`'s undo module is genuinely reachable code, matching a real editor
  // integration rather than a bundle-inflation placeholder.
  undo(view.state, view.dispatch);
  void redo;

  // Cold-start marker for `spikes/coldstart-measure`'s p95 measurement: by the time this line
  // runs, the dynamically-imported chunk has been fetched+evaluated, `loro-crdt`'s WASM has been
  // synchronously instantiated (module-eval time, see `vite.config.ts`'s comment on the browser
  // entry's synchronous XHR load), and the ProseMirror `EditorView` has completed its first mount
  // -- i.e. this is "the Flow editor is interactive", the real cold-start budget target, not a
  // network-only proxy metric.
  performance.mark("flow-ready");

  return view;
}

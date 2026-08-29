/**
 * ADR-0015 R4 `engine_runtime_ready` dedicated entry for the `yrs-yjs` candidate. See
 * `loro-engine-only-entry.ts` for the full rationale -- identical shape, other candidate.
 *
 * Imports ONLY `yjs` (same package + version pinned by `../../bundle-yrs-yjs/package.json`,
 * resolved via `probe/vite.config.mjs`'s alias at the exact same `node_modules/yjs` install
 * bundle-yrs-yjs itself uses). Does NOT import `prosemirror-model`, `prosemirror-state`,
 * `prosemirror-view`, or `y-prosemirror` -- none of those packages appear anywhere in this file
 * or anything it imports.
 */
import * as Y from "yjs";

export interface ProbeBlock {
  id: string;
  type: string;
  parent: string;
  index: number;
  text: string;
}

export const BLOCK_ID = "block-0";
export const BLOCK_TYPE = "paragraph";
export const BLOCK_PARENT = "root";
export const BLOCK_INDEX = 0;

export interface EngineDocHandle {
  doc: Y.Doc;
  block: Y.Map<unknown>;
  text: Y.Text;
}

/** Step 1: adapter/binding initialization. */
export function initEngineDoc(): EngineDocHandle {
  const doc = new Y.Doc();
  const blocks = doc.getMap("blocks");
  const block = new Y.Map<unknown>();
  blocks.set(BLOCK_ID, block);
  block.set("id", BLOCK_ID);
  block.set("type", BLOCK_TYPE);
  block.set("parent", BLOCK_PARENT);
  block.set("index", BLOCK_INDEX);
  const text = new Y.Text();
  block.set("text", text);
  return { doc, block, text };
}

/** Step 2: fixed fixture bootstrap. */
export function bootstrapFixture(handle: EngineDocHandle, fixtureText: string): void {
  handle.text.insert(0, fixtureText);
}

/** Step 3: probe write -- second, separate CRDT operation appending probeText (mirrors the DOM
 * probe's two-sequential-writes shape -- see loro-engine-only-entry.ts's comment). */
export function writeProbe(handle: EngineDocHandle, probeText: string): void {
  handle.text.insert(handle.text.length, probeText);
}

/** Step 4: independent read-back via Yjs's own CRDT API only (Y.Map.get / Y.Text.toString) --
 * never from a DOM tree (there is none in this harness page). */
export function readBackBlocks(handle: EngineDocHandle): ProbeBlock[] {
  return [
    {
      id: String(handle.block.get("id")),
      type: String(handle.block.get("type")),
      parent: String(handle.block.get("parent")),
      index: Number(handle.block.get("index")),
      text: handle.text.toString(),
    },
  ];
}

/**
 * ADR-0015 R4 `engine_runtime_ready` dedicated entry for the `loro` candidate
 * (/opt/working/sylvode-flow/decisions/ADR-0015-cold-start-measurement-split.md section 3.2 item 4
 * + coordinator directive 2026-08-29: "给 engine_runtime_ready 用专用 entry ... 构建产物的模块图里
 * ProseMirror 相关模块数必须为 0").
 *
 * This module imports ONLY `loro-crdt` (the same package + version pinned by
 * `../../bundle-loro/package.json`, resolved via this build's `resolve.alias` at the exact same
 * `node_modules/loro-crdt` install bundle-loro itself uses -- see `probe/vite.config.mjs`). It does
 * NOT import `prosemirror-model`, `prosemirror-state`, `prosemirror-view`, or `loro-prosemirror` --
 * none of those packages appear anywhere in this file or anything it imports, so there is nothing
 * for `check-module-graph.mjs`'s static input-list assertion to find.
 *
 * Adapter-API-only probe (ADR-0015 3.2 item 4, engine_runtime_ready half): writes and reads back
 * through `loro-crdt`'s own CRDT container API (`LoroDoc.getMap`, `LoroMap.set`/`setContainer`,
 * `LoroText.insert`/`toString`) exclusively. No DOM element is created, read, or referenced by this
 * file at all -- `document`/`window.document` do not appear in this module's source.
 */
import { LoroDoc, LoroMap, LoroText } from "loro-crdt";

export interface ProbeBlock {
  id: string;
  type: string;
  parent: string;
  index: number;
  text: string;
}

// Fixed block identity (coordinator correction 2026-08-29 item 5 / R4 3.2 item 4: a real,
// addressable id -- not reconstructed from DOM). This is a LoroMap container KEY under the
// `blocks` root map, i.e. an actual CRDT-addressable identity, not a DOM position guess.
export const BLOCK_ID = "block-0";
export const BLOCK_TYPE = "paragraph";
export const BLOCK_PARENT = "root";
export const BLOCK_INDEX = 0;

export interface EngineDocHandle {
  doc: LoroDoc;
  block: LoroMap;
  text: LoroText;
}

/** Step 1 of the timed window: adapter/binding initialization -- constructs the CRDT document and
 * the fixed block's container structure (id/type/parent/index as plain map fields, a nested
 * LoroText container for the text field), but writes no fixture content yet. */
export function initEngineDoc(): EngineDocHandle {
  const doc = new LoroDoc();
  const blocks = doc.getMap("blocks");
  const block = blocks.setContainer(BLOCK_ID, new LoroMap());
  block.set("id", BLOCK_ID);
  block.set("type", BLOCK_TYPE);
  block.set("parent", BLOCK_PARENT);
  block.set("index", BLOCK_INDEX);
  const text = block.setContainer("text", new LoroText());
  return { doc, block, text };
}

/** Step 2: fixed fixture bootstrap (ADR-0015 3.2 postcondition 3). */
export function bootstrapFixture(handle: EngineDocHandle, fixtureText: string): void {
  handle.text.insert(0, fixtureText);
}

/** Step 3: the read/write probe's WRITE half (ADR-0015 3.2 postcondition 4) -- a second, separate
 * CRDT operation appending probeText, matching the two-sequential-writes shape the DOM-based route
 * probe also uses (see probe/coldstart-probe-v1.json's flow_route_cold_load_probe for why: neither candidate's
 * ProseMirror wiring has a paragraph-split command, so the DOM probe is also two writes into one
 * block -- this engine-only probe deliberately mirrors that same two-write shape at the CRDT layer
 * for a like-for-like comparison, even though nothing here touches ProseMirror). */
export function writeProbe(handle: EngineDocHandle, probeText: string): void {
  handle.text.insert(handle.text.length, probeText);
}

/** Step 4: the read/write probe's READ half -- reads back EXCLUSIVELY through the CRDT container
 * API (LoroMap.get / LoroText.toString), never from a DOM tree (there is none in this harness page
 * -- see probe/harness.html). This is the "独立读取 CRDT state 做比较" ADR-0015 3.2 requirement. */
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

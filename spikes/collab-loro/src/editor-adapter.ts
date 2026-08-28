import { LoroDoc } from "loro-crdt";
import { createNodeFromLoroObj, updateLoroToPmState, type LoroDocType, type LoroNodeMapping } from "loro-prosemirror";
import { Schema, type Node as PmNode } from "prosemirror-model";
import { EditorState, TextSelection } from "prosemirror-state";
import type { EditorAdapter, EditorOrigin, EngineDiff, RelativeSelection, RichTextCommand } from "@sylvode/collab-shared";

/**
 * Minimal ProseMirror schema for this spike's single rich-text block:
 * a doc of paragraphs of plain text. Real production schema (headings,
 * lists, code, marks) is out of scope for this convergence-focused round.
 */
const schema = new Schema({
  nodes: {
    doc: { content: "paragraph+" },
    paragraph: { content: "text*", toDOM: () => ["p", 0], parseDOM: [{ tag: "p" }] },
    text: {},
  },
  marks: {},
});

const emptyDoc = (): PmNode => schema.node("doc", null, [schema.node("paragraph", null, [])]);

/** This spike's local command vocabulary (not part of the candidate-agnostic contract -- RichTextCommand.payload is deliberately `unknown` at the contract level). */
export type LoroRichTextCommandPayload =
  | Readonly<{ type: "insertText"; pos: number; text: string }>
  | Readonly<{ type: "deleteRange"; from: number; to: number }>;

function interpretCommand(state: EditorState, command: RichTextCommand) {
  const payload = command.payload as LoroRichTextCommandPayload;
  const tr = state.tr;
  if (payload.type === "insertText") {
    tr.insertText(payload.text, payload.pos);
  } else {
    tr.delete(payload.from, payload.to);
  }
  return tr;
}

function encodeNumber(value: number): Uint8Array {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, true);
  return bytes;
}

function decodeNumber(bytes: Uint8Array): number {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0, true);
}

/**
 * Real loro-prosemirror@0.4.3 implementation of the shared EditorAdapter
 * contract (ADR-0006). Runs headlessly (no DOM `EditorView`): local edits go
 * through a plain `prosemirror-state` `EditorState.apply(tr)`, and are
 * pushed into the LoroDoc via the real `updateLoroToPmState` diffing helper;
 * remote changes are pulled back out via the real `createNodeFromLoroObj`
 * reader. `mount()`'s `host` parameter is accepted for interface
 * conformance but not used -- there is no DOM in this test environment, so
 * the DOM-bound half of loro-prosemirror (`LoroSyncPlugin`'s `view()` hook,
 * `LoroCursorPlugin`, `LoroUndoPlugin`) is NOT exercised here. See the
 * delivery report for exactly what this does and does not verify.
 */
export class LoroEditorAdapter implements EditorAdapter {
  private readonly doc: LoroDocType;
  private readonly mapping: LoroNodeMapping = new Map();
  private editorState: EditorState;

  constructor(doc: LoroDocType = new LoroDoc() as LoroDocType) {
    this.doc = doc;
    this.editorState = EditorState.create({ schema, doc: emptyDoc() });
  }

  async mount(_host: HTMLElement, _blockId: string): Promise<void> {
    this.refreshFromLoro();
  }

  applyRemote(_change: EngineDiff): void {
    this.refreshFromLoro();
  }

  transact(command: RichTextCommand, _origin: EditorOrigin): Uint8Array {
    const beforeFrontiers = this.doc.frontiers();
    const tr = interpretCommand(this.editorState, command);
    this.editorState = this.editorState.apply(tr);
    updateLoroToPmState(this.doc, this.mapping, this.editorState);
    const from = this.doc.frontiersToVV(beforeFrontiers);
    return this.doc.export({ mode: "update", from });
  }

  getSelection(): RelativeSelection | null {
    const { selection } = this.editorState;
    return { anchor: encodeNumber(selection.anchor), head: encodeNumber(selection.head) };
  }

  restoreSelection(selection: RelativeSelection): void {
    const anchor = Math.min(decodeNumber(selection.anchor), this.editorState.doc.content.size);
    const head = Math.min(decodeNumber(selection.head), this.editorState.doc.content.size);
    const tr = this.editorState.tr.setSelection(TextSelection.create(this.editorState.doc, anchor, head));
    this.editorState = this.editorState.apply(tr);
  }

  undoLocal(): boolean {
    // loro-prosemirror's LoroUndoPlugin is a live-EditorView plugin (it hooks transaction
    // dispatch); wiring it up headlessly is out of scope for this convergence-focused round.
    return false;
  }

  redoLocal(): boolean {
    return false;
  }

  async destroy(): Promise<void> {
    this.doc.free();
  }

  /** Test/debug helper: current plain-text content, used by this candidate's own editor tests. Not part of EditorAdapter. */
  currentText(): string {
    let out = "";
    this.editorState.doc.descendants((node) => {
      if (node.isText) out += node.text ?? "";
    });
    return out;
  }

  private refreshFromLoro(): void {
    const rootMap = this.doc.getMap("doc");
    if (rootMap.get("nodeName") == null) {
      this.editorState = EditorState.create({ schema, doc: emptyDoc() });
      return;
    }
    const nodes = createNodeFromLoroObj(schema, rootMap, this.mapping);
    const doc = Array.isArray(nodes) ? emptyDoc() : nodes;
    this.editorState = EditorState.create({ schema, doc });
  }
}

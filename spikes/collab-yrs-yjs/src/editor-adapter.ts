import * as Y from "yjs";
import { updateYFragment, yXmlFragmentToProseMirrorRootNode } from "y-prosemirror";
import { Schema, type MarkType, type Node as PmNode } from "prosemirror-model";
import { EditorState, TextSelection } from "prosemirror-state";
import type { EditorAdapter, EditorOrigin, EngineDiff, RelativeSelection, RichTextCommand } from "@sylvode/collab-shared";

/** Same minimal schema as the Loro candidate's editor-adapter.ts -- ADR-0006 fixes the ProseMirror core across both candidates. */
const schema = new Schema({
  nodes: {
    doc: { content: "paragraph+" },
    paragraph: { content: "text*", toDOM: () => ["p", 0], parseDOM: [{ tag: "p" }] },
    text: {},
  },
  marks: {},
});

/**
 * `doc: { content: "paragraph+" }` requires at least one paragraph, but a
 * freshly created (or not-yet-synced) `Y.XmlFragment` is genuinely empty --
 * `initProseMirrorDoc`/`yXmlFragmentToProseMirrorRootNode` on it produce a
 * schema-invalid zero-child doc (`content.size === 0`), which then rejects
 * any position-based transaction. Mirrors the Loro candidate's
 * `emptyDoc()` fallback: a JS-only placeholder, not written into the CRDT
 * until the first real `transact()`.
 */
const emptyDoc = (): PmNode => schema.node("doc", null, [schema.node("paragraph", null, [])]);

export type YjsRichTextCommandPayload =
  | Readonly<{ type: "insertText"; pos: number; text: string }>
  | Readonly<{ type: "deleteRange"; from: number; to: number }>;

function interpretCommand(state: EditorState, command: RichTextCommand) {
  const payload = command.payload as YjsRichTextCommandPayload;
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
 * Real y-prosemirror@1.3.7 implementation of the shared EditorAdapter
 * contract (ADR-0006). Runs headlessly (no DOM `EditorView`): the
 * read-direction (`Y.XmlFragment` -> ProseMirror `Node`) uses the real,
 * documented headless helper `yXmlFragmentToProseMirrorRootNode` (with an
 * `emptyDoc()` fallback for a genuinely empty fragment, which that helper
 * turns into a schema-invalid zero-child doc); the write-direction (ProseMirror
 * `Node` -> `Y.XmlFragment`) uses the real `updateYFragment` diffing helper
 * that `ySyncPlugin`'s live view binding also calls internally. `mount()`'s
 * `host` parameter is accepted for interface conformance but not used --
 * `ySyncPlugin`, `yCursorPlugin`, and `yUndoPlugin` (the DOM/EditorView-bound
 * halves of y-prosemirror) are NOT exercised here. See the delivery report
 * for exactly what this does and does not verify.
 */
export class YjsEditorAdapter implements EditorAdapter {
  private readonly doc: Y.Doc;
  private readonly fragment: Y.XmlFragment;
  private readonly mapping = new Map<Y.AbstractType<unknown>, PmNode | PmNode[]>();
  private readonly isOMark = new Map<MarkType, boolean>();
  private editorState: EditorState;

  constructor(doc: Y.Doc = new Y.Doc(), fragmentName = "prosemirror") {
    this.doc = doc;
    this.fragment = doc.getXmlFragment(fragmentName);
    this.editorState = EditorState.create({ schema, doc: this.readFromFragment() });
  }

  async mount(_host: HTMLElement, _blockId: string): Promise<void> {
    this.refreshFromYjs();
  }

  applyRemote(_change: EngineDiff): void {
    this.refreshFromYjs();
  }

  transact(command: RichTextCommand, _origin: EditorOrigin): Uint8Array {
    const before = Y.encodeStateVector(this.doc);
    const tr = interpretCommand(this.editorState, command);
    this.editorState = this.editorState.apply(tr);
    updateYFragment(this.doc, this.fragment, this.editorState.doc, { mapping: this.mapping, isOMark: this.isOMark });
    return Y.encodeStateAsUpdate(this.doc, before);
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
    // y-prosemirror's yUndoPlugin hooks into a live EditorView's transaction
    // dispatch; wiring it up headlessly is out of scope for this
    // convergence-focused round.
    return false;
  }

  redoLocal(): boolean {
    return false;
  }

  async destroy(): Promise<void> {
    this.doc.destroy();
  }

  /** Test/debug helper: current plain-text content, used by this candidate's own editor tests. Not part of EditorAdapter. */
  currentText(): string {
    let out = "";
    this.editorState.doc.descendants((node) => {
      if (node.isText) out += node.text ?? "";
    });
    return out;
  }

  private refreshFromYjs(): void {
    this.editorState = EditorState.create({ schema, doc: this.readFromFragment() });
  }

  private readFromFragment(): PmNode {
    if (this.fragment.length === 0) {
      return emptyDoc();
    }
    return yXmlFragmentToProseMirrorRootNode(this.fragment, schema) as PmNode;
  }
}

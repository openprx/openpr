import { describe, expect, test } from "bun:test";
import * as Y from "yjs";

import { YjsEditorAdapter } from "../src/editor-adapter";

describe("YjsEditorAdapter (real yjs + y-prosemirror@1.3.7, headless: no DOM EditorView in this environment -- see delivery report)", () => {
  test("transact() applies a real ProseMirror transaction and writes it into the real Y.XmlFragment via updateYFragment", () => {
    const doc = new Y.Doc();
    const adapter = new YjsEditorAdapter(doc);

    const updateBytes = adapter.transact(
      { name: "insertText", payload: { type: "insertText", pos: 1, text: "hello" } },
      { surface: "web", sessionId: "s1", transactionId: "t1" },
    );

    expect(adapter.currentText()).toBe("hello");
    expect(updateBytes.length).toBeGreaterThan(0);
    // the "prosemirror" root Y.XmlFragment is real Yjs content now, not just local ProseMirror state.
    expect(doc.getXmlFragment("prosemirror").toString().length).toBeGreaterThan(0);
  });

  test("a second replica imports the update and applyRemote() reconstructs the identical ProseMirror doc", () => {
    const docA = new Y.Doc();
    const adapterA = new YjsEditorAdapter(docA);
    const updateBytes = adapterA.transact(
      { name: "insertText", payload: { type: "insertText", pos: 1, text: "sync me" } },
      { surface: "web", sessionId: "s1", transactionId: "t1" },
    );

    const docB = new Y.Doc();
    Y.applyUpdate(docB, Y.encodeStateAsUpdate(docA));
    const adapterB = new YjsEditorAdapter(docB);
    adapterB.applyRemote({ changedContainerIds: [] });

    expect(adapterB.currentText()).toBe("sync me");
    expect(updateBytes.length).toBeGreaterThan(0);
  });

  test("getSelection()/restoreSelection() round-trip a relative selection", () => {
    const adapter = new YjsEditorAdapter();
    adapter.transact(
      { name: "insertText", payload: { type: "insertText", pos: 1, text: "abcdef" } },
      { surface: "web", sessionId: "s1", transactionId: "t1" },
    );
    adapter.restoreSelection({ anchor: encodeU32(2), head: encodeU32(4) });
    const selection = adapter.getSelection();
    expect(selection).not.toBeNull();
    expect(decodeU32(selection?.anchor ?? new Uint8Array(4))).toBe(2);
    expect(decodeU32(selection?.head ?? new Uint8Array(4))).toBe(4);
  });

  test("undoLocal()/redoLocal() report unsupported (headless -- yUndoPlugin needs a live EditorView) rather than silently no-op-and-claim-success", () => {
    const adapter = new YjsEditorAdapter();
    expect(adapter.undoLocal()).toBe(false);
    expect(adapter.redoLocal()).toBe(false);
  });

  test("destroy() destroys the underlying Y.Doc", async () => {
    const doc = new Y.Doc();
    const adapter = new YjsEditorAdapter(doc);
    await adapter.destroy();
    expect(doc.isDestroyed).toBe(true);
  });
});

function encodeU32(value: number): Uint8Array {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, true);
  return bytes;
}

function decodeU32(bytes: Uint8Array): number {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0, true);
}

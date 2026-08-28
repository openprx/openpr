import { describe, expect, test } from "bun:test";
import { LoroDoc } from "loro-crdt";
import type { LoroDocType } from "loro-prosemirror";

import { LoroEditorAdapter } from "../src/editor-adapter";

describe(
  "LoroEditorAdapter (real loro-crdt + loro-prosemirror@0.4.3, headless: no DOM EditorView in this environment -- see delivery report)",
  () => {
    test("transact() applies a real ProseMirror transaction and writes it into the real LoroDoc via updateLoroToPmState", () => {
      const doc = new LoroDoc() as LoroDocType;
      const adapter = new LoroEditorAdapter(doc);

      const updateBytes = adapter.transact(
        { name: "insertText", payload: { type: "insertText", pos: 1, text: "hello" } },
        { surface: "web", sessionId: "s1", transactionId: "t1" },
      );

      expect(adapter.currentText()).toBe("hello");
      expect(updateBytes.length).toBeGreaterThan(0);
      // "doc" root map is real Loro content now, not just local ProseMirror state.
      expect(doc.getMap("doc").get("nodeName")).toBe("doc");
    });

    test("a second replica imports the update and applyRemote() reconstructs the identical ProseMirror doc", () => {
      const docA = new LoroDoc() as LoroDocType;
      const adapterA = new LoroEditorAdapter(docA);
      const updateBytes = adapterA.transact(
        { name: "insertText", payload: { type: "insertText", pos: 1, text: "sync me" } },
        { surface: "web", sessionId: "s1", transactionId: "t1" },
      );

      const docB = new LoroDoc() as LoroDocType;
      docB.import(docA.export({ mode: "snapshot" }));
      const adapterB = new LoroEditorAdapter(docB);
      adapterB.applyRemote({ changedContainerIds: [] });

      expect(adapterB.currentText()).toBe("sync me");
      expect(updateBytes.length).toBeGreaterThan(0);
    });

    test("getSelection()/restoreSelection() round-trip a relative selection", () => {
      const adapter = new LoroEditorAdapter();
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

    test("undoLocal()/redoLocal() report unsupported (headless -- LoroUndoPlugin needs a live EditorView) rather than silently no-op-and-claim-success", () => {
      const adapter = new LoroEditorAdapter();
      expect(adapter.undoLocal()).toBe(false);
      expect(adapter.redoLocal()).toBe(false);
    });

    test("destroy() frees the underlying LoroDoc", async () => {
      const doc = new LoroDoc() as LoroDocType;
      const adapter = new LoroEditorAdapter(doc);
      await adapter.destroy();
      expect(() => doc.getText("t")).toThrow();
    });
  },
);

function encodeU32(value: number): Uint8Array {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, true);
  return bytes;
}

function decodeU32(bytes: Uint8Array): number {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0, true);
}

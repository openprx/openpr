import { describe, expect, test } from "bun:test";

import { InMemoryEngineStore } from "../src/engine-store";
import { YjsScenarioAdapter } from "../src/engine";

describe("InMemoryEngineStore (interface-level fake for EngineStore; real y-indexeddb path is unverified -- see delivery report)", () => {
  test("load() on an empty store returns null", async () => {
    const store = new InMemoryEngineStore();
    expect(await store.load("doc-1")).toBeNull();
  });

  test("save() then load() round-trips a real Yjs snapshot + update byte-for-byte", async () => {
    const store = new InMemoryEngineStore();

    const adapter = new YjsScenarioAdapter();
    await adapter.applyLocalOp({ kind: "text.insert", container: "body", index: 0, text: "hello" });
    const snapshot = await adapter.exportSnapshot();
    const update = await adapter.applyLocalOp({ kind: "text.insert", container: "body", index: 5, text: " world" });
    await adapter.dispose();

    await store.save("doc-1", { snapshot, updates: [update] });
    const loaded = await store.load("doc-1");

    expect(loaded).not.toBeNull();
    expect([...(loaded?.snapshot ?? [])]).toEqual([...snapshot]);
    expect(loaded?.updates.length).toBe(1);
    expect([...(loaded?.updates[0] ?? [])]).toEqual([...update]);

    // and the round-tripped bytes are still valid Yjs input, not just byte-identical
    const replica = new YjsScenarioAdapter();
    await replica.load(loaded?.snapshot ?? new Uint8Array());
    await replica.importUpdate(loaded?.updates[0] ?? new Uint8Array());
    expect(replica.toDebugJson()).toEqual({ "text:body": "hello world" });
    await replica.dispose();

    await store.close();
  });

  test("remove() deletes a stored document", async () => {
    const store = new InMemoryEngineStore();
    await store.save("doc-1", { snapshot: new Uint8Array([1, 2, 3]), updates: [] });
    expect(await store.load("doc-1")).not.toBeNull();
    await store.remove("doc-1");
    expect(await store.load("doc-1")).toBeNull();
  });

  test("round-trips a document with zero updates and an empty snapshot", async () => {
    const store = new InMemoryEngineStore();
    await store.save("doc-1", { snapshot: new Uint8Array(), updates: [] });
    const loaded = await store.load("doc-1");
    expect(loaded).toEqual({ snapshot: new Uint8Array(), updates: [] });
  });
});

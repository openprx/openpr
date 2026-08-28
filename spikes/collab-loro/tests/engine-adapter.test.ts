import { describe, expect, test } from "bun:test";

import {
  runConcurrentBlockMoveCase,
  runCorruptUpdateRejectedCase,
  runDuplicateApplyIdempotentCase,
  runSameTextRangeEditCase,
} from "@sylvode/collab-shared";

import { LoroScenarioAdapter } from "../src/engine";

const factory = () => new LoroScenarioAdapter();

describe("LoroScenarioAdapter against the shared corpus runner (real loro-crdt@1.14.1)", () => {
  test("same_text_range_edit: concurrent inserts into the same range converge to an identical semantic hash", async () => {
    const result = await runSameTextRangeEditCase(factory, "loro-corpus-seed-1");
    expect(result.status).toBe("passed");
    expect(result.operationLog.length).toBe(2);
    expect(result.semanticHash).toMatch(/^[0-9a-f]{64}$/);
  });

  test("concurrent_block_move: a child moved to two different parents converges to a single parent with no cycle", async () => {
    const result = await runConcurrentBlockMoveCase(factory, "loro-corpus-seed-2");
    expect(result.status).toBe("passed");

    // Independently re-derive the invariant from the real tree, not just the runner's hash comparison.
    const replica = new LoroScenarioAdapter();
    await replica.applyLocalOp({ kind: "tree.createNode", nodeId: "parent-1", parentId: null });
    await replica.applyLocalOp({ kind: "tree.createNode", nodeId: "parent-2", parentId: null });
    await replica.applyLocalOp({ kind: "tree.createNode", nodeId: "parent-3", parentId: null });
    await replica.applyLocalOp({ kind: "tree.createNode", nodeId: "child-1", parentId: "parent-1" });
    const snapshot = await replica.exportSnapshot();

    const other = new LoroScenarioAdapter();
    await other.load(snapshot);
    const updA = await replica.applyLocalOp({ kind: "tree.move", nodeId: "child-1", parentId: "parent-2" });
    const updB = await other.applyLocalOp({ kind: "tree.move", nodeId: "child-1", parentId: "parent-3" });
    await replica.importUpdate(updB);
    await other.importUpdate(updA);

    const parentOnReplica = replica.parentExternalIdOf("child-1");
    const parentOnOther = other.parentExternalIdOf("child-1");
    expect(parentOnReplica).toBe(parentOnOther as string);
    expect(["parent-2", "parent-3"]).toContain(parentOnReplica);
    expect(replica.liveNodeIds().sort()).toEqual(["child-1", "parent-1", "parent-2", "parent-3"]);
    expect(other.liveNodeIds().sort()).toEqual(["child-1", "parent-1", "parent-2", "parent-3"]);

    await replica.dispose();
    await other.dispose();
  });

  test("duplicate_out_of_order_partial_batch: re-importing the same update is idempotent", async () => {
    const result = await runDuplicateApplyIdempotentCase(factory, "loro-corpus-seed-3");
    expect(result.status).toBe("passed");
  });

  test("adversarial_update_limits: a truncated update is rejected and leaves head/frontier unchanged", async () => {
    const result = await runCorruptUpdateRejectedCase(factory, "loro-corpus-seed-4");
    expect(result.status).toBe("passed");
    expect(result.detail).toContain("rejected");
  });
});

import { describe, expect, test } from "bun:test";

import {
  runConcurrentBlockMoveCase,
  runCorruptUpdateRejectedCase,
  runDuplicateApplyIdempotentCase,
  runSameTextRangeEditCase,
} from "../src/corpus-runner";
import { FakeScenarioAdapter } from "./support/fake-scenario-adapter";

const factory = () => new FakeScenarioAdapter();

describe("corpus-runner (control-flow tests against a toy op-log adapter; real engine correctness is covered by collab-loro/collab-yrs-yjs's own tests)", () => {
  test("runSameTextRangeEditCase: exchanging concurrent updates converges both replicas' operation logs", async () => {
    const result = await runSameTextRangeEditCase(factory, "shared-runner-seed-1");
    expect(result.status).toBe("passed");
    expect(result.category).toBe("same_text_range_edit");
    expect(result.operationLog.length).toBe(2);
    expect(result.semanticHash).toMatch(/^[0-9a-f]{64}$/);
  });

  test("runConcurrentBlockMoveCase: exchanging concurrent moves converges both replicas' operation logs", async () => {
    const result = await runConcurrentBlockMoveCase(factory, "shared-runner-seed-2");
    expect(result.status).toBe("passed");
    expect(result.category).toBe("concurrent_block_move");
  });

  test("runDuplicateApplyIdempotentCase: the runner detects a genuinely non-idempotent adapter as a failure", async () => {
    class NonIdempotentAdapter extends FakeScenarioAdapter {
      private importCount = 0;
      override async importUpdate(update: Uint8Array) {
        this.importCount += 1;
        const diff = await super.importUpdate(update);
        // Force a state mutation on every import, including duplicates -- this must fail the case.
        if (this.importCount > 0) {
          await super.applyLocalOp({ kind: "text.insert", container: "poison", index: 0, text: String(this.importCount) });
        }
        return diff;
      }
    }
    const result = await runDuplicateApplyIdempotentCase(() => new NonIdempotentAdapter(), "shared-runner-seed-3");
    expect(result.status).toBe("failed");
    expect(result.detail).toContain("duplicate import mutated state");
  });

  test("runDuplicateApplyIdempotentCase: a real idempotent adapter passes", async () => {
    const result = await runDuplicateApplyIdempotentCase(factory, "shared-runner-seed-3");
    expect(result.status).toBe("passed");
  });

  test("runCorruptUpdateRejectedCase: a truncated update is rejected by JSON.parse and leaves state unchanged", async () => {
    const result = await runCorruptUpdateRejectedCase(factory, "shared-runner-seed-4");
    expect(result.status).toBe("passed");
    expect(result.detail).toContain("rejected");
  });

  test("runCorruptUpdateRejectedCase: the runner detects an adapter that silently accepts corrupt input as a failure", async () => {
    class PermissiveAdapter extends FakeScenarioAdapter {
      override async importUpdate(update: Uint8Array) {
        try {
          return await super.importUpdate(update);
        } catch {
          // swallow decode errors and pretend nothing happened -- to the runner this is
          // indistinguishable from silent acceptance, which must fail the case.
          return { changedContainerIds: [] };
        }
      }
    }
    const result = await runCorruptUpdateRejectedCase(() => new PermissiveAdapter(), "shared-runner-seed-4");
    expect(result.status).toBe("failed");
    expect(result.detail).toContain("expected rejection");
  });
});

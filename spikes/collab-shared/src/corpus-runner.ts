import {
  generateCorruptUpdateFixture,
  generateDuplicateApplyFixture,
  generateTextCoEditFixture,
  generateTreeMoveFixture,
} from "./fixture";
import { semanticHash } from "./hash";
import type { AbstractOp, ScenarioAdapterFactory } from "./testing";

export type CaseCategory =
  | "same_text_range_edit"
  | "concurrent_block_move"
  | "duplicate_out_of_order_partial_batch"
  | "adversarial_update_limits";

export type CaseRunResult = Readonly<{
  id: string;
  category: CaseCategory;
  seed: string;
  status: "passed" | "failed";
  operationLog: readonly AbstractOp[];
  finalSemanticJson: unknown;
  semanticHash: string;
  binarySizes: Readonly<{ updateBytes: number; snapshotBytes: number }>;
  detail: string;
}>;

async function withAdapters<T>(
  factory: ScenarioAdapterFactory,
  count: number,
  fn: (adapters: ReturnType<ScenarioAdapterFactory>[]) => Promise<T>,
): Promise<T> {
  const adapters = Array.from({ length: count }, () => factory());
  try {
    return await fn(adapters);
  } finally {
    await Promise.all(adapters.map((adapter) => adapter.dispose()));
  }
}

/**
 * Two replicas concurrently insert into the same text range from a shared
 * base snapshot, exchange updates, and must converge to a byte-identical
 * semantic state on both sides.
 */
export async function runSameTextRangeEditCase(factory: ScenarioAdapterFactory, corpusSeed: string): Promise<CaseRunResult> {
  const fixture = generateTextCoEditFixture(corpusSeed);
  const operationLog: AbstractOp[] = [];
  return withAdapters(factory, 2, async ([replicaA, replicaB]) => {
    if (!replicaA || !replicaB) throw new Error("expected two replicas");

    await replicaA.applyLocalOp({ kind: "text.insert", container: fixture.container, index: 0, text: fixture.baseText });
    const snapshot = await replicaA.exportSnapshot();
    await replicaB.load(snapshot);

    const updateA = await replicaA.applyLocalOp(fixture.replicaAOp);
    operationLog.push(fixture.replicaAOp);
    const updateB = await replicaB.applyLocalOp(fixture.replicaBOp);
    operationLog.push(fixture.replicaBOp);

    await replicaA.importUpdate(updateB);
    await replicaB.importUpdate(updateA);

    const jsonA = replicaA.toDebugJson();
    const jsonB = replicaB.toDebugJson();
    const hashA = await semanticHash(jsonA);
    const hashB = await semanticHash(jsonB);
    const passed = hashA === hashB;

    return {
      id: `${corpusSeed}:same_text_range_edit`,
      category: "same_text_range_edit",
      seed: corpusSeed,
      status: passed ? "passed" : "failed",
      operationLog,
      finalSemanticJson: jsonA,
      semanticHash: hashA,
      binarySizes: { updateBytes: updateA.length + updateB.length, snapshotBytes: snapshot.length },
      detail: passed
        ? "both replicas converged to an identical semantic hash after exchanging concurrent inserts into the same range"
        : `replica hashes diverged: A=${hashA} B=${hashB}`,
    };
  });
}

/**
 * A single child node gets moved to two different parents by two replicas
 * that were offline from each other; after exchanging updates both replicas
 * must agree on exactly one parent for the child, with no cycle and no lost
 * subtree.
 */
export async function runConcurrentBlockMoveCase(factory: ScenarioAdapterFactory, corpusSeed: string): Promise<CaseRunResult> {
  const fixture = generateTreeMoveFixture(corpusSeed);
  const operationLog: AbstractOp[] = [...fixture.createOps];
  return withAdapters(factory, 2, async ([replicaA, replicaB]) => {
    if (!replicaA || !replicaB) throw new Error("expected two replicas");

    for (const op of fixture.createOps) {
      await replicaA.applyLocalOp(op);
    }
    const snapshot = await replicaA.exportSnapshot();
    await replicaB.load(snapshot);

    const updateA = await replicaA.applyLocalOp(fixture.replicaAMove);
    operationLog.push(fixture.replicaAMove);
    const updateB = await replicaB.applyLocalOp(fixture.replicaBMove);
    operationLog.push(fixture.replicaBMove);

    await replicaA.importUpdate(updateB);
    await replicaB.importUpdate(updateA);

    const jsonA = replicaA.toDebugJson();
    const jsonB = replicaB.toDebugJson();
    const hashA = await semanticHash(jsonA);
    const hashB = await semanticHash(jsonB);
    const converged = hashA === hashB;

    const detail = converged
      ? "both replicas converged on a single parent for the moved child with no cycle or lost subtree"
      : `replica hashes diverged: A=${hashA} B=${hashB}`;

    return {
      id: `${corpusSeed}:concurrent_block_move`,
      category: "concurrent_block_move",
      seed: corpusSeed,
      status: converged ? "passed" : "failed",
      operationLog,
      finalSemanticJson: jsonA,
      semanticHash: hashA,
      binarySizes: { updateBytes: updateA.length + updateB.length, snapshotBytes: snapshot.length },
      detail,
    };
  });
}

/** Importing the same update bytes twice must be a no-op the second time: identical frontier, identical semantic hash. */
export async function runDuplicateApplyIdempotentCase(factory: ScenarioAdapterFactory, corpusSeed: string): Promise<CaseRunResult> {
  const fixture = generateDuplicateApplyFixture(corpusSeed);
  const operationLog: AbstractOp[] = [fixture.op];
  return withAdapters(factory, 2, async ([writer, reader]) => {
    if (!writer || !reader) throw new Error("expected writer + reader replicas");

    await writer.applyLocalOp({ kind: "text.insert", container: fixture.container, index: 0, text: fixture.baseText });
    const snapshot = await writer.exportSnapshot();
    await reader.load(snapshot);

    const update = await writer.applyLocalOp(fixture.op);
    await reader.importUpdate(update);
    const jsonAfterFirst = reader.toDebugJson();
    const hashAfterFirst = await semanticHash(jsonAfterFirst);
    const frontierAfterFirst = reader.frontier().bytes;

    await reader.importUpdate(update);
    const jsonAfterDuplicate = reader.toDebugJson();
    const hashAfterDuplicate = await semanticHash(jsonAfterDuplicate);
    const frontierAfterDuplicate = reader.frontier().bytes;

    const hashUnchanged = hashAfterFirst === hashAfterDuplicate;
    const frontierUnchanged =
      frontierAfterFirst.length === frontierAfterDuplicate.length &&
      frontierAfterFirst.every((byte, index) => byte === frontierAfterDuplicate[index]);
    const passed = hashUnchanged && frontierUnchanged;

    return {
      id: `${corpusSeed}:duplicate_out_of_order_partial_batch`,
      category: "duplicate_out_of_order_partial_batch",
      seed: corpusSeed,
      status: passed ? "passed" : "failed",
      operationLog,
      finalSemanticJson: jsonAfterDuplicate,
      semanticHash: hashAfterDuplicate,
      binarySizes: { updateBytes: update.length, snapshotBytes: snapshot.length },
      detail: passed
        ? "re-importing the same update byte-for-byte left semantic hash and frontier unchanged"
        : `duplicate import mutated state: hashUnchanged=${hashUnchanged} frontierUnchanged=${frontierUnchanged}`,
    };
  });
}

/** A byte-truncated update must be rejected (reader throws) and must not mutate head/frontier. */
export async function runCorruptUpdateRejectedCase(factory: ScenarioAdapterFactory, corpusSeed: string): Promise<CaseRunResult> {
  const fixture = generateCorruptUpdateFixture(corpusSeed);
  const operationLog: AbstractOp[] = [fixture.op];
  return withAdapters(factory, 2, async ([writer, reader]) => {
    if (!writer || !reader) throw new Error("expected writer + reader replicas");

    await writer.applyLocalOp({ kind: "text.insert", container: fixture.container, index: 0, text: fixture.baseText });
    const snapshot = await writer.exportSnapshot();
    await reader.load(snapshot);

    const update = await writer.applyLocalOp(fixture.op);
    const truncatedLength = fixture.truncateTo(update.length);
    const corrupted = update.slice(0, truncatedLength);

    const jsonBefore = reader.toDebugJson();
    const hashBefore = await semanticHash(jsonBefore);
    const frontierBefore = reader.frontier().bytes;

    let rejected = false;
    let rejectionMessage = "";
    try {
      await reader.importUpdate(corrupted);
    } catch (error) {
      rejected = true;
      rejectionMessage = error instanceof Error ? error.message : String(error);
    }

    const jsonAfter = reader.toDebugJson();
    const hashAfter = await semanticHash(jsonAfter);
    const frontierAfter = reader.frontier().bytes;
    const stateUnchanged =
      hashBefore === hashAfter &&
      frontierBefore.length === frontierAfter.length &&
      frontierBefore.every((byte, index) => byte === frontierAfter[index]);
    const passed = rejected && stateUnchanged;

    return {
      id: `${corpusSeed}:adversarial_update_limits`,
      category: "adversarial_update_limits",
      seed: corpusSeed,
      status: passed ? "passed" : "failed",
      operationLog,
      finalSemanticJson: jsonAfter,
      semanticHash: hashAfter,
      binarySizes: { updateBytes: corrupted.length, snapshotBytes: snapshot.length },
      detail: passed
        ? `truncated update (${corrupted.length}/${update.length} bytes) was rejected: "${rejectionMessage}"; head/frontier unchanged`
        : `expected rejection with unchanged state, got rejected=${rejected} stateUnchanged=${stateUnchanged}`,
    };
  });
}

export async function runFullCorpus(factory: ScenarioAdapterFactory, corpusSeed: string): Promise<readonly CaseRunResult[]> {
  return [
    await runSameTextRangeEditCase(factory, corpusSeed),
    await runConcurrentBlockMoveCase(factory, corpusSeed),
    await runDuplicateApplyIdempotentCase(factory, corpusSeed),
    await runCorruptUpdateRejectedCase(factory, corpusSeed),
  ];
}

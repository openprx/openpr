import { SplitMix64 } from "./prng";
import type { AbstractTextOp, AbstractTreeOp } from "./testing";

// BMP-only (no astral/surrogate-pair characters, e.g. emoji): every code
// point here is exactly one UTF-16 code unit, so a JS-string `.length`
// offset is always a valid insertion boundary for both engines' text index.
// Surrogate-pair-safe indexing is required by the `unicode_ime` corpus
// category, which this package does not implement yet -- see the delivery
// report.
const TEXT_ALPHABET = "abcdefghijklmnopqrstuvwxyz 中文あいう";

/**
 * `same_text_range_edit`: two replicas start from an identical base text and
 * both make a local insert that lands inside (or immediately adjacent to)
 * the same range, so the runner exercises real interleaving, not disjoint edits.
 */
export function generateTextCoEditFixture(corpusSeed: string): Readonly<{
  container: string;
  baseText: string;
  replicaAOp: AbstractTextOp;
  replicaBOp: AbstractTextOp;
}> {
  const rng = SplitMix64.fromSeedString(`${corpusSeed}:same_text_range_edit`);
  const container = "body";
  const baseText = rng.nextString(TEXT_ALPHABET, 12, 24);
  const rangeStart = rng.nextInt(baseText.length + 1);
  const insertA = rng.nextString(TEXT_ALPHABET, 1, 6);
  const insertB = rng.nextString(TEXT_ALPHABET, 1, 6);
  return {
    container,
    baseText,
    replicaAOp: { kind: "text.insert", container, index: rangeStart, text: insertA },
    replicaBOp: { kind: "text.insert", container, index: rangeStart, text: insertB },
  };
}

/**
 * `concurrent_block_move`: one child node, three candidate parents; replica A
 * and replica B each move the same child under a different parent while offline.
 */
export function generateTreeMoveFixture(corpusSeed: string): Readonly<{
  createOps: readonly AbstractTreeOp[];
  childId: string;
  replicaAMove: AbstractTreeOp;
  replicaBMove: AbstractTreeOp;
}> {
  const rng = SplitMix64.fromSeedString(`${corpusSeed}:concurrent_block_move`);
  const parentIds = ["parent-1", "parent-2", "parent-3"];
  const childId = "child-1";
  const initialParent = rng.pick(parentIds);
  const remainingParents = parentIds.filter((id) => id !== initialParent);
  const [parentForA, parentForB] = rng.nextBool() ? remainingParents : [remainingParents[1], remainingParents[0]];
  const createOps: AbstractTreeOp[] = [
    ...parentIds.map((nodeId): AbstractTreeOp => ({ kind: "tree.createNode", nodeId, parentId: null })),
    { kind: "tree.createNode", nodeId: childId, parentId: initialParent },
  ];
  return {
    createOps,
    childId,
    replicaAMove: { kind: "tree.move", nodeId: childId, parentId: parentForA as string },
    replicaBMove: { kind: "tree.move", nodeId: childId, parentId: parentForB as string },
  };
}

/** `duplicate_out_of_order_partial_batch` (idempotent replay half): a single local op whose update bytes get imported twice. */
export function generateDuplicateApplyFixture(corpusSeed: string): Readonly<{
  container: string;
  baseText: string;
  op: AbstractTextOp;
}> {
  const rng = SplitMix64.fromSeedString(`${corpusSeed}:duplicate_out_of_order_partial_batch`);
  const container = "body";
  const baseText = rng.nextString(TEXT_ALPHABET, 4, 10);
  return {
    container,
    baseText,
    op: { kind: "text.insert", container, index: baseText.length, text: rng.nextString(TEXT_ALPHABET, 1, 8) },
  };
}

/** `adversarial_update_limits` (corrupt/truncated half): a valid update byte-truncated to a deterministic shorter length. */
export function generateCorruptUpdateFixture(corpusSeed: string): Readonly<{
  container: string;
  baseText: string;
  op: AbstractTextOp;
  truncateTo: (validUpdateLength: number) => number;
}> {
  const rng = SplitMix64.fromSeedString(`${corpusSeed}:adversarial_update_limits`);
  const container = "body";
  const baseText = rng.nextString(TEXT_ALPHABET, 4, 10);
  const fraction = 0.2 + rng.nextFloat() * 0.6; // keep 20%-80% of the bytes: always non-empty, always truncated
  return {
    container,
    baseText,
    op: { kind: "text.insert", container, index: baseText.length, text: rng.nextString(TEXT_ALPHABET, 4, 12) },
    truncateTo: (validUpdateLength: number) => Math.max(1, Math.min(validUpdateLength - 1, Math.floor(validUpdateLength * fraction))),
  };
}

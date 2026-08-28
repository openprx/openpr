import type { EngineAdapter } from "./contracts";

/**
 * Fixture-level operation vocabulary the corpus runner emits. Deliberately
 * candidate-agnostic: node/container identity is a plain string chosen by
 * the fixture generator, not an engine-native id. Each candidate's
 * ScenarioAdapter is responsible for mapping these ids onto whatever its
 * real engine needs (e.g. Loro TreeID, Yjs Y.Map key).
 */
export type AbstractTextOp =
  | Readonly<{ kind: "text.insert"; container: string; index: number; text: string }>
  | Readonly<{ kind: "text.delete"; container: string; index: number; length: number }>;

export type AbstractTreeOp =
  | Readonly<{ kind: "tree.createNode"; nodeId: string; parentId: string | null }>
  | Readonly<{ kind: "tree.move"; nodeId: string; parentId: string | null }>;

export type AbstractOp = AbstractTextOp | AbstractTreeOp;

/**
 * Extends the frozen production `EngineAdapter` contract with the two hooks
 * the corpus runner needs and production code never calls: a way to apply a
 * fixture op locally (returning the update bytes it produced) and a way to
 * read back a debuggable semantic snapshot for hashing. Kept out of
 * `contracts.ts` on purpose -- that file is the real UI-surface contract.
 */
export interface ScenarioAdapter extends EngineAdapter {
  applyLocalOp(op: AbstractOp): Promise<Uint8Array>;
  toDebugJson(): unknown;
}

export type ScenarioAdapterFactory = () => ScenarioAdapter;

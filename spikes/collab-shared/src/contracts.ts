export type CandidateName = "loro" | "yrs-yjs";

export type Frontier = Readonly<{
  bytes: Uint8Array;
}>;

export type EngineDiff = Readonly<{
  changedContainerIds: readonly string[];
}>;

/**
 * Candidate-agnostic CRDT engine boundary. Both spikes/collab-loro and
 * spikes/collab-yrs-yjs implement this against their real engine (loro-crdt
 * or yjs) -- this file is the single source of truth for the shape, per
 * v0.3-foundation.md work package 3b. Neither candidate directory may hold
 * its own copy.
 */
export interface EngineAdapter {
  readonly candidate: CandidateName;
  load(snapshot: Uint8Array): Promise<void>;
  importUpdate(update: Uint8Array): Promise<EngineDiff>;
  exportSnapshot(): Promise<Uint8Array>;
  exportFrom(frontier: Frontier): Promise<Uint8Array>;
  frontier(): Frontier;
  dispose(): Promise<void>;
}

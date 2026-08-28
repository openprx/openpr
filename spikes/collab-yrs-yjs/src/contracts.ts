export type CandidateName = "loro" | "yrs-yjs";

export type Frontier = Readonly<{
  bytes: Uint8Array;
}>;

export type EngineDiff = Readonly<{
  changedContainerIds: readonly string[];
}>;

export interface EngineAdapter {
  readonly candidate: CandidateName;
  load(snapshot: Uint8Array): Promise<void>;
  importUpdate(update: Uint8Array): Promise<EngineDiff>;
  exportSnapshot(): Promise<Uint8Array>;
  exportFrom(frontier: Frontier): Promise<Uint8Array>;
  frontier(): Frontier;
  dispose(): Promise<void>;
}

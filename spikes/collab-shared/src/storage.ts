export type StoredDocument = Readonly<{
  snapshot: Uint8Array;
  updates: readonly Uint8Array[];
}>;

/**
 * Candidate-agnostic offline persistence boundary. Both candidates reach
 * storage only through this shape, which keeps the workload entry point
 * symmetric -- see spikes/collab-shared/README notes in the delivery report
 * for why "same interface" does not mean "same implementation cost":
 * collab-yrs-yjs backs this with the upstream y-indexeddb@9.0.12 package,
 * while collab-loro backs it with a project-internal IndexedDB store because
 * no maintained IndexedDB persistence adapter ships with loro-crdt.
 */
export interface EngineStore {
  load(documentId: string): Promise<StoredDocument | null>;
  save(documentId: string, document: StoredDocument): Promise<void>;
  remove(documentId: string): Promise<void>;
  close(): Promise<void>;
}

/** @deprecated kept only as the historical name used by the y-indexeddb binding docs. */
export type IndexedDbEngineStore = EngineStore;

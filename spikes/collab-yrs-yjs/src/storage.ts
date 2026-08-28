// Moved to spikes/collab-shared/src/storage.ts (work package 3b). This
// candidate's concrete implementation wraps the upstream y-indexeddb@9.0.12
// IndexeddbPersistence -- see the delivery report for the storage-asymmetry
// note (the Loro candidate has no maintained upstream IndexedDB adapter and
// backs the same interface with a project-internal implementation instead).
export type { EngineStore, IndexedDbEngineStore, StoredDocument } from "@sylvode/collab-shared";

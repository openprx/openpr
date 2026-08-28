// Moved to spikes/collab-shared/src/storage.ts (work package 3b). This
// candidate's concrete implementation is IndexedDbEngineStore in this same
// directory's engine binding module -- see the delivery report for the
// storage-asymmetry note (this candidate has no maintained upstream
// IndexedDB adapter for loro-crdt, unlike y-indexeddb on the Yjs side).
export type { EngineStore, IndexedDbEngineStore, StoredDocument } from "@sylvode/collab-shared";

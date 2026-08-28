import * as Y from "yjs";
import { IndexeddbPersistence } from "y-indexeddb";
import type { IndexedDbEngineStore, StoredDocument } from "@sylvode/collab-shared";

/**
 * Real y-indexeddb@9.0.12 `IndexeddbPersistence` wrapped to the shared
 * `EngineStore` shape. `IndexeddbPersistence` owns its own `Y.Doc` (it
 * persists live updates as they happen rather than being handed a
 * `{snapshot, updates}` pair to write once), so `save()` replays the given
 * document's bytes into a scratch `Y.Doc` bound to the persistence instance
 * and waits for its debounced write to flush; `load()` reads that `Y.Doc`
 * back out as a single snapshot (`updates: []` -- `y-indexeddb` does not
 * expose its internal update log, only the merged doc state).
 *
 * `y-indexeddb`'s `IndexeddbPersistence` constructor calls the global
 * `indexedDB.open(...)` synchronously (via `lib0/indexeddb`), which does
 * not exist in Bun/Node -- confirmed by hand: `new IndexeddbPersistence(name, doc)`
 * throws `ReferenceError: indexedDB is not defined` outside a browser. This
 * class is wired against the real upstream API but its browser code path is
 * unverified in this repository's test environment; see the delivery report.
 */
export class YjsIndexedDbStore implements IndexedDbEngineStore {
  private readonly databaseNamePrefix: string;

  constructor(databaseNamePrefix = "sylvode-flow-collab") {
    this.databaseNamePrefix = databaseNamePrefix;
  }

  async load(documentId: string): Promise<StoredDocument | null> {
    const doc = new Y.Doc();
    const persistence = new IndexeddbPersistence(this.databaseName(documentId), doc);
    await persistence.whenSynced;
    const snapshot = Y.encodeStateAsUpdate(doc);
    await persistence.destroy();
    doc.destroy();
    // An untouched database round-trips to an empty (but valid) update, not a real absence signal
    // -- y-indexeddb has no "does this database exist" query, so this checks the merged doc instead.
    return snapshot.length <= 2 ? null : { snapshot, updates: [] };
  }

  async save(documentId: string, document: StoredDocument): Promise<void> {
    const doc = new Y.Doc();
    Y.applyUpdate(doc, document.snapshot);
    for (const update of document.updates) {
      Y.applyUpdate(doc, update);
    }
    const persistence = new IndexeddbPersistence(this.databaseName(documentId), doc);
    await persistence.whenSynced;
    await persistence.destroy();
    doc.destroy();
  }

  async remove(documentId: string): Promise<void> {
    const { clearDocument } = await import("y-indexeddb");
    await clearDocument(this.databaseName(documentId));
  }

  async close(): Promise<void> {
    // No shared connection is held between calls -- each load()/save() opens and closes its own
    // IndexeddbPersistence, so there is nothing further to release here.
  }

  private databaseName(documentId: string): string {
    return `${this.databaseNamePrefix}:${documentId}`;
  }
}

/**
 * Interface-level in-memory fake for `EngineStore`, used by this candidate's
 * own tests because `indexedDB` is unavailable under Bun/Node (see
 * `YjsIndexedDbStore` above, which throws `ReferenceError: indexedDB is
 * not defined` when constructed here). Exercises the `EngineStore` contract
 * end-to-end; does not exercise y-indexeddb itself.
 */
export class InMemoryEngineStore implements IndexedDbEngineStore {
  private readonly entries = new Map<string, StoredDocument>();

  async load(documentId: string): Promise<StoredDocument | null> {
    return this.entries.get(documentId) ?? null;
  }

  async save(documentId: string, document: StoredDocument): Promise<void> {
    this.entries.set(documentId, document);
  }

  async remove(documentId: string): Promise<void> {
    this.entries.delete(documentId);
  }

  async close(): Promise<void> {
    this.entries.clear();
  }
}

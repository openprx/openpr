import type { IndexedDbEngineStore, StoredDocument } from "@sylvode/collab-shared";

/**
 * The narrow key-value shape `LoroIndexedDbStore` needs. Exists so the same
 * store logic (framing, load/save/remove semantics) runs against a real
 * browser `indexedDB` backend or an in-memory fake for tests, without
 * duplicating that logic per backend.
 */
export interface KeyValueBackend {
  get(key: string): Promise<Uint8Array | null>;
  set(key: string, value: Uint8Array): Promise<void>;
  delete(key: string): Promise<void>;
  close(): Promise<void>;
}

export class InMemoryKeyValueBackend implements KeyValueBackend {
  private readonly entries = new Map<string, Uint8Array>();

  async get(key: string): Promise<Uint8Array | null> {
    return this.entries.get(key) ?? null;
  }

  async set(key: string, value: Uint8Array): Promise<void> {
    this.entries.set(key, value);
  }

  async delete(key: string): Promise<void> {
    this.entries.delete(key);
  }

  async close(): Promise<void> {
    this.entries.clear();
  }
}

const STORE_NAME = "loro-engine-store";
const DB_VERSION = 1;

/**
 * Real `indexedDB` (the browser global, part of the DOM lib types this
 * package already targets) backed KeyValueBackend. This is the
 * "project-internal IndexedDB store" declared in candidate.ts
 * (`offlineAdapter: "project-internal"`) -- there is no maintained upstream
 * IndexedDB persistence adapter for loro-crdt, unlike y-indexeddb on the
 * Yjs side (see the delivery report for the storage-asymmetry note this
 * package's runner is required to document per work package 3b).
 *
 * `indexedDB` does not exist as a global in Bun/Node, so this class cannot
 * be exercised by `bun test` in this repository -- it is wired against the
 * real IndexedDB API surface but its browser code path is unverified here.
 */
export class BrowserIndexedDbBackend implements KeyValueBackend {
  private dbPromise: Promise<IDBDatabase> | null = null;

  constructor(private readonly databaseName: string) {}

  private openDb(): Promise<IDBDatabase> {
    if (this.dbPromise !== null) {
      return this.dbPromise;
    }
    this.dbPromise = new Promise((resolve, reject) => {
      const request = indexedDB.open(this.databaseName, DB_VERSION);
      request.onupgradeneeded = () => {
        if (!request.result.objectStoreNames.contains(STORE_NAME)) {
          request.result.createObjectStore(STORE_NAME);
        }
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("indexedDB.open failed"));
    });
    return this.dbPromise;
  }

  async get(key: string): Promise<Uint8Array | null> {
    const db = await this.openDb();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readonly");
      const request = tx.objectStore(STORE_NAME).get(key);
      request.onsuccess = () => resolve((request.result as Uint8Array | undefined) ?? null);
      request.onerror = () => reject(request.error ?? new Error("indexedDB get failed"));
    });
  }

  async set(key: string, value: Uint8Array): Promise<void> {
    const db = await this.openDb();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readwrite");
      tx.objectStore(STORE_NAME).put(value, key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error("indexedDB put failed"));
    });
  }

  async delete(key: string): Promise<void> {
    const db = await this.openDb();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readwrite");
      tx.objectStore(STORE_NAME).delete(key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error("indexedDB delete failed"));
    });
  }

  async close(): Promise<void> {
    const db = await this.openDb();
    db.close();
    this.dbPromise = null;
  }
}

/** Little-endian u32-length-prefixed framing: [snapshotLen, snapshot, updateCount, (updateLen, update)*]. */
function encodeStoredDocument(document: StoredDocument): Uint8Array {
  const parts: Uint8Array[] = [];
  const pushLengthPrefixed = (bytes: Uint8Array) => {
    const header = new Uint8Array(4);
    new DataView(header.buffer).setUint32(0, bytes.length, true);
    parts.push(header, bytes);
  };
  pushLengthPrefixed(document.snapshot);
  const countHeader = new Uint8Array(4);
  new DataView(countHeader.buffer).setUint32(0, document.updates.length, true);
  parts.push(countHeader);
  for (const update of document.updates) {
    pushLengthPrefixed(update);
  }
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function decodeStoredDocument(bytes: Uint8Array): StoredDocument {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = 0;
  const readLengthPrefixed = (): Uint8Array => {
    const length = view.getUint32(offset, true);
    offset += 4;
    const slice = bytes.slice(offset, offset + length);
    offset += length;
    return slice;
  };
  const snapshot = readLengthPrefixed();
  const updateCount = view.getUint32(offset, true);
  offset += 4;
  const updates: Uint8Array[] = [];
  for (let i = 0; i < updateCount; i += 1) {
    updates.push(readLengthPrefixed());
  }
  return { snapshot, updates };
}

export class LoroIndexedDbStore implements IndexedDbEngineStore {
  constructor(private readonly backend: KeyValueBackend) {}

  async load(documentId: string): Promise<StoredDocument | null> {
    const raw = await this.backend.get(documentId);
    return raw === null ? null : decodeStoredDocument(raw);
  }

  async save(documentId: string, document: StoredDocument): Promise<void> {
    await this.backend.set(documentId, encodeStoredDocument(document));
  }

  async remove(documentId: string): Promise<void> {
    await this.backend.delete(documentId);
  }

  async close(): Promise<void> {
    await this.backend.close();
  }
}

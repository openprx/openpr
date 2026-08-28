export type StoredDocument = Readonly<{
  snapshot: Uint8Array;
  updates: readonly Uint8Array[];
}>;

export interface IndexedDbEngineStore {
  load(documentId: string): Promise<StoredDocument | null>;
  save(documentId: string, document: StoredDocument): Promise<void>;
  remove(documentId: string): Promise<void>;
  close(): Promise<void>;
}

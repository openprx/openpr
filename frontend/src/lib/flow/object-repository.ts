// `ObjectRepository`: snapshot+tail load, engine lifecycle, IndexedDB accepted cache, reference
// counting (`contracts/ui-surface-v1.md` "职责边界"). This is the ONLY place that constructs a
// document's Loro engine doc ("同一 object 的 engine doc 只由 Repository 创建").
//
// v0.4 baseline note: `GET .../bootstrap` is not routed server-side yet (see `api/flow.ts`'s
// header comment), so `bootstrap()`/hydration in this package rides the WebSocket `snapshot`
// frame via a composed `ObjectSession` rather than a separate REST call -- there is exactly one
// WebSocket connection per open object, and this class is what owns starting it. This is a
// disclosed adaptation to the actual backend surface at this repo's baseline, not a second engine
// doc or a second update-sending path: `ObjectSession` (composed here, never constructed by a
// component) remains the only thing that ever calls `.submit()`.
//
// `loro-crdt` is imported dynamically, same rule as `editor-adapter.ts`: this keeps the engine
// chunk out of every route that isn't Flow.

import { writable, type Readable } from 'svelte/store';
import { flowApi, type FlowObjectView } from '$lib/api/flow';
import { LoroObjectSession, type AcceptedNotice, type SnapshotPayload } from './object-session';
import { LiveProjectionStore } from './projection-store';
import type { EngineDiff, FlowError, ObjectHandle, ObjectProjection, SyncState } from './types';
// `LoroDocType` (from `loro-prosemirror`, type-only) is the specific `{doc, data}` container
// shape `LoroSyncPlugin`/`LoroUndoPlugin` require; `loro-crdt`'s own `LoroDoc` generic defaults
// to an unconstrained container-shape record. Both describe the same runtime object -- Loro
// containers are created on demand by string key, so this is a nominal typing gap between two
// packages, not a real structural mismatch. The single `as unknown as LoroDocType` cast below,
// at the one place the doc is constructed, resolves it without weakening any other signature in
// this module to `any`.
import type { LoroDocType } from 'loro-prosemirror';

const DB_NAME = 'sylvode-flow-cache-v1';
const STORE_NAME = 'snapshots';

interface CachedSnapshot {
	objectId: string;
	snapshotBytes: Uint8Array;
	headSeq: number;
	headFrontier: string;
	cachedAt: number;
}

function openIndexedDb(): Promise<IDBDatabase | null> {
	return new Promise((resolve) => {
		if (typeof indexedDB === 'undefined') {
			resolve(null);
			return;
		}
		const request = indexedDB.open(DB_NAME, 1);
		request.onupgradeneeded = () => {
			request.result.createObjectStore(STORE_NAME, { keyPath: 'objectId' });
		};
		request.onsuccess = () => resolve(request.result);
		request.onerror = () => resolve(null);
	});
}

async function readCachedSnapshot(objectId: string): Promise<CachedSnapshot | null> {
	const db = await openIndexedDb();
	if (!db) return null;
	return new Promise((resolve) => {
		const tx = db.transaction(STORE_NAME, 'readonly');
		const req = tx.objectStore(STORE_NAME).get(objectId);
		req.onsuccess = () => resolve((req.result as CachedSnapshot | undefined) ?? null);
		req.onerror = () => resolve(null);
	});
}

async function writeCachedSnapshot(entry: CachedSnapshot): Promise<void> {
	const db = await openIndexedDb();
	if (!db) return;
	await new Promise<void>((resolve) => {
		const tx = db.transaction(STORE_NAME, 'readwrite');
		tx.objectStore(STORE_NAME).put(entry);
		tx.oncomplete = () => resolve();
		tx.onerror = () => resolve();
	});
}

/** Structural shape of a Loro map, wide enough to cover any container name (see `setTitle`'s
 * comment for why `LoroDocType.getMap` needs widening for non-`doc`/`data` containers). */
export interface NamedLoroMap {
	get(key: string): unknown;
	set(key: string, value: unknown): void;
	delete(key: string): void;
	getShallowValue(): Record<string, unknown>;
}

/** Reads/creates a Loro map container by name outside the two well-known ones `LoroDocType`
 * exposes. Used for the Navigator document's `order` container (`domain-model-v1.md`: "Navigator
 * 一个 workspace/project 的导航排序与显示元数据") and the shared `meta` container `setTitle` uses. */
export function getNamedMap(doc: LoroDocType, name: string): NamedLoroMap {
	return (doc as unknown as { getMap(key: string): NamedLoroMap }).getMap(name);
}

export interface OpenFlowObjectResult {
	readonly handle: ObjectHandle;
	readonly session: LoroObjectSession;
	readonly projection: LiveProjectionStore;
	readonly doc: LoroDocType;
	readonly flowError: Readable<FlowError | null>;
}

interface OpenEntry extends OpenFlowObjectResult {
	refCount: number;
}

async function loadLoro(): Promise<typeof import('loro-crdt')> {
	return import('loro-crdt');
}

export class FlowObjectRepository {
	private readonly open_ = new Map<string, OpenEntry>();

	async open(input: { workspaceId: string; objectId: string; signal: AbortSignal }): Promise<OpenFlowObjectResult> {
		const existing = this.open_.get(input.objectId);
		if (existing) {
			existing.refCount += 1;
			return existing;
		}

		const objectResult = await flowApi.getObject(input.objectId);
		if (objectResult.code !== 0 || !objectResult.data) {
			throw {
				code: objectResult.code === 404 ? 'not_found' : 'forbidden',
				recoverable: false
			} satisfies FlowError;
		}
		const object: FlowObjectView = objectResult.data;
		if (input.signal.aborted) {
			throw { code: 'not_found', recoverable: false } satisfies FlowError;
		}

		const { LoroDoc } = await loadLoro();
		const doc = new LoroDoc() as unknown as LoroDocType;

		const projection = new LiveProjectionStore(objectToProjection(object));
		const flowErrorStore = writable<FlowError | null>(null);
		let lastKnownFrontierBase64 = '';

		const session = new LoroObjectSession({
			onSnapshot: (payload: SnapshotPayload) => {
				doc.import(payload.snapshotBytes);
				for (const update of payload.tailUpdates) {
					doc.import(update.bytes);
				}
				lastKnownFrontierBase64 = payload.headFrontier;
				projection.applyEngineDiff({ documentId: object.document_id } satisfies EngineDiff, payload.headSeq);
				void writeCachedSnapshot({
					objectId: input.objectId,
					snapshotBytes: doc.export({ mode: 'snapshot' }),
					headSeq: payload.headSeq,
					headFrontier: payload.headFrontier,
					cachedAt: Date.now()
				});
			},
			onAccepted: (notice: AcceptedNotice) => {
				lastKnownFrontierBase64 = notice.headFrontier;
				projection.applyEngineDiff({ documentId: object.document_id } satisfies EngineDiff, notice.headSeq);
			},
			onFlowError: (error: FlowError) => {
				flowErrorStore.set(error);
			}
		});

		// Every local mutation of `doc` -- whether from `EditorAdapter`'s ProseMirror binding, a
		// navigator reorder, or a title edit -- funnels through this single subscription, so there
		// is exactly one path that ever calls `session.submit()` no matter which UI surface
		// produced the change ("同一 update 只由 Session 发出").
		doc.subscribeLocalUpdates((bytes: Uint8Array) => {
			void session
				.submit({ bytes, baseFrontier: lastKnownFrontierBase64 }, { kind: 'content_edit' })
				.catch((error: FlowError) => flowErrorStore.set(error));
		});

		const handle: ObjectHandle = {
			workspaceId: input.workspaceId,
			objectId: input.objectId,
			objectType: object.object_type,
			documentId: object.document_id,
			object
		};

		// Cold-start hint only: paints the last known state immediately while the real WS
		// snapshot loads, then gets overwritten wholesale by the first `onSnapshot` above. Not a
		// second writable source -- `doc` is not mutated from this branch, only read for a
		// display-only best-effort seed value that the projection re-derives from the real doc.
		const cached = await readCachedSnapshot(input.objectId);
		if (cached && !input.signal.aborted) {
			projection.applyEngineDiff({ documentId: object.document_id } satisfies EngineDiff, cached.headSeq);
		}

		await session.connect(handle);

		const entry: OpenEntry = { handle, session, projection, doc, flowError: flowErrorStore, refCount: 1 };
		this.open_.set(input.objectId, entry);
		return entry;
	}

	async close(objectId: string): Promise<void> {
		const entry = this.open_.get(objectId);
		if (!entry) return;
		entry.refCount -= 1;
		if (entry.refCount > 0) return;
		this.open_.delete(objectId);
		await entry.session.dispose();
		entry.doc.free();
	}

	getProjection(objectId: string): Readable<ObjectProjection> {
		const entry = this.open_.get(objectId);
		if (!entry) {
			throw new Error(`FlowObjectRepository.getProjection: ${objectId} is not open`);
		}
		return entry.projection.object;
	}

	getSyncState(objectId: string): Readable<SyncState> | null {
		return this.open_.get(objectId)?.session.state ?? null;
	}

	/** Sets `meta.title` directly on the Loro doc (canonical source, `ADR-0002`) and lets the
	 * existing `subscribeLocalUpdates` hook above ship it out like any other local edit. `meta` is
	 * a server-defined container (`crates/collab-core::engine::LoroCollabEngine::set_title`)
	 * outside `loro-prosemirror`'s narrowed `{doc, data}` container names. */
	setTitle(objectId: string, title: string): void {
		const entry = this.open_.get(objectId);
		if (!entry) return;
		getNamedMap(entry.doc, 'meta').set('title', title);
		entry.doc.commit();
		entry.projection.updateObjectMeta({ title });
	}

	/** `exportRecoveryDraft` (`ui-surface-v1.md`): a best-effort local snapshot the user can save
	 * when an intent could not be replayed. Exports the current in-memory doc snapshot bytes --
	 * not a semantic/markdown rendering, since that would require the full projection pipeline. */
	exportRecoveryDraft(objectId: string): Blob {
		const entry = this.open_.get(objectId);
		if (!entry) throw new Error(`FlowObjectRepository.exportRecoveryDraft: ${objectId} is not open`);
		const bytes = entry.doc.export({ mode: 'snapshot' });
		return new Blob([new Uint8Array(bytes)], { type: 'application/octet-stream' });
	}
}

function objectToProjection(object: FlowObjectView): ObjectProjection {
	return {
		objectId: object.id,
		objectType: object.object_type,
		title: object.title,
		documentSeq: object.document_seq,
		frontier: object.frontier,
		projectionSeq: object.projection_seq,
		parentId: object.parent_id ?? null
	};
}

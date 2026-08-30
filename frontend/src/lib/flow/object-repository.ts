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
import { flowApi, type FlowBootstrap, type FlowObjectView } from '$lib/api/flow';
import { FlowCommandService } from './command-service';
import { LoroObjectSession, type AcceptedNotice, type SnapshotPayload } from './object-session';
import { LiveProjectionStore } from './projection-store';
import { negotiateFlowLimitsVersion } from './limits';
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

function fromBase64(value: string): Uint8Array {
	const binary = atob(value);
	const out = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
	return out;
}

// Deliberately NOT `implements ObjectRepositoryContract`: `open()` here returns the richer
// `OpenFlowObjectResult` (handle + session + projection + doc + flowError), not the frozen
// interface's bare `ObjectHandle` -- callers (`FlowNavigator`/route pages) need the session and
// projection in the same call, not a second lookup. Every OTHER member below matches the contract
// signature exactly (`bootstrap`/`replaceWithAccepted`/`exportRecoveryDraft`/`close`/
// `getProjection`), so the shape is still the frozen one, just not a formal `implements` given
// TypeScript's structural typing would reject `open`'s covariant return.
export class FlowObjectRepository {
	private readonly open_ = new Map<string, OpenEntry>();
	// Ticket issuance is the only thing `ObjectSession` needs from `CommandService`
	// (`command-service.ts`'s `TicketIssuer`); owned here since `CommandService` has no
	// per-request state of its own and every open session shares one workspace-agnostic instance.
	private readonly commandService = new FlowCommandService();

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
		}, this.commandService);

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

	/** `GET .../bootstrap` (`ObjectRepositoryContract.bootstrap`). Independent of the live
	 * WebSocket session -- callers that need a fresh snapshot without tearing down the current
	 * connection (e.g. a manual "reload from server" recovery action) use this instead of
	 * `ObjectSession.reconnect`. */
	async bootstrap(objectId: string, known?: { seq: number; frontier: string }): Promise<FlowBootstrap> {
		const result = await flowApi.getBootstrap(objectId, known ? { known_seq: known.seq, known_frontier: known.frontier } : {});
		if (result.code !== 0 || !result.data) {
			throw {
				code: result.code === 404 ? 'not_found' : result.code === 409 ? 'resync_required' : 'forbidden',
				recoverable: result.code === 409
			} satisfies FlowError;
		}
		this.adoptBootstrapLimits(result.data);
		return result.data;
	}

	/** Feeds a fresh `Bootstrap.limits` payload through version negotiation and hands the outcome
	 * to the matching open session. This is the only place the client adopts server ceilings: a
	 * `supported` payload replaces the session's pre-bootstrap fallback with the server's real
	 * numbers, an unrecognised `version` drops that session to read-only
	 * (`contracts/limits-v1.md`: "Client 不得自行放宽或缓存跨 `version` limits"). */
	private adoptBootstrapLimits(bootstrap: FlowBootstrap): void {
		const entry = this.open_.get(bootstrap.object_id);
		if (!entry) return;
		entry.session.adoptLimits(negotiateFlowLimitsVersion(bootstrap.limits));
	}

	/** `ObjectRepositoryContract.replaceWithAccepted`: imports a bootstrap's snapshot+tail into the
	 * matching open document (`bootstrap.object_id`), wholesale. Loro's `import` is CRDT-merge, not
	 * destructive replace, so this only ever advances the doc toward the server's accepted state --
	 * it never rolls back content the doc already has that the server has since superseded. */
	async replaceWithAccepted(bootstrap: FlowBootstrap): Promise<void> {
		const entry = this.open_.get(bootstrap.object_id);
		if (!entry) return;
		entry.doc.import(fromBase64(bootstrap.snapshot_base64));
		for (const update of bootstrap.tail_updates) {
			entry.doc.import(fromBase64(update.bytes));
		}
		entry.projection.applyEngineDiff({ documentId: entry.handle.documentId } satisfies EngineDiff, bootstrap.head_seq);
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

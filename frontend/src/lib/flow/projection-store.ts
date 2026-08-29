// `ProjectionStore`: engine event -> immutable semantic projection. Never writes back to the
// engine, never fetches the network (`contracts/ui-surface-v1.md` "职责边界"). This is the ONLY
// thing Svelte components read for content -- the "single writable source" rule
// (`ui-surface-v1.md`/task brief "组件只订阅 projection，不同时维护服务器 JSON、本地 draft 和 CRDT
// 三个可写源") means no component keeps its own copy of block text; every keystroke round-trips
// through `EditorAdapter` -> Loro doc -> this store's derived read, never the other way.

import { derived, writable, type Readable } from 'svelte/store';
import type {
	BlockProjection,
	EngineDiff,
	ObjectProjection,
	ProjectionStoreContract,
	SelectionProjection
} from './types';

export class LiveProjectionStore implements ProjectionStoreContract {
	private readonly objectStore;
	private readonly blocksStore = writable<ReadonlyArray<BlockProjection>>([]);
	private readonly selectionStore = writable<SelectionProjection>({ blockId: null, anchor: 0, head: 0 });
	private highestKnownSeq = 0;

	constructor(initial: ObjectProjection) {
		this.objectStore = writable<ObjectProjection>(initial);
	}

	get object(): Readable<ObjectProjection> {
		return derived(this.objectStore, (v) => v);
	}

	get blocks(): Readable<ReadonlyArray<BlockProjection>> {
		return derived(this.blocksStore, (v) => v);
	}

	get selection(): Readable<SelectionProjection> {
		return derived(this.selectionStore, (v) => v);
	}

	setBlocks(blocks: ReadonlyArray<BlockProjection>): void {
		this.blocksStore.set(blocks);
	}

	setSelection(selection: SelectionProjection): void {
		this.selectionStore.set(selection);
	}

	updateObjectMeta(patch: Partial<ObjectProjection>): void {
		this.objectStore.update((current) => ({ ...current, ...patch }));
	}

	applyEngineDiff(_diff: EngineDiff, seq: number): void {
		this.highestKnownSeq = Math.max(this.highestKnownSeq, seq);
		this.objectStore.update((current) => ({ ...current, documentSeq: seq }));
	}

	assertAtOrBehind(headSeq: number): void {
		if (this.highestKnownSeq > headSeq) {
			throw new Error(
				`ProjectionStore invariant violated: local seq ${this.highestKnownSeq} is ahead of server head ${headSeq}`
			);
		}
	}
}

// Shared types for the five Flow adapters (`contracts/ui-surface-v1.md` "五个 adapter").
// Kept engine-agnostic where the contract requires it; only `editor-adapter.ts` and
// `object-repository.ts` (engine lifecycle) import `loro-crdt`/`loro-prosemirror` directly, and
// only ever from inside a dynamic `import()` so the engine chunk stays out of every non-Flow
// route bundle (`contracts/ui-surface-v1.md` "engine chunk 只从 `(app)/flow` 动态 import").

import type { FlowObjectType, FlowObjectView } from '$lib/api/flow';
import type { Readable } from 'svelte/store';

/** One raw CRDT update, produced locally by an `EditorAdapter` or a navigator reorder. */
export interface EngineUpdate {
	readonly bytes: Uint8Array;
	readonly baseFrontier: string;
}

/** Why a local update was produced, for the outbox/recovery-draft path. */
export type SemanticIntent =
	| { readonly kind: 'content_edit'; readonly blockId?: string }
	| { readonly kind: 'navigator_reorder'; readonly objectId: string };

/** Sync indicator states (`contracts/ui-surface-v1.md` "连接、refresh 与恢复状态机"). */
export type SyncState =
	| 'local'
	| 'saving'
	| 'saved'
	| 'offline'
	| 'reconnecting'
	| 'resyncing'
	| 'auth_required'
	| 'read_only'
	| 'error';

/** The stable error vocabulary shared by REST/WS (`contracts/error-mapping-v1.md`). */
export type FlowErrorCode =
	| 'unauthenticated'
	| 'forbidden'
	| 'feature_disabled'
	| 'not_found'
	| 'unsupported_protocol'
	| 'stale_frontier'
	| 'invalid_update'
	| 'policy_rejected'
	| 'limit_exceeded'
	| 'resync_required'
	| 'server_draining';

export interface FlowDrainDetails {
	readonly reason: 'drain' | 'contention';
	readonly retry_after_ms?: number;
}

export interface FlowError {
	readonly code: FlowErrorCode;
	readonly recoverable: boolean;
	readonly details?: unknown;
}

/** `sylvode.flow.limits.v1` (`contracts/limits-v1.md`) -- the client-side pre-check subset. */
export interface FlowLimitsV1 {
	readonly frameBytesMax: number;
	readonly updateBytesMax: number;
	readonly presenceBytesMax: number;
	readonly presenceTtlSecondsMax: number;
	readonly treeDepthMax: number;
	readonly containerCountMax: number;
	readonly documentBlockCountMax: number;
	readonly textBlockCharsMax: number;
	readonly documentTextCharsMax: number;
	readonly semanticPatchOperationsMax: number;
}

/** A block in the immutable projection a component reads (`ProjectionStore.blocks`). */
export interface BlockProjection {
	readonly id: string;
	readonly type: 'paragraph' | 'heading' | 'bulletList' | 'orderedList' | 'listItem' | 'codeBlock';
	readonly text: string;
	readonly level?: number;
	readonly indent: number;
	readonly position: number;
}

export interface SelectionProjection {
	readonly blockId: string | null;
	readonly anchor: number;
	readonly head: number;
}

export interface ObjectProjection {
	readonly objectId: string;
	readonly objectType: FlowObjectType;
	readonly title: string;
	readonly documentSeq: number;
	readonly frontier: string;
	readonly projectionSeq: number;
	readonly parentId: string | null;
}

/** One entry of a Navigator document's order map (`domain-model-v1.md` "Document boundaries"). */
export interface NavigatorEntry {
	readonly objectId: string;
	readonly position: string;
}

export interface ObjectHandle {
	readonly workspaceId: string;
	readonly objectId: string;
	readonly objectType: FlowObjectType;
	readonly documentId: string;
	readonly object: FlowObjectView;
}

export interface RelativeSelection {
	readonly anchor: number;
	readonly head: number;
}

export type EditorOrigin = 'local' | 'remote';

export interface RichTextCommand {
	readonly type: string;
	readonly payload?: unknown;
}

export interface EngineDiff {
	readonly documentId: string;
}

export interface HistorySummaryItem {
	readonly seq: number;
	readonly actor: string;
	readonly message: string | null;
	readonly summary: string;
	readonly createdAt: string;
}

// The five frozen adapter interfaces (`contracts/ui-surface-v1.md` "五个 adapter"), narrowed to
// what this v0.4 delivery actually implements. Fields the contract lists but v0.4 does not use
// (relation/search/grants reads, legacy-pages/package import/export) are intentionally omitted
// here rather than stubbed, so `bun run check` cannot be satisfied by a fake implementation --
// see the delivery report for the exact list of contract members not built this round.

export interface ObjectHandleInput {
	readonly workspaceId: string;
	readonly objectId: string;
	readonly signal: AbortSignal;
}

export interface ObjectRepositoryContract {
	open(input: ObjectHandleInput): Promise<ObjectHandle>;
	close(objectId: string): Promise<void>;
	getProjection(objectId: string): Readable<ObjectProjection>;
}

export interface ObjectSessionContract {
	readonly state: Readable<SyncState>;
	connect(handle: ObjectHandle): Promise<void>;
	submit(update: EngineUpdate, intent: SemanticIntent): Promise<void>;
	reconnect(reason: 'manual' | 'network' | 'stale_frontier' | 'auth_expired'): Promise<void>;
	setPresence(value: unknown): void;
	dispose(): Promise<void>;
}

export interface ProjectionStoreContract {
	readonly object: Readable<ObjectProjection>;
	readonly blocks: Readable<ReadonlyArray<BlockProjection>>;
	readonly selection: Readable<SelectionProjection>;
	applyEngineDiff(diff: EngineDiff, seq: number): void;
	assertAtOrBehind(headSeq: number): void;
}

export interface EditorAdapterContract {
	mount(host: HTMLElement, blockId: string): Promise<void>;
	applyRemote(change: EngineDiff): void;
	getSelection(): RelativeSelection | null;
	restoreSelection(value: RelativeSelection): void;
	undoLocal(): boolean;
	redoLocal(): boolean;
	destroy(): Promise<void>;
}

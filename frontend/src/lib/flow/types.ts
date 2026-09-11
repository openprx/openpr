// Shared types for the five Flow adapters (`contracts/ui-surface-v1.md` "五个 adapter").
// Kept engine-agnostic where the contract requires it; only `editor-adapter.ts` and
// `object-repository.ts` (engine lifecycle) import `loro-crdt`/`loro-prosemirror` directly, and
// only ever from inside a dynamic `import()` so the engine chunk stays out of every non-Flow
// route bundle (`contracts/ui-surface-v1.md` "engine chunk 只从 `(app)/flow` 动态 import").

import type { FlowBootstrap, FlowObjectType, FlowObjectView } from '$lib/api/flow';
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
	| 'authorization_churn'
	| 'server_draining';

export interface FlowDrainDetails {
	readonly reason: 'drain' | 'contention';
	readonly retry_after_ms?: number;
}

export interface FlowError {
	readonly code: FlowErrorCode;
	readonly recoverable: boolean;
	readonly details?: unknown;
	/** Who produced this error.
	 *
	 * `'server'` means it was read off a wire surface -- a REST envelope's `error_code`, a
	 * WebSocket `rejected` frame, or a frozen close code -- and its `details` are the server's own
	 * required discriminators. `'client'` means this build synthesised it locally (a connect
	 * timeout, a teardown that had to reject in-flight waiters, a pre-flight limit check).
	 *
	 * The distinction is not cosmetic. `contracts/error-mapping-v1.md` freezes `server_draining`'s
	 * `details.reason` as a REQUIRED, server-produced discriminator, and a client that can mint
	 * the identical shape for its own local conditions makes "the UI honoured the server's
	 * discriminator" untestable -- any test asserting it would also pass against a build that
	 * ignores the wire entirely and fabricates the reason itself. Absent means `'server'` only
	 * because the wire constructors set it explicitly; local constructors must not omit it. */
	readonly origin?: 'server' | 'client';
}

/** `sylvode.flow.limits.v1` (`contracts/limits-v1.md` "Bootstrap.limits wire schema").
 *
 * The COMPLETE effective ceiling set, one field per wire field, in the wire's own order. The
 * server (`apps/api/src/flow/collab/limits.rs::FlowLimitsV1`) returns all of them verbatim on
 * every `Bootstrap`; the client never assembles a partial subset and never relaxes a value of
 * its own accord (`limits-v1.md`: "Client 不得自行放宽或缓存跨 `version` limits").
 *
 * Field names are the lowerCamelCase of the wire's snake_case names, one-for-one -- the wire
 * name is derived mechanically (`limits.ts::wireFieldName`), never hand-mapped, so a rename on
 * either side cannot silently produce a half-populated limits object.
 *
 * Only a subset is used for client-side pre-checks today (`limits.ts::checkUpdateBytes` and
 * friends); the rest are carried so the UI reports the server's real ceilings rather than a
 * stale hand-copied guess. `version` is deliberately NOT a member: it is negotiated
 * (`FlowLimitsNegotiation`) before any of these values are trusted. */
export interface FlowLimitsV1 {
	readonly updateBytesMax: number;
	readonly websocketFrameBytesMax: number;
	readonly presencePayloadBytesMax: number;
	readonly presenceTtlSecondsMax: number;
	readonly bootstrapDecodedBytesMax: number;
	readonly bootstrapResponseBytesMax: number;
	readonly treeDepthMax: number;
	readonly containerCountMax: number;
	readonly documentBlockCountMax: number;
	readonly textBlockCharsMax: number;
	readonly documentTextCharsMax: number;
	readonly semanticPatchOperationsMax: number;
	readonly semanticPatchJsonBytesMax: number;
	readonly decodeApplyCpuMsMax: number;
	readonly decodeApplyWallMsMax: number;
	readonly isolatedApplyMemoryBytesMax: number;
	readonly openDocumentsPerConnectionMax: number;
	readonly connectionsPerUserMax: number;
	readonly connectionsPerDocumentMax: number;
	readonly connectionsPerWorkspaceMax: number;
	readonly presenceEntriesPerConnectionMax: number;
	readonly presenceEntriesPerDocumentMax: number;
	readonly framesPerConnectionPerSecond: number;
	readonly frameBurstMax: number;
	readonly updatesPerConnectionPerSecond: number;
	readonly updateBurstMax: number;
	readonly slowConsumerQueueFramesMax: number;
	readonly slowConsumerQueueBytesMax: number;
	readonly pageLimitDefault: number;
	readonly pageLimitMax: number;
	readonly authorizedScanRowsMax: number;
	readonly importArchiveBytesMax: number;
	readonly importExpandedBytesMax: number;
	readonly importEntryCountMax: number;
	readonly importCompressionRatioMax: number;
}

/** Why a `Bootstrap.limits` payload could not be adopted as authoritative.
 *
 * `unknownVersion` covers every shape the client cannot prove it understands: a missing/
 * non-string `version`, a `version` other than `sylvode.flow.limits.v1`, or a recognised version
 * whose payload does not carry all of `FlowLimitsV1`'s fields as non-negative finite numbers.
 * All three mean the same thing operationally -- this client cannot know the real ceilings. */
export type FlowLimitsRejectionReason =
	| 'missing_payload'
	| 'missing_version'
	| 'unsupported_version'
	| 'incomplete_payload';

/** Result of negotiating a live `Bootstrap.limits` payload against the version this client
 * implements (`limits.ts::negotiateFlowLimitsVersion`).
 *
 * `limits-v1.md`: "Client 不得自行放宽或缓存跨 `version` limits" -- so an unrecognised version can
 * never fall back to this build's compiled-in numbers and keep writing. It degrades the session
 * to the `read_only` sync state (`ui-surface-v1.md`'s fixed indicator vocabulary) and blocks
 * local writes; the returned `limits` are still populated (for read-side display only) and must
 * not be used to authorise an edit while `readOnly` is true. */
export type FlowLimitsNegotiation =
	| {
			readonly outcome: 'supported';
			readonly version: string;
			readonly limits: FlowLimitsV1;
			readonly readOnly: false;
	  }
	| {
			readonly outcome: 'unknownVersion';
			readonly version: string | null;
			readonly reason: FlowLimitsRejectionReason;
			/** Missing/invalid wire field names, in wire (snake_case) spelling. Empty unless
			 * `reason === 'incomplete_payload'`. */
			readonly missingFields: readonly string[];
			readonly limits: FlowLimitsV1;
			readonly readOnly: true;
	  };

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
	/** `GET .../bootstrap` (`rest-api-v1.md`), now routed server-side at this baseline. Used by the
	 * recovery flow to fetch a fresh snapshot+tail independent of the live WebSocket session. */
	bootstrap(objectId: string, known?: { seq: number; frontier: string }): Promise<FlowBootstrap>;
	/** Imports a bootstrap's snapshot+tail into the matching open document (keyed by
	 * `bootstrap.object_id`), replacing local accepted state wholesale. */
	replaceWithAccepted(bootstrap: FlowBootstrap): Promise<void>;
	/** Best-effort local snapshot export for an intent that could not be replayed
	 * (`ui-surface-v1.md` step 5: "policy_rejected/invalid_update...不能重放的 intent 进入
	 * recovery draft"). */
	exportRecoveryDraft(objectId: string): Blob;
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

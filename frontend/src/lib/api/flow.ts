// REST-only adapter for the Flow surface (`contracts/rest-api-v1.md`, v0.4 Flow Alpha table).
//
// This module owns HTTP shape only: it never holds an engine doc, a WebSocket, or UI store
// (`contracts/ui-surface-v1.md` "五个 adapter"). `ObjectRepository`/`CommandService` compose it;
// components never import it directly.
//
// v0.4 baseline note: this repo's baseline now routes `POST /flow/objects/{object_id}/commands`
// and `GET /flow/objects/{object_id}/bootstrap` server-side (`apps/api/src/routes/flow.rs`'s
// `post_flow_object_command`/`get_flow_object_bootstrap`, wired in `main.rs`) -- both are exposed
// below as `executeCommand`/`getBootstrap`. Content edits during an open session still go over
// `ObjectSession`/WebSocket `update` frames, not `commands`; `executeCommand` backs the
// server-governed lifecycle actions (`set_title|insert_block|update_block|delete_block|
// move_block|archive|restore`) `CommandService.execute` needs, and `getBootstrap` backs recovery
// (`ObjectRepository.bootstrap`/`replaceWithAccepted`). `GET /flow/objects/{object_id}/diff` is
// still in the frozen contract table but is NOT wired into the server router at this baseline --
// `getDiff` below is a typed-but-unrouted method (same documented-gap pattern this file already
// uses elsewhere): it compiles against the frozen contract and will 404 until the server routes
// it, which is a normal `ApiResult` error a caller can already handle, not a silent wrong result.

import { apiClient, type ApiResult } from './client';

export type FlowObjectType = 'page' | 'navigator';

export interface FlowObjectView {
	id: string;
	workspace_id: string;
	project_id: string | null;
	object_type: FlowObjectType;
	lifecycle_status: string;
	governance_metadata: Record<string, unknown>;
	title: string;
	semantic_content: unknown;
	document_id: string;
	document_seq: number;
	frontier: string;
	projection_seq: number;
	projection_lag: number;
	created_at: string;
	updated_at: string;
	archived_at: string | null;
	parent_id?: string | null;
}

export interface AcceptedChange {
	object: FlowObjectView;
	accepted_seq: number;
	head_frontier: string;
	projection_seq: number;
	semantic_diff: unknown;
	affected_object_ids: string[];
	event_id: string;
	command_result?: unknown;
}

export interface FlowObjectListResponse {
	items: FlowObjectView[];
	next_cursor?: string;
}

export interface FlowHistoryEntry {
	seq: number;
	actor: string;
	origin: string;
	message: string | null;
	// Server-observed shape (`apps/api/src/flow/query.rs`) is a structured JSON object (accepted
	// seq/changed block ids/document/object id), not the rendered string `rest-api-v1.md`'s
	// public-type sketch implies -- typed `unknown` here and rendered defensively.
	semantic_summary: unknown;
	created_at: string;
}

export interface FlowHistoryResponse {
	items: FlowHistoryEntry[];
	next_before_seq?: number;
}

export interface CreateFlowObjectInput {
	object_type: FlowObjectType;
	project_id?: string;
	parent_object_id?: string;
	title: string;
	idempotency_key: string;
	message?: string;
}

export interface ListFlowObjectsQuery {
	project_id?: string;
	unprojected?: boolean;
	object_type?: FlowObjectType;
	parent_id?: string;
	q?: string;
	cursor?: string;
	limit?: number;
	include_archived?: boolean;
}

export interface CreateTicketInput {
	workspace_id: string;
	document_id: string;
	client_id: string;
	origin: string;
}

export interface CollabTicket {
	ticket: string;
	expires_at: string;
	websocket_url: string;
}

export interface CollabDiagnostics {
	document_id: string;
	engine: string;
	format_version: number;
	snapshot_seq: number;
	head_seq: number;
	frontier: string;
	update_count: number;
	byte_size: number;
	last_compacted_at: string | null;
	projection_seq: number;
	integrity_state: string;
}

export interface FlowFeatureFlags {
	flow_enabled: boolean;
	default_member_level: 'full_access' | 'edit' | 'comment' | 'view';
	authz_epoch: number;
	updated_at: string;
	updated_by: string | null;
}

/** v0.4 command types (`rest-api-v1.md`'s `POST .../commands` row). */
export type FlowCommandType =
	| 'set_title'
	| 'insert_block'
	| 'update_block'
	| 'delete_block'
	| 'move_block'
	| 'archive'
	| 'restore';

export interface FlowCommandInput {
	type: FlowCommandType;
	payload?: unknown;
}

export interface ExecuteFlowCommandInput {
	command: FlowCommandInput;
	expected_frontier?: string;
	idempotency_key: string;
	message?: string;
}

export interface TailUpdateEntry {
	seq: number;
	update_id: string;
	/** Base64 of the raw CRDT update bytes. */
	bytes: string;
	before_frontier: string;
	after_frontier: string;
}

/** `GET .../bootstrap` response (`apps/api/src/flow/model.rs::Bootstrap`). */
export interface FlowBootstrap {
	object_id: string;
	document_id: string;
	engine: string;
	format_version: string;
	snapshot_seq: number;
	head_seq: number;
	/** Base64 of the full document snapshot bytes. */
	snapshot_base64: string;
	tail_updates: TailUpdateEntry[];
	head_frontier: string;
	/** Raw `sylvode.flow.limits.v1` wire object (snake_case fields + `version`). Deliberately
	 * `unknown`: it is untrusted until `flow/limits.ts::negotiateFlowLimitsVersion` proves the
	 * `version` is one this client implements and the payload is complete. */
	limits: unknown;
	websocket_path: string;
}

/** `GET .../diff` response (`rest-api-v1.md`row 148) -- typed, but NOT routed server-side at this
 * baseline; see this file's header comment. */
export interface FlowDiffResponse {
	object_id: string;
	from_seq: number;
	to_seq: number;
	from_frontier: string;
	to_frontier: string;
	semantic_diff: unknown;
	rendered?: string;
}

function buildQuery(params: Record<string, string | number | boolean | undefined>): string {
	const search = new URLSearchParams();
	for (const [key, value] of Object.entries(params)) {
		if (value !== undefined && value !== null && value !== '') {
			search.set(key, String(value));
		}
	}
	const qs = search.toString();
	return qs ? `?${qs}` : '';
}

export const flowApi = {
	createObject(workspaceId: string, input: CreateFlowObjectInput): Promise<ApiResult<AcceptedChange>> {
		return apiClient.post<AcceptedChange>(`/api/v1/workspaces/${workspaceId}/flow/objects`, input);
	},

	listObjects(workspaceId: string, query: ListFlowObjectsQuery = {}): Promise<ApiResult<FlowObjectListResponse>> {
		const qs = buildQuery({
			project_id: query.project_id,
			unprojected: query.unprojected,
			object_type: query.object_type,
			parent_id: query.parent_id,
			q: query.q,
			cursor: query.cursor,
			limit: query.limit,
			include_archived: query.include_archived
		});
		return apiClient.get<FlowObjectListResponse>(`/api/v1/workspaces/${workspaceId}/flow/objects${qs}`);
	},

	getObject(
		objectId: string,
		query: { at_seq?: number; render?: 'semantic_json' | 'markdown' } = {}
	): Promise<ApiResult<FlowObjectView>> {
		const qs = buildQuery({ at_seq: query.at_seq, render: query.render });
		return apiClient.get<FlowObjectView>(`/api/v1/flow/objects/${objectId}${qs}`);
	},

	getHistory(
		objectId: string,
		query: { before_seq?: number; limit?: number } = {}
	): Promise<ApiResult<FlowHistoryResponse>> {
		const qs = buildQuery({ before_seq: query.before_seq, limit: query.limit ?? 20 });
		return apiClient.get<FlowHistoryResponse>(`/api/v1/flow/objects/${objectId}/history${qs}`);
	},

	createTicket(input: CreateTicketInput): Promise<ApiResult<CollabTicket>> {
		return apiClient.post<CollabTicket>('/api/v1/collab/tickets', input);
	},

	getCollabDiagnostics(objectId: string): Promise<ApiResult<CollabDiagnostics>> {
		return apiClient.get<CollabDiagnostics>(`/api/v1/flow/objects/${objectId}/collab?include_sizes=true`);
	},

	/**
	 * `GET /workspaces/{workspace_id}/features/flow` (`rest-api-v1.md` v0.4 table). Not yet routed
	 * server-side at this repo's baseline -- callers MUST treat any non-success `ApiResult`
	 * (including a transport-level failure from calling an unrouted path) as `flow_enabled=false`,
	 * matching the fail-closed default `flow_workspace_settings.flow_enabled` already has in
	 * `apps/api/src/flow/repository.rs::fetch_flow_enabled`.
	 */
	getFeatureFlags(workspaceId: string): Promise<ApiResult<FlowFeatureFlags>> {
		return apiClient.get<FlowFeatureFlags>(`/api/v1/workspaces/${workspaceId}/features/flow`);
	},

	executeCommand(objectId: string, input: ExecuteFlowCommandInput): Promise<ApiResult<AcceptedChange>> {
		return apiClient.post<AcceptedChange>(`/api/v1/flow/objects/${objectId}/commands`, input);
	},

	getBootstrap(
		objectId: string,
		known: { known_seq?: number; known_frontier?: string } = {}
	): Promise<ApiResult<FlowBootstrap>> {
		const qs = buildQuery({ known_seq: known.known_seq, known_frontier: known.known_frontier });
		return apiClient.get<FlowBootstrap>(`/api/v1/flow/objects/${objectId}/bootstrap${qs}`);
	},

	/** Typed but unrouted server-side at this baseline -- see this file's header comment. */
	getDiff(
		objectId: string,
		query: { from_seq: number; to_seq: number; render?: 'semantic_json' | 'markdown' }
	): Promise<ApiResult<FlowDiffResponse>> {
		const qs = buildQuery({ from_seq: query.from_seq, to_seq: query.to_seq, render: query.render });
		return apiClient.get<FlowDiffResponse>(`/api/v1/flow/objects/${objectId}/diff${qs}`);
	}
};

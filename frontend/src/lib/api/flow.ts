// REST-only adapter for the Flow surface (`contracts/rest-api-v1.md`, v0.4 Flow Alpha table).
//
// This module owns HTTP shape only: it never holds an engine doc, a WebSocket, or UI store
// (`contracts/ui-surface-v1.md` "五个 adapter"). `ObjectRepository`/`CommandService` compose it;
// components never import it directly.
//
// v0.4 baseline note: `POST /flow/objects/{object_id}/commands` and
// `GET /flow/objects/{object_id}/bootstrap` are in the frozen contract table but are not wired
// into the server router at this repo's baseline (`e8f335c`) -- only create/list/get/history,
// collab ticket/ws/diagnostics/verify, and (not yet) `features/flow` exist server-side. This file
// still exposes typed methods for the full v0.4 table (so callers compile against the frozen
// contract and the gap is a single well-known TODO, not silent), but the Web v0.4 delivery in
// this package only calls the methods the server actually serves: content edits go over
// `ObjectSession`/WebSocket `update` frames, not `commands`/`bootstrap`.

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
	}
};

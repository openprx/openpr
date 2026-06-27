import { apiClient, type ApiResult, type PaginatedData } from './client';

export type ConnectorKind = 'webhook' | 'mcp' | 'rest' | 'cli' | 'openprx_tunnel';
export type InvocationStatus = 'pending' | 'dispatched' | 'running' | 'completed' | 'failed' | 'cancelled';
export type InvocationToolCallStatus = 'succeeded' | 'failed';

export interface Connector {
	id: string;
	workspace_id: string;
	project_id?: string | null;
	webhook_id?: string | null;
	kind: ConnectorKind;
	name: string;
	description?: string | null;
	endpoint?: string | null;
	auth_policy: Record<string, unknown>;
	capability_manifest: Record<string, unknown>;
	is_active: boolean;
	created_by?: string | null;
	created_at: string;
	updated_at: string;
}

export interface CreateConnectorData {
	project_id?: string;
	kind: ConnectorKind;
	name: string;
	description?: string;
	endpoint?: string;
	auth_policy?: Record<string, unknown>;
	capability_manifest?: Record<string, unknown>;
	is_active?: boolean;
}

export interface CreateInvocationData {
	connector_id?: string;
	target_agent_id?: string;
	trigger_kind: string;
	trigger_ref_type?: string;
	trigger_ref_id?: string;
	payload?: Record<string, unknown>;
	actor_id?: string;
}

export interface ConnectorReceiptData {
	status: 'received' | 'completed' | 'failed';
	idempotency_key?: string;
	payload?: Record<string, unknown>;
	error_message?: string;
}

export interface Invocation {
	id: string;
	workspace_id: string;
	project_id?: string | null;
	actor_id?: string | null;
	target_agent_id?: string | null;
	source_task_id?: string | null;
	trigger_kind: string;
	trigger_ref_type?: string | null;
	trigger_ref_id?: string | null;
	connector_id?: string | null;
	connector_kind?: ConnectorKind | null;
	status: InvocationStatus;
	payload: Record<string, unknown>;
	result?: Record<string, unknown> | null;
	error_message?: string | null;
	audit_chain_id?: string | null;
	created_at: string;
	updated_at: string;
}

export interface InvocationToolCall {
	id: string;
	invocation_id: string;
	workspace_id: string;
	project_id?: string | null;
	actor_id?: string | null;
	tool_name: string;
	transport: string;
	status: InvocationToolCallStatus;
	arguments: Record<string, unknown>;
	result_summary?: string | null;
	error_message?: string | null;
	duration_ms: number;
	started_at: string;
	completed_at: string;
	created_at: string;
}

export type EventInboxStatus = 'received' | 'processing' | 'processed' | 'failed';

export interface EventInbox {
	id: string;
	workspace_id: string;
	project_id?: string | null;
	source_kind: string;
	source_id?: string | null;
	idempotency_key: string;
	event_type: string;
	payload: Record<string, unknown>;
	status: EventInboxStatus;
	attempts: number;
	last_error?: string | null;
	received_at: string;
	processed_at?: string | null;
	updated_at: string;
}

export const connectorsApi = {
	list(workspaceId: string, params?: { project_id?: string; kind?: ConnectorKind }): Promise<ApiResult<Connector[]>> {
		const query = new URLSearchParams();
		if (params?.project_id) query.set('project_id', params.project_id);
		if (params?.kind) query.set('kind', params.kind);
		const suffix = query.size ? `?${query.toString()}` : '';
		return apiClient.get<Connector[]>(`/api/v1/workspaces/${workspaceId}/connectors${suffix}`);
	},

	get(workspaceId: string, connectorId: string): Promise<ApiResult<Connector>> {
		return apiClient.get<Connector>(`/api/v1/workspaces/${workspaceId}/connectors/${connectorId}`);
	},

	create(workspaceId: string, data: CreateConnectorData): Promise<ApiResult<Connector>> {
		return apiClient.post<Connector>(`/api/v1/workspaces/${workspaceId}/connectors`, data);
	},

	update(workspaceId: string, connectorId: string, data: Partial<CreateConnectorData>): Promise<ApiResult<Connector>> {
		return apiClient.patch<Connector>(`/api/v1/workspaces/${workspaceId}/connectors/${connectorId}`, data);
	},

	delete(workspaceId: string, connectorId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workspaces/${workspaceId}/connectors/${connectorId}`);
	},

	listInvocations(
		projectId: string,
		params?: { status?: InvocationStatus; page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<Invocation>>> {
		const query = new URLSearchParams();
		if (params?.status) query.set('status', params.status);
		if (params?.page) query.set('page', params.page.toString());
		if (params?.per_page) query.set('per_page', params.per_page.toString());
		const suffix = query.size ? `?${query.toString()}` : '';
		return apiClient.get<PaginatedData<Invocation>>(`/api/v1/projects/${projectId}/invocations${suffix}`);
	},

	createInvocation(projectId: string, data: CreateInvocationData): Promise<ApiResult<Invocation>> {
		return apiClient.post<Invocation>(`/api/v1/projects/${projectId}/invocations`, data);
	},

	getInvocation(invocationId: string): Promise<ApiResult<Invocation>> {
		return apiClient.get<Invocation>(`/api/v1/invocations/${invocationId}`);
	},

	listInvocationToolCalls(
		invocationId: string,
		params?: { status?: InvocationToolCallStatus; tool_name?: string; page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<InvocationToolCall>>> {
		const query = new URLSearchParams();
		if (params?.status) query.set('status', params.status);
		if (params?.tool_name) query.set('tool_name', params.tool_name);
		if (params?.page) query.set('page', params.page.toString());
		if (params?.per_page) query.set('per_page', params.per_page.toString());
		const suffix = query.size ? `?${query.toString()}` : '';
		return apiClient.get<PaginatedData<InvocationToolCall>>(`/api/v1/invocations/${invocationId}/tool-calls${suffix}`);
	},

	listInvocationInbox(
		invocationId: string,
		params?: { status?: EventInboxStatus; page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<EventInbox>>> {
		const query = new URLSearchParams();
		if (params?.status) query.set('status', params.status);
		if (params?.page) query.set('page', params.page.toString());
		if (params?.per_page) query.set('per_page', params.per_page.toString());
		const suffix = query.size ? `?${query.toString()}` : '';
		return apiClient.get<PaginatedData<EventInbox>>(`/api/v1/invocations/${invocationId}/inbox${suffix}`);
	},

	replayInvocationInbox(invocationId: string, inboxId: string): Promise<ApiResult<EventInbox>> {
		return apiClient.post<EventInbox>(`/api/v1/invocations/${invocationId}/inbox/${inboxId}/replay`);
	},

	listFormInbox(
		formId: string,
		params?: { status?: EventInboxStatus; page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<EventInbox>>> {
		const query = new URLSearchParams();
		if (params?.status) query.set('status', params.status);
		if (params?.page) query.set('page', params.page.toString());
		if (params?.per_page) query.set('per_page', params.per_page.toString());
		const suffix = query.size ? `?${query.toString()}` : '';
		return apiClient.get<PaginatedData<EventInbox>>(`/api/v1/forms/${formId}/inbox${suffix}`);
	},

	replayFormInbox(formId: string, inboxId: string): Promise<ApiResult<EventInbox>> {
		return apiClient.post<EventInbox>(`/api/v1/forms/${formId}/inbox/${inboxId}/replay`);
	},

	cancelInvocation(invocationId: string): Promise<ApiResult<Invocation>> {
		return apiClient.post<Invocation>(`/api/v1/invocations/${invocationId}/cancel`);
	},

	reportReceipt(invocationId: string, data: ConnectorReceiptData): Promise<ApiResult<Invocation>> {
		return apiClient.post<Invocation>(`/api/v1/invocations/${invocationId}/receipt`, data);
	}
};

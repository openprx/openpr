import { apiClient, type ApiResult, type PaginatedData } from './client';

export interface WorkflowStateDef {
	key: string;
	display_name: string;
	category: string;
	position: number;
	color?: string;
	is_initial: boolean;
	is_terminal: boolean;
}

export interface EffectiveWorkflow {
	workflow_id: string;
	source: 'project' | 'workspace' | 'system' | string;
	project_id?: string;
	workspace_id?: string;
	name: string;
	states: WorkflowStateDef[];
}

export interface WorkflowSummary {
	id: string;
	workspace_id: string | null;
	name: string;
	description: string;
	is_system_default: boolean;
	state_count: number;
	created_at: string;
	updated_at: string;
}

export interface WorkflowDetail {
	id: string;
	workspace_id: string | null;
	name: string;
	description: string;
	is_system_default: boolean;
	states: WorkflowStateResponse[];
	created_at: string;
	updated_at: string;
}

export interface WorkflowStateResponse {
	id: string;
	workflow_id: string;
	key: string;
	display_name: string;
	category: string;
	position: number;
	color: string | null;
	is_initial: boolean;
	is_terminal: boolean;
}

export const workflowsApi = {
	getEffectiveByProject(projectId: string): Promise<ApiResult<EffectiveWorkflow>> {
		return apiClient.get<EffectiveWorkflow>(`/api/v1/projects/${projectId}/workflow/effective`);
	},

	listByWorkspace(workspaceId: string): Promise<ApiResult<PaginatedData<WorkflowSummary>>> {
		return apiClient.get<PaginatedData<WorkflowSummary>>(`/api/v1/workspaces/${workspaceId}/workflows`);
	},

	create(workspaceId: string, data: { name: string; description?: string }): Promise<ApiResult<WorkflowDetail>> {
		return apiClient.post<WorkflowDetail>(`/api/v1/workspaces/${workspaceId}/workflows`, data);
	},

	get(workflowId: string): Promise<ApiResult<WorkflowDetail>> {
		return apiClient.get<WorkflowDetail>(`/api/v1/workflows/${workflowId}`);
	},

	update(workflowId: string, data: { name?: string; description?: string }): Promise<ApiResult<WorkflowDetail>> {
		return apiClient.put<WorkflowDetail>(`/api/v1/workflows/${workflowId}`, data);
	},

	delete(workflowId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workflows/${workflowId}`);
	},

	createState(
		workflowId: string,
		data: { key: string; display_name: string; category: string; color?: string; is_initial?: boolean; is_terminal?: boolean }
	): Promise<ApiResult<WorkflowStateResponse>> {
		return apiClient.post<WorkflowStateResponse>(`/api/v1/workflows/${workflowId}/states`, data);
	},

	updateState(
		stateId: string,
		data: { display_name?: string; category?: string; color?: string; is_initial?: boolean; is_terminal?: boolean }
	): Promise<ApiResult<WorkflowStateResponse>> {
		return apiClient.put<WorkflowStateResponse>(`/api/v1/workflow-states/${stateId}`, data);
	},

	deleteState(stateId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workflow-states/${stateId}`);
	},

	reorderStates(workflowId: string, stateIds: string[]): Promise<ApiResult<null>> {
		return apiClient.put<null>(`/api/v1/workflows/${workflowId}/states/reorder`, { state_ids: stateIds });
	},

	setProjectWorkflow(projectId: string, workflowId: string | null): Promise<ApiResult<null>> {
		return apiClient.put<null>(`/api/v1/projects/${projectId}/workflow`, { workflow_id: workflowId });
	}
};

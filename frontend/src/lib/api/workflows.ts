import { apiClient, type ApiResult } from './client';

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

export const workflowsApi = {
	getEffectiveByProject(projectId: string): Promise<ApiResult<EffectiveWorkflow>> {
		return apiClient.get<EffectiveWorkflow>(`/api/v1/projects/${projectId}/workflow/effective`);
	}
};

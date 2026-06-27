import { apiClient, type ApiResult, type PaginatedData } from './client';

export interface IssueCounts {
	by_state: Record<string, number>;
	total: number;
}

export interface Project {
	id: string;
	workspace_id: string;
	name: string;
	key: string;
	description?: string;
	type_key: string;
	type_settings: Record<string, unknown>;
	created_at: string;
	updated_at: string;
	issue_counts?: IssueCounts | null;
}

export interface ProjectType {
	key: string;
	workspace_id?: string | null;
	name: string;
	description: string;
	domain: string;
	default_workflow_id?: string | null;
	enabled_capabilities: unknown[];
	field_schema: Record<string, unknown>;
	artifact_schema: Record<string, unknown>;
	default_connectors: unknown[];
	created_at: string;
	updated_at: string;
}

export interface ScenarioTemplate {
	id: string;
	key: string;
	workspace_id?: string | null;
	name: string;
	description: string;
	industry: string;
	project_type_key: string;
	audience_label: string;
	workflow_template: Record<string, unknown>;
	field_schema: Record<string, unknown>;
	resource_schema: Record<string, unknown>;
	ai_roles: unknown[];
	governance_policy: Record<string, unknown>;
	connector_suggestions: unknown[];
	sample_data: Record<string, unknown>;
	created_at: string;
	updated_at: string;
}

export type ProjectResourceKind =
	| 'repo'
	| 'directory'
	| 'document_library'
	| 'crm_account'
	| 'erp_order'
	| 'equipment'
	| 'site'
	| 'custom';

export interface ProjectResource {
	id: string;
	project_id: string;
	kind: ProjectResourceKind;
	name: string;
	locator: Record<string, unknown>;
	permission_policy: Record<string, unknown>;
	sync_status: string;
	created_by?: string | null;
	created_at: string;
	updated_at: string;
}

export interface ProjectAgentPolicy {
	project_id: string;
	project_type: string;
	capabilities: unknown[];
	connector_kinds: string[];
	action_classes: Record<string, unknown>;
	mcp?: {
		writes_create_invocation?: boolean;
		workspace_scope_required?: boolean;
		project_context_required?: boolean;
		tool_registry?: {
			source?: string;
			groups?: Record<string, string[]>;
			enabled_tools?: string[];
		};
	};
}

export interface ProjectContext {
	project: Project;
	project_type?: ProjectType | null;
	resources: ProjectResource[];
	connectors: unknown[];
	governance?: unknown;
	workflow?: unknown;
	recent_decisions?: unknown[];
	agent_policy: ProjectAgentPolicy;
}

export const projectsApi = {
	list(workspaceId: string, params?: { page?: number; per_page?: number }): Promise<ApiResult<PaginatedData<Project>>> {
		const query = new URLSearchParams();
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());

		const endpoint = query.size
			? `/api/v1/workspaces/${workspaceId}/projects?${query.toString()}`
			: `/api/v1/workspaces/${workspaceId}/projects`;

		return apiClient.get<PaginatedData<Project>>(endpoint);
	},

	get(projectId: string): Promise<ApiResult<Project>> {
		return apiClient.get<Project>(`/api/v1/projects/${projectId}`);
	},

	getContext(projectId: string): Promise<ApiResult<ProjectContext>> {
		return apiClient.get<ProjectContext>(`/api/v1/projects/${projectId}/context`);
	},

	getAgentPolicy(projectId: string): Promise<ApiResult<ProjectAgentPolicy>> {
		return apiClient.get<ProjectAgentPolicy>(`/api/v1/projects/${projectId}/agent-policy`);
	},

	create(
		workspaceId: string,
		data: {
			name: string;
			key: string;
			description?: string;
			type_key?: string;
			type_settings?: Record<string, unknown>;
			scenario_template_key?: string;
		}
	): Promise<ApiResult<Project>> {
		return apiClient.post<Project>(`/api/v1/workspaces/${workspaceId}/projects`, data);
	},

	update(projectId: string, data: { name?: string; description?: string; type_key?: string; type_settings?: Record<string, unknown> }): Promise<ApiResult<Project>> {
		return apiClient.put<Project>(`/api/v1/projects/${projectId}`, data);
	},

	delete(projectId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/projects/${projectId}`);
	},

	listTypes(workspaceId: string): Promise<ApiResult<PaginatedData<ProjectType>>> {
		return apiClient.get<PaginatedData<ProjectType>>(`/api/v1/workspaces/${workspaceId}/project-types`);
	},

	createType(
		workspaceId: string,
		data: {
			key: string;
			name: string;
			description?: string;
			domain?: string;
		}
	): Promise<ApiResult<ProjectType>> {
		return apiClient.post<ProjectType>(`/api/v1/workspaces/${workspaceId}/project-types`, data);
	},

	updateType(
		key: string,
		data: {
			name?: string;
			description?: string;
			domain?: string;
		}
	): Promise<ApiResult<ProjectType>> {
		return apiClient.patch<ProjectType>(`/api/v1/project-types/${key}`, data);
	},

	listScenarioTemplates(params?: { project_type_key?: string; industry?: string }): Promise<ApiResult<PaginatedData<ScenarioTemplate>>> {
		const query = new URLSearchParams();
		if (params?.project_type_key) query.append('project_type_key', params.project_type_key);
		if (params?.industry) query.append('industry', params.industry);
		const endpoint = query.size ? `/api/v1/scenario-templates?${query.toString()}` : '/api/v1/scenario-templates';
		return apiClient.get<PaginatedData<ScenarioTemplate>>(endpoint);
	},

	getScenarioTemplate(key: string): Promise<ApiResult<ScenarioTemplate>> {
		return apiClient.get<ScenarioTemplate>(`/api/v1/scenario-templates/${key}`);
	},

	listResources(projectId: string): Promise<ApiResult<PaginatedData<ProjectResource>>> {
		return apiClient.get<PaginatedData<ProjectResource>>(`/api/v1/projects/${projectId}/resources`);
	},

	createResource(
		projectId: string,
		data: {
			kind: ProjectResourceKind;
			name: string;
			locator?: Record<string, unknown>;
			permission_policy?: Record<string, unknown>;
			sync_status?: string;
		}
	): Promise<ApiResult<ProjectResource>> {
		return apiClient.post<ProjectResource>(`/api/v1/projects/${projectId}/resources`, data);
	},

	updateResource(
		projectId: string,
		resourceId: string,
		data: {
			kind?: ProjectResourceKind;
			name?: string;
			locator?: Record<string, unknown>;
			permission_policy?: Record<string, unknown>;
			sync_status?: string;
		}
	): Promise<ApiResult<ProjectResource>> {
		return apiClient.patch<ProjectResource>(`/api/v1/projects/${projectId}/resources/${resourceId}`, data);
	},

	deleteResource(projectId: string, resourceId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/projects/${projectId}/resources/${resourceId}`);
	}
};

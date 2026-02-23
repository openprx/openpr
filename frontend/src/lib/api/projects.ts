import { apiClient, type ApiResult, type PaginatedData } from './client';

export interface IssueCounts {
	backlog: number;
	todo: number;
	in_progress: number;
	done: number;
	total: number;
}

export interface Project {
	id: string;
	workspace_id: string;
	name: string;
	key: string;
	description?: string;
	created_at: string;
	updated_at: string;
	issue_counts?: IssueCounts | null;
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

	create(workspaceId: string, data: { name: string; key: string; description?: string }): Promise<ApiResult<Project>> {
		return apiClient.post<Project>(`/api/v1/workspaces/${workspaceId}/projects`, data);
	},

	update(projectId: string, data: { name?: string; description?: string }): Promise<ApiResult<Project>> {
		return apiClient.put<Project>(`/api/v1/projects/${projectId}`, data);
	},

	delete(projectId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/projects/${projectId}`);
	}
};

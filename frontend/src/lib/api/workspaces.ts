import { apiClient, type ApiResult, type PaginatedData } from './client';

export interface Workspace {
	id: string;
	slug: string;
	name: string;
	description?: string;
	created_at: string;
	updated_at: string;
}

export interface WorkspaceMember {
	user_id: string;
	workspace_id: string;
	role: 'owner' | 'admin' | 'member';
	joined_at: string;
}

export const workspacesApi = {
	list(): Promise<ApiResult<PaginatedData<Workspace>>> {
		return apiClient.get<PaginatedData<Workspace>>('/api/v1/workspaces');
	},

	get(workspaceId: string): Promise<ApiResult<Workspace>> {
		return apiClient.get<Workspace>(`/api/v1/workspaces/${workspaceId}`);
	},

	create(data: { slug: string; name: string; description?: string }): Promise<ApiResult<Workspace>> {
		return apiClient.post<Workspace>('/api/v1/workspaces', data);
	},

	update(workspaceId: string, data: { name?: string; description?: string }): Promise<ApiResult<Workspace>> {
		return apiClient.put<Workspace>(`/api/v1/workspaces/${workspaceId}`, data);
	},

	delete(workspaceId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workspaces/${workspaceId}`);
	},

	getMembers(workspaceId: string): Promise<ApiResult<PaginatedData<WorkspaceMember>>> {
		return apiClient.get<PaginatedData<WorkspaceMember>>(`/api/v1/workspaces/${workspaceId}/members`);
	}
};

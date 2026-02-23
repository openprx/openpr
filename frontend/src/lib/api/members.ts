import { apiClient, type ApiResult, type PaginatedData } from './client';

export type WorkspaceMemberRole = 'owner' | 'admin' | 'member';

export interface WorkspaceMember {
	user_id: string;
	email: string;
	name: string;
	entity_type?: 'human' | 'bot' | string;
	agent_type?: 'openclaw' | 'webhook' | 'custom' | string | null;
	role: WorkspaceMemberRole;
	joined_at: string;
}

export const membersApi = {
	list(workspaceId: string): Promise<ApiResult<PaginatedData<WorkspaceMember>>> {
		return apiClient.get<PaginatedData<WorkspaceMember>>(`/api/v1/workspaces/${workspaceId}/members`);
	},

	add(
		workspaceId: string,
		data: { user_id: string; role: Exclude<WorkspaceMemberRole, 'owner'> }
	): Promise<ApiResult<WorkspaceMember>> {
		return apiClient.post<WorkspaceMember>(`/api/v1/workspaces/${workspaceId}/members`, data);
	},

	remove(workspaceId: string, userId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workspaces/${workspaceId}/members/${userId}`);
	},

	updateRole(
		workspaceId: string,
		userId: string,
		role: Exclude<WorkspaceMemberRole, 'owner'>
	): Promise<ApiResult<null>> {
		return apiClient.patch<null>(`/api/v1/workspaces/${workspaceId}/members/${userId}`, { role });
	},

	searchUsers(
		workspaceId: string,
		params: { search?: string; limit?: number } = {}
	): Promise<ApiResult<PaginatedData<{ id: string; email: string; name: string }>>> {
		const query = new URLSearchParams();
		if (params.search?.trim()) query.set('search', params.search.trim());
		if (typeof params.limit === 'number') query.set('limit', String(params.limit));
		const suffix = query.toString() ? `?${query.toString()}` : '';
		return apiClient.get<PaginatedData<{ id: string; email: string; name: string }>>(
			`/api/v1/admin/users${suffix}`
		);
	}
};

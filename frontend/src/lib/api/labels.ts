import { apiClient, type ApiResult, type PaginatedData } from './client';

export interface Label {
	id: string;
	project_id: string;
	name: string;
	color: string;
	created_at: string;
}

export const labelsApi = {
	list(workspaceId: string): Promise<ApiResult<PaginatedData<Label>>> {
		return apiClient.get<PaginatedData<Label>>(`/api/v1/workspaces/${workspaceId}/labels`);
	},

	create(workspaceId: string, data: { name: string; color: string }): Promise<ApiResult<Label>> {
		return apiClient.post<Label>(`/api/v1/workspaces/${workspaceId}/labels`, data);
	},

	addToIssue(issueId: string, labelId: string): Promise<ApiResult<null>> {
		return apiClient.post<null>(`/api/v1/issues/${issueId}/labels/${labelId}`, {});
	},

	listForIssue(issueId: string): Promise<ApiResult<PaginatedData<Label>>> {
		return apiClient.get<PaginatedData<Label>>(`/api/v1/issues/${issueId}/labels`);
	},

	removeFromIssue(issueId: string, labelId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/issues/${issueId}/labels/${labelId}`);
	}
};

import { apiClient, type ApiResult, type PaginatedData } from './client';

export type SprintStatus = 'planned' | 'active' | 'completed';

export interface Sprint {
	id: string;
	project_id: string;
	name: string;
	goal?: string;
	start_date: string;
	end_date: string;
	status: SprintStatus;
	created_at: string;
	updated_at: string;
}

export interface CreateSprintData {
	name: string;
	goal?: string;
	start_date: string;
	end_date: string;
	status?: SprintStatus;
}

export interface UpdateSprintData {
	name?: string;
	goal?: string;
	start_date?: string;
	end_date?: string;
	status?: SprintStatus;
}

export const sprintsApi = {
	list(projectId: string): Promise<ApiResult<PaginatedData<Sprint>>> {
		return apiClient.get<PaginatedData<Sprint>>(`/api/v1/projects/${projectId}/sprints`);
	},

	get(id: string): Promise<ApiResult<Sprint>> {
		return apiClient.get<Sprint>(`/api/v1/sprints/${id}`);
	},

	create(projectId: string, data: CreateSprintData): Promise<ApiResult<Sprint>> {
		return apiClient.post<Sprint>(`/api/v1/projects/${projectId}/sprints`, data);
	},

	update(id: string, data: UpdateSprintData): Promise<ApiResult<Sprint>> {
		return apiClient.put<Sprint>(`/api/v1/sprints/${id}`, data);
	},

	delete(id: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/sprints/${id}`);
	}
};

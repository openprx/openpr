import { apiClient, type ApiResult, type PaginatedData } from './client';
import type { User } from './auth';

export interface AdminUser extends Omit<User, 'role' | 'status'> {
	role?: string;
	status?: string;
	is_active?: boolean;
	entity_type?: 'human' | 'bot' | string;
	agent_type?: 'openclaw' | 'webhook' | 'custom' | string | null;
	agent_config?: Record<string, unknown> | null;
}

export const adminApi = {
	async listUsers(params: {
		search?: string;
		page?: number;
		limit?: number;
		entity_type?: string;
	} = {}): Promise<ApiResult<PaginatedData<AdminUser>>> {
		const query = new URLSearchParams();
		if (params.search?.trim()) query.set('search', params.search.trim());
		if (typeof params.page === 'number') query.set('page', String(params.page));
		if (typeof params.limit === 'number') query.set('limit', String(params.limit));
		if (params.entity_type?.trim()) query.set('entity_type', params.entity_type.trim());
		const suffix = query.toString() ? `?${query.toString()}` : '';
		return apiClient.get<PaginatedData<AdminUser>>(`/api/v1/admin/users${suffix}`);
	},

	async updateUser(
		userId: string,
		data: { name?: string; email?: string; role?: string }
	): Promise<ApiResult<User>> {
		return apiClient.put<User>(`/api/v1/admin/users/${userId}`, data);
	},

	async toggleUserStatus(userId: string): Promise<ApiResult<null>> {
		return apiClient.patch<null>(`/api/v1/admin/users/${userId}/status`, {});
	},

	async resetPassword(userId: string, newPassword: string): Promise<ApiResult<null>> {
		return apiClient.put<null>(`/api/v1/admin/users/${userId}/password`, {
			new_password: newPassword
		});
	},

	async createUser(data: {
		name: string;
		email: string;
		password?: string;
		entity_type?: string;
		agent_type?: string;
		agent_config?: Record<string, unknown>;
		role?: string;
	}): Promise<ApiResult<User>> {
		return apiClient.post<User>('/api/v1/admin/users', data);
	},

	async listBots(): Promise<ApiResult<PaginatedData<AdminUser>>> {
		return apiClient.get<PaginatedData<AdminUser>>('/api/v1/admin/users?entity_type=bot');
	}
};

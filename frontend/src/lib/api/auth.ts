import { apiClient, type ApiResult } from './client';

export interface LoginRequest {
	email: string;
	password: string;
}

export interface User {
	id: string;
	email: string;
	name: string;
	role?: 'owner' | 'admin' | 'member';
	status?: 'active' | 'disabled';
	avatar_url?: string;
	created_at: string;
	updated_at?: string;
}

export interface RegisterRequest {
	email: string;
	password: string;
	name: string;
}

export interface LoginResponse {
	tokens: {
		access_token: string;
		refresh_token: string;
		token_type: string;
		access_expires_in: number;
		refresh_expires_in: number;
	};
	user: User;
}

export const authApi = {
	async login(credentials: LoginRequest): Promise<ApiResult<LoginResponse>> {
		const result = await apiClient.post<LoginResponse>('/api/v1/auth/login', credentials);
		if (result.code === 0 && result.data?.tokens?.access_token) {
			apiClient.setToken(result.data.tokens.access_token);
			if (result.data.tokens.refresh_token) {
				apiClient.setRefreshToken(result.data.tokens.refresh_token);
			}
		}
		return result;
	},

	register(data: RegisterRequest): Promise<ApiResult<{ user: User }>> {
		return apiClient.post<{ user: User }>('/api/v1/auth/register', data);
	},

	me(): Promise<ApiResult<{ user: User }>> {
		return apiClient.get<{ user: User }>('/api/v1/auth/me');
	},

	async logout(): Promise<ApiResult<null>> {
		const result = await apiClient.post<null>('/api/v1/auth/logout');
		apiClient.clearAuth();
		return result;
	},

	async refreshToken(): Promise<ApiResult<{ token: string }>> {
		const result = await apiClient.post<LoginResponse>('/api/v1/auth/refresh');
		if (result.code !== 0 || !result.data?.tokens?.access_token) {
			return { code: result.code, message: result.message, data: null };
		}

		const token = result.data.tokens.access_token;
		apiClient.setToken(token);
		if (result.data.tokens.refresh_token) {
			apiClient.setRefreshToken(result.data.tokens.refresh_token);
		}
		return { code: 0, message: result.message, data: { token } };
	}
};

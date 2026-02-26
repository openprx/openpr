const API_BASE_URL = import.meta.env.VITE_API_BASE_URL ?? '';

export interface ApiResult<T> {
	code: number;
	message: string;
	data: T | null;
}

export interface PaginatedData<T> {
	items: T[];
	total: number;
	page: number;
	per_page: number;
	total_pages: number;
}

class ApiClient {
	private baseUrl: string;
	private token: string | null = null;
	private refreshTokenValue: string | null = null;
	private refreshInFlight: Promise<boolean> | null = null;

	constructor(baseUrl: string = API_BASE_URL) {
		this.baseUrl = baseUrl;
		if (typeof window !== 'undefined') {
			this.token = localStorage.getItem('auth_token');
			this.refreshTokenValue = localStorage.getItem('refresh_token');
		}
	}

	setToken(token: string | null) {
		this.token = token;
		if (typeof window !== 'undefined') {
			if (token) {
				localStorage.setItem('auth_token', token);
			} else {
				localStorage.removeItem('auth_token');
			}
		}
	}

	clearToken() {
		this.setToken(null);
	}

	setRefreshToken(token: string | null) {
		this.refreshTokenValue = token;
		if (typeof window !== 'undefined') {
			if (token) {
				localStorage.setItem('refresh_token', token);
			} else {
				localStorage.removeItem('refresh_token');
			}
		}
	}

	getRefreshToken(): string | null {
		return this.refreshTokenValue;
	}

	clearAuth() {
		this.setToken(null);
		this.setRefreshToken(null);
	}

	private redirectToLogin() {
		if (typeof window !== 'undefined') {
			window.location.href = '/auth/login';
		}
	}

	private async performTokenRefresh(): Promise<boolean> {
		const refreshToken = this.getRefreshToken();
		const body = refreshToken ? JSON.stringify({ refresh_token: refreshToken }) : undefined;
		const url = `${this.baseUrl}/api/v1/auth/refresh`;
		const headers = new Headers();
		headers.set('Content-Type', 'application/json');

		try {
			const res = await fetch(url, {
				method: 'POST',
				headers,
				body,
				credentials: 'include'
			});
			const parsed = (await res.json()) as Partial<
				ApiResult<{
					tokens?: { access_token?: string; refresh_token?: string };
				}>
			>;

			if (
				parsed.code === 0 &&
				parsed.data?.tokens?.access_token &&
				typeof parsed.data.tokens.access_token === 'string'
			) {
				this.setToken(parsed.data.tokens.access_token);
				if (typeof parsed.data.tokens.refresh_token === 'string') {
					this.setRefreshToken(parsed.data.tokens.refresh_token);
				}
				return true;
			}
		} catch (_error) {
			return false;
		}

		return false;
	}

	private async refreshAccessTokenOnce(): Promise<boolean> {
		if (!this.refreshInFlight) {
			this.refreshInFlight = this.performTokenRefresh().finally(() => {
				this.refreshInFlight = null;
			});
		}
		return this.refreshInFlight;
	}

	getToken(): string | null {
		return this.token;
	}

	async request<T>(
		method: string,
		endpoint: string,
		body?: unknown,
		retryAfterRefresh: boolean = true
	): Promise<ApiResult<T>> {
		const url = `${this.baseUrl}${endpoint}`;
		const headers = new Headers();
		headers.set('Content-Type', 'application/json');
		if (this.token) {
			headers.set('Authorization', `Bearer ${this.token}`);
		}

		try {
			const res = await fetch(url, {
				method,
				headers,
				body: body ? JSON.stringify(body) : undefined,
				credentials: 'include'
			});

			const parsed = (await res.json()) as Partial<ApiResult<T>>;
			const result: ApiResult<T> = {
				code: typeof parsed.code === 'number' ? parsed.code : 500,
				message: typeof parsed.message === 'string' ? parsed.message : 'Invalid response format',
				data: (parsed.data as T | null) ?? null
			};

			if (result.code === 401) {
				const isRefreshEndpoint = endpoint === '/api/v1/auth/refresh';
				if (!isRefreshEndpoint && retryAfterRefresh) {
					const refreshed = await this.refreshAccessTokenOnce();
					if (refreshed) {
						return this.request<T>(method, endpoint, body, false);
					}
				}
				this.clearAuth();
				this.redirectToLogin();
			}

			return result;
		} catch (error) {
			return {
				code: 500,
				message: error instanceof Error ? error.message : 'Network error',
				data: null
			};
		}
	}

	get<T>(endpoint: string): Promise<ApiResult<T>> {
		return this.request<T>('GET', endpoint);
	}

	post<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('POST', endpoint, data);
	}

	patch<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('PATCH', endpoint, data);
	}

	put<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('PUT', endpoint, data);
	}

	delete<T>(endpoint: string): Promise<ApiResult<T>> {
		return this.request<T>('DELETE', endpoint);
	}
}

export const apiClient = new ApiClient();

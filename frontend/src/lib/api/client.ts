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

	constructor(baseUrl: string = API_BASE_URL) {
		this.baseUrl = baseUrl;
		if (typeof window !== 'undefined') {
			this.token = localStorage.getItem('auth_token');
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

	getToken(): string | null {
		return this.token;
	}

	async request<T>(method: string, endpoint: string, body?: unknown): Promise<ApiResult<T>> {
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
				body: body ? JSON.stringify(body) : undefined
			});

			const parsed = (await res.json()) as Partial<ApiResult<T>>;
			const result: ApiResult<T> = {
				code: typeof parsed.code === 'number' ? parsed.code : 500,
				message: typeof parsed.message === 'string' ? parsed.message : 'Invalid response format',
				data: (parsed.data as T | null) ?? null
			};

			if (result.code === 401) {
				this.clearToken();
				if (typeof window !== 'undefined') {
					window.location.href = '/auth/login';
				}
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

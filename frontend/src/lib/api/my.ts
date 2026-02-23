import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export type MyIssueStatus = 'backlog' | 'todo' | 'in_progress' | 'done';
export type MyIssuePriority = 'low' | 'medium' | 'high' | 'urgent';

export interface MyIssue {
	id: string;
	project_id: string;
	workspace_id: string;
	project_name: string;
	title: string;
	status: MyIssueStatus;
	priority: MyIssuePriority;
	assignee_id?: string;
	updated_at: string;
}

export interface MyActivity {
	id: string;
	issue_id: string;
	project_id: string;
	workspace_id: string;
	project_name: string;
	issue_title: string;
	user_id?: string;
	author_name?: string;
	action: string;
	detail: Record<string, unknown>;
	created_at: string;
}

function mapIssue(raw: Record<string, unknown>): MyIssue {
	return {
		id: String(raw.id ?? ''),
		project_id: String(raw.project_id ?? ''),
		workspace_id: String(raw.workspace_id ?? ''),
		project_name: String(raw.project_name ?? ''),
		title: String(raw.title ?? ''),
		status: String(raw.state ?? raw.status ?? 'todo') as MyIssueStatus,
		priority: String(raw.priority ?? 'medium') as MyIssuePriority,
		assignee_id: raw.assignee_id ? String(raw.assignee_id) : undefined,
		updated_at: String(raw.updated_at ?? '')
	};
}

function mapActivity(raw: Record<string, unknown>): MyActivity {
	const detailRaw = raw.detail ?? raw.details ?? raw.payload;
	const detail = detailRaw && typeof detailRaw === 'object' ? (detailRaw as Record<string, unknown>) : {};
	return {
		id: String(raw.id ?? ''),
		issue_id: String(raw.issue_id ?? raw.resource_id ?? ''),
		project_id: String(raw.project_id ?? ''),
		workspace_id: String(raw.workspace_id ?? ''),
		project_name: String(raw.project_name ?? ''),
		issue_title: String(raw.issue_title ?? ''),
		user_id: raw.user_id ? String(raw.user_id) : raw.actor_id ? String(raw.actor_id) : undefined,
		author_name: raw.author_name ? String(raw.author_name) : undefined,
		action: String(raw.action ?? raw.event_type ?? ''),
		detail,
		created_at: String(raw.created_at ?? '')
	};
}

export const myApi = {
	async getMyIssues(params?: { page?: number; per_page?: number }): Promise<ApiResult<PaginatedData<MyIssue>>> {
		const queryParams = new URLSearchParams();
		if (params?.page) queryParams.set('page', String(params.page));
		if (params?.per_page) queryParams.set('per_page', String(params.per_page));
		const queryString = queryParams.toString();
		const endpoint = queryString ? `/api/v1/my/issues?${queryString}` : '/api/v1/my/issues';
		
		const result = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.loadMyIssuesFailed'), data: null };
		}
		return {
			code: 0,
			message: result.message,
			data: {
				...result.data,
				items: Array.isArray(result.data.items) ? result.data.items.map(mapIssue) : []
			}
		};
	},

	async getMyActivities(params?: { page?: number; per_page?: number }): Promise<ApiResult<PaginatedData<MyActivity>>> {
		const queryParams = new URLSearchParams();
		if (params?.page) queryParams.set('page', String(params.page));
		if (params?.per_page) queryParams.set('per_page', String(params.per_page));
		const queryString = queryParams.toString();
		const endpoint = queryString ? `/api/v1/my/activities?${queryString}` : '/api/v1/my/activities';
		
		const result = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.loadRecentActivityFailed'), data: null };
		}
		return {
			code: 0,
			message: result.message,
			data: {
				...result.data,
				items: Array.isArray(result.data.items) ? result.data.items.map(mapActivity) : []
			}
		};
	}
};

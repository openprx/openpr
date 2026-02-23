import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export interface Activity {
	id: string;
	issue_id: string;
	user_id: string;
	actor_id?: string;
	author_name?: string;
	action: string;
	detail: Record<string, unknown>;
	details?: unknown;
	created_at: string;
}

function normalizeDetail(raw: Record<string, unknown>): Record<string, unknown> {
	const detail = raw.detail ?? raw.details ?? raw.payload;
	if (!detail || typeof detail !== 'object') {
		return {};
	}
	return detail as Record<string, unknown>;
}

function mapActivity(raw: Record<string, unknown>, issueId: string): Activity {
	const userIdRaw = raw.user_id ?? raw.actor_id ?? '';
	const issueIdRaw = raw.issue_id ?? raw.resource_id ?? issueId;
	return {
		id: String(raw.id ?? ''),
		issue_id: String(issueIdRaw ?? issueId),
		user_id: String(userIdRaw ?? ''),
		actor_id: raw.actor_id ? String(raw.actor_id) : undefined,
		author_name: raw.author_name ? String(raw.author_name) : undefined,
		action: String(raw.action ?? raw.event_type ?? ''),
		detail: normalizeDetail(raw),
		details: raw.details,
		created_at: String(raw.created_at ?? '')
	};
}

export const activityApi = {
	async list(issueId: string): Promise<ApiResult<PaginatedData<Activity>>> {
		const result = await apiClient.get<PaginatedData<Record<string, unknown>>>(`/api/v1/issues/${issueId}/activities`);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.loadActivityFailed'), data: null };
		}
		return {
			code: 0,
			message: result.message,
			data: {
				...result.data,
				items: Array.isArray(result.data.items) ? result.data.items.map((item) => mapActivity(item, issueId)) : []
			}
		};
	}
};

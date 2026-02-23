import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export interface Comment {
	id: string;
	issue_id: string;
	user_id: string;
	author_name?: string;
	content: string;
	created_at: string;
	updated_at: string;
}

export interface CreateCommentInput {
	content: string;
	mentions?: string[];
}

export interface UpdateCommentInput {
	content: string;
	mentions?: string[];
}

function mapComment(raw: Record<string, unknown>): Comment {
	return {
		...(raw as unknown as Comment),
		issue_id: (raw.issue_id ?? raw.work_item_id ?? '') as string,
		user_id: (raw.user_id ?? raw.author_id ?? '') as string,
		content: (raw.content ?? raw.body ?? '') as string,
		author_name: (raw.author_name ?? '') as string
	};
}

export const commentsApi = {
	async list(issueId: string): Promise<ApiResult<PaginatedData<Comment>>> {
		const result = await apiClient.get<PaginatedData<Record<string, unknown>>>(`/api/v1/issues/${issueId}/comments`);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.loadCommentFailed'), data: null };
		}
		return {
			code: 0,
			message: result.message,
			data: {
				...result.data,
				items: Array.isArray(result.data.items) ? result.data.items.map(mapComment) : []
			}
		};
	},

	async create(issueId: string, data: CreateCommentInput): Promise<ApiResult<Comment>> {
		const result = await apiClient.post<Record<string, unknown>>(`/api/v1/issues/${issueId}/comments`, {
			content: data.content,
			mentions: data.mentions
		});
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.createCommentFailed'), data: null };
		}
		return { code: 0, message: result.message, data: mapComment(result.data) };
	},

	async update(id: string, data: UpdateCommentInput): Promise<ApiResult<Comment>> {
		const result = await apiClient.put<Record<string, unknown>>(`/api/v1/comments/${id}`, {
			content: data.content,
			mentions: data.mentions
		});
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.updateCommentFailed'), data: null };
		}
		return { code: 0, message: result.message, data: mapComment(result.data) };
	},

	delete(id: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/comments/${id}`);
	}
};

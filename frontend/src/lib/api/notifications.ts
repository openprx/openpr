import { apiClient, type ApiResult, type PaginatedData } from './client';

export type NotificationType =
	| 'mention'
	| 'assignment'
	| 'comment_reply'
	| 'issue_update'
	| 'project_update'
	| 'info'
	| string;

export interface Notification {
	id: string;
	user_id: string;
	workspace_id?: string;
	type: NotificationType;
	title: string;
	content: string;
	link?: string;
	related_issue_id?: string;
	issue_title?: string;
	related_comment_id?: string;
	related_project_id?: string;
	is_read: boolean;
	created_at: string;
	read_at?: string;
}

export interface NotificationsListResponse extends PaginatedData<Notification> {
	unread_count: number;
}

export const notificationsApi = {
	list(params?: { page?: number; limit?: number }): Promise<ApiResult<NotificationsListResponse>> {
		const search = new URLSearchParams();
		if (params?.page && params.page > 0) {
			search.set('page', String(params.page));
		}
		if (params?.limit && params.limit > 0) {
			search.set('limit', String(params.limit));
		}
		const query = search.toString();
		const endpoint = query ? `/api/v1/notifications?${query}` : '/api/v1/notifications';
		return apiClient.get<NotificationsListResponse>(endpoint);
	},

	getUnreadCount(): Promise<ApiResult<{ count: number }>> {
		return apiClient.get<{ count: number }>('/api/v1/notifications/unread-count');
	},

	markRead(id: string): Promise<ApiResult<null>> {
		return apiClient.put<null>(`/api/v1/notifications/${id}/read`);
	},

	markAllRead(): Promise<ApiResult<null>> {
		return apiClient.put<null>('/api/v1/notifications/read-all');
	},

	delete(id: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/notifications/${id}`);
	}
};

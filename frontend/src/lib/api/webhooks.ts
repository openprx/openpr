import { apiClient, type ApiResult, type PaginatedData } from './client';

export interface Webhook {
	id: string;
	workspace_id: string;
	name?: string;
	url: string;
	events: string[];
	active?: boolean;
	is_active: boolean;
	secret?: string;
	bot_user_id?: string | null;
	created_at: string;
}

export interface CreateWebhookData {
	name?: string;
	url: string;
	events: string[];
	is_active?: boolean;
	active?: boolean;
	secret?: string;
	bot_user_id?: string;
}

export interface WebhookDelivery {
	id: string;
	webhook_id: string;
	event_type: string;
	payload: Record<string, unknown>;
	request_headers: Record<string, unknown> | null;
	response_status: number | null;
	response_body: string | null;
	error: string | null;
	duration_ms: number | null;
	success: boolean;
	delivered_at: string | null;
	created_at: string;
}

export const webhooksApi = {
	list(workspaceId: string): Promise<ApiResult<PaginatedData<Webhook>>> {
		return apiClient.get<PaginatedData<Webhook>>(`/api/v1/workspaces/${workspaceId}/webhooks`);
	},

	get(workspaceId: string, webhookId: string): Promise<ApiResult<Webhook>> {
		return apiClient.get<Webhook>(`/api/v1/workspaces/${workspaceId}/webhooks/${webhookId}`);
	},

	create(workspaceId: string, data: CreateWebhookData): Promise<ApiResult<Webhook>> {
		return apiClient.post<Webhook>(`/api/v1/workspaces/${workspaceId}/webhooks`, data);
	},

	update(workspaceId: string, webhookId: string, data: Partial<Webhook>): Promise<ApiResult<Webhook>> {
		return apiClient.patch<Webhook>(`/api/v1/workspaces/${workspaceId}/webhooks/${webhookId}`, data);
	},

	delete(workspaceId: string, webhookId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workspaces/${workspaceId}/webhooks/${webhookId}`);
	},

	listDeliveries(
		workspaceId: string,
		webhookId: string,
		page: number = 1,
		perPage: number = 20
	): Promise<ApiResult<PaginatedData<WebhookDelivery>>> {
		return apiClient.get<PaginatedData<WebhookDelivery>>(
			`/api/v1/workspaces/${workspaceId}/webhooks/${webhookId}/deliveries?page=${page}&per_page=${perPage}`
		);
	},

	getDelivery(
		workspaceId: string,
		webhookId: string,
		deliveryId: string
	): Promise<ApiResult<WebhookDelivery>> {
		return apiClient.get<WebhookDelivery>(
			`/api/v1/workspaces/${workspaceId}/webhooks/${webhookId}/deliveries/${deliveryId}`
		);
	}
};

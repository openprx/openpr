import { apiClient, type ApiResult } from './client';

export type BotOperationSurface = 'mcp_http' | 'mcp_sse' | 'mcp_stdio' | 'cli' | 'rest';
export type BotOperationOutcome = 'ok' | 'error';

export interface BotOperationLog {
	id: string;
	workspace_id: string;
	bot_id: string;
	bot_name: string | null;
	tool_name: string | null;
	surface: BotOperationSurface;
	method: string;
	path: string;
	business_code: number;
	outcome: BotOperationOutcome;
	error_message: string | null;
	duration_ms: number;
	request_id: string;
	created_at: string;
}

export interface BotOperationLogPage {
	items: BotOperationLog[];
	next_cursor: string | null;
}

export interface BotOperationLogQuery {
	bot_id?: string;
	tool_name?: string;
	outcome?: BotOperationOutcome;
	cursor?: string;
	limit?: number;
}

export const operationLogsApi = {
	list(workspaceId: string, query: BotOperationLogQuery = {}): Promise<ApiResult<BotOperationLogPage>> {
		const params = new URLSearchParams();
		if (query.bot_id) params.set('bot_id', query.bot_id);
		if (query.tool_name) params.set('tool_name', query.tool_name);
		if (query.outcome) params.set('outcome', query.outcome);
		if (query.cursor) params.set('cursor', query.cursor);
		params.set('limit', String(query.limit ?? 50));
		return apiClient.get<BotOperationLogPage>(
			`/api/v1/workspaces/${workspaceId}/bot-operation-logs?${params.toString()}`
		);
	}
};

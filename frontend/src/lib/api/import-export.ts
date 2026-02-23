import { apiClient, type ApiResult } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export interface ImportedProjectSummary {
	id: string;
	workspace_id: string;
	name: string;
	key: string;
	created_at: string;
}

interface ExportPayload {
	format: string;
	filename: string;
	content: string;
}

export const importExportApi = {
	async exportProject(projectId: string): Promise<ApiResult<unknown>> {
		const result = await apiClient.get<ExportPayload>(`/api/v1/export/project/${projectId}`);
		if (result.code !== 0) {
			return { code: result.code, message: result.message || get(t)('project.exportFailed'), data: null };
		}

		if (result.data?.format === 'json' && typeof result.data.content === 'string') {
			try {
				return { code: 0, message: result.message, data: JSON.parse(result.data.content) as unknown };
			} catch {
				return { code: 0, message: result.message, data: result.data.content };
			}
		}

		return { code: 0, message: result.message, data: result.data };
	},

	importProject(workspaceId: string, payload: unknown): Promise<ApiResult<ImportedProjectSummary>> {
		return apiClient.post<ImportedProjectSummary>(`/api/v1/workspaces/${workspaceId}/import/project`, payload);
	}
};

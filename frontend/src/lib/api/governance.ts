import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export interface GovernanceConfig {
	id: number;
	project_id: string;
	review_required: boolean;
	auto_review_days: number;
	review_reminder_days: number;
	audit_report_cron: string;
	trust_update_mode: string;
	config: Record<string, unknown>;
	updated_by?: string;
	created_at: string;
	updated_at: string;
}

export interface GovernanceAuditLog {
	id: number;
	project_id: string;
	actor_id?: string;
	action: string;
	resource_type: string;
	resource_id?: string;
	old_value?: unknown;
	new_value?: unknown;
	metadata?: unknown;
	created_at: string;
}

export interface GovernanceAuditLogsQuery {
	project_id?: string;
	action?: string;
	resource_type?: string;
	actor_id?: string;
	start_at?: string;
	end_at?: string;
	page?: number;
	per_page?: number;
}

function mapGovernanceConfig(raw: Record<string, unknown>): GovernanceConfig {
	return {
		id: Number(raw.id ?? 0),
		project_id: String(raw.project_id ?? ''),
		review_required: Boolean(raw.review_required ?? true),
		auto_review_days: Number(raw.auto_review_days ?? 30),
		review_reminder_days: Number(raw.review_reminder_days ?? 7),
		audit_report_cron: String(raw.audit_report_cron ?? ''),
		trust_update_mode: String(raw.trust_update_mode ?? ''),
		config: (raw.config && typeof raw.config === 'object' ? raw.config : {}) as Record<string, unknown>,
		updated_by: raw.updated_by ? String(raw.updated_by) : undefined,
		created_at: String(raw.created_at ?? ''),
		updated_at: String(raw.updated_at ?? '')
	};
}

function mapAuditLog(raw: Record<string, unknown>): GovernanceAuditLog {
	return {
		id: Number(raw.id ?? 0),
		project_id: String(raw.project_id ?? ''),
		actor_id: raw.actor_id ? String(raw.actor_id) : undefined,
		action: String(raw.action ?? ''),
		resource_type: String(raw.resource_type ?? ''),
		resource_id: raw.resource_id ? String(raw.resource_id) : undefined,
		old_value: raw.old_value,
		new_value: raw.new_value,
		metadata: raw.metadata,
		created_at: String(raw.created_at ?? '')
	};
}

export const governanceApi = {
	async getConfig(projectId: string): Promise<ApiResult<GovernanceConfig>> {
		const query = new URLSearchParams({ project_id: projectId });
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/governance/config?${query.toString()}`);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('governanceConfig.loadFailed'),
				data: null
			};
		}
		return { code: 0, message: res.message, data: mapGovernanceConfig(res.data) };
	},

	async updateConfig(payload: {
		project_id: string;
		review_required?: boolean;
		auto_review_days?: number;
		review_reminder_days?: number;
		audit_report_cron?: string;
		trust_update_mode?: string;
		config?: Record<string, unknown>;
	}): Promise<ApiResult<GovernanceConfig>> {
		const res = await apiClient.put<Record<string, unknown>>('/api/v1/governance/config', payload);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('governanceConfig.updateFailed'),
				data: null
			};
		}
		return { code: 0, message: res.message, data: mapGovernanceConfig(res.data) };
	},

	async listAuditLogs(params: GovernanceAuditLogsQuery): Promise<ApiResult<PaginatedData<GovernanceAuditLog>>> {
		const query = new URLSearchParams();
		if (params.project_id) query.set('project_id', params.project_id);
		if (params.action) query.set('action', params.action);
		if (params.resource_type) query.set('resource_type', params.resource_type);
		if (params.actor_id) query.set('actor_id', params.actor_id);
		if (params.start_at) query.set('start_at', params.start_at);
		if (params.end_at) query.set('end_at', params.end_at);
		if (params.page) query.set('page', String(params.page));
		if (params.per_page) query.set('per_page', String(params.per_page));

		const endpoint = query.size
			? `/api/v1/governance/audit-logs?${query.toString()}`
			: '/api/v1/governance/audit-logs';
		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('governanceAuditLogs.loadFailed'),
				data: null
			};
		}
		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapAuditLog) : []
			}
		};
	}
};

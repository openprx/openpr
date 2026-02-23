import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export interface ProposalTemplate {
	id: string;
	project_id: string;
	name: string;
	description?: string;
	template_type: string;
	content: Record<string, unknown>;
	is_default: boolean;
	is_active: boolean;
	created_by?: string;
	created_at: string;
	updated_at: string;
}

export interface ListProposalTemplatesParams {
	project_id: string;
	template_type?: string;
	is_active?: boolean;
}

export interface CreateProposalTemplateInput {
	project_id: string;
	name: string;
	description?: string;
	template_type?: string;
	content: Record<string, unknown>;
	is_default?: boolean;
	is_active?: boolean;
}

export interface UpdateProposalTemplateInput {
	name?: string;
	description?: string;
	template_type?: string;
	content?: Record<string, unknown>;
	is_default?: boolean;
	is_active?: boolean;
}

function mapTemplate(raw: Record<string, unknown>): ProposalTemplate {
	return {
		id: String(raw.id ?? ''),
		project_id: String(raw.project_id ?? ''),
		name: String(raw.name ?? ''),
		description: raw.description ? String(raw.description) : undefined,
		template_type: String(raw.template_type ?? 'governance'),
		content: (raw.content && typeof raw.content === 'object' ? raw.content : {}) as Record<string, unknown>,
		is_default: Boolean(raw.is_default ?? false),
		is_active: Boolean(raw.is_active ?? true),
		created_by: raw.created_by ? String(raw.created_by) : undefined,
		created_at: String(raw.created_at ?? ''),
		updated_at: String(raw.updated_at ?? '')
	};
}

export const proposalTemplatesApi = {
	async list(params: ListProposalTemplatesParams): Promise<ApiResult<PaginatedData<ProposalTemplate>>> {
		const query = new URLSearchParams();
		query.set('project_id', params.project_id);
		if (params.template_type) query.set('template_type', params.template_type);
		if (params.is_active !== undefined) query.set('is_active', String(params.is_active));
		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(
			`/api/v1/proposal-templates?${query.toString()}`
		);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('proposalTemplates.loadFailed'),
				data: null
			};
		}
		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapTemplate) : []
			}
		};
	},

	async get(id: string): Promise<ApiResult<ProposalTemplate>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/proposal-templates/${id}`);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('proposalTemplates.loadDetailFailed'),
				data: null
			};
		}
		return { code: 0, message: res.message, data: mapTemplate(res.data) };
	},

	async create(payload: CreateProposalTemplateInput): Promise<ApiResult<ProposalTemplate>> {
		const res = await apiClient.post<Record<string, unknown>>('/api/v1/proposal-templates', payload);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('proposalTemplates.createFailed'),
				data: null
			};
		}
		return { code: 0, message: res.message, data: mapTemplate(res.data) };
	},

	async update(id: string, payload: UpdateProposalTemplateInput): Promise<ApiResult<ProposalTemplate>> {
		const res = await apiClient.put<Record<string, unknown>>(`/api/v1/proposal-templates/${id}`, payload);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('proposalTemplates.updateFailed'),
				data: null
			};
		}
		return { code: 0, message: res.message, data: mapTemplate(res.data) };
	},

	delete(id: string): Promise<ApiResult<null>> {
		return apiClient.delete(`/api/v1/proposal-templates/${id}`);
	}
};

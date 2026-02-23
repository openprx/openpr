import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export type AiLevel = 'observer' | 'advisor' | 'voter' | 'vetoer' | 'autonomous';

export interface AiAgent {
	id: string;
	project_id: string;
	name: string;
	model: string;
	provider: string;
	api_endpoint?: string;
	capabilities: string[];
	domain_overrides?: Record<string, string>;
	max_domain_level: AiLevel;
	can_veto_human_consensus: boolean;
	reason_min_length: number;
	is_active: boolean;
	registered_by: string;
	last_active_at?: string;
	created_at: string;
}

export interface AiAgentStats {
	id: string;
	project_id: string;
	total_votes: number;
	yes_votes: number;
	no_votes: number;
	abstain_votes: number;
	total_comments: number;
	last_voted_at?: string;
	last_commented_at?: string;
	last_active_at?: string;
}

function normalizeLevel(value: unknown): AiLevel {
	const normalized = String(value ?? 'voter').toLowerCase();
	if (normalized === 'observer' || normalized === 'advisor' || normalized === 'voter' || normalized === 'vetoer' || normalized === 'autonomous') {
		return normalized;
	}
	return 'voter';
}

function parseStringList(value: unknown): string[] {
	if (Array.isArray(value)) {
		return value.map((item) => String(item)).filter(Boolean);
	}
	if (typeof value === 'string') {
		try {
			const parsed = JSON.parse(value) as unknown;
			return Array.isArray(parsed) ? parsed.map((item) => String(item)).filter(Boolean) : [];
		} catch {
			return [];
		}
	}
	return [];
}

function parseDomainOverrides(value: unknown): Record<string, string> | undefined {
	const parsed = typeof value === 'string' ? (() => {
		try {
			return JSON.parse(value) as unknown;
		} catch {
			return null;
		}
	})() : value;

	if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
		return undefined;
	}

	const result: Record<string, string> = {};
	for (const [key, rawValue] of Object.entries(parsed as Record<string, unknown>)) {
		const k = String(key).trim();
		const v = String(rawValue ?? '').trim();
		if (k && v) {
			result[k] = v;
		}
	}
	return Object.keys(result).length > 0 ? result : undefined;
}

function mapAiAgent(raw: Record<string, unknown>): AiAgent {
	return {
		id: String(raw.id ?? ''),
		project_id: String(raw.project_id ?? ''),
		name: String(raw.name ?? ''),
		model: String(raw.model ?? ''),
		provider: String(raw.provider ?? ''),
		api_endpoint: raw.api_endpoint ? String(raw.api_endpoint) : undefined,
		capabilities: parseStringList(raw.capabilities),
		domain_overrides: parseDomainOverrides(raw.domain_overrides),
		max_domain_level: normalizeLevel(raw.max_domain_level),
		can_veto_human_consensus: Boolean(raw.can_veto_human_consensus),
		reason_min_length: Number(raw.reason_min_length ?? 0),
		is_active: Boolean(raw.is_active),
		registered_by: String(raw.registered_by ?? ''),
		last_active_at: raw.last_active_at ? String(raw.last_active_at) : undefined,
		created_at: String(raw.created_at ?? '')
	};
}

export const aiAgentsApi = {
	async list(projectId: string): Promise<ApiResult<PaginatedData<AiAgent>>> {
		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(`/api/v1/projects/${projectId}/ai-participants`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadAiAgentsFailed'), data: null };
		}
		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapAiAgent) : []
			}
		};
	},

	async create(
		projectId: string,
		payload: {
			id: string;
			name: string;
			model: string;
			provider: string;
			api_endpoint?: string;
			capabilities: string[];
			domain_overrides?: Record<string, string>;
			max_domain_level?: AiLevel;
			can_veto_human_consensus?: boolean;
			reason_min_length?: number;
		}
	): Promise<ApiResult<AiAgent>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/projects/${projectId}/ai-participants`, payload);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.createAiAgentFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapAiAgent(res.data) };
	},

	async update(
		projectId: string,
		id: string,
		payload: {
			name?: string;
			model?: string;
			provider?: string;
			api_endpoint?: string;
			capabilities?: string[];
			domain_overrides?: Record<string, string>;
			max_domain_level?: AiLevel;
			can_veto_human_consensus?: boolean;
			reason_min_length?: number;
			is_active?: boolean;
		}
	): Promise<ApiResult<AiAgent>> {
		const res = await apiClient.patch<Record<string, unknown>>(`/api/v1/projects/${projectId}/ai-participants/${id}`, payload);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.updateAiAgentFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapAiAgent(res.data) };
	},

	delete(projectId: string, id: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/projects/${projectId}/ai-participants/${id}`);
	},

	async stats(projectId: string, id: string): Promise<ApiResult<AiAgentStats>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/projects/${projectId}/ai-participants/${id}/stats`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadAiAgentStatsFailed'), data: null };
		}

		return {
			code: 0,
			message: res.message,
			data: {
				id: String(res.data.id ?? ''),
				project_id: String(res.data.project_id ?? ''),
				total_votes: Number(res.data.total_votes ?? 0),
				yes_votes: Number(res.data.yes_votes ?? 0),
				no_votes: Number(res.data.no_votes ?? 0),
				abstain_votes: Number(res.data.abstain_votes ?? 0),
				total_comments: Number(res.data.total_comments ?? 0),
				last_voted_at: res.data.last_voted_at ? String(res.data.last_voted_at) : undefined,
				last_commented_at: res.data.last_commented_at ? String(res.data.last_commented_at) : undefined,
				last_active_at: res.data.last_active_at ? String(res.data.last_active_at) : undefined
			}
		};
	}
};

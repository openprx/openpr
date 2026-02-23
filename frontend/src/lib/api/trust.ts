import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export type TrustLevel = 'observer' | 'advisor' | 'voter' | 'vetoer' | 'autonomous';
export type ParticipantType = 'human' | 'ai';
export type AppealStatus = 'pending' | 'accepted' | 'rejected';

export interface TrustScoreRank {
	user_id: string;
	user_type: ParticipantType;
	project_id: string;
	domain: string;
	score: number;
	level: TrustLevel;
	vote_weight: number;
	consecutive_rejections: number;
	cooldown_until?: string;
	updated_at: string;
}

export interface UserTrustScore {
	project_id: string;
	domain: string;
	score: number;
	level: TrustLevel;
	vote_weight: number;
	can_veto: boolean;
	consecutive_rejections: number;
	cooldown_until?: string;
	updated_at: string;
}

export interface UserTrustProfile {
	user_id: string;
	user_type: ParticipantType;
	scores: UserTrustScore[];
}

export interface Appeal {
	id: number;
	log_id: number;
	project_id: string;
	domain: string;
	appellant_id: string;
	reason: string;
	evidence: unknown;
	status: AppealStatus;
	reviewer_id?: string;
	review_note?: string;
	created_at: string;
	resolved_at?: string;
}

function asTrustLevel(value: unknown): TrustLevel {
	const normalized = String(value ?? 'observer').toLowerCase();
	if (normalized === 'advisor' || normalized === 'voter' || normalized === 'vetoer' || normalized === 'autonomous') {
		return normalized;
	}
	return 'observer';
}

function mapTrustScoreRank(raw: Record<string, unknown>): TrustScoreRank {
	return {
		user_id: String(raw.user_id ?? ''),
		user_type: String(raw.user_type ?? 'human') === 'ai' ? 'ai' : 'human',
		project_id: String(raw.project_id ?? ''),
		domain: String(raw.domain ?? 'global'),
		score: Number(raw.score ?? 0),
		level: asTrustLevel(raw.level),
		vote_weight: Number(raw.vote_weight ?? 1),
		consecutive_rejections: Number(raw.consecutive_rejections ?? 0),
		cooldown_until: raw.cooldown_until ? String(raw.cooldown_until) : undefined,
		updated_at: String(raw.updated_at ?? '')
	};
}

function mapAppeal(raw: Record<string, unknown>): Appeal {
	const statusRaw = String(raw.status ?? 'pending').toLowerCase();
	const status: AppealStatus = statusRaw === 'accepted' || statusRaw === 'rejected' ? statusRaw : 'pending';
	return {
		id: Number(raw.id ?? 0),
		log_id: Number(raw.log_id ?? 0),
		project_id: String(raw.project_id ?? ''),
		domain: String(raw.domain ?? ''),
		appellant_id: String(raw.appellant_id ?? ''),
		reason: String(raw.reason ?? ''),
		evidence: raw.evidence,
		status,
		reviewer_id: raw.reviewer_id ? String(raw.reviewer_id) : undefined,
		review_note: raw.review_note ? String(raw.review_note) : undefined,
		created_at: String(raw.created_at ?? ''),
		resolved_at: raw.resolved_at ? String(raw.resolved_at) : undefined
	};
}

export const trustApi = {
	async listTrustScores(params?: {
		project_id?: string;
		domain?: string;
		page?: number;
		per_page?: number;
	}): Promise<ApiResult<PaginatedData<TrustScoreRank>>> {
		const query = new URLSearchParams();
		if (params?.project_id) query.set('project_id', params.project_id);
		if (params?.domain) query.set('domain', params.domain);
		if (params?.page) query.set('page', String(params.page));
		if (params?.per_page) query.set('per_page', String(params.per_page));

		const endpoint = query.size ? `/api/v1/trust-scores?${query.toString()}` : '/api/v1/trust-scores';
		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadTrustScoresFailed'), data: null };
		}

		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapTrustScoreRank) : []
			}
		};
	},

	async getUserTrust(userId: string, projectId?: string): Promise<ApiResult<UserTrustProfile>> {
		const suffix = projectId ? `?project_id=${encodeURIComponent(projectId)}` : '';
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/users/${userId}/trust${suffix}`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadUserTrustFailed'), data: null };
		}

		const rawScores = Array.isArray(res.data.scores) ? (res.data.scores as Array<Record<string, unknown>>) : [];
		return {
			code: 0,
			message: res.message,
			data: {
				user_id: String(res.data.user_id ?? userId),
				user_type: String(res.data.user_type ?? 'human') === 'ai' ? 'ai' : 'human',
				scores: rawScores.map((row) => ({
					project_id: String(row.project_id ?? ''),
					domain: String(row.domain ?? 'global'),
					score: Number(row.score ?? 0),
					level: asTrustLevel(row.level),
					vote_weight: Number(row.vote_weight ?? 1),
					can_veto: Boolean(row.can_veto),
					consecutive_rejections: Number(row.consecutive_rejections ?? 0),
					cooldown_until: row.cooldown_until ? String(row.cooldown_until) : undefined,
					updated_at: String(row.updated_at ?? '')
				}))
			}
		};
	},

	async listAppeals(params?: { status?: AppealStatus | 'all'; mine?: boolean }): Promise<ApiResult<PaginatedData<Appeal>>> {
		const query = new URLSearchParams();
		if (params?.status && params.status !== 'all') query.set('status', params.status);
		if (typeof params?.mine === 'boolean') query.set('mine', String(params.mine));
		const endpoint = query.size
			? `/api/v1/trust-scores/appeals?${query.toString()}`
			: '/api/v1/trust-scores/appeals';

		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadAppealsFailed'), data: null };
		}

		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items)
					? res.data.items.map((row) => mapAppeal(row as Record<string, unknown>))
					: []
			}
		};
	},

	async createAppeal(payload: { log_id: number; reason: string; evidence?: unknown }): Promise<ApiResult<Appeal>> {
		const res = await apiClient.post<Record<string, unknown>>('/api/v1/trust-scores/appeals', payload);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.createAppealFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapAppeal(res.data) };
	},

	async updateAppeal(id: number, payload: { status: 'accepted' | 'rejected'; review_note?: string }): Promise<ApiResult<Appeal>> {
		const res = await apiClient.patch<Record<string, unknown>>(`/api/v1/trust-scores/appeals/${id}`, payload);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.updateAppealFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapAppeal(res.data) };
	},

	deleteAppeal(id: number): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/trust-scores/appeals/${id}`);
	}
};

import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export type VetoStatus = 'active' | 'overturned' | 'upheld' | 'withdrawn';

export interface EscalationVotes {
	ballots: Record<string, boolean>;
	overturned: number;
	upheld: number;
}

export interface VetoEvent {
	id: number;
	proposal_id: string;
	vetoer_id: string;
	domain: string;
	reason: string;
	status: VetoStatus;
	escalation_started_at?: string;
	escalation_result?: string;
	escalation_votes: EscalationVotes;
	created_at: string;
}

export interface VetoerScope {
	user_id: string;
	project_id: string;
	domain: string;
	granted_by: string;
	granted_at: string;
}

function mapEscalationVotes(value: unknown): EscalationVotes {
	const fallback: EscalationVotes = { ballots: {}, overturned: 0, upheld: 0 };
	if (!value || typeof value !== 'object') {
		return fallback;
	}

	const raw = value as Record<string, unknown>;
	const ballotsRaw = raw.ballots;
	const ballots: Record<string, boolean> = {};
	if (ballotsRaw && typeof ballotsRaw === 'object' && !Array.isArray(ballotsRaw)) {
		for (const [k, v] of Object.entries(ballotsRaw as Record<string, unknown>)) {
			ballots[k] = Boolean(v);
		}
	}

	return {
		ballots,
		overturned: Number(raw.overturned ?? 0),
		upheld: Number(raw.upheld ?? 0)
	};
}

function mapVetoEvent(raw: Record<string, unknown>): VetoEvent {
	const statusRaw = String(raw.status ?? 'active').toLowerCase();
	const status: VetoStatus =
		statusRaw === 'overturned' || statusRaw === 'upheld' || statusRaw === 'withdrawn'
			? statusRaw
			: 'active';

	return {
		id: Number(raw.id ?? raw.veto_id ?? 0),
		proposal_id: String(raw.proposal_id ?? ''),
		vetoer_id: String(raw.vetoer_id ?? ''),
		domain: String(raw.domain ?? ''),
		reason: String(raw.reason ?? ''),
		status,
		escalation_started_at: raw.escalation_started_at ? String(raw.escalation_started_at) : undefined,
		escalation_result: raw.escalation_result ? String(raw.escalation_result) : undefined,
		escalation_votes: mapEscalationVotes(raw.escalation_votes),
		created_at: String(raw.created_at ?? '')
	};
}

export const vetoApi = {
	async get(proposalId: string): Promise<ApiResult<VetoEvent>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/proposals/${proposalId}/veto`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadVetoFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapVetoEvent(res.data) };
	},

	async startEscalation(proposalId: string): Promise<ApiResult<VetoEvent>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/proposals/${proposalId}/veto/escalation`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.startEscalationFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapVetoEvent(res.data) };
	},

	async voteEscalation(proposalId: string, overturn: boolean): Promise<ApiResult<VetoEvent>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/proposals/${proposalId}/veto/escalation/vote`, {
			overturn
		});
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.voteEscalationFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapVetoEvent(res.data) };
	},

	async withdraw(proposalId: string): Promise<ApiResult<VetoEvent>> {
		const res = await apiClient.delete<Record<string, unknown>>(`/api/v1/proposals/${proposalId}/veto`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.withdrawVetoFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapVetoEvent(res.data) };
	},

	listVetoers(params?: { project_id?: string; domain?: string }): Promise<ApiResult<PaginatedData<VetoerScope>>> {
		const query = new URLSearchParams();
		if (params?.project_id) query.set('project_id', params.project_id);
		if (params?.domain) query.set('domain', params.domain);
		const endpoint = query.size ? `/api/v1/vetoers?${query.toString()}` : '/api/v1/vetoers';
		return apiClient.get<PaginatedData<VetoerScope>>(endpoint);
	}
};

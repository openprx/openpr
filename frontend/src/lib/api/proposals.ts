import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export type ProposalType = 'feature' | 'architecture' | 'priority' | 'resource' | 'governance' | 'bugfix';
export type ProposalStatus = 'draft' | 'open' | 'voting' | 'approved' | 'rejected' | 'vetoed' | 'archived';
export type VotingRule = 'simple_majority' | 'absolute_majority' | 'consensus';
export type CycleTemplate = 'rapid' | 'fast' | 'standard' | 'critical';
export type VoteChoice = 'yes' | 'no' | 'abstain';

export interface Proposal {
	id: string;
	title: string;
	proposal_type: ProposalType;
	status: ProposalStatus;
	author_id: string;
	author_type: 'human' | 'ai';
	content: string;
	domains: string[];
	voting_rule: VotingRule;
	cycle_template: CycleTemplate;
	template_id?: string;
	created_at: string;
	submitted_at?: string;
	voting_started_at?: string;
	voting_ended_at?: string;
	archived_at?: string;
}

export interface ProposalDetail {
	proposal: Proposal;
	tally: {
		yes: number;
		no: number;
		abstain: number;
	};
	decision_id?: string;
}

export interface ProposalComment {
	id: number;
	proposal_id: string;
	author_id: string;
	author_name?: string;
	author_avatar?: string;
	author_type: 'human' | 'ai';
	comment_type: string;
	content: string;
	created_at: string;
}

export interface ProposalVote {
	id: number;
	proposal_id: string;
	voter_id: string;
	voter_type: 'human' | 'ai';
	choice: VoteChoice;
	weight: number;
	reason?: string;
	voted_at: string;
}

export interface ProposalIssueLink {
	id: number;
	proposal_id: string;
	issue_id: string;
	issue_title: string;
	issue_state: string;
	created_at: string;
}

export interface CreateProposalInput {
	title?: string;
	proposal_type?: ProposalType;
	content?: string;
	domains?: string[];
	voting_rule?: VotingRule;
	cycle_template?: CycleTemplate;
	template_id?: string;
}

export interface UpdateProposalInput {
	title?: string;
	proposal_type?: ProposalType;
	content?: string;
	domains?: string[];
	voting_rule?: VotingRule;
	cycle_template?: CycleTemplate;
}

export interface ListProposalParams {
	status?: ProposalStatus;
	proposal_type?: ProposalType;
	domain?: string;
	page?: number;
	per_page?: number;
	sort?: string;
}

export interface ProposalIssueSearchResult {
	id: string;
	title: string;
	state: string;
	project_id: string;
}

function mapProposal(raw: Record<string, unknown>): Proposal {
	const parsedDomains = Array.isArray(raw.domains)
		? raw.domains.map((item) => String(item))
		: Array.isArray((raw.domains as Record<string, unknown> | undefined)?.items)
			? ((raw.domains as Record<string, unknown>).items as unknown[]).map((item) => String(item))
			: [];

	return {
		id: String(raw.id ?? ''),
		title: String(raw.title ?? ''),
		proposal_type: (raw.proposal_type ?? 'feature') as ProposalType,
		status: (raw.status ?? 'draft') as ProposalStatus,
		author_id: String(raw.author_id ?? ''),
		author_type: (raw.author_type ?? 'human') as 'human' | 'ai',
		content: String(raw.content ?? ''),
		domains: parsedDomains,
		voting_rule: (raw.voting_rule ?? 'simple_majority') as VotingRule,
		cycle_template: (raw.cycle_template ?? 'fast') as CycleTemplate,
		template_id: raw.template_id ? String(raw.template_id) : undefined,
		created_at: String(raw.created_at ?? ''),
		submitted_at: raw.submitted_at ? String(raw.submitted_at) : undefined,
		voting_started_at: raw.voting_started_at ? String(raw.voting_started_at) : undefined,
		voting_ended_at: raw.voting_ended_at ? String(raw.voting_ended_at) : undefined,
		archived_at: raw.archived_at ? String(raw.archived_at) : undefined
	};
}

export const proposalsApi = {
	async list(params?: ListProposalParams): Promise<ApiResult<PaginatedData<Proposal>>> {
		const query = new URLSearchParams();
		if (params?.status) query.set('status', params.status);
		if (params?.proposal_type) query.set('proposal_type', params.proposal_type);
		if (params?.domain) query.set('domain', params.domain);
		if (params?.page) query.set('page', String(params.page));
		if (params?.per_page) query.set('per_page', String(params.per_page));
		if (params?.sort) query.set('sort', params.sort);
		const qs = query.toString();
		const endpoint = qs ? `/api/v1/proposals?${qs}` : '/api/v1/proposals';

		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadProposalsFailed'), data: null };
		}

		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapProposal) : []
			}
		};
	},

	async get(id: string): Promise<ApiResult<ProposalDetail>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/proposals/${id}`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadProposalFailed'), data: null };
		}

		const proposalRaw = (res.data.proposal ?? {}) as Record<string, unknown>;
		const tallyRaw = (res.data.tally ?? {}) as Record<string, unknown>;
		return {
			code: 0,
			message: res.message,
			data: {
				proposal: mapProposal(proposalRaw),
				tally: {
					yes: Number(tallyRaw.yes ?? 0),
					no: Number(tallyRaw.no ?? 0),
					abstain: Number(tallyRaw.abstain ?? 0)
				},
				decision_id: res.data.decision_id ? String(res.data.decision_id) : undefined
			}
		};
	},

	async create(data: CreateProposalInput): Promise<ApiResult<Record<string, unknown>>> {
		const res = await apiClient.post<Record<string, unknown>>('/api/v1/proposals', data);
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.createProposalFailed'), data: null }
			: res;
	},

	async update(id: string, data: UpdateProposalInput): Promise<ApiResult<Proposal>> {
		const res = await apiClient.patch<Record<string, unknown>>(`/api/v1/proposals/${id}`, data);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.updateProposalFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapProposal(res.data) };
	},

	async delete(id: string): Promise<ApiResult<null>> {
		const res = await apiClient.delete<null>(`/api/v1/proposals/${id}`);
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.deleteProposalFailed'), data: null }
			: res;
	},

	async submit(id: string): Promise<ApiResult<Record<string, unknown>>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/proposals/${id}/submit`);
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.submitProposalFailed'), data: null }
			: res;
	},

	async startVoting(id: string): Promise<ApiResult<Record<string, unknown>>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/proposals/${id}/start-voting`);
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.startProposalVotingFailed'), data: null }
			: res;
	},

	async archive(id: string): Promise<ApiResult<Record<string, unknown>>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/proposals/${id}/archive`);
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.archiveProposalFailed'), data: null }
			: res;
	},

	async vote(id: string, choice: VoteChoice, reason?: string): Promise<ApiResult<Record<string, unknown>>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/proposals/${id}/votes`, {
			choice,
			reason
		});
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.proposalVoteFailed'), data: null }
			: res;
	},

	listVotes(id: string): Promise<ApiResult<Record<string, unknown>>> {
		return apiClient.get(`/api/v1/proposals/${id}/votes`);
	},

	async withdrawVote(id: string): Promise<ApiResult<null>> {
		const res = await apiClient.delete<null>(`/api/v1/proposals/${id}/votes/mine`);
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.withdrawProposalVoteFailed'), data: null }
			: res;
	},

	async listComments(id: string): Promise<ApiResult<PaginatedData<ProposalComment>>> {
		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(`/api/v1/proposals/${id}/comments`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.loadProposalCommentsFailed'), data: null };
		}
		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items)
					? res.data.items.map((raw) => ({
						id: Number(raw.id ?? 0),
						proposal_id: String(raw.proposal_id ?? ''),
						author_id: String(raw.author_id ?? ''),
						author_name: raw.author_name ? String(raw.author_name) : undefined,
						author_avatar: raw.author_avatar ? String(raw.author_avatar) : undefined,
						author_type: (raw.author_type ?? 'human') as 'human' | 'ai',
						comment_type: String(raw.comment_type ?? 'general'),
						content: String(raw.content ?? ''),
						created_at: String(raw.created_at ?? '')
					}))
					: []
			}
		};
	},

	async comment(
		id: string,
		content: string,
		commentType = 'general',
		mentions?: string[]
	): Promise<ApiResult<ProposalComment>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/proposals/${id}/comments`, {
			content,
			comment_type: commentType,
			mentions
		});
		return res.code !== 0 || !res.data
			? { code: res.code, message: res.message || get(t)('api.createProposalCommentFailed'), data: null }
			: {
				code: 0,
				message: res.message,
				data: {
					id: Number(res.data.id ?? 0),
					proposal_id: String(res.data.proposal_id ?? ''),
					author_id: String(res.data.author_id ?? ''),
					author_name: res.data.author_name ? String(res.data.author_name) : undefined,
					author_avatar: res.data.author_avatar ? String(res.data.author_avatar) : undefined,
					author_type: (res.data.author_type ?? 'human') as 'human' | 'ai',
					comment_type: String(res.data.comment_type ?? 'general'),
					content: String(res.data.content ?? ''),
					created_at: String(res.data.created_at ?? '')
				}
			};
	},

	deleteComment(id: string, commentId: number): Promise<ApiResult<null>> {
		return apiClient.delete(`/api/v1/proposals/${id}/comments/${commentId}`);
	},

	async listIssues(id: string): Promise<ApiResult<PaginatedData<ProposalIssueLink>>> {
		const res = await apiClient.get<PaginatedData<ProposalIssueLink>>(`/api/v1/proposals/${id}/issues`);
		return res.code !== 0 || !res.data
			? { code: res.code, message: res.message || get(t)('api.loadProposalIssuesFailed'), data: null }
			: res;
	},

	async linkIssue(id: string, issueId: string): Promise<ApiResult<null>> {
		const res = await apiClient.post<null>(`/api/v1/proposals/${id}/issues`, { issue_id: issueId });
		return res.code !== 0
			? { code: res.code, message: res.message || get(t)('api.linkProposalIssueFailed'), data: null }
			: res;
	},

	unlinkIssue(id: string, issueId: string): Promise<ApiResult<null>> {
		return apiClient.delete(`/api/v1/proposals/${id}/issues/${issueId}`);
	},

	async searchIssues(query: string): Promise<ApiResult<ProposalIssueSearchResult[]>> {
		const encoded = encodeURIComponent(query.trim());
		const res = await apiClient.get<Record<string, unknown>>(
			`/api/v1/search?q=${encoded}&type=issue&limit=10`
		);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('api.searchProposalIssuesFailed'), data: null };
		}
		const rawResults = Array.isArray(res.data.results) ? (res.data.results as Array<Record<string, unknown>>) : [];
		const issues = rawResults
			.filter((item) => String(item.type ?? '') === 'issue')
			.map((item) => ({
				id: String(item.id ?? ''),
				title: String(item.title ?? ''),
				state: String(item.state ?? ''),
				project_id: String(item.project_id ?? '')
			}))
			.filter((item) => item.id);
		return { code: 0, message: res.message, data: issues };
	}
};

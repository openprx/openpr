import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export interface TimelineEvent {
	timestamp: string;
	event_type: string;
	description: string;
	actor?: string;
}

export interface ProposalChainProposal {
	id: string;
	title: string;
	proposal_type: string;
	status: string;
	author_id: string;
	author_type: string;
	created_at: string;
	submitted_at?: string;
	voting_started_at?: string;
	voting_ended_at?: string;
	archived_at?: string;
}

export interface ProposalChainIssue {
	issue_id: string;
	issue_title: string;
	issue_state: string;
	created_at: string;
}

export interface ProposalChainDecision {
	id: string;
	result: string;
	approval_rate?: number;
	total_votes: number;
	yes_votes: number;
	no_votes: number;
	abstain_votes: number;
	decided_at: string;
}

export interface ProposalChainVote {
	voter_id: string;
	voter_type: string;
	choice: string;
	weight: number;
	reason?: string;
	voted_at: string;
	trust_delta?: number;
}

export interface ProposalChainImpactReview {
	id: string;
	status: string;
	rating?: string;
	reviewer_id?: string;
	trust_score_applied: boolean;
	scheduled_at?: string;
	conducted_at?: string;
	created_at: string;
}

export interface FeedbackProposal {
	proposal_id: string;
	title: string;
	status: string;
	link_type: string;
	created_at: string;
}

export interface ProposalChain {
	proposal: ProposalChainProposal;
	issues: ProposalChainIssue[];
	link_status: string;
	decision?: ProposalChainDecision;
	votes: ProposalChainVote[];
	timeline: TimelineEvent[];
	impact_review?: ProposalChainImpactReview;
	feedback_proposals: FeedbackProposal[];
}

export interface ProposalTimeline {
	proposal_id: string;
	events: TimelineEvent[];
}

export interface DecisionOverview {
	total_decisions: number;
	approved_count: number;
	rejected_count: number;
	vetoed_count: number;
	pass_rate: number;
	avg_cycle_hours?: number;
}

export interface DecisionGroupStat {
	proposal_type?: string;
	domain?: string;
	total_decisions: number;
	approved_count: number;
	rejected_count: number;
	vetoed_count: number;
	pass_rate: number;
	avg_cycle_hours?: number;
}

export interface DecisionAnalytics {
	project_id?: string;
	time_range: {
		start_at?: string;
		end_at?: string;
	};
	overview: DecisionOverview;
	by_type: DecisionGroupStat[];
	by_domain: DecisionGroupStat[];
}

export interface AuditReport {
	id: string;
	project_id: string;
	period_start: string;
	period_end: string;
	total_proposals: number;
	approved_proposals: number;
	rejected_proposals: number;
	vetoed_proposals: number;
	reviewed_proposals: number;
	avg_review_rating?: number;
	rating_distribution: Record<string, number>;
	top_contributors?: unknown;
	domain_stats?: unknown;
	ai_participation_stats?: unknown;
	key_insights?: unknown;
	generated_at: string;
	generated_by: string;
}

export interface AiLearningRecord {
	id: number;
	ai_participant_id: string;
	review_id: string;
	proposal_id: string;
	domain: string;
	review_rating: string;
	ai_vote_choice?: string;
	ai_vote_reason?: string;
	outcome_alignment: string;
	lesson_learned?: string;
	will_change?: string;
	follow_up_improvement?: string;
	created_at: string;
}

export interface AiAlignmentStats {
	ai_participant_id: string;
	total: number;
	aligned: number;
	misaligned: number;
	neutral: number;
	overall_alignment_rate: number;
	recent_alignment_rate: number;
	improvement_trend: number;
}

function asRecord(value: unknown): Record<string, unknown> {
	return value && typeof value === 'object' ? (value as Record<string, unknown>) : {};
}

function mapTimelineEvent(raw: Record<string, unknown>): TimelineEvent {
	return {
		timestamp: String(raw.timestamp ?? ''),
		event_type: String(raw.event_type ?? ''),
		description: String(raw.description ?? ''),
		actor: raw.actor ? String(raw.actor) : undefined
	};
}

function mapAuditReport(raw: Record<string, unknown>): AuditReport {
	const rating = asRecord(raw.rating_distribution);
	return {
		id: String(raw.id ?? ''),
		project_id: String(raw.project_id ?? ''),
		period_start: String(raw.period_start ?? ''),
		period_end: String(raw.period_end ?? ''),
		total_proposals: Number(raw.total_proposals ?? 0),
		approved_proposals: Number(raw.approved_proposals ?? 0),
		rejected_proposals: Number(raw.rejected_proposals ?? 0),
		vetoed_proposals: Number(raw.vetoed_proposals ?? 0),
		reviewed_proposals: Number(raw.reviewed_proposals ?? 0),
		avg_review_rating: raw.avg_review_rating === null ? undefined : Number(raw.avg_review_rating ?? 0),
		rating_distribution: {
			S: Number(rating.S ?? 0),
			A: Number(rating.A ?? 0),
			B: Number(rating.B ?? 0),
			C: Number(rating.C ?? 0),
			F: Number(rating.F ?? 0)
		},
		top_contributors: raw.top_contributors,
		domain_stats: raw.domain_stats,
		ai_participation_stats: raw.ai_participation_stats,
		key_insights: raw.key_insights,
		generated_at: String(raw.generated_at ?? ''),
		generated_by: String(raw.generated_by ?? '')
	};
}

function mapLearning(raw: Record<string, unknown>): AiLearningRecord {
	return {
		id: Number(raw.id ?? 0),
		ai_participant_id: String(raw.ai_participant_id ?? ''),
		review_id: String(raw.review_id ?? ''),
		proposal_id: String(raw.proposal_id ?? ''),
		domain: String(raw.domain ?? ''),
		review_rating: String(raw.review_rating ?? ''),
		ai_vote_choice: raw.ai_vote_choice ? String(raw.ai_vote_choice) : undefined,
		ai_vote_reason: raw.ai_vote_reason ? String(raw.ai_vote_reason) : undefined,
		outcome_alignment: String(raw.outcome_alignment ?? ''),
		lesson_learned: raw.lesson_learned ? String(raw.lesson_learned) : undefined,
		will_change: raw.will_change ? String(raw.will_change) : undefined,
		follow_up_improvement: raw.follow_up_improvement ? String(raw.follow_up_improvement) : undefined,
		created_at: String(raw.created_at ?? '')
	};
}

export const governanceExtApi = {
	async getProposalChain(proposalId: string): Promise<ApiResult<ProposalChain>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/proposals/${proposalId}/chain`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.loadFailed'), data: null };
		}

		const proposal = asRecord(res.data.proposal);
		const decisionRaw = res.data.decision ? asRecord(res.data.decision) : null;
		const reviewRaw = res.data.impact_review ? asRecord(res.data.impact_review) : null;
		const feedbackRaw = Array.isArray(res.data.feedback_proposals) ? res.data.feedback_proposals : [];

		return {
			code: 0,
			message: res.message,
			data: {
				proposal: {
					id: String(proposal.id ?? ''),
					title: String(proposal.title ?? ''),
					proposal_type: String(proposal.proposal_type ?? ''),
					status: String(proposal.status ?? ''),
					author_id: String(proposal.author_id ?? ''),
					author_type: String(proposal.author_type ?? ''),
					created_at: String(proposal.created_at ?? ''),
					submitted_at: proposal.submitted_at ? String(proposal.submitted_at) : undefined,
					voting_started_at: proposal.voting_started_at ? String(proposal.voting_started_at) : undefined,
					voting_ended_at: proposal.voting_ended_at ? String(proposal.voting_ended_at) : undefined,
					archived_at: proposal.archived_at ? String(proposal.archived_at) : undefined
				},
				issues: Array.isArray(res.data.issues)
					? res.data.issues.map((item) => {
						const value = asRecord(item);
						return {
							issue_id: String(value.issue_id ?? ''),
							issue_title: String(value.issue_title ?? ''),
							issue_state: String(value.issue_state ?? ''),
							created_at: String(value.created_at ?? '')
						};
					})
					: [],
				link_status: String(res.data.link_status ?? ''),
				decision: decisionRaw
					? {
						id: String(decisionRaw.id ?? ''),
						result: String(decisionRaw.result ?? ''),
						approval_rate: decisionRaw.approval_rate === null ? undefined : Number(decisionRaw.approval_rate ?? 0),
						total_votes: Number(decisionRaw.total_votes ?? 0),
						yes_votes: Number(decisionRaw.yes_votes ?? 0),
						no_votes: Number(decisionRaw.no_votes ?? 0),
						abstain_votes: Number(decisionRaw.abstain_votes ?? 0),
						decided_at: String(decisionRaw.decided_at ?? '')
					}
					: undefined,
				votes: Array.isArray(res.data.votes)
					? res.data.votes.map((item) => {
						const value = asRecord(item);
						return {
							voter_id: String(value.voter_id ?? ''),
							voter_type: String(value.voter_type ?? ''),
							choice: String(value.choice ?? ''),
							weight: Number(value.weight ?? 0),
							reason: value.reason ? String(value.reason) : undefined,
							voted_at: String(value.voted_at ?? ''),
							trust_delta: value.trust_delta === null ? undefined : Number(value.trust_delta ?? 0)
						};
					})
					: [],
				timeline: Array.isArray(res.data.timeline)
					? res.data.timeline.map((item) => mapTimelineEvent(asRecord(item)))
					: [],
				impact_review: reviewRaw
					? {
						id: String(reviewRaw.id ?? ''),
						status: String(reviewRaw.status ?? ''),
						rating: reviewRaw.rating ? String(reviewRaw.rating) : undefined,
						reviewer_id: reviewRaw.reviewer_id ? String(reviewRaw.reviewer_id) : undefined,
						trust_score_applied: Boolean(reviewRaw.trust_score_applied ?? false),
						scheduled_at: reviewRaw.scheduled_at ? String(reviewRaw.scheduled_at) : undefined,
						conducted_at: reviewRaw.conducted_at ? String(reviewRaw.conducted_at) : undefined,
						created_at: String(reviewRaw.created_at ?? '')
					}
					: undefined,
				feedback_proposals: feedbackRaw.map((item) => {
					const value = asRecord(item);
					return {
						proposal_id: String(value.proposal_id ?? ''),
						title: String(value.title ?? ''),
						status: String(value.status ?? ''),
						link_type: String(value.link_type ?? ''),
						created_at: String(value.created_at ?? '')
					};
				})
			}
		};
	},

	async getProposalTimeline(proposalId: string): Promise<ApiResult<ProposalTimeline>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/proposals/${proposalId}/timeline`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.loadFailed'), data: null };
		}
		return {
			code: 0,
			message: res.message,
			data: {
				proposal_id: String(res.data.proposal_id ?? ''),
				events: Array.isArray(res.data.events)
					? res.data.events.map((item) => mapTimelineEvent(asRecord(item)))
					: []
			}
		};
	},

	async getDecisionAnalytics(params: {
		project_id?: string;
		start_at?: string;
		end_at?: string;
	}): Promise<ApiResult<DecisionAnalytics>> {
		const query = new URLSearchParams();
		if (params.project_id) query.set('project_id', params.project_id);
		if (params.start_at) query.set('start_at', params.start_at);
		if (params.end_at) query.set('end_at', params.end_at);
		const endpoint = query.size
			? `/api/v1/decisions/analytics?${query.toString()}`
			: '/api/v1/decisions/analytics';

		const res = await apiClient.get<Record<string, unknown>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.analyticsLoadFailed'), data: null };
		}

		const overview = asRecord(res.data.overview);
		const timeRange = asRecord(res.data.time_range);
		return {
			code: 0,
			message: res.message,
			data: {
				project_id: res.data.project_id ? String(res.data.project_id) : undefined,
				time_range: {
					start_at: timeRange.start_at ? String(timeRange.start_at) : undefined,
					end_at: timeRange.end_at ? String(timeRange.end_at) : undefined
				},
				overview: {
					total_decisions: Number(overview.total_decisions ?? 0),
					approved_count: Number(overview.approved_count ?? 0),
					rejected_count: Number(overview.rejected_count ?? 0),
					vetoed_count: Number(overview.vetoed_count ?? 0),
					pass_rate: Number(overview.pass_rate ?? 0),
					avg_cycle_hours: overview.avg_cycle_hours === null ? undefined : Number(overview.avg_cycle_hours ?? 0)
				},
				by_type: Array.isArray(res.data.by_type)
					? res.data.by_type.map((item) => {
						const row = asRecord(item);
						return {
							proposal_type: String(row.proposal_type ?? ''),
							total_decisions: Number(row.total_decisions ?? 0),
							approved_count: Number(row.approved_count ?? 0),
							rejected_count: Number(row.rejected_count ?? 0),
							vetoed_count: Number(row.vetoed_count ?? 0),
							pass_rate: Number(row.pass_rate ?? 0),
							avg_cycle_hours: row.avg_cycle_hours === null ? undefined : Number(row.avg_cycle_hours ?? 0)
						};
					})
					: [],
				by_domain: Array.isArray(res.data.by_domain)
					? res.data.by_domain.map((item) => {
						const row = asRecord(item);
						return {
							domain: String(row.domain ?? ''),
							total_decisions: Number(row.total_decisions ?? 0),
							approved_count: Number(row.approved_count ?? 0),
							rejected_count: Number(row.rejected_count ?? 0),
							vetoed_count: Number(row.vetoed_count ?? 0),
							pass_rate: Number(row.pass_rate ?? 0),
							avg_cycle_hours: row.avg_cycle_hours === null ? undefined : Number(row.avg_cycle_hours ?? 0)
						};
					})
					: []
			}
		};
	},

	async listProjectAuditReports(
		projectId: string,
		params?: { page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<AuditReport>>> {
		const query = new URLSearchParams();
		if (params?.page) query.set('page', String(params.page));
		if (params?.per_page) query.set('per_page', String(params.per_page));
		const endpoint = query.size
			? `/api/v1/projects/${projectId}/audit-reports?${query.toString()}`
			: `/api/v1/projects/${projectId}/audit-reports`;

		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.auditLoadFailed'), data: null };
		}

		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapAuditReport) : []
			}
		};
	},

	async createProjectAuditReport(
		projectId: string,
		payload?: { period_start?: string; period_end?: string }
	): Promise<ApiResult<AuditReport>> {
		const res = await apiClient.post<Record<string, unknown>>(`/api/v1/projects/${projectId}/audit-reports`, payload);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.auditGenerateFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapAuditReport(res.data) };
	},

	async getProjectAuditReport(projectId: string, reportId: string): Promise<ApiResult<AuditReport>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/projects/${projectId}/audit-reports/${reportId}`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.auditLoadFailed'), data: null };
		}
		return { code: 0, message: res.message, data: mapAuditReport(res.data) };
	},

	async getAiReviewFeedback(reviewId: string): Promise<ApiResult<{ items: AiLearningRecord[] }>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/ai-learning/${reviewId}/feedback`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.learningLoadFailed'), data: null };
		}
		return {
			code: 0,
			message: res.message,
			data: {
				items: Array.isArray(res.data.items) ? res.data.items.map((item) => mapLearning(asRecord(item))) : []
			}
		};
	},

	async getAiParticipantLearning(
		aiParticipantId: string,
		params?: { domain?: string; page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<AiLearningRecord>>> {
		const query = new URLSearchParams();
		if (params?.domain) query.set('domain', params.domain);
		if (params?.page) query.set('page', String(params.page));
		if (params?.per_page) query.set('per_page', String(params.per_page));
		const endpoint = query.size
			? `/api/v1/ai-participants/${aiParticipantId}/learning?${query.toString()}`
			: `/api/v1/ai-participants/${aiParticipantId}/learning`;

		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.learningLoadFailed'), data: null };
		}

		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapLearning) : []
			}
		};
	},

	async getAiParticipantAlignmentStats(aiParticipantId: string): Promise<ApiResult<AiAlignmentStats>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/ai-participants/${aiParticipantId}/alignment-stats`);
		if (res.code !== 0 || !res.data) {
			return { code: res.code, message: res.message || get(t)('governanceExt.learningLoadFailed'), data: null };
		}

		return {
			code: 0,
			message: res.message,
			data: {
				ai_participant_id: String(res.data.ai_participant_id ?? aiParticipantId),
				total: Number(res.data.total ?? 0),
				aligned: Number(res.data.aligned ?? 0),
				misaligned: Number(res.data.misaligned ?? 0),
				neutral: Number(res.data.neutral ?? 0),
				overall_alignment_rate: Number(res.data.overall_alignment_rate ?? 0),
				recent_alignment_rate: Number(res.data.recent_alignment_rate ?? 0),
				improvement_trend: Number(res.data.improvement_trend ?? 0)
			}
		};
	}
};

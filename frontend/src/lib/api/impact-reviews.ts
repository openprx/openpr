import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export type ReviewStatus = 'pending' | 'collecting' | 'completed' | 'skipped';
export type ReviewRating = 'S' | 'A' | 'B' | 'C' | 'F';

export interface ReviewParticipant {
	id: number;
	review_id: string;
	user_id: string;
	role: string;
	vote_choice?: string;
	exercised_veto: boolean;
	veto_overturned: boolean;
	feedback_submitted: boolean;
	feedback_content?: string;
	trust_score_change?: number;
}

export interface ReviewSummary {
	review_id: string;
	participants_count: number;
	feedback_submitted_count: number;
	trust_delta_total: number;
	trust_delta_avg: number;
}

export interface ImpactReview {
	id: string;
	proposal_id: string;
	project_id: string;
	status: ReviewStatus;
	rating?: ReviewRating;
	metrics?: Record<string, unknown>;
	goal_achievements?: unknown;
	achievements?: string;
	lessons?: string;
	reviewer_id?: string;
	is_auto_triggered: boolean;
	data_sources?: Record<string, unknown>;
	trust_score_applied: boolean;
	scheduled_at?: string;
	conducted_at?: string;
	created_at: string;
}

export interface ImpactReviewDetail {
	review: ImpactReview;
	participants: ReviewParticipant[];
	summary: ReviewSummary;
}

export interface ListImpactReviewsParams {
	project_id?: string;
	status?: ReviewStatus;
	rating?: ReviewRating;
	page?: number;
	per_page?: number;
}

function mapImpactReview(raw: Record<string, unknown>): ImpactReview {
	return {
		id: String(raw.id ?? ''),
		proposal_id: String(raw.proposal_id ?? ''),
		project_id: String(raw.project_id ?? ''),
		status: (raw.status ?? 'pending') as ReviewStatus,
		rating: raw.rating ? (String(raw.rating) as ReviewRating) : undefined,
		metrics: raw.metrics && typeof raw.metrics === 'object' ? (raw.metrics as Record<string, unknown>) : undefined,
		goal_achievements: raw.goal_achievements,
		achievements: raw.achievements ? String(raw.achievements) : undefined,
		lessons: raw.lessons ? String(raw.lessons) : undefined,
		reviewer_id: raw.reviewer_id ? String(raw.reviewer_id) : undefined,
		is_auto_triggered: Boolean(raw.is_auto_triggered ?? false),
		data_sources:
			raw.data_sources && typeof raw.data_sources === 'object'
				? (raw.data_sources as Record<string, unknown>)
				: undefined,
		trust_score_applied: Boolean(raw.trust_score_applied ?? false),
		scheduled_at: raw.scheduled_at ? String(raw.scheduled_at) : undefined,
		conducted_at: raw.conducted_at ? String(raw.conducted_at) : undefined,
		created_at: String(raw.created_at ?? '')
	};
}

function mapParticipant(raw: Record<string, unknown>): ReviewParticipant {
	return {
		id: Number(raw.id ?? 0),
		review_id: String(raw.review_id ?? ''),
		user_id: String(raw.user_id ?? ''),
		role: String(raw.role ?? ''),
		vote_choice: raw.vote_choice ? String(raw.vote_choice) : undefined,
		exercised_veto: Boolean(raw.exercised_veto ?? false),
		veto_overturned: Boolean(raw.veto_overturned ?? false),
		feedback_submitted: Boolean(raw.feedback_submitted ?? false),
		feedback_content: raw.feedback_content ? String(raw.feedback_content) : undefined,
		trust_score_change:
			raw.trust_score_change === null || raw.trust_score_change === undefined
				? undefined
				: Number(raw.trust_score_change)
	};
}

function mapSummary(raw: Record<string, unknown>): ReviewSummary {
	return {
		review_id: String(raw.review_id ?? ''),
		participants_count: Number(raw.participants_count ?? 0),
		feedback_submitted_count: Number(raw.feedback_submitted_count ?? 0),
		trust_delta_total: Number(raw.trust_delta_total ?? 0),
		trust_delta_avg: Number(raw.trust_delta_avg ?? 0)
	};
}

function mapDetail(raw: Record<string, unknown>): ImpactReviewDetail {
	return {
		review: mapImpactReview((raw.review ?? {}) as Record<string, unknown>),
		participants: Array.isArray(raw.participants)
			? raw.participants.map((item) => mapParticipant((item ?? {}) as Record<string, unknown>))
			: [],
		summary: mapSummary((raw.summary ?? {}) as Record<string, unknown>)
	};
}

export const impactReviewsApi = {
	async getByProposal(proposalId: string): Promise<ApiResult<ImpactReviewDetail>> {
		const res = await apiClient.get<Record<string, unknown>>(`/api/v1/proposals/${proposalId}/impact-review`);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('impactReview.loadFailed'),
				data: null
			};
		}
		return { code: 0, message: res.message, data: mapDetail(res.data) };
	},

	createForProposal(
		proposalId: string,
		payload?: { reviewer_id?: string; scheduled_at?: string }
	): Promise<ApiResult<Record<string, unknown>>> {
		return apiClient.post(`/api/v1/proposals/${proposalId}/impact-review`, payload);
	},

	async updateByProposal(
		proposalId: string,
		payload: {
			status?: ReviewStatus;
			rating?: ReviewRating;
			metrics?: Record<string, unknown>;
			goal_achievements?: unknown;
			achievements?: string;
			lessons?: string;
			data_sources?: Record<string, unknown>;
		}
	): Promise<ApiResult<ImpactReviewDetail>> {
		const res = await apiClient.patch<Record<string, unknown>>(
			`/api/v1/proposals/${proposalId}/impact-review`,
			payload
		);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('impactReview.updateFailed'),
				data: null
			};
		}
		return { code: 0, message: res.message, data: mapDetail(res.data) };
	},

	deleteByProposal(proposalId: string): Promise<ApiResult<null>> {
		return apiClient.delete(`/api/v1/proposals/${proposalId}/impact-review`);
	},

	async list(params: ListImpactReviewsParams): Promise<ApiResult<PaginatedData<ImpactReview>>> {
		const query = new URLSearchParams();
		if (params.project_id) query.set('project_id', params.project_id);
		if (params.status) query.set('status', params.status);
		if (params.rating) query.set('rating', params.rating);
		if (params.page) query.set('page', String(params.page));
		if (params.per_page) query.set('per_page', String(params.per_page));
		const endpoint = query.size ? `/api/v1/impact-reviews?${query.toString()}` : '/api/v1/impact-reviews';
		const res = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (res.code !== 0 || !res.data) {
			return {
				code: res.code,
				message: res.message || get(t)('impactReview.loadListFailed'),
				data: null
			};
		}
		return {
			code: 0,
			message: res.message,
			data: {
				...res.data,
				items: Array.isArray(res.data.items) ? res.data.items.map(mapImpactReview) : []
			}
		};
	}
};

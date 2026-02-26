import { apiClient, type ApiResult, type PaginatedData } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export type IssueStatus = 'backlog' | 'todo' | 'in_progress' | 'done';
export type IssuePriority = 'low' | 'medium' | 'high' | 'urgent';

export interface Issue {
	id: string;
	project_id: string;
	title: string;
	description?: string;
	status: IssueStatus;
	priority: IssuePriority;
	assignee_id?: string;
	sprint_id?: string;
	reporter_id: string;
	proposal_id?: string;
	governance_exempt?: boolean;
	governance_exempt_reason?: string;
	labels?: Array<{
		id: string;
		name: string;
		color: string;
	}>;
	created_at: string;
	updated_at: string;
	key?: string;
	type?: string;
}

export interface CreateIssueInput {
	title: string;
	description?: string;
	status?: IssueStatus;
	priority?: IssuePriority;
	assignee_id?: string;
	sprint_id?: string;
}

export interface UpdateIssueInput {
	title?: string;
	description?: string;
	status?: IssueStatus;
	priority?: IssuePriority;
	assignee_id?: string;
	sprint_id?: string;
}

export interface Comment {
	id: string;
	issue_id: string;
	user_id: string;
	content: string;
	created_at: string;
	updated_at: string;
	work_item_id?: string;
}

export interface Activity {
	id: string;
	issue_id: string;
	user_id: string;
	action: string;
	details?: unknown;
	field?: string;
	old_value?: string;
	new_value?: string;
	created_at: string;
	work_item_id?: string;
}

export interface ListIssuesParams {
	page?: number;
	per_page?: number;
	status?: IssueStatus;
	state?: IssueStatus;
	priority?: IssuePriority;
	assignee_id?: string;
	label_ids?: string;
	search?: string;
	sort_by?: 'updated_at' | 'created_at' | 'priority' | 'title';
	sort_order?: 'asc' | 'desc';
}

function mapIssue(raw: Record<string, unknown>): Issue {
	const labels = Array.isArray(raw.labels)
		? (raw.labels as Array<Record<string, unknown>>).map((label) => ({
				id: String(label.id ?? ''),
				name: String(label.name ?? ''),
				color: String(label.color ?? '')
			}))
		: undefined;

	const sprintId = raw.sprint_id;

	return {
		...(raw as unknown as Issue),
		status: ((raw.status ?? raw.state ?? 'todo') as IssueStatus),
		sprint_id: sprintId === null || sprintId === undefined || sprintId === '' ? undefined : String(sprintId),
		proposal_id:
			raw.proposal_id === null || raw.proposal_id === undefined || raw.proposal_id === ''
				? undefined
				: String(raw.proposal_id),
		governance_exempt: Boolean(raw.governance_exempt ?? false),
		governance_exempt_reason:
			raw.governance_exempt_reason === null ||
			raw.governance_exempt_reason === undefined ||
			raw.governance_exempt_reason === ''
				? undefined
				: String(raw.governance_exempt_reason),
		labels
	};
}

function mapComment(raw: Record<string, unknown>): Comment {
	return {
		...(raw as unknown as Comment),
		issue_id: (raw.issue_id ?? raw.work_item_id ?? '') as string,
		user_id: (raw.user_id ?? raw.author_id ?? '') as string,
		content: (raw.content ?? raw.body ?? '') as string
	};
}

export const issuesApi = {
	async list(projectId: string, params?: ListIssuesParams): Promise<ApiResult<PaginatedData<Issue>>> {
		const query = new URLSearchParams();
		if (params?.page) query.set('page', String(params.page));
		if (params?.per_page) query.set('per_page', String(params.per_page));
		if (params?.status) query.set('state', params.status);
		if (params?.state) query.set('state', params.state);
		if (params?.priority) query.set('priority', params.priority);
		if (params?.assignee_id) query.set('assignee_id', params.assignee_id);
		if (params?.label_ids) query.set('label_ids', params.label_ids);
		if (params?.search) query.set('search', params.search);
		if (params?.sort_by) query.set('sort_by', params.sort_by);
		if (params?.sort_order) query.set('sort_order', params.sort_order);

		const queryString = query.toString();
		const endpoint = queryString
			? `/api/v1/projects/${projectId}/issues?${queryString}`
			: `/api/v1/projects/${projectId}/issues`;
		const result = await apiClient.get<PaginatedData<Record<string, unknown>>>(endpoint);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.loadIssueFailed'), data: null };
		}
		return {
			code: 0,
			message: result.message,
			data: {
				...result.data,
				items: Array.isArray(result.data.items) ? result.data.items.map(mapIssue) : []
			}
		};
	},

	async get(id: string): Promise<ApiResult<Issue>> {
		const result = await apiClient.get<Record<string, unknown>>(`/api/v1/issues/${id}`);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.loadIssueFailed'), data: null };
		}
		return { code: 0, message: result.message, data: mapIssue(result.data) };
	},

	async create(projectId: string, data: CreateIssueInput): Promise<ApiResult<Issue>> {
		const result = await apiClient.post<Record<string, unknown>>(`/api/v1/projects/${projectId}/issues`, {
			title: data.title,
			description: data.description,
			state: data.status,
			priority: data.priority,
			assignee_id: data.assignee_id,
			sprint_id: data.sprint_id
		});
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.createIssueFailed'), data: null };
		}
		return { code: 0, message: result.message, data: mapIssue(result.data) };
	},

	async update(id: string, data: UpdateIssueInput): Promise<ApiResult<Issue>> {
		const result = await apiClient.put<Record<string, unknown>>(`/api/v1/issues/${id}`, {
			title: data.title,
			description: data.description,
			state: data.status,
			priority: data.priority,
			assignee_id: data.assignee_id,
			sprint_id: data.sprint_id
		});
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.updateIssueFailed'), data: null };
		}
		return { code: 0, message: result.message, data: mapIssue(result.data) };
	},

	delete(id: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/issues/${id}`);
	},

	async getComments(issueId: string): Promise<ApiResult<PaginatedData<Comment>>> {
		const result = await apiClient.get<PaginatedData<Record<string, unknown>>>(`/api/v1/issues/${issueId}/comments`);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.loadCommentFailed'), data: null };
		}
		return {
			code: 0,
			message: result.message,
			data: {
				...result.data,
				items: Array.isArray(result.data.items) ? result.data.items.map(mapComment) : []
			}
		};
	},

	async createComment(issueId: string, content: string): Promise<ApiResult<Comment>> {
		const result = await apiClient.post<Record<string, unknown>>(`/api/v1/issues/${issueId}/comments`, {
			body: content
		});
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message || get(t)('api.createCommentFailed'), data: null };
		}
		return { code: 0, message: result.message, data: mapComment(result.data) };
	},

	getActivities(issueId: string): Promise<ApiResult<PaginatedData<Activity>>> {
		return apiClient.get<PaginatedData<Activity>>(`/api/v1/issues/${issueId}/activities`);
	}
};

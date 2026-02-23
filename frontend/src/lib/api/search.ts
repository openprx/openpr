import { apiClient, type ApiResult } from './client';

export interface IssueSearchResult {
	type: 'issue';
	id: string;
	title: string;
	description?: string;
	state: string;
	project_id: string;
	workspace_id: string;
}

export interface ProjectSearchResult {
	type: 'project';
	id: string;
	key: string;
	name: string;
	description?: string;
	workspace_id: string;
}

export interface CommentSearchResult {
	type: 'comment';
	id: string;
	body: string;
	issue_id: string;
	project_id: string;
	workspace_id: string;
	author_id?: string;
	created_at: string;
}

export type SearchResult = IssueSearchResult | ProjectSearchResult | CommentSearchResult;

export interface SearchResponse {
	query: string;
	results: SearchResult[];
	total: number;
}

export const searchApi = {
	search(query: string): Promise<ApiResult<SearchResponse>> {
		const encoded = encodeURIComponent(query);
		return apiClient.get<SearchResponse>(`/api/v1/search?q=${encoded}`);
	}
};

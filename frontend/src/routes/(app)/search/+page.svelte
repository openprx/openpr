<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import {
		searchApi,
		type CommentSearchResult,
		type IssueSearchResult,
		type ProjectSearchResult,
		type SearchResult
	} from '$lib/api/search';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';
	import { previewText } from '$lib/utils/search';

	let query = $state('');
	let searching = $state(false);
	let results = $state<SearchResult[]>([]);

	onMount(async () => {
		const initialQuery = $page.url.searchParams.get('q');
		if (initialQuery) {
			query = initialQuery;
			await runSearch();
		}
	});

	const issues = $derived(
		results.filter((result): result is IssueSearchResult => result.type === 'issue')
	);
	const projects = $derived(
		results.filter((result): result is ProjectSearchResult => result.type === 'project')
	);
	const comments = $derived(
		results.filter((result): result is CommentSearchResult => result.type === 'comment')
	);

	async function runSearch() {
		if (!query.trim()) {
			results = [];
			goto('/search', { replaceState: true });
			return;
		}

		searching = true;
		goto(`/search?q=${encodeURIComponent(query.trim())}`, { replaceState: true });
		const response = await searchApi.search(query.trim());
		if (response.code !== 0) {
			toast.error(response.message);
			results = [];
		} else if (response.data) {
			results = response.data.results ?? [];
		}
		searching = false;
	}

	function normalizeId(value: unknown): string {
		if (typeof value !== 'string') {
			return '';
		}
		const next = value.trim();
		if (!next || next === 'undefined' || next === 'null') {
			return '';
		}
		return next;
	}

	function resolveHref(result: SearchResult): string | null {
		const anyResult = result as unknown as Record<string, unknown>;
		const workspaceId = normalizeId(
			anyResult.workspace_id ?? anyResult.workspaceId ?? $page.params.workspaceId
		);
		if (!workspaceId) {
			return null;
		}

		if (result.type === 'project') {
			const projectId = normalizeId(result.id);
			return projectId ? `/workspace/${workspaceId}/projects/${projectId}` : null;
		}
		if (result.type === 'issue') {
			const projectId = normalizeId(anyResult.project_id ?? anyResult.projectId);
			const issueId = normalizeId(result.id);
			return projectId && issueId
				? `/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}`
				: null;
		}
		if (result.type === 'comment') {
			const projectId = normalizeId(anyResult.project_id ?? anyResult.projectId);
			const issueId = normalizeId(anyResult.issue_id ?? anyResult.issueId);
			return projectId && issueId
				? `/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}`
				: null;
		}
		return null;
	}

	function handleResultClick(event: MouseEvent, result: SearchResult) {
		if (resolveHref(result)) {
			return;
		}
		event.preventDefault();
		toast.error(get(t)('search.missingWorkspace'));
	}
</script>

<div class="mx-auto max-w-6xl space-y-6">
	<div>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('search.title')}</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('search.helper')}</p>
	</div>

	<form
		onsubmit={(event) => {
			event.preventDefault();
			runSearch();
		}}
		class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4"
	>
		<div class="flex gap-3">
			<input
				type="search"
				bind:value={query}
				placeholder={$t('search.placeholder')}
				class="flex-1 rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			/>
			<Button type="submit" loading={searching}>{$t('common.search')}</Button>
		</div>
	</form>

	{#if searching}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-slate-500 dark:text-slate-400">{$t('search.searching')}</div>
	{:else if query.trim() && results.length === 0}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-10 text-center text-slate-500 dark:text-slate-400">{$t('search.noResults')}</div>
	{:else if results.length > 0}
		<div class="space-y-4">
			{#if issues.length > 0}
				<section class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4">
					<h2 class="text-sm font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">Issues ({issues.length})</h2>
					<div class="mt-3 space-y-2">
						{#each issues as item (item.id)}
							<a
								href={resolveHref(item) ?? '#'}
								onclick={(event) => handleResultClick(event, item)}
								class="block rounded-md border border-slate-200 dark:border-slate-700 p-3 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
							>
								<p class="font-medium text-slate-900 dark:text-slate-100">{item.title}</p>
								{#if item.description}
									<p class="mt-1 text-sm text-slate-600 dark:text-slate-300 line-clamp-2">{previewText(item.description, 100)}</p>
								{/if}
							</a>
						{/each}
					</div>
				</section>
			{/if}

			{#if projects.length > 0}
				<section class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4">
					<h2 class="text-sm font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">Projects ({projects.length})</h2>
					<div class="mt-3 space-y-2">
						{#each projects as item (item.id)}
							<a
								href={resolveHref(item) ?? '#'}
								onclick={(event) => handleResultClick(event, item)}
								class="block rounded-md border border-slate-200 dark:border-slate-700 p-3 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
							>
								<p class="font-medium text-slate-900 dark:text-slate-100">{item.name}</p>
								{#if item.description}
									<p class="mt-1 text-sm text-slate-600 dark:text-slate-300 line-clamp-2">{previewText(item.description, 100)}</p>
								{/if}
							</a>
						{/each}
					</div>
				</section>
			{/if}

			{#if comments.length > 0}
				<section class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4">
					<h2 class="text-sm font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">Comments ({comments.length})</h2>
					<div class="mt-3 space-y-2">
						{#each comments as item (item.id)}
							<a
								href={resolveHref(item) ?? '#'}
								onclick={(event) => handleResultClick(event, item)}
								class="block rounded-md border border-slate-200 dark:border-slate-700 p-3 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
							>
								<p class="font-medium text-slate-900 dark:text-slate-100">{$t('issue.comments')}</p>
								<p class="mt-1 text-sm text-slate-600 dark:text-slate-300 line-clamp-2">{previewText(item.body, 100)}</p>
							</a>
						{/each}
					</div>
				</section>
			{/if}
		</div>
	{/if}
</div>

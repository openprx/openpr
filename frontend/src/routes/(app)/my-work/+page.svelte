<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { myApi, type MyIssue } from '$lib/api/my';
	import { toast } from '$lib/stores/toast';
	import { goto } from '$app/navigation';
	import Card from '$lib/components/Card.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { timeAgo } from '$lib/utils/timeago';

	let loading = $state(true);
	let myIssues = $state<MyIssue[]>([]);

	const groupedIssues = $derived.by(() => {
		const groups: Record<string, MyIssue[]> = {
			backlog: [],
			todo: [],
			in_progress: [],
			done: []
		};
		myIssues.forEach((issue) => {
			if (groups[issue.status]) {
				groups[issue.status].push(issue);
			}
		});
		return groups;
	});

	onMount(async () => {
		await loadMyIssues();
	});

	async function loadMyIssues() {
		loading = true;
		const response = await myApi.getMyIssues();

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			myIssues = response.data.items ?? [];
		}

		loading = false;
	}

	function goToIssue(workspaceId: string, projectId: string, issueId: string) {
		window.open(`/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}`, '_blank');
	}

	function formatRelativeTime(dateText: string): string {
		return timeAgo(dateText);
	}

	function formatStatus(status: string): string {
		const map: Record<string, string> = {
			backlog: get(t)('issue.backlog'),
			todo: get(t)('issue.todo'),
			in_progress: get(t)('issue.inProgress'),
			done: get(t)('issue.done')
		};
		return map[status] || status;
	}

	function formatPriority(priority: string): string {
		const map: Record<string, string> = {
			low: get(t)('issue.low'),
			medium: get(t)('issue.medium'),
			high: get(t)('issue.high'),
			urgent: get(t)('issue.urgent')
		};
		return map[priority] || priority;
	}

	function getStatusClass(status: string): string {
		const map: Record<string, string> = {
			backlog: 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300',
			todo: 'bg-blue-500/20 dark:bg-blue-500/30 text-blue-700 dark:text-blue-300',
			in_progress: 'bg-amber-500/20 dark:bg-amber-500/30 text-amber-700 dark:text-amber-300',
			done: 'bg-emerald-500/20 dark:bg-emerald-500/30 text-emerald-700 dark:text-emerald-300'
		};
		return map[status] || 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300';
	}

	function getPriorityClass(priority: string): string {
		const map: Record<string, string> = {
			low: 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300',
			medium: 'bg-blue-500/20 dark:bg-blue-500/30 text-blue-700 dark:text-blue-300',
			high: 'bg-orange-500/20 dark:bg-orange-500/30 text-orange-700 dark:text-orange-300',
			urgent: 'bg-red-500/20 dark:bg-red-500/30 text-red-700 dark:text-red-300'
		};
		return map[priority] || 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300';
	}

	function getStatusIcon(status: string): string {
		const map: Record<string, string> = {
			backlog: 'M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z',
			todo: 'M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2',
			in_progress: 'M13 10V3L4 14h7v7l9-11h-7z',
			done: 'M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z'
		};
		return map[status] || 'M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z';
	}
</script>

<div class="mx-auto max-w-7xl">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="mb-2 text-3xl font-bold text-slate-900 dark:text-slate-100">{$t('workspace.myWork')}</h1>
		<p class="text-slate-600 dark:text-slate-400">{$t('workspace.myWorkDesc')}</p>
	</div>

	<!-- Loading State -->
	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		</div>

	<!-- Empty State -->
	{:else if myIssues.length === 0}
		<EmptyState icon="clipboard" title={$t('workspace.noWork')} description={$t('workspace.noWorkDesc')}>
		</EmptyState>

	<!-- Issues by Status -->
	{:else}
		<div class="grid grid-cols-1 gap-6 lg:grid-cols-2 xl:grid-cols-4">
			{#each ['backlog', 'todo', 'in_progress', 'done'] as status}
				{@const issues = groupedIssues[status]}
				<Card>
					<div class="mb-4 flex items-center justify-between">
						<h3 class="flex items-center gap-2 text-lg font-semibold text-slate-900 dark:text-slate-100">
							<svg class="h-5 w-5 text-slate-500 dark:text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d={getStatusIcon(status)}></path>
							</svg>
							{formatStatus(status)}
						</h3>
						<span class="rounded-full bg-slate-100 px-2 py-1 text-xs font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-400">
							{issues.length}
						</span>
					</div>

					{#if issues.length === 0}
						<div class="py-8 text-center text-sm text-slate-500 dark:text-slate-400">{$t('common.noItems')}</div>
					{:else}
						<div class="space-y-3">
							{#each issues as issue (issue.id)}
								<button
									class="w-full rounded-md border border-slate-200 bg-white p-3 text-left transition-all hover:border-blue-300 hover:shadow-sm dark:border-slate-700 dark:bg-slate-900 dark:hover:border-blue-500"
									onclick={() => goToIssue(issue.workspace_id, issue.project_id, issue.id)}
								>
									<div class="mb-2 flex items-start justify-between gap-2">
										<p class="line-clamp-2 text-sm font-medium text-slate-900 dark:text-slate-100">{issue.title}</p>
									</div>
									<div class="mb-2 text-xs text-slate-500 dark:text-slate-400">
										{issue.project_name}
									</div>
									<div class="flex flex-wrap items-center gap-2">
										<span class={`rounded-full px-2 py-1 text-xs ${getPriorityClass(issue.priority)}`}>
											{formatPriority(issue.priority)}
										</span>
										<span class="text-xs text-slate-500 dark:text-slate-400">
											{formatRelativeTime(issue.updated_at)}
										</span>
									</div>
								</button>
							{/each}
						</div>
					{/if}
				</Card>
			{/each}
		</div>

		<!-- Summary Stats -->
		<div class="mt-8 grid grid-cols-2 gap-6 md:grid-cols-4">
			<Card>
				<div class="text-center">
					<div class="text-3xl font-bold text-slate-900 dark:text-slate-100">{myIssues.length}</div>
					<div class="mt-1 text-sm text-slate-600 dark:text-slate-400">{$t('common.total')}</div>
				</div>
			</Card>
			{#each ['backlog', 'todo', 'in_progress', 'done'] as status}
				<Card>
					<div class="text-center">
						<div class="text-3xl font-bold text-slate-900 dark:text-slate-100">{groupedIssues[status].length}</div>
						<div class="mt-1 text-sm text-slate-600 dark:text-slate-400">{formatStatus(status)}</div>
					</div>
				</Card>
			{/each}
		</div>
	{/if}
</div>

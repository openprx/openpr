<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { membersApi } from '$lib/api/members';
	import { issuesApi, type Issue, type IssuePriority, type IssueStatus } from '$lib/api/issues';
	import { sprintsApi, type Sprint } from '$lib/api/sprints';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import Badge from '$lib/components/Badge.svelte';
	import Avatar from '$lib/components/Avatar.svelte';
	import BotIcon from '$lib/components/BotIcon.svelte';
	import IssueCreateModal from '$lib/components/IssueCreateModal.svelte';

	const workspaceId = $derived(requireRouteParam($page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam($page.params.projectId, 'projectId'));

	let issues = $state<Issue[]>([]);
	let sprints = $state<Sprint[]>([]);
	let loading = $state(true);
	let keyword = $state('');
	let statusFilter = $state<'all' | IssueStatus>('all');
	let priorityFilter = $state<'all' | IssuePriority>('all');
	let assigneeFilter = $state('all');
	let memberNameMap = $state<Record<string, string>>({});
	let memberEntityTypeMap = $state<Record<string, string>>({});
	let sortBy = $state<'updated_at' | 'created_at' | 'priority' | 'title'>('updated_at');
	let sortOrder = $state<'asc' | 'desc'>('desc');
	let showCreateModal = $state(false);
	let currentPage = $state(1);
	let totalPages = $state(1);
	let total = $state(0);
	const perPage = 15;

	onMount(async () => {
		await Promise.all([loadSprints(), loadMembers()]);
		await loadIssues(1);
	});

	async function loadIssues(pageToLoad: number = currentPage) {
		loading = true;
		const response = await issuesApi.list(projectId, {
			page: pageToLoad,
			per_page: perPage,
			status: statusFilter === 'all' ? undefined : statusFilter,
			priority: priorityFilter === 'all' ? undefined : priorityFilter,
			assignee_id: assigneeFilter === 'all' ? undefined : assigneeFilter,
			search: keyword.trim() || undefined,
			sort_by: sortBy,
			sort_order: sortOrder
		});

		if (response.code !== 0) {
			toast.error(response.message);
			issues = [];
			total = 0;
			totalPages = 1;
		} else if (response.data) {
			issues = response.data.items ?? [];
			total = response.data.total ?? 0;
			const computedTotalPages = response.data.total_pages ?? Math.ceil(total / perPage);
			totalPages = Math.max(1, computedTotalPages);
			currentPage = Math.min(Math.max(1, pageToLoad), totalPages);
		}

		loading = false;
	}

	async function loadMembers() {
		const res = await membersApi.list(workspaceId);
		if (res.code === 0 && res.data) {
			const nameMap: Record<string, string> = {};
			const entityMap: Record<string, string> = {};
			for (const m of (res.data.items ?? []) as any[]) {
				const memberId = m.user_id || m.id;
				nameMap[memberId] = m.name || m.email || memberId.substring(0, 8);
				entityMap[memberId] = m.entity_type || 'human';
			}
			memberNameMap = nameMap;
			memberEntityTypeMap = entityMap;
		}
	}

	async function loadSprints() {
		const response = await sprintsApi.list(projectId);
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		if (response.data) {
			sprints = response.data.items ?? [];
		}
	}

	const assigneeOptions = $derived.by(() => {
		return Object.keys(memberNameMap);
	});

	const sprintNameMap = $derived.by(() => {
		const map: Record<string, string> = {};
		for (const sprint of sprints) {
			map[sprint.id] = sprint.name;
		}
		return map;
	});

	function goToIssue(issueId: string) {
		goto(`/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}`);
	}

	function formatIssueCode(issue: Issue): string {
		if (issue.key?.trim()) {
			return issue.key;
		}
		if (/^\d+$/.test(issue.id)) {
			return `#${issue.id}`;
		}
		return `#${issue.id.slice(0, 8)}`;
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function isBotAssignee(assigneeId: string): boolean {
		return memberEntityTypeMap[assigneeId] === 'bot';
	}

	function getAssigneeLabel(assigneeId: string): string {
		const name = memberNameMap[assigneeId] || assigneeId.substring(0, 8);
		return isBotAssignee(assigneeId) ? `[Bot] ${name}` : name;
	}

	function handleIssueCreated(issue: Issue) {
		void issue;
		currentPage = 1;
		void loadIssues(1);
	}

	function getPageNumbers(current: number, totalCount: number): Array<number | '...'> {
		if (totalCount <= 7) {
			return Array.from({ length: totalCount }, (_, i) => i + 1);
		}

		const pages = new Set<number>([1, totalCount, current - 1, current, current + 1]);
		if (current <= 3) {
			pages.add(2);
			pages.add(3);
		}
		if (current >= totalCount - 2) {
			pages.add(totalCount - 1);
			pages.add(totalCount - 2);
		}

		const sorted = [...pages].filter((p) => p >= 1 && p <= totalCount).sort((a, b) => a - b);
		const result: Array<number | '...'> = [];
		for (let i = 0; i < sorted.length; i += 1) {
			const pageNum = sorted[i];
			const prev = sorted[i - 1];
			if (i > 0 && pageNum - prev > 1) {
				result.push('...');
			}
			result.push(pageNum);
		}
		return result;
	}

	function goToPage(pageNum: number) {
		if (pageNum < 1 || pageNum > totalPages || pageNum === currentPage) return;
		currentPage = pageNum;
		void loadIssues(pageNum);
	}

	function resetPageAndLoad() {
		currentPage = 1;
		void loadIssues(1);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="rounded-lg border border-slate-200 bg-white p-4 sm:p-5 dark:border-slate-700 dark:bg-slate-800">
		<div class="mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div class="text-sm text-slate-600 dark:text-slate-400">
				<a href="/workspace" class="hover:text-blue-600">{$t('nav.myWorkspace')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<a href="/workspace/{workspaceId}/projects" class="hover:text-blue-600">{$t('project.title')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<a href="/workspace/{workspaceId}/projects/{projectId}" class="hover:text-blue-600">{$t('project.detail')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<span class="font-medium text-slate-900 dark:text-slate-100">{$t('issue.list')}</span>
			</div>
			<button
				type="button"
				class="inline-flex items-center justify-center rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white shadow-sm dark:shadow-slate-900/50 transition hover:bg-blue-700"
				onclick={() => (showCreateModal = true)}
			>
				+ {$t('issue.create')}
			</button>
		</div>

		<div class="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-6">
			<div class="xl:col-span-2">
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="keyword">{$t('common.search')}</label>
				<input
					id="keyword"
					type="search"
					bind:value={keyword}
					oninput={resetPageAndLoad}
					placeholder={$t('issue.searchPlaceholder')}
					class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
				/>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="statusFilter">{$t('common.status')}</label>
				<select
					id="statusFilter"
					bind:value={statusFilter}
					onchange={resetPageAndLoad}
					class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
				>
					<option value="all">{$t('issue.all')}</option>
					<option value="backlog">{$t('issue.backlog')}</option>
					<option value="todo">{$t('issue.todo')}</option>
					<option value="in_progress">{$t('issue.inProgress')}</option>
					<option value="done">{$t('issue.done')}</option>
				</select>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="priorityFilter">{$t('common.priority')}</label>
				<select
					id="priorityFilter"
					bind:value={priorityFilter}
					onchange={resetPageAndLoad}
					class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
				>
					<option value="all">{$t('issue.all')}</option>
					<option value="low">{$t('issue.low')}</option>
					<option value="medium">{$t('issue.medium')}</option>
					<option value="high">{$t('issue.high')}</option>
					<option value="urgent">{$t('issue.urgent')}</option>
				</select>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="assigneeFilter">{$t('common.assignee')}</label>
				<select
					id="assigneeFilter"
					bind:value={assigneeFilter}
					onchange={resetPageAndLoad}
					class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
				>
					<option value="all">{$t('issue.all')}</option>
					{#each assigneeOptions as assigneeId}
						<option value={assigneeId}>{getAssigneeLabel(assigneeId)}</option>
					{/each}
				</select>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="sortBy">{$t('issue.sortBy')}</label>
				<div class="flex gap-2">
					<select
						id="sortBy"
						bind:value={sortBy}
						onchange={resetPageAndLoad}
						class="w-full rounded-md border border-slate-300 bg-white px-2 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
					>
						<option value="updated_at">{$t('common.updatedAt')}</option>
						<option value="created_at">{$t('common.createdAt')}</option>
						<option value="priority">{$t('common.priority')}</option>
						<option value="title">{$t('issue.title')}</option>
					</select>
					<select
						bind:value={sortOrder}
						onchange={resetPageAndLoad}
						class="rounded-md border border-slate-300 bg-white px-2 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
					>
						<option value="desc">{$t('common.desc')}</option>
						<option value="asc">{$t('common.asc')}</option>
					</select>
				</div>
			</div>
		</div>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
			<div class="space-y-3">
				{#each Array(5) as _, index (`skeleton-${index}`)}
					<div class="h-12 animate-pulse rounded bg-slate-100 dark:bg-slate-700"></div>
				{/each}
			</div>
		</div>
	{:else if issues.length === 0}
		<div class="rounded-lg border border-slate-200 bg-white p-12 text-center text-slate-500 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-400">{$t('issue.noMatched')}</div>
	{:else}
		<div class="overflow-hidden rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-800">
			<div class="hidden overflow-x-auto md:block">
				<table class="min-w-full divide-y divide-slate-200 dark:divide-slate-700">
					<thead class="bg-slate-50 dark:bg-slate-900">
						<tr>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('issue.title')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.status')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.priority')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.assignee')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.updatedAt')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-200 bg-white dark:divide-slate-700 dark:bg-slate-800">
						{#each issues as issue (issue.id)}
							<tr class="cursor-pointer hover:bg-slate-50 dark:bg-slate-950 dark:hover:bg-slate-700" onclick={() => goToIssue(issue.id)}>
								<td class="px-4 py-3">
									<div class="space-y-1">
										<p class="text-sm font-medium text-slate-900 dark:text-slate-100">{issue.title}</p>
										<p class="font-mono text-xs text-slate-500 dark:text-slate-400">{formatIssueCode(issue)}</p>
										{#if issue.labels?.length}
											<div class="mt-1 flex flex-wrap gap-1">
												{#each issue.labels as label}
													<span
														class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium"
														style={`background-color: ${label.color}20; color: ${label.color}; border: 1px solid ${label.color}40;`}
													>
														{label.name}
													</span>
												{/each}
											</div>
										{/if}
										{#if issue.sprint_id}
											<span class="inline-flex w-fit items-center rounded-full border border-indigo-200 bg-indigo-50 px-2 py-0.5 text-xs font-medium text-indigo-700">
												<svg class="mr-1 h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
													<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 4v16m0-14h10l-2 4 2 4H5"></path>
												</svg>
												{sprintNameMap[issue.sprint_id] ?? $t('issue.sprint')}
											</span>
										{/if}
									</div>
								</td>
								<td class="px-4 py-3"><Badge value={issue.status} /></td>
								<td class="px-4 py-3"><Badge kind="priority" value={issue.priority} /></td>
								<td class="px-4 py-3">
									{#if issue.assignee_id}
										<div class="flex items-center gap-2">
											<Avatar userId={issue.assignee_id} />
											{#if isBotAssignee(issue.assignee_id)}
												<BotIcon class="h-3.5 w-3.5 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
											{/if}
											<span class="text-xs text-slate-600 dark:text-slate-400">{memberNameMap[issue.assignee_id] || issue.assignee_id.substring(0, 8)}</span>
										</div>
									{:else}
										<span class="text-xs text-slate-400">{$t('common.unassigned')}</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-400">{formatDate(issue.updated_at)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			<div class="divide-y divide-slate-200 dark:divide-slate-700 md:hidden">
				{#each issues as issue (issue.id)}
					<button class="w-full space-y-2 p-4 text-left hover:bg-slate-50 dark:bg-slate-950 dark:hover:bg-slate-700" onclick={() => goToIssue(issue.id)}>
						<div class="flex items-start justify-between gap-3">
							<div>
								<p class="text-sm font-medium text-slate-900 dark:text-slate-100">{issue.title}</p>
								<p class="font-mono text-xs text-slate-500 dark:text-slate-400">{formatIssueCode(issue)}</p>
									{#if issue.labels?.length}
										<div class="mt-1 flex flex-wrap gap-1">
										{#each issue.labels as label}
											<span
												class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium"
												style={`background-color: ${label.color}20; color: ${label.color}; border: 1px solid ${label.color}40;`}
											>
												{label.name}
											</span>
											{/each}
										</div>
									{/if}
									{#if issue.sprint_id}
										<span class="inline-flex w-fit items-center rounded-full border border-indigo-200 bg-indigo-50 px-2 py-0.5 text-xs font-medium text-indigo-700">
											<svg class="mr-1 h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
												<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 4v16m0-14h10l-2 4 2 4H5"></path>
											</svg>
											{sprintNameMap[issue.sprint_id] ?? $t('issue.sprint')}
										</span>
									{/if}
								</div>
							<Badge kind="priority" value={issue.priority} />
						</div>
						<div class="flex items-center justify-between">
							<Badge value={issue.status} />
							{#if issue.assignee_id}
								<div class="flex items-center gap-2">
									<Avatar userId={issue.assignee_id} />
									{#if isBotAssignee(issue.assignee_id)}
										<BotIcon class="h-3.5 w-3.5 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
									{/if}
									<span class="text-xs text-slate-600 dark:text-slate-400">{memberNameMap[issue.assignee_id] || issue.assignee_id.substring(0, 8)}</span>
								</div>
							{:else}
								<span class="text-xs text-slate-400">{$t('common.unassigned')}</span>
							{/if}
						</div>
						<p class="text-xs text-slate-500 dark:text-slate-400">{$t('project.updatedAt')} {formatDate(issue.updated_at)}</p>
					</button>
				{/each}
			</div>
		</div>

		{#if totalPages > 1}
			<div class="mt-6 flex items-center justify-between px-2">
				<span class="text-sm text-slate-500 dark:text-slate-400">{$t('issue.pagination', { values: { total, current: currentPage, pages: totalPages } })}</span>
				<div class="flex items-center gap-1">
					<button
						class="rounded border px-3 py-1 text-sm {currentPage === 1 ? 'cursor-not-allowed text-slate-300' : 'hover:bg-slate-100 dark:hover:bg-slate-800 dark:bg-slate-800'}"
						onclick={() => goToPage(currentPage - 1)}
						disabled={currentPage === 1}
					>
						{$t('issue.prevPage')}
					</button>
					{#each getPageNumbers(currentPage, totalPages) as p}
						{#if p === '...'}
							<span class="px-2 text-slate-400">...</span>
						{:else}
							<button
								class="rounded px-3 py-1 text-sm {p === currentPage ? 'bg-blue-600 text-white' : 'border hover:bg-slate-100 dark:hover:bg-slate-800 dark:bg-slate-800'}"
								onclick={() => goToPage(p)}
							>
								{p}
							</button>
						{/if}
					{/each}
					<button
						class="rounded border px-3 py-1 text-sm {currentPage === totalPages ? 'cursor-not-allowed text-slate-300' : 'hover:bg-slate-100 dark:hover:bg-slate-800 dark:bg-slate-800'}"
						onclick={() => goToPage(currentPage + 1)}
						disabled={currentPage === totalPages}
					>
						{$t('issue.nextPage')}
					</button>
				</div>
			</div>
		{/if}
	{/if}
</div>

<IssueCreateModal
	bind:open={showCreateModal}
	{workspaceId}
	{projectId}
	onCreated={handleIssueCreated}
/>

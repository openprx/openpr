<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { membersApi } from '$lib/api/members';
	import { issuesApi, type Issue, type IssuePriority, type IssueStatus } from '$lib/api/issues';
	import { sprintsApi, type Sprint } from '$lib/api/sprints';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import Input from '$lib/components/Input.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Avatar from '$lib/components/Avatar.svelte';
	import BotIcon from '$lib/components/BotIcon.svelte';
	import IssueCreateModal from '$lib/components/IssueCreateModal.svelte';

	const workspaceId = $derived(requireRouteParam($page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam($page.params.projectId, 'projectId'));

	const columns: IssueStatus[] = ['backlog', 'todo', 'in_progress', 'done'];
	
	function getColumnName(status: IssueStatus): string {
		const keys: Record<IssueStatus, string> = {
			backlog: 'issue.backlog',
			todo: 'issue.todo',
			in_progress: 'issue.inProgress',
			done: 'issue.done'
		};
		return get(t)(keys[status]);
	}

	let issues = $state<Issue[]>([]);
	let sprints = $state<Sprint[]>([]);
	let memberNameMap = $state<Record<string, string>>({});
	let memberEntityTypeMap = $state<Record<string, string>>({});
	let loading = $state(true);
	let creatingQuick = $state(false);
	let updatingIssueId = $state<string | null>(null);

	let keyword = $state('');
	let priorityFilter = $state<'all' | IssuePriority>('all');
	let assigneeFilter = $state('all');
	const sprintFilterId = $derived($page.url.searchParams.get('sprintId') ?? '');

	let quickTitle = $state('');
	let showDetailedCreate = $state(false);

	let draggingIssueId = $state<string | null>(null);
	let dropTargetStatus = $state<IssueStatus | null>(null);

	onMount(async () => {
		await Promise.all([loadIssues(), loadSprints()]);
	});

	async function loadIssues() {
		loading = true;
		const response = await issuesApi.list(projectId, { page: 1, per_page: 500 });

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			issues = response.data.items ?? [];
		}

		loading = false;

		membersApi.list(workspaceId).then((res) => {
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
		}).catch(() => {});
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
		const values = issues
			.map((issue) => issue.assignee_id)
			.filter((assigneeId): assigneeId is string => Boolean(assigneeId));
		return Array.from(new Set(values));
	});

	const filteredIssues = $derived.by(() => {
		const query = keyword.trim().toLowerCase();
		return issues.filter((issue) => {
			const keywordMatched =
				query.length === 0 ||
				issue.title.toLowerCase().includes(query) ||
				issue.description?.toLowerCase().includes(query) ||
				issue.id.toLowerCase().includes(query) ||
				issue.key?.toLowerCase().includes(query);

			const priorityMatched = priorityFilter === 'all' || issue.priority === priorityFilter;
			const assigneeMatched = assigneeFilter === 'all' || issue.assignee_id === assigneeFilter;
			const sprintMatched = !sprintFilterId || issue.sprint_id === sprintFilterId;

			return keywordMatched && priorityMatched && assigneeMatched && sprintMatched;
		});
	});

	const sprintNameMap = $derived.by(() => {
		const map: Record<string, string> = {};
		for (const sprint of sprints) {
			map[sprint.id] = sprint.name;
		}
		return map;
	});

	function getIssuesByStatus(status: IssueStatus): Issue[] {
		return filteredIssues.filter((issue) => issue.status === status);
	}

	function getIssueCode(issue: Issue): string {
		if (issue.key?.trim()) {
			return issue.key;
		}
		if (/^\d+$/.test(issue.id)) {
			return `#${issue.id}`;
		}
		return `#${issue.id.slice(0, 8)}`;
	}

	function getPriorityBarClass(priority: IssuePriority): string {
		const classes: Record<IssuePriority, string> = {
			low: 'bg-slate-300',
			medium: 'bg-blue-400',
			high: 'bg-orange-400',
			urgent: 'bg-red-500'
		};
		return classes[priority];
	}

	function isBotAssignee(assigneeId: string): boolean {
		return memberEntityTypeMap[assigneeId] === 'bot';
	}

	function getAssigneeLabel(assigneeId: string): string {
		const name = memberNameMap[assigneeId] || assigneeId.substring(0, 8);
		return isBotAssignee(assigneeId) ? `[Bot] ${name}` : name;
	}

	async function handleQuickCreate() {
		if (creatingQuick) return;

		const title = quickTitle.trim();
		if (!title) {
			toast.error(get(t)('issue.enterTitle'));
			return;
		}

		creatingQuick = true;
		const response = await issuesApi.create(projectId, {
			title,
			status: 'backlog',
			priority: 'medium',
			sprint_id: sprintFilterId || undefined
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			issues = [response.data, ...issues];
			quickTitle = '';
			toast.success(get(t)('issue.createSuccess'));
		}

		creatingQuick = false;
	}

	function handleDetailedCreated(issue: Issue) {
		issues = [issue, ...issues];
	}

	function handleDragStart(issueId: string) {
		draggingIssueId = issueId;
	}

	function handleDragEnd() {
		draggingIssueId = null;
		dropTargetStatus = null;
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
	}

	function handleDragEnter(status: IssueStatus) {
		if (draggingIssueId) {
			dropTargetStatus = status;
		}
	}

	function handleDragLeave(event: DragEvent, status: IssueStatus) {
		const current = event.currentTarget;
		const related = event.relatedTarget;

		if (current instanceof HTMLElement && related instanceof Node && current.contains(related)) {
			return;
		}

		if (dropTargetStatus === status) {
			dropTargetStatus = null;
		}
	}

	async function handleDrop(event: DragEvent, targetStatus: IssueStatus) {
		event.preventDefault();
		dropTargetStatus = null;

		if (!draggingIssueId) {
			return;
		}

		const issueId = draggingIssueId;
		draggingIssueId = null;

		const currentIssue = issues.find((issue) => issue.id === issueId);
		if (!currentIssue) {
			return;
		}
		if (currentIssue.status === targetStatus) {
			return;
		}

		const previousIssues = issues;
		issues = issues.map((issue) =>
			issue.id === issueId ? { ...issue, status: targetStatus, updated_at: new Date().toISOString() } : issue
		);
		updatingIssueId = issueId;

		const response = await issuesApi.update(issueId, { status: targetStatus });
		updatingIssueId = null;

		if (response.code !== 0) {
			issues = previousIssues;
			toast.error(get(t)('issue.statusUpdateFailed', { values: { message: response.message } }));
			return;
		}

		if (response.data) {
			const updatedIssue = response.data;
			issues = issues.map((issue) => (issue.id === issueId ? updatedIssue : issue));
		}
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="rounded-lg border border-slate-200 bg-white p-4 sm:p-5 dark:border-slate-700 dark:bg-slate-800">
		<div class="mb-4 flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
			<div class="text-sm text-slate-600 dark:text-slate-400">
				<a href="/workspace" class="hover:text-blue-600">{$t('nav.myWorkspace')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<a href="/workspace/{workspaceId}/projects" class="hover:text-blue-600">{$t('project.title')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<a href="/workspace/{workspaceId}/projects/{projectId}" class="hover:text-blue-600">{$t('project.detail')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<span class="font-medium text-slate-900 dark:text-slate-100">{$t('board.title')}</span>
			</div>
		</div>
		{#if sprintFilterId}
			<div class="mb-4 flex items-center justify-between rounded-md border border-blue-200 bg-blue-50 px-3 py-2 text-sm">
				<span class="text-blue-700">{$t('board.filterBySprint', { values: { sprint: sprintNameMap[sprintFilterId] ?? sprintFilterId } })}</span>
				<a
					href="/workspace/{workspaceId}/projects/{projectId}/board"
					class="text-blue-700 underline hover:text-blue-800"
				>
					{$t('board.clearFilter')}
				</a>
			</div>
		{/if}

		<div class="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-5">
			<div class="xl:col-span-2">
				<Input label={$t('common.search')} placeholder={$t('issue.searchPlaceholder')} bind:value={keyword} type="search" />
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="priorityFilter">{$t('common.priority')}</label>
				<select
					id="priorityFilter"
					bind:value={priorityFilter}
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
					class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
				>
					<option value="all">{$t('issue.all')}</option>
					{#each assigneeOptions as assigneeId}
						<option value={assigneeId}>{getAssigneeLabel(assigneeId)}</option>
					{/each}
				</select>
			</div>
			<div class="flex items-end gap-2">
				<Button variant="secondary" onclick={() => (showDetailedCreate = true)}>{$t('issue.detailedCreate')}</Button>
			</div>
		</div>

		<div class="mt-4 flex flex-col gap-2 sm:flex-row">
			<input
				type="text"
				bind:value={quickTitle}
				placeholder={$t('issue.quickCreatePlaceholder')}
				class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
			/>
			<Button loading={creatingQuick} onclick={handleQuickCreate}>+ {$t('issue.create')}</Button>
		</div>
	</div>

	{#if loading}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
			{#each columns as column}
				<div class="rounded-lg border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-800">
					<div class="mb-3 h-5 w-24 animate-pulse rounded bg-slate-200"></div>
					<div class="space-y-3">
						{#each Array(2) as _, index (`${column}-${index}`)}
							<div class="h-24 animate-pulse rounded-md border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900"></div>
						{/each}
					</div>
				</div>
			{/each}
		</div>
	{:else}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
			{#each columns as column}
				<div
					class={`rounded-lg border p-3 transition-colors ${dropTargetStatus === column
						? 'border-blue-400 bg-blue-50 dark:bg-blue-500/20'
						: 'border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-800'}`}
					role="region"
					ondragover={handleDragOver}
					ondragenter={() => handleDragEnter(column)}
					ondragleave={(event) => handleDragLeave(event, column)}
					ondrop={(event) => handleDrop(event, column)}
				>
					<div class="mb-3 flex items-center justify-between">
						<h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{getColumnName(column)}</h2>
						<span class="rounded-full bg-white px-2 py-1 text-xs text-slate-600 dark:bg-slate-700 dark:text-slate-300">
							{getIssuesByStatus(column).length}
						</span>
					</div>

					<div class="space-y-3">
						{#each getIssuesByStatus(column) as issue (issue.id)}
							<div
								draggable="true"
								role="article"
								ondragstart={() => handleDragStart(issue.id)}
								ondragend={handleDragEnd}
								class={`group relative overflow-hidden rounded-md border bg-white p-3 shadow-sm dark:shadow-slate-900/50 transition hover:shadow-md dark:shadow-slate-900/50 dark:border-slate-700 dark:bg-slate-900 ${draggingIssueId === issue.id ? 'opacity-50' : ''}`}
							>
								<div class={`absolute inset-y-0 left-0 w-1 ${getPriorityBarClass(issue.priority)}`}></div>
								<div class="ml-2 space-y-2">
									<div class="flex items-start justify-between gap-2">
										<a
											href="/workspace/{workspaceId}/projects/{projectId}/issues/{issue.id}"
											class="line-clamp-2 text-sm font-medium text-slate-900 hover:text-blue-600 dark:text-slate-100"
										>
											{issue.title}
										</a>
										{#if updatingIssueId === issue.id}
											<span class="text-[10px] text-blue-600">{$t('common.updating')}</span>
										{/if}
									</div>
									{#if issue.labels?.length}
										<div class="flex flex-wrap gap-1">
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
									<div class="flex items-center justify-between gap-2">
										<div class="flex items-center gap-2">
											<Badge kind="priority" value={issue.priority} />
											<span class="font-mono text-xs text-slate-500 dark:text-slate-400">{getIssueCode(issue)}</span>
										</div>
										{#if issue.assignee_id}
											<div class="flex items-center gap-1.5">
												<Avatar userId={issue.assignee_id} />
												{#if isBotAssignee(issue.assignee_id)}
													<BotIcon class="h-3.5 w-3.5 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
												{/if}
											</div>
										{/if}
									</div>
								</div>
							</div>
						{/each}

						{#if getIssuesByStatus(column).length === 0}
							<div class="rounded-md border border-dashed border-slate-300 bg-white/70 p-4 text-center text-sm text-slate-400 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-400">
								{$t('issue.none')}
							</div>
						{/if}
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>

<IssueCreateModal
	bind:open={showDetailedCreate}
	{workspaceId}
	{projectId}
	initialSprintId={sprintFilterId}
	onCreated={handleDetailedCreated}
/>

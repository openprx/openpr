<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { issuesApi, type Issue } from '$lib/api/issues';
	import {
		sprintsApi,
		type CreateSprintData,
		type Sprint,
		type SprintStatus,
		type UpdateSprintData
	} from '$lib/api/sprints';
	import ProgressBar from '$lib/components/ProgressBar.svelte';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam($page.params.projectId, 'projectId');

	let loading = $state(true);
	let saving = $state(false);
	let sprints = $state<Sprint[]>([]);
	let projectIssues = $state<Issue[]>([]);
	let errorMessage = $state('');

	let showCreateModal = $state(false);
	let showEditModal = $state(false);
	let editingSprint = $state<Sprint | null>(null);

	type SprintForm = {
		name: string;
		goal: string;
		start_date: string;
		end_date: string;
	};

	const emptyForm: SprintForm = {
		name: '',
		goal: '',
		start_date: '',
		end_date: ''
	};

	let createForm = $state<SprintForm>({ ...emptyForm });
	let editForm = $state<SprintForm>({ ...emptyForm });

	onMount(async () => {
		await loadData();
	});

	const activeSprints = $derived.by(() =>
		sprints
			.filter((item) => item.status === 'active')
			.sort((a, b) => new Date(a.start_date).getTime() - new Date(b.start_date).getTime())
	);
	const plannedSprints = $derived.by(() =>
		sprints
			.filter((item) => item.status === 'planned')
			.sort((a, b) => new Date(a.start_date).getTime() - new Date(b.start_date).getTime())
	);
	const completedSprints = $derived.by(() =>
		sprints
			.filter((item) => item.status === 'completed')
			.sort((a, b) => new Date(b.end_date).getTime() - new Date(a.end_date).getTime())
	);

	const sprintProgress = $derived.by(() => {
		const stats: Record<string, { done: number; total: number; percent: number }> = {};
		for (const sprint of sprints) {
			const issues = projectIssues.filter((item) => item.sprint_id === sprint.id);
			const done = issues.filter((item) => item.status === 'done').length;
			const total = issues.length;
			const percent = total > 0 ? Math.round((done / total) * 100) : 0;
			stats[sprint.id] = { done, total, percent };
		}
		return stats;
	});

	const sprintIssues = $derived.by(() => {
		const map: Record<string, Issue[]> = {};
		for (const sprint of sprints) {
			map[sprint.id] = projectIssues
				.filter((item) => item.sprint_id === sprint.id)
				.sort((a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime());
		}
		return map;
	});

	async function loadData() {
		loading = true;
		errorMessage = '';

		const [sprintsResponse, issuesResponse] = await Promise.all([
			sprintsApi.list(projectId),
			issuesApi.list(projectId, { page: 1, per_page: 1000 })
		]);

		if (sprintsResponse.code !== 0) {
			errorMessage = sprintsResponse.message;
			sprints = [];
		} else if (sprintsResponse.data) {
			sprints = sprintsResponse.data.items ?? [];
		}

		if (issuesResponse.code !== 0) {
			projectIssues = [];
		} else if (issuesResponse.data) {
			projectIssues = issuesResponse.data.items ?? [];
		}

		loading = false;
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleDateString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit'
		});
	}

	function toDateInputValue(dateText: string): string {
		const date = new Date(dateText);
		const year = date.getFullYear();
		const month = String(date.getMonth() + 1).padStart(2, '0');
		const day = String(date.getDate()).padStart(2, '0');
		return `${year}-${month}-${day}`;
	}

	function getStatusLabel(status: SprintStatus): string {
		const map: Record<SprintStatus, string> = {
			planned: 'Planned',
			active: 'Active',
			completed: 'Completed'
		};
		return map[status];
	}

	function getStatusClass(status: SprintStatus): string {
		const map: Record<SprintStatus, string> = {
			planned: 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300',
			active: 'bg-green-100 text-green-700',
			completed: 'bg-blue-100 text-blue-700'
		};
		return map[status];
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

	function openCreateModal() {
		createForm = { ...emptyForm };
		showCreateModal = true;
	}

	function openEditModal(sprint: Sprint) {
		editingSprint = sprint;
		editForm = {
			name: sprint.name,
			goal: sprint.goal ?? '',
			start_date: toDateInputValue(sprint.start_date),
			end_date: toDateInputValue(sprint.end_date)
		};
		showEditModal = true;
	}

	function buildPayload(form: SprintForm): CreateSprintData {
		return {
			name: form.name.trim(),
			goal: form.goal.trim() || undefined,
			start_date: form.start_date,
			end_date: form.end_date
		};
	}

	function validateForm(form: SprintForm): string | null {
		if (!form.name.trim()) {
			return get(t)('cycles.enterName');
		}
		if (!form.start_date || !form.end_date) {
			return get(t)('cycles.selectDates');
		}
		if (new Date(form.start_date).getTime() > new Date(form.end_date).getTime()) {
			return get(t)('cycles.endDateAfterStart');
		}
		return null;
	}

	async function submitCreate() {
		if (saving) return;
		const validateError = validateForm(createForm);
		if (validateError) {
			toast.error(validateError);
			return;
		}

		saving = true;
		const response = await sprintsApi.create(projectId, buildPayload(createForm));
		saving = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		showCreateModal = false;
		toast.success(get(t)('cycles.createSuccess'));
		await loadData();
	}

	async function submitEdit() {
		if (saving || !editingSprint) return;
		const validateError = validateForm(editForm);
		if (validateError) {
			toast.error(validateError);
			return;
		}

		saving = true;
		const payload: UpdateSprintData = buildPayload(editForm);
		const response = await sprintsApi.update(editingSprint.id, payload);
		saving = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		showEditModal = false;
		editingSprint = null;
		toast.success(get(t)('cycles.updateSuccess'));
		await loadData();
	}

	async function removeSprint(sprint: Sprint) {
		const confirmed = window.confirm(get(t)('cycles.deleteConfirm', { values: { name: sprint.name } }));
		if (!confirmed) return;

		const response = await sprintsApi.delete(sprint.id);
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		toast.success(get(t)('cycles.deleteSuccess'));
		await loadData();
	}

	async function updateSprintStatus(sprint: Sprint, status: SprintStatus) {
		const response = await sprintsApi.update(sprint.id, { status });
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		toast.success(status === 'active' ? get(t)('cycles.started') : get(t)('cycles.completed'));
		await loadData();
	}

	function goSprintBoard(sprintId: string) {
		goto(`/workspace/${workspaceId}/projects/${projectId}/board?sprintId=${sprintId}`);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:p-5">
		<div class="mb-4 text-sm text-slate-600 dark:text-slate-300">
			<a href="/workspace" class="hover:text-blue-600">{$t('nav.myWorkspace')}</a>
			<span class="mx-1 text-slate-400">/</span>
			<a href="/workspace/{workspaceId}/projects" class="hover:text-blue-600">{$t('project.title')}</a>
			<span class="mx-1 text-slate-400">/</span>
			<a href="/workspace/{workspaceId}/projects/{projectId}" class="hover:text-blue-600">{$t('project.detail')}</a>
			<span class="mx-1 text-slate-400">/</span>
			<span class="font-medium text-slate-900 dark:text-slate-100">{$t('cycles.title')}</span>
		</div>

		<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('cycles.title')}</h1>
				<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('cycles.subtitle')}</p>
			</div>
			<button
				onclick={openCreateModal}
				class="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700"
			>
				+ {$t('cycles.create')}
			</button>
		</div>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-8 text-center text-slate-500 dark:text-slate-400">
			<div class="mx-auto mb-3 h-6 w-6 animate-spin rounded-full border-2 border-blue-600 border-t-transparent"></div>
			{$t('cycles.loading')}
		</div>
	{:else if errorMessage}
		<div class="rounded-lg border border-red-200 bg-red-50 p-6 text-center">
			<p class="mb-3 text-sm text-red-700">{errorMessage}</p>
			<button onclick={loadData} class="rounded-md bg-red-600 px-4 py-2 text-sm text-white hover:bg-red-700">
				{$t('common.retry')}
			</button>
		</div>
	{:else}
		<section class="space-y-3">
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('cycles.current')}</h2>
			{#if activeSprints.length === 0}
				<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-sm text-slate-500 dark:text-slate-400">{$t('cycles.noActive')}</div>
			{:else}
				<div class="space-y-3">
					{#each activeSprints as sprint (sprint.id)}
						<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:p-5">
							<div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
								<div>
									<h3 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{sprint.name}</h3>
									<p class="text-sm text-slate-600 dark:text-slate-300">
										{formatDate(sprint.start_date)} ~ {formatDate(sprint.end_date)}
									</p>
								</div>
								<span class="inline-flex w-fit rounded px-2 py-1 text-xs font-medium {getStatusClass(sprint.status)}">
									{getStatusLabel(sprint.status)}
								</span>
							</div>

							{#if sprint.goal}
								<p class="mb-3 text-sm text-slate-700 dark:text-slate-300">{$t('cycles.goal')}: {sprint.goal}</p>
							{/if}

							<ProgressBar
								value={sprintProgress[sprint.id]?.percent ?? 0}
								label={$t('cycles.progressLabel', { values: { done: sprintProgress[sprint.id]?.done ?? 0, total: sprintProgress[sprint.id]?.total ?? 0 } })}
							/>
							<div class="mt-4 rounded-md border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 p-3">
								<p class="mb-2 text-xs font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
									{$t('cycles.linkedIssues', { values: { count: sprintIssues[sprint.id]?.length ?? 0 } })}
								</p>
								{#if (sprintIssues[sprint.id]?.length ?? 0) === 0}
									<p class="text-sm text-slate-500 dark:text-slate-400">{$t('cycles.noLinkedIssue')}</p>
								{:else}
									<div class="space-y-2">
										{#each sprintIssues[sprint.id].slice(0, 5) as issue (issue.id)}
											<a
												href="/workspace/{workspaceId}/projects/{projectId}/issues/{issue.id}"
												class="block rounded border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 px-3 py-2 text-sm hover:border-blue-200 hover:bg-blue-50"
											>
												<p class="font-medium text-slate-900 dark:text-slate-100">{issue.title}</p>
												<p class="font-mono text-xs text-slate-500 dark:text-slate-400">{formatIssueCode(issue)}</p>
											</a>
										{/each}
									</div>
								{/if}
							</div>

							<div class="mt-4 flex flex-wrap gap-2">
								<button
									onclick={() => goSprintBoard(sprint.id)}
									class="rounded-md border border-slate-300 dark:border-slate-600 px-3 py-1.5 text-sm text-slate-700 dark:text-slate-300 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
								>
									{$t('cycles.viewDetail')}
								</button>
								<button
									onclick={() => updateSprintStatus(sprint, 'completed')}
									class="rounded-md bg-green-600 px-3 py-1.5 text-sm text-white hover:bg-green-700"
								>
									{$t('cycles.completeSprint')}
								</button>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section class="space-y-3">
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('cycles.planned')}</h2>
			{#if plannedSprints.length === 0}
				<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-sm text-slate-500 dark:text-slate-400">{$t('cycles.noPlanned')}</div>
			{:else}
				<div class="space-y-3">
					{#each plannedSprints as sprint (sprint.id)}
						<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:p-5">
							<div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
								<div>
									<h3 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{sprint.name}</h3>
									<p class="text-sm text-slate-600 dark:text-slate-300">
										{formatDate(sprint.start_date)} ~ {formatDate(sprint.end_date)}
									</p>
								</div>
								<span class="inline-flex w-fit rounded px-2 py-1 text-xs font-medium {getStatusClass(sprint.status)}">
									{getStatusLabel(sprint.status)}
								</span>
							</div>

								{#if sprint.goal}
									<p class="mb-3 text-sm text-slate-700 dark:text-slate-300">{$t('cycles.goal')}: {sprint.goal}</p>
								{/if}
								<div class="rounded-md border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 p-3">
									<p class="mb-2 text-xs font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
										{$t('cycles.linkedIssues', { values: { count: sprintIssues[sprint.id]?.length ?? 0 } })}
									</p>
									{#if (sprintIssues[sprint.id]?.length ?? 0) === 0}
										<p class="text-sm text-slate-500 dark:text-slate-400">{$t('cycles.noLinkedIssue')}</p>
									{:else}
										<div class="space-y-2">
											{#each sprintIssues[sprint.id].slice(0, 5) as issue (issue.id)}
												<a
													href="/workspace/{workspaceId}/projects/{projectId}/issues/{issue.id}"
													class="block rounded border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 px-3 py-2 text-sm hover:border-blue-200 hover:bg-blue-50"
												>
													<p class="font-medium text-slate-900 dark:text-slate-100">{issue.title}</p>
													<p class="font-mono text-xs text-slate-500 dark:text-slate-400">{formatIssueCode(issue)}</p>
												</a>
											{/each}
										</div>
									{/if}
								</div>

								<div class="mt-4 flex flex-wrap gap-2">
								<button
									onclick={() => openEditModal(sprint)}
									class="rounded-md border border-slate-300 dark:border-slate-600 px-3 py-1.5 text-sm text-slate-700 dark:text-slate-300 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
								>
									{$t('common.edit')}
								</button>
								<button
									onclick={() => removeSprint(sprint)}
									class="rounded-md border border-red-200 px-3 py-1.5 text-sm text-red-700 hover:bg-red-50"
								>
									{$t('common.delete')}
								</button>
								<button
									onclick={() => updateSprintStatus(sprint, 'active')}
									class="rounded-md bg-blue-600 px-3 py-1.5 text-sm text-white hover:bg-blue-700"
								>
									{$t('cycles.startSprint')}
								</button>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:p-5">
			<details>
				<summary class="cursor-pointer list-none text-lg font-semibold text-slate-900 dark:text-slate-100">
					<span class="inline-flex items-center gap-2">{$t('cycles.completedList')} <span class="text-sm text-slate-500 dark:text-slate-400">({completedSprints.length})</span></span>
				</summary>
				<div class="mt-4 space-y-3">
					{#if completedSprints.length === 0}
						<p class="text-sm text-slate-500 dark:text-slate-400">{$t('cycles.noCompleted')}</p>
					{:else}
						{#each completedSprints as sprint (sprint.id)}
							<div class="rounded-lg border border-slate-200 dark:border-slate-700 p-4">
								<div class="mb-2 flex items-start justify-between gap-2">
									<div>
										<p class="font-medium text-slate-900 dark:text-slate-100">{sprint.name}</p>
										<p class="text-sm text-slate-600 dark:text-slate-300">
											{formatDate(sprint.start_date)} ~ {formatDate(sprint.end_date)}
										</p>
									</div>
									<span class="inline-flex rounded px-2 py-1 text-xs font-medium {getStatusClass(sprint.status)}">
										{getStatusLabel(sprint.status)}
									</span>
								</div>
								<ProgressBar
									value={sprintProgress[sprint.id]?.percent ?? 0}
									label={$t('cycles.completedLabel', { values: { done: sprintProgress[sprint.id]?.done ?? 0, total: sprintProgress[sprint.id]?.total ?? 0 } })}
								/>
								<div class="mt-3 rounded-md border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 p-3">
									<p class="mb-2 text-xs font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
										{$t('cycles.linkedIssues', { values: { count: sprintIssues[sprint.id]?.length ?? 0 } })}
									</p>
									{#if (sprintIssues[sprint.id]?.length ?? 0) === 0}
										<p class="text-sm text-slate-500 dark:text-slate-400">{$t('cycles.noLinkedIssue')}</p>
									{:else}
										<div class="space-y-2">
											{#each sprintIssues[sprint.id].slice(0, 5) as issue (issue.id)}
												<a
													href="/workspace/{workspaceId}/projects/{projectId}/issues/{issue.id}"
													class="block rounded border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 px-3 py-2 text-sm hover:border-blue-200 hover:bg-blue-50"
												>
													<p class="font-medium text-slate-900 dark:text-slate-100">{issue.title}</p>
													<p class="font-mono text-xs text-slate-500 dark:text-slate-400">{formatIssueCode(issue)}</p>
												</a>
											{/each}
										</div>
									{/if}
								</div>
							</div>
						{/each}
					{/if}
				</div>
			</details>
		</section>
	{/if}
</div>

{#if showCreateModal}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 dark:bg-black/70">
		<div class="w-full max-w-lg rounded-lg bg-white dark:bg-slate-900 p-6 shadow-xl dark:shadow-slate-900/50">
			<h2 class="mb-4 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('cycles.create')}</h2>
			<div class="space-y-4">
				<div>
					<label for="create-name" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.name')}</label>
						<input
							id="create-name"
							type="text"
							bind:value={createForm.name}
							placeholder={$t('cycles.namePlaceholder')}
							class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
						/>
				</div>
				<div>
					<label for="create-goal" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.goal')}</label>
					<textarea
						id="create-goal"
						bind:value={createForm.goal}
						rows="3"
						placeholder={$t('cycles.goalPlaceholder')}
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					></textarea>
				</div>
				<div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
					<div>
						<label for="create-start-date" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.startDate')}</label>
						<input
							id="create-start-date"
							type="date"
							bind:value={createForm.start_date}
							class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
						/>
					</div>
					<div>
						<label for="create-end-date" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.endDate')}</label>
						<input
							id="create-end-date"
							type="date"
							bind:value={createForm.end_date}
							class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
						/>
					</div>
				</div>
			</div>
			<div class="mt-6 flex justify-end gap-2">
				<button
					onclick={() => (showCreateModal = false)}
					class="rounded-md border border-slate-300 dark:border-slate-600 px-4 py-2 text-sm text-slate-700 dark:text-slate-300 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
				>
					{$t('common.cancel')}
				</button>
				<button
					onclick={submitCreate}
					disabled={saving}
					class="rounded-md bg-blue-600 px-4 py-2 text-sm text-white hover:bg-blue-700 disabled:opacity-60"
				>
					{saving ? $t('common.creating') : $t('common.create')}
				</button>
			</div>
		</div>
	</div>
{/if}

{#if showEditModal}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 dark:bg-black/70">
		<div class="w-full max-w-lg rounded-lg bg-white dark:bg-slate-900 p-6 shadow-xl dark:shadow-slate-900/50">
			<h2 class="mb-4 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('cycles.edit')}</h2>
			<div class="space-y-4">
				<div>
					<label for="edit-name" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.name')}</label>
					<input
						id="edit-name"
						type="text"
						bind:value={editForm.name}
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					/>
				</div>
				<div>
					<label for="edit-goal" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.goal')}</label>
					<textarea
						id="edit-goal"
						bind:value={editForm.goal}
						rows="3"
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					></textarea>
				</div>
				<div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
					<div>
						<label for="edit-start-date" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.startDate')}</label>
						<input
							id="edit-start-date"
							type="date"
							bind:value={editForm.start_date}
							class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
						/>
					</div>
					<div>
						<label for="edit-end-date" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('cycles.endDate')}</label>
						<input
							id="edit-end-date"
							type="date"
							bind:value={editForm.end_date}
							class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
						/>
					</div>
				</div>
			</div>
			<div class="mt-6 flex justify-end gap-2">
				<button
					onclick={() => {
						showEditModal = false;
						editingSprint = null;
					}}
					class="rounded-md border border-slate-300 dark:border-slate-600 px-4 py-2 text-sm text-slate-700 dark:text-slate-300 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
				>
					{$t('common.cancel')}
				</button>
				<button
					onclick={submitEdit}
					disabled={saving}
					class="rounded-md bg-blue-600 px-4 py-2 text-sm text-white hover:bg-blue-700 disabled:opacity-60"
				>
					{saving ? $t('common.saving') : $t('common.save')}
				</button>
			</div>
		</div>
	</div>
{/if}

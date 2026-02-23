<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { authStore } from '$lib/stores/auth';
	import { workspacesApi, type Workspace } from '$lib/api/workspaces';
	import { myApi, type MyActivity, type MyIssue } from '$lib/api/my';
	import { toast } from '$lib/stores/toast';
	import { goto } from '$app/navigation';
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import Card from '$lib/components/Card.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { timeAgo } from '$lib/utils/timeago';

	type InputEventHandler = (event: Event & { currentTarget: EventTarget & HTMLInputElement }) => void;

	let workspaces = $state<Workspace[]>([]);
	let loading = $state(true);
	let loadingDashboard = $state(true);
	let myIssues = $state<MyIssue[]>([]);
	let myActivities = $state<MyActivity[]>([]);
	let activitiesPage = $state(1);
	let activitiesTotalPages = $state(1);
	let loadingMoreActivities = $state(false);

	let showCreateModal = $state(false);
	let createForm = $state({ slug: '', name: '', description: '' });
	let creating = $state(false);

	let showEditModal = $state(false);
	let editingWorkspace = $state<Workspace | null>(null);
	let editForm = $state({ name: '', description: '' });
	let updating = $state(false);

	let showDeleteModal = $state(false);
	let deletingWorkspace = $state<Workspace | null>(null);
	let deleting = $state(false);

	onMount(async () => {
		await Promise.all([loadWorkspaces(), loadDashboardData()]);
	});

	async function loadWorkspaces() {
		loading = true;
		const response = await workspacesApi.list();

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			workspaces = response.data.items ?? [];
		}

		loading = false;
	}

	async function loadDashboardData() {
		loadingDashboard = true;
		const [issuesResponse, activitiesResponse] = await Promise.all([
			myApi.getMyIssues(),
			myApi.getMyActivities({ page: 1, per_page: 10 })
		]);

		if (issuesResponse.code !== 0) {
			toast.error(issuesResponse.message);
		} else if (issuesResponse.data) {
			myIssues = issuesResponse.data.items ?? [];
		}

		if (activitiesResponse.code !== 0) {
			toast.error(activitiesResponse.message);
		} else if (activitiesResponse.data) {
			myActivities = activitiesResponse.data.items ?? [];
			activitiesPage = activitiesResponse.data.page;
			activitiesTotalPages = activitiesResponse.data.total_pages;
		}

		loadingDashboard = false;
	}

	async function loadMoreActivities() {
		if (loadingMoreActivities || activitiesPage >= activitiesTotalPages) return;
		
		loadingMoreActivities = true;
		const nextPage = activitiesPage + 1;
		const response = await myApi.getMyActivities({ page: nextPage, per_page: 10 });

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			myActivities = [...myActivities, ...(response.data.items ?? [])];
			activitiesPage = response.data.page;
			activitiesTotalPages = response.data.total_pages;
		}

		loadingMoreActivities = false;
	}

	function goToWorkspace(workspaceId: string) {
		goto(`/workspace/${workspaceId}/projects`);
	}

	function goToIssue(workspaceId: string, projectId: string, issueId: string) {
		goto(`/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}`);
	}

	function openCreateModal() {
		createForm = { slug: '', name: '', description: '' };
		showCreateModal = true;
	}

	function generateSlug(name: string): string {
		return name
			.toLowerCase()
			.replace(/[^a-z0-9\s-]/g, '')
			.replace(/\s+/g, '-')
			.replace(/-+/g, '-')
			.trim();
	}

	function handleNameChange(name: string) {
		createForm.name = name;
		if (!createForm.slug || createForm.slug === generateSlug(createForm.name)) {
			createForm.slug = generateSlug(name);
		}
	}

	const handleWorkspaceNameInput: InputEventHandler = (event) => {
		handleNameChange(event.currentTarget.value);
	};

	async function handleCreate() {
		if (creating) return;

		if (!createForm.name.trim()) {
			toast.error(get(t)('workspace.enterName'));
			return;
		}

		if (!createForm.slug.trim()) {
			toast.error(get(t)('workspace.enterSlug'));
			return;
		}

		// Validate slug format
		if (!/^[a-z0-9-]+$/.test(createForm.slug)) {
			toast.error(get(t)('workspace.slugHint'));
			return;
		}

		creating = true;
		const response = await workspacesApi.create({
			slug: createForm.slug.trim(),
			name: createForm.name.trim(),
			description: createForm.description.trim() || undefined
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			toast.success(get(t)('workspace.createSuccess'));
			createForm = { slug: '', name: '', description: '' };
			showCreateModal = false;
			await loadWorkspaces();
		}

		creating = false;
	}

	function openEditModal(workspace: Workspace, event: Event) {
		event.stopPropagation();
		editingWorkspace = workspace;
		editForm = {
			name: workspace.name,
			description: workspace.description || ''
		};
		showEditModal = true;
	}

	async function handleUpdate() {
		if (!editingWorkspace || !editForm.name.trim()) {
			toast.error(get(t)('workspace.enterName'));
			return;
		}

		updating = true;
		const response = await workspacesApi.update(editingWorkspace.id, {
			name: editForm.name.trim(),
			description: editForm.description.trim() || undefined
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			toast.success(get(t)('workspace.updateSuccess'));
			showEditModal = false;
			await loadWorkspaces();
		}

		updating = false;
	}

	function openDeleteModal(workspace: Workspace, event: Event) {
		event.stopPropagation();
		deletingWorkspace = workspace;
		showDeleteModal = true;
	}

	async function handleDelete() {
		if (!deletingWorkspace) return;

		deleting = true;
		const response = await workspacesApi.delete(deletingWorkspace.id);

		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('workspace.deleteSuccess'));
			showDeleteModal = false;
			await loadWorkspaces();
		}

		deleting = false;
	}

	function formatDate(dateString: string): string {
		const currentLocale = get(locale) === 'en' ? 'en-US' : 'zh-CN';
		return new Date(dateString).toLocaleDateString(currentLocale, {
			year: 'numeric',
			month: 'long',
			day: 'numeric'
		});
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

	function getMemberDisplay(userId: string | undefined): string {
		if (!userId) {
			return '';
		}
		return userId.substring(0, 8);
	}

	function getSprintDisplay(sprintId: string | undefined): string {
		if (!sprintId) {
			return '';
		}
		return sprintId.substring(0, 8);
	}

	function getActivityAuthor(activity: MyActivity): string {
		return activity.author_name || getMemberDisplay(activity.user_id) || get(t)('common.user');
	}

	function formatActivity(activity: MyActivity): string {
		const detail = activity.detail || {};
		const from = typeof detail.from === 'string' ? detail.from : '';
		const to = typeof detail.to === 'string' ? detail.to : '';
		const toName = typeof detail.to_name === 'string' ? detail.to_name : '';
		const labelName = typeof detail.label_name === 'string' ? detail.label_name : '';

		switch (activity.action) {
			case 'assigned':
				if (to) {
					return get(t)('activity.assigned', { values: { name: toName || getMemberDisplay(to) } });
				}
				return get(t)('activity.unassigned');
			case 'status_changed':
				return get(t)('activity.statusChanged', { values: { from: formatStatus(from), to: formatStatus(to) } });
			case 'priority_changed':
				return get(t)('activity.priorityChanged', { values: { from: formatPriority(from), to: formatPriority(to) } });
			case 'sprint_changed':
				if (to) {
					return get(t)('activity.sprintChanged', { values: { name: toName || getSprintDisplay(to) } });
				}
				return get(t)('activity.sprintRemoved');
			case 'label_added':
				return get(t)('activity.labelAdded', { values: { name: labelName } });
			case 'label_removed':
				return get(t)('activity.labelRemoved', { values: { name: labelName } });
			case 'comment_added':
				return get(t)('activity.commentAdded');
			case 'comment_edited':
				return get(t)('activity.commentEdited');
			case 'comment_deleted': {
				const preview =
					typeof detail.content_preview === 'string' ? detail.content_preview.trim() : '';
				return preview
					? get(t)('activity.commentDeletedWithPreview', { values: { preview } })
					: get(t)('activity.commentDeleted');
			}
			default:
				return activity.action || get(t)('common.unknownAction');
		}
	}
</script>

<div class="mx-auto max-w-7xl">
	<!-- Header -->
	<div class="mb-8 flex items-center justify-between">
		<div>
			<h1 class="mb-2 text-3xl font-bold text-slate-900 dark:text-slate-100">
				{$t('workspace.welcome')}{#if $authStore.user}, {$authStore.user.name || $authStore.user.email}{/if}!
			</h1>
			<p class="text-slate-600 dark:text-slate-400">{$t('workspace.selectWorkspace')}</p>
		</div>
		<Button onclick={openCreateModal}>
			<svg class="w-5 h-5 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4"
				></path>
			</svg>
			{$t('workspace.createWorkspace')}
		</Button>
	</div>

	<!-- Loading State -->
	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		</div>

		<!-- Empty State -->
	{:else if workspaces.length === 0}
		<EmptyState icon="office" title={$t('workspace.none')} description={$t('workspace.noneDesc')}>
			{#snippet action()}
				<Button onclick={openCreateModal}>{$t('workspace.createWorkspace')}</Button>
			{/snippet}
		</EmptyState>

		<!-- Workspace Grid -->
	{:else}
		<div class="grid grid-cols-1 gap-6 md:grid-cols-2 lg:grid-cols-3">
			{#each workspaces as workspace}
				<Card clickable hover onclick={() => goToWorkspace(workspace.id)}>
					<div class="flex items-center justify-between mb-3">
						<div
							class="w-12 h-12 bg-gradient-to-br from-blue-500 to-purple-600 rounded-lg flex items-center justify-center text-white text-xl font-bold"
						>
							{workspace.name.charAt(0).toUpperCase()}
						</div>
						<div class="flex items-center gap-2">
							<button
								onclick={(e) => openEditModal(workspace, e)}
								class="p-1 text-slate-400 transition-colors hover:text-blue-600 dark:text-slate-400"
								aria-label={$t('common.edit')}
							>
								<svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
									></path>
								</svg>
							</button>
							<button
								onclick={(e) => openDeleteModal(workspace, e)}
								class="p-1 text-slate-400 transition-colors hover:text-red-600 dark:text-slate-400"
								aria-label={$t('workspace.deleteWorkspace')}
							>
								<svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
									></path>
								</svg>
							</button>
							<svg
								class="h-5 w-5 text-slate-400 transition-colors group-hover:text-blue-600 dark:text-slate-400"
								fill="none"
								stroke="currentColor"
								viewBox="0 0 24 24"
							>
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									stroke-width="2"
									d="M9 5l7 7-7 7"
								></path>
							</svg>
						</div>
					</div>
					<h3 class="mb-2 text-lg font-semibold text-slate-900 dark:text-slate-100">{workspace.name}</h3>
					{#if workspace.description}
						<p class="mb-3 line-clamp-2 text-sm text-slate-600 dark:text-slate-400">{workspace.description}</p>
					{/if}
					<div class="mt-4 border-t border-slate-100 pt-4 dark:border-slate-700">
						<div class="text-xs text-slate-500 dark:text-slate-400">
							{$t('common.createdAt')} {formatDate(workspace.created_at)}
						</div>
					</div>
				</Card>
			{/each}
		</div>
	{/if}

	{#if workspaces.length > 0}
		<div class="mt-12 grid grid-cols-1 gap-6 xl:grid-cols-5">
			<div class="xl:col-span-3">
				<Card>
					<div class="mb-4 flex items-center justify-between">
						<h3 class="flex items-center gap-2 text-lg font-semibold text-slate-900 dark:text-slate-100">
							<svg class="h-5 w-5 text-slate-500 dark:text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 20V10m5 10V4m5 16v-7M4 20h16"></path>
							</svg>
							{$t('workspace.myWork')} ({myIssues.length})
						</h3>
					</div>
					{#if loadingDashboard}
						<div class="py-8 text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
					{:else if myIssues.length === 0}
						<div class="py-8 text-sm text-slate-500 dark:text-slate-400">{$t('workspace.noWork')}</div>
					{:else}
						<div class="space-y-3">
							{#each myIssues as issue (issue.id)}
								<button
									class="w-full rounded-md border border-slate-200 bg-white p-3 text-left transition-colors hover:border-blue-300 hover:bg-blue-50/40 dark:border-slate-700 dark:bg-slate-900 dark:hover:bg-slate-800"
									onclick={() => goToIssue(issue.workspace_id, issue.project_id, issue.id)}
								>
									<div class="mb-2 flex items-center justify-between gap-2">
										<p class="line-clamp-1 text-sm font-medium text-slate-900 dark:text-slate-100">{issue.title}</p>
										<span class="text-xs text-slate-500 dark:text-slate-400">{formatRelativeTime(issue.updated_at)}</span>
									</div>
									<div class="flex flex-wrap items-center gap-2 text-xs">
										<span class={`rounded-full px-2 py-1 ${getStatusClass(issue.status)}`}>
											{formatStatus(issue.status)}
										</span>
										<span class={`rounded-full px-2 py-1 ${getPriorityClass(issue.priority)}`}>
											{formatPriority(issue.priority)}
										</span>
										<span class="text-slate-500 dark:text-slate-400">{issue.project_name}</span>
									</div>
								</button>
							{/each}
						</div>
					{/if}
				</Card>
			</div>
			<div class="xl:col-span-2">
				<Card>
					<div class="mb-4 flex items-center justify-between">
						<h3 class="flex items-center gap-2 text-lg font-semibold text-slate-900 dark:text-slate-100">
							<svg class="h-5 w-5 text-slate-500 dark:text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z"></path>
							</svg>
							{$t('workspace.recentActivity')}
						</h3>
					</div>
					{#if loadingDashboard}
						<div class="py-8 text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
					{:else if myActivities.length === 0}
						<div class="py-8 text-sm text-slate-500 dark:text-slate-400">{$t('workspace.noActivity')}</div>
					{:else}
						<div class="space-y-3">
							{#each myActivities as activity, index (activity.id)}
								<div class="flex items-start gap-3 py-1">
									<div class="flex min-h-10 flex-col items-center">
										<div class="h-2 w-2 rounded-full bg-blue-500"></div>
										<div class="w-0.5 flex-1 bg-slate-200 dark:bg-slate-700" class:opacity-0={index === myActivities.length - 1}></div>
									</div>
									<button
										class="flex-1 text-left"
										onclick={() => goToIssue(activity.workspace_id, activity.project_id, activity.issue_id)}
									>
										<p class="text-sm text-slate-700 dark:text-slate-300">
											<span class="font-medium">{getActivityAuthor(activity)}</span>
											<span class="ml-1">{formatActivity(activity)}</span>
										</p>
										<p class="text-xs text-slate-500 dark:text-slate-400">
											#{activity.issue_id.slice(0, 8)} · {activity.issue_title} · {formatRelativeTime(activity.created_at)}
										</p>
									</button>
								</div>
							{/each}
						</div>
						{#if activitiesPage < activitiesTotalPages}
							<div class="mt-4 flex justify-center">
								<button
									onclick={loadMoreActivities}
									disabled={loadingMoreActivities}
									class="rounded-md px-4 py-2 text-sm text-blue-600 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-400 dark:hover:bg-slate-800"
								>
									{loadingMoreActivities ? $t('common.loading') : '查看更多'}
								</button>
							</div>
						{/if}
					{/if}
				</Card>
			</div>
		</div>
	{/if}
</div>

<!-- Create Modal -->
<Modal bind:open={showCreateModal} title={$t('workspace.createWorkspace')}>
	<form onsubmit={(e) => { e.preventDefault(); handleCreate(); }} class="space-y-4">
		<Input
			label={$t('workspace.name')}
			placeholder={$t('workspace.namePlaceholder')}
			value={createForm.name}
			oninput={handleWorkspaceNameInput}
			required
			maxlength={50}
		/>
		<Input
			label={$t('workspace.slug')}
			placeholder={$t('workspace.slugPlaceholder')}
			bind:value={createForm.slug}
			required
			pattern="[a-z0-9-]+"
			maxlength={30}
			helperText={$t('workspace.slugHint')}
		/>
		<Input
			label={$t('workspace.description')}
			placeholder={$t('workspace.descriptionPlaceholder')}
			bind:value={createForm.description}
			maxlength={200}
		/>
	</form>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showCreateModal = false)}>{$t('common.cancel')}</Button>
		<Button type="button" loading={creating} onclick={handleCreate}>{$t('common.create')}</Button>
	{/snippet}
</Modal>

<!-- Edit Modal -->
<Modal bind:open={showEditModal} title={$t('workspace.editWorkspace')}>
	<form onsubmit={(e) => { e.preventDefault(); handleUpdate(); }} class="space-y-4">
		<Input
			label={$t('workspace.name')}
			placeholder={$t('workspace.namePlaceholder')}
			bind:value={editForm.name}
			required
			maxlength={50}
		/>
		<Input
			label={$t('workspace.description')}
			placeholder={$t('workspace.descriptionPlaceholder')}
			bind:value={editForm.description}
			maxlength={200}
		/>
	</form>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showEditModal = false)}>{$t('common.cancel')}</Button>
		<Button type="submit" loading={updating} onclick={handleUpdate}>{$t('common.save')}</Button>
	{/snippet}
</Modal>

<!-- Delete Modal -->
<Modal bind:open={showDeleteModal} title={$t('workspace.deleteWorkspace')}>
	<div class="space-y-4">
		<p class="text-slate-700 dark:text-slate-300">
			{$t('workspace.deleteConfirm', { values: { name: deletingWorkspace?.name } })}
		</p>
		<div class="rounded-md border border-red-200 bg-red-50 p-4 dark:border-red-500/40 dark:bg-red-500/20">
			<p class="inline-flex items-start gap-2 text-sm text-red-800 dark:text-red-300">
				<svg class="mt-0.5 h-4 w-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v3m0 4h.01M4.93 19h14.14a2 2 0 001.73-3L13.73 3a2 2 0 00-3.46 0L3.2 16a2 2 0 001.73 3z"></path>
				</svg>
				<span>{$t('workspace.deleteWarning')}</span>
			</p>
		</div>
	</div>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showDeleteModal = false)}>{$t('common.cancel')}</Button>
		<Button variant="danger" loading={deleting} onclick={handleDelete}>{$t('workspace.confirmDelete')}</Button>
	{/snippet}
</Modal>

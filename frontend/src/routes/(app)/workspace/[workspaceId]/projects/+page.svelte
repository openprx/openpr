<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { projectsApi, type Project } from '$lib/api/projects';
	import { workspacesApi, type Workspace } from '$lib/api/workspaces';
	import { toast } from '$lib/stores/toast';
	import { goto } from '$app/navigation';
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { requireRouteParam } from '$lib/utils/route-params';
	import { timeAgo } from '$lib/utils/timeago';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');

	let workspace = $state<Workspace | null>(null);
	let projects = $state<Project[]>([]);
	let filteredProjects = $state<Project[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let searchQuery = $state('');

	let showCreateModal = $state(false);
	let createForm = $state({ name: '', key: '', description: '' });
	let creating = $state(false);

	let showEditModal = $state(false);
	let editingProject = $state<Project | null>(null);
	let editForm = $state({ name: '', description: '' });
	let updating = $state(false);

	let showDeleteModal = $state(false);
	let deletingProject = $state<Project | null>(null);
	let deleting = $state(false);

	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		loading = true;
		error = null;

		const wsResponse = await workspacesApi.get(workspaceId);
		if (wsResponse.code !== 0) {
			error = wsResponse.message;
			toast.error(wsResponse.message);
		} else if (wsResponse.data) {
			workspace = wsResponse.data;
		}

		const response = await projectsApi.list(workspaceId);
		if (response.code !== 0) {
			error = response.message;
			toast.error(response.message);
		} else if (response.data) {
			projects = response.data.items ?? [];
			filteredProjects = projects;
		}

		loading = false;
	}

	$effect(() => {
		const query = searchQuery.toLowerCase().trim();
		if (!query) {
			filteredProjects = projects;
		} else {
			filteredProjects = projects.filter(
				(project) =>
					project.name.toLowerCase().includes(query) ||
					project.key.toLowerCase().includes(query) ||
					project.description?.toLowerCase().includes(query)
			);
		}
	});

	function goToProject(projectId: string) {
		goto(`/workspace/${workspaceId}/projects/${projectId}`);
	}

	function openCreateModal() {
		createForm = { name: '', key: '', description: '' };
		showCreateModal = true;
	}

	async function handleCreate() {
		if (!createForm.name.trim()) {
			toast.error(get(t)('project.enterName'));
			return;
		}

		if (!createForm.key.trim()) {
			toast.error(get(t)('project.enterKey'));
			return;
		}

		const keyPattern = /^[A-Z0-9]{2,10}$/;
		if (!keyPattern.test(createForm.key.toUpperCase())) {
			toast.error(get(t)('project.keyValidation'));
			return;
		}

		creating = true;
		const response = await projectsApi.create(workspaceId, {
			name: createForm.name.trim(),
			key: createForm.key.toUpperCase(),
			description: createForm.description.trim() || undefined
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			toast.success(get(t)('project.createSuccess'));
			showCreateModal = false;
			await loadData();
		}

		creating = false;
	}

	function openEditModal(project: Project, event: Event) {
		event.stopPropagation();
		editingProject = project;
		editForm = {
			name: project.name,
			description: project.description || ''
		};
		showEditModal = true;
	}

	async function handleUpdate() {
		if (!editingProject || !editForm.name.trim()) {
			toast.error(get(t)('project.enterName'));
			return;
		}

		updating = true;
		const response = await projectsApi.update(editingProject.id, {
			name: editForm.name.trim(),
			description: editForm.description.trim() || undefined
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			toast.success(get(t)('project.updateSuccess'));
			showEditModal = false;
			await loadData();
		}

		updating = false;
	}

	function openDeleteModal(project: Project, event: Event) {
		event.stopPropagation();
		deletingProject = project;
		showDeleteModal = true;
	}

	async function handleDelete() {
		if (!deletingProject) return;

		deleting = true;
		const response = await projectsApi.delete(deletingProject.id);

		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('project.deleteSuccess'));
			showDeleteModal = false;
			await loadData();
		}

		deleting = false;
	}

	function formatRelativeTime(dateText: string): string {
		return timeAgo(dateText);
	}

	function getIssueCount(project: Project, state: 'backlog' | 'todo' | 'in_progress' | 'done'): number {
		const counts = project.issue_counts;
		if (!counts) return 0;
		return counts[state] ?? 0;
	}
</script>

<div class="mx-auto max-w-7xl">
	<div class="mb-6">
		<nav class="mb-4 flex items-center gap-2 text-sm text-slate-600 dark:text-slate-300">
			<a href="/workspace" class="hover:text-blue-600">{$t('nav.myWorkspace')}</a>
			<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"></path>
			</svg>
			<span class="font-medium text-slate-900 dark:text-slate-100">{workspace?.name || $t('common.loading')}</span>
		</nav>

		<div class="flex items-center justify-between">
			<div>
				<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('project.title')}</h1>
				<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('project.count', { values: { count: filteredProjects.length } })}</p>
			</div>
			<Button onclick={openCreateModal}>
				<svg class="mr-2 h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4"></path>
				</svg>
				{$t('project.create')}
			</Button>
		</div>
	</div>

	<div class="mb-6 flex items-center gap-4">
		<div class="relative flex-1">
			<svg
				class="absolute left-3 top-1/2 h-5 w-5 -translate-y-1/2 text-slate-400"
				fill="none"
				stroke="currentColor"
				viewBox="0 0 24 24"
			>
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
				></path>
			</svg>
			<input
				type="search"
				placeholder={$t('project.searchPlaceholder')}
				bind:value={searchQuery}
				class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 py-2 pl-10 pr-4 text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 shadow-sm dark:shadow-slate-900/50 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			/>
		</div>
	</div>

	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		</div>
	{:else if error}
		<div class="rounded-md border border-red-200 bg-red-50 p-4">
			<p class="text-red-800">{error}</p>
		</div>
	{:else if projects.length === 0}
		<EmptyState icon="box" title={$t('project.none')} description={$t('project.noneDesc')}>
			{#snippet action()}
				<Button onclick={openCreateModal}>{$t('project.create')}</Button>
			{/snippet}
		</EmptyState>
	{:else if filteredProjects.length === 0}
		<EmptyState icon="search" title={$t('project.notFound')} description={$t('project.notFoundDesc')} />
	{:else}
		<div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
			{#each filteredProjects as project}
				<article class="group rounded-xl border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-5 shadow-sm dark:shadow-slate-900/50 transition-all hover:-translate-y-0.5 hover:shadow-md dark:shadow-slate-900/50">
					<div class="mb-3 flex items-start justify-between gap-3">
						<div class="min-w-0">
							<button
								type="button"
								onclick={() => goToProject(project.id)}
								class="truncate text-left text-lg font-semibold text-slate-900 dark:text-slate-100 transition-colors hover:text-blue-600"
							>
								{project.name}
							</button>
							<div class="mt-1">
								<span class="inline-flex items-center rounded-md bg-blue-50 px-2 py-0.5 text-xs font-semibold text-blue-700">
									{project.key}
								</span>
							</div>
						</div>
						<div class="flex items-center gap-1">
							<button
								onclick={(e) => openEditModal(project, e)}
								class="rounded-md p-1 text-slate-400 transition-colors hover:bg-slate-100 dark:hover:bg-slate-800 dark:bg-slate-800 hover:text-blue-600"
								aria-label={$t('common.edit')}
							>
								<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
									></path>
								</svg>
							</button>
							<button
								onclick={(e) => openDeleteModal(project, e)}
								class="rounded-md p-1 text-slate-400 transition-colors hover:bg-slate-100 dark:hover:bg-slate-800 dark:bg-slate-800 hover:text-red-600"
								aria-label={$t('common.delete')}
							>
								<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
									></path>
								</svg>
							</button>
						</div>
					</div>

					<p class="mb-4 min-h-10 text-sm text-slate-600 dark:text-slate-300 line-clamp-2">{project.description || $t('project.noDescription')}</p>

					<div class="mb-4 grid grid-cols-4 gap-2">
						<div class="rounded-lg bg-slate-100 dark:bg-slate-800 px-2 py-2 text-center text-slate-600 dark:text-slate-300">
							<div class="text-base font-semibold">{getIssueCount(project, 'backlog')}</div>
							<div class="text-[11px] font-medium">Backlog</div>
						</div>
						<div class="rounded-lg bg-blue-50 px-2 py-2 text-center text-blue-600">
							<div class="text-base font-semibold">{getIssueCount(project, 'todo')}</div>
							<div class="text-[11px] font-medium">To Do</div>
						</div>
						<div class="rounded-lg bg-amber-50 px-2 py-2 text-center text-amber-600">
							<div class="text-base font-semibold">{getIssueCount(project, 'in_progress')}</div>
							<div class="text-[11px] font-medium">In Progress</div>
						</div>
						<div class="rounded-lg bg-emerald-50 px-2 py-2 text-center text-emerald-600">
							<div class="text-base font-semibold">{getIssueCount(project, 'done')}</div>
							<div class="text-[11px] font-medium">Done</div>
						</div>
					</div>

					<div class="flex items-center justify-between gap-3 border-t border-slate-100 pt-3">
						<div class="flex flex-wrap items-center gap-2">
							<a
								href={`/workspace/${workspaceId}/projects/${project.id}/issues`}
								class="inline-flex items-center rounded-md border border-slate-200 dark:border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-700 dark:text-slate-300 transition-colors hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700"
							>
								{$t('project.workItems')}
							</a>
							<a
								href={`/workspace/${workspaceId}/projects/${project.id}/board`}
								class="inline-flex items-center rounded-md border border-slate-200 dark:border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-700 dark:text-slate-300 transition-colors hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700"
							>
								{$t('project.boardView')}
							</a>
							<a
								href={`/workspace/${workspaceId}/projects/${project.id}/cycles`}
								class="inline-flex items-center rounded-md border border-slate-200 dark:border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-700 dark:text-slate-300 transition-colors hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700"
							>
								{$t('project.sprintPlan')}
							</a>
						</div>
						<span class="text-xs text-slate-500 dark:text-slate-400">{$t('project.updatedAt')} {formatRelativeTime(project.updated_at)}</span>
					</div>
				</article>
			{/each}
		</div>
	{/if}
</div>

<Modal bind:open={showCreateModal} title={$t('project.create')}>
	<form onsubmit={(e) => { e.preventDefault(); handleCreate(); }} class="space-y-4">
		<Input
			label={$t('project.name')}
			placeholder={$t('project.namePlaceholder')}
			bind:value={createForm.name}
			required
			maxlength={100}
		/>
		<Input
			label={$t('project.key')}
			placeholder={$t('project.keyPlaceholder')}
			bind:value={createForm.key}
			oninput={(e) => { const t = e.currentTarget; t.value = t.value.toUpperCase(); createForm.key = t.value; }}
			required
			maxlength={10}
			hint={$t('project.keyHint')}
		/>
		<div class="space-y-1">
			<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="create-project-description">{$t('project.description')}</label>
			<textarea
				id="create-project-description"
				class="block w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 shadow-sm dark:shadow-slate-900/50 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 resize-y"
				placeholder={$t('project.descriptionPlaceholder')}
				bind:value={createForm.description}
				maxlength={350}
				rows="3"
			></textarea>
			<p class="text-xs text-slate-400 text-right">{createForm.description.length}/350</p>
		</div>
	</form>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showCreateModal = false)}>{$t('common.cancel')}</Button>
		<Button type="submit" loading={creating} onclick={handleCreate}>{$t('common.create')}</Button>
	{/snippet}
</Modal>

<Modal bind:open={showEditModal} title={$t('project.editProject')}>
	<form onsubmit={(e) => { e.preventDefault(); handleUpdate(); }} class="space-y-4">
		<Input
			label={$t('project.name')}
			placeholder={$t('project.namePlaceholder')}
			bind:value={editForm.name}
			required
			maxlength={100}
		/>
		<div class="space-y-1">
			<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="edit-project-description">{$t('project.description')}</label>
			<textarea
				id="edit-project-description"
				class="block w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 shadow-sm dark:shadow-slate-900/50 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 resize-y"
				placeholder={$t('project.descriptionPlaceholder')}
				bind:value={editForm.description}
				maxlength={350}
				rows="3"
			></textarea>
			<p class="text-xs text-slate-400 text-right">{editForm.description.length}/350</p>
		</div>
		<div class="text-sm text-slate-500 dark:text-slate-400">
			<strong>{$t('project.note')}:</strong> {$t('project.keyImmutable', { values: { key: editingProject?.key } })}
		</div>
	</form>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showEditModal = false)}>{$t('common.cancel')}</Button>
		<Button type="submit" loading={updating} onclick={handleUpdate}>{$t('common.save')}</Button>
	{/snippet}
</Modal>

<Modal bind:open={showDeleteModal} title={$t('project.deleteProject')}>
	<div class="space-y-4">
		<p class="text-slate-700 dark:text-slate-300">
			{$t('project.deleteConfirm', { values: { name: deletingProject?.name, key: deletingProject?.key } })}
		</p>
		<div class="rounded-md border border-red-200 bg-red-50 p-4">
			<p class="inline-flex items-start gap-2 text-sm text-red-800">
				<svg class="mt-0.5 h-4 w-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v3m0 4h.01M4.93 19h14.14a2 2 0 001.73-3L13.73 3a2 2 0 00-3.46 0L3.2 16a2 2 0 001.73 3z"></path>
				</svg>
				<span>{$t('project.deleteWarning')}</span>
			</p>
		</div>
	</div>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showDeleteModal = false)}>{$t('common.cancel')}</Button>
		<Button variant="danger" loading={deleting} onclick={handleDelete}>{$t('workspace.confirmDelete')}</Button>
	{/snippet}
</Modal>

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { projectsApi, type Project } from '$lib/api/projects';
	import { issuesApi, type Issue } from '$lib/api/issues';
	import { importExportApi } from '$lib/api/import-export';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';
	import { requireRouteParam } from '$lib/utils/route-params';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam($page.params.projectId, 'projectId');

	let project = $state<Project | null>(null);
	let recentIssues = $state<Issue[]>([]);
	let loading = $state(true);
	let exporting = $state(false);
	let importing = $state(false);

	onMount(async () => {
		await Promise.all([loadProject(), loadRecentIssues()]);
		loading = false;
	});

	async function loadProject() {
		const response = await projectsApi.get(projectId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			project = response.data;
		}
	}

	async function loadRecentIssues() {
		const response = await issuesApi.list(projectId, { page: 1, per_page: 5, sort_by: 'updated_at', sort_order: 'desc' });
		if (response.data) {
			recentIssues = response.data.items ?? [];
		}
	}

	async function handleExportProject() {
		exporting = true;
		const response = await importExportApi.exportProject(projectId);
		if ((response.code !== 0) || !response.data) {
			toast.error(response.message || get(t)('project.exportFailed'));
			exporting = false;
			return;
		}

		const payload = JSON.stringify(response.data, null, 2);
		const blob = new Blob([payload], { type: 'application/json' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${project?.key ?? 'project'}-export.json`;
		document.body.append(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);

		toast.success(get(t)('project.exportSuccess'));
		exporting = false;
	}

	async function handleImportFile(event: Event) {
		const target = event.currentTarget as HTMLInputElement;
		const file = target.files?.[0];
		if (!file) {
			return;
		}

		importing = true;
		try {
			const content = await file.text();
			const payload = JSON.parse(content) as unknown;
			const response = await importExportApi.importProject(workspaceId, payload);
			if (response.code !== 0) {
				toast.error(response.message);
			} else {
				toast.success(get(t)('project.importSuccess'));
			}
		} catch {
			toast.error(get(t)('project.importInvalidFile'));
		} finally {
			target.value = '';
			importing = false;
		}
	}
</script>

<div class="max-w-7xl mx-auto">
	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		</div>
	{:else if project}
		<div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<div class="mb-2 flex items-center gap-3">
					<span class="rounded bg-slate-100 dark:bg-slate-800 px-2 py-1 text-sm text-slate-600 dark:text-slate-300">{project.key}</span>
					<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{project.name}</h1>
				</div>
				{#if project.description}
					<p class="text-slate-600 dark:text-slate-300">{project.description}</p>
				{/if}
			</div>
			<div class="flex flex-wrap gap-2">
				<Button variant="secondary" loading={exporting} onclick={handleExportProject}>{$t('project.exportJson')}</Button>
				<label class="inline-flex cursor-pointer items-center justify-center rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700">
					{importing ? $t('project.importingJson') : $t('project.importJson')}
					<input
						type="file"
						accept="application/json"
						class="hidden"
						disabled={importing}
						onchange={handleImportFile}
					/>
				</label>
			</div>
		</div>

		<div class="mb-8 grid grid-cols-1 gap-4 md:grid-cols-3">
			<a
				href="/workspace/{workspaceId}/projects/{projectId}/issues"
				class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 transition-shadow hover:shadow-md dark:shadow-slate-900/50"
			>
				<h3 class="mb-2 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('project.workItems')}</h3>
				<p class="text-sm text-slate-600 dark:text-slate-300">{$t('project.workItemsDesc')}</p>
			</a>

			<a
				href="/workspace/{workspaceId}/projects/{projectId}/board"
				class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 transition-shadow hover:shadow-md dark:shadow-slate-900/50"
			>
				<h3 class="mb-2 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('project.boardView')}</h3>
				<p class="text-sm text-slate-600 dark:text-slate-300">{$t('project.boardViewDesc')}</p>
			</a>

			<a
				href="/workspace/{workspaceId}/projects/{projectId}/cycles"
				class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 transition-shadow hover:shadow-md dark:shadow-slate-900/50"
			>
				<h3 class="mb-2 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('project.sprintPlan')}</h3>
				<p class="text-sm text-slate-600 dark:text-slate-300">{$t('project.sprintPlanDesc')}</p>
			</a>
		</div>

		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6">
			<h2 class="mb-4 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('project.recentUpdates')}</h2>
			{#if recentIssues.length === 0}
				<p class="text-slate-500 dark:text-slate-400">{$t('project.noRecentWorkItems')}</p>
			{:else}
				<div class="space-y-3">
					{#each recentIssues as issue}
						<a
							href="/workspace/{workspaceId}/projects/{projectId}/issues/{issue.id}"
							class="block rounded-md p-3 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
						>
							<div class="space-y-1.5">
								<div class="flex items-center gap-2">
									<span class="font-mono text-sm text-slate-500 dark:text-slate-400">{issue.key}</span>
									<span class="text-sm font-medium text-slate-900 dark:text-slate-100">{issue.title}</span>
									<span
										class="ml-auto rounded px-2 py-1 text-xs {issue.status === 'done'
											? 'bg-green-100 text-green-700'
											: 'bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-300'}"
									>
										{issue.status}
									</span>
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
							</div>
						</a>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
</div>

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { governanceApi, type GovernanceAuditLog } from '$lib/api/governance';
	import { toast } from '$lib/stores/toast';
	import { projectOptionsStore } from '$lib/stores/project-options';
	import { groupProjectOptionsByWorkspace } from '$lib/utils/project-options';

	let loading = $state(true);
	const projects = $derived($projectOptionsStore.items);
	const groupedProjects = $derived.by(() => groupProjectOptionsByWorkspace(projects));
	let logs = $state<GovernanceAuditLog[]>([]);
	let selectedProjectId = $state('');
	let action = $state('');
	let resourceType = $state('');
	let startAt = $state('');
	let endAt = $state('');
	let page = $state(1);
	let perPage = $state(20);
	let total = $state(0);
	let totalPages = $state(0);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		await projectOptionsStore.ensureLoaded();
		if (projects.length > 0) {
			selectedProjectId = projects[0].id;
		}
		await loadLogs();
		loading = false;
	}

	async function loadLogs() {
		const res = await governanceApi.listAuditLogs({
			project_id: selectedProjectId || undefined,
			action: action.trim() || undefined,
			resource_type: resourceType.trim() || undefined,
			start_at: startAt ? new Date(startAt).toISOString() : undefined,
			end_at: endAt ? new Date(endAt).toISOString() : undefined,
			page,
			per_page: perPage
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceAuditLogs.loadFailed'));
			logs = [];
			total = 0;
			totalPages = 0;
			return;
		}
		logs = res.data.items ?? [];
		total = res.data.total;
		totalPages = res.data.total_pages;
	}

	async function onSearch() {
		page = 1;
		await loadLogs();
	}

	async function prevPage() {
		if (page <= 1) return;
		page -= 1;
		await loadLogs();
	}

	async function nextPage() {
		if (totalPages > 0 && page >= totalPages) return;
		page += 1;
		await loadLogs();
	}

	function renderJson(value: unknown): string {
		if (value === null || value === undefined) return '-';
		try {
			return JSON.stringify(value);
		} catch {
			return '-';
		}
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.governanceAuditLogs')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceAuditLogs.title')}</h1>
		<p class="mt-1 text-sm text-slate-500">{$t('governanceAuditLogs.subtitle')}</p>
	</div>

	<div class="grid grid-cols-1 gap-3 rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800 md:grid-cols-6">
		<div class="md:col-span-2">
			<label for="audit-log-project" class="mb-1 block text-sm text-slate-600">{$t('impactReview.project')}</label>
			<select id="audit-log-project" bind:value={selectedProjectId} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
				<option value="">{`- ${$t('impactReview.allProjects')} -`}</option>
				{#each groupedProjects as group}
					<optgroup label={group.workspaceName}>
						{#each group.items as project}
							<option value={project.id}>{project.name}</option>
						{/each}
					</optgroup>
				{/each}
			</select>
		</div>
		<div>
			<label for="audit-log-action" class="mb-1 block text-sm text-slate-600">{$t('governanceAuditLogs.action')}</label>
			<input id="audit-log-action" bind:value={action} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div>
			<label for="audit-log-resource-type" class="mb-1 block text-sm text-slate-600">{$t('governanceAuditLogs.resourceType')}</label>
			<input id="audit-log-resource-type" bind:value={resourceType} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div>
			<label for="audit-log-start-at" class="mb-1 block text-sm text-slate-600">{$t('governanceAuditLogs.startAt')}</label>
			<input id="audit-log-start-at" bind:value={startAt} type="datetime-local" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div>
			<label for="audit-log-end-at" class="mb-1 block text-sm text-slate-600">{$t('governanceAuditLogs.endAt')}</label>
			<input id="audit-log-end-at" bind:value={endAt} type="datetime-local" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div class="md:col-span-6">
			<button type="button" onclick={onSearch} class="rounded-md bg-blue-600 px-4 py-2 text-sm text-white hover:bg-blue-700">
				{$t('common.search')}
			</button>
		</div>
	</div>

	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
		{#if loading}
			<p class="text-sm text-slate-500">{$t('common.loading')}</p>
		{:else if logs.length === 0}
			<p class="text-sm text-slate-500">{$t('governanceAuditLogs.empty')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full text-left text-sm">
					<thead>
						<tr class="border-b border-slate-200 text-xs text-slate-500 dark:border-slate-700">
							<th class="px-2 py-2">ID</th>
							<th class="px-2 py-2">{$t('common.createdAt')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.action')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.resourceType')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.actor')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.resourceId')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.changes')}</th>
						</tr>
					</thead>
					<tbody>
						{#each logs as item}
							<tr class="border-b border-slate-100 align-top dark:border-slate-800">
								<td class="px-2 py-2 text-xs text-slate-500">{item.id}</td>
								<td class="px-2 py-2 text-xs text-slate-500">{new Date(item.created_at).toLocaleString()}</td>
								<td class="px-2 py-2 text-xs">{item.action}</td>
								<td class="px-2 py-2 text-xs">{item.resource_type}</td>
								<td class="px-2 py-2 text-xs">{item.actor_id || '-'}</td>
								<td class="px-2 py-2 text-xs">{item.resource_id || '-'}</td>
								<td class="max-w-xl px-2 py-2 text-xs text-slate-500">
									<div class="space-y-1">
										<p><span class="font-medium">old:</span> {renderJson(item.old_value)}</p>
										<p><span class="font-medium">new:</span> {renderJson(item.new_value)}</p>
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		<div class="mt-4 flex items-center justify-between border-t border-slate-200 pt-3 text-sm dark:border-slate-700">
			<p class="text-slate-500">{$t('governance.pagination', { values: { total, page, pages: totalPages || 1 } })}</p>
			<div class="flex gap-2">
				<button type="button" onclick={prevPage} class="rounded border border-slate-300 px-3 py-1.5 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900" disabled={page <= 1}>
					{$t('governance.prevPage')}
				</button>
				<button type="button" onclick={nextPage} class="rounded border border-slate-300 px-3 py-1.5 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900" disabled={totalPages > 0 && page >= totalPages}>
					{$t('governance.nextPage')}
				</button>
			</div>
		</div>
	</div>
</div>

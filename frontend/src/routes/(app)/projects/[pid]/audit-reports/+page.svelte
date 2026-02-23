<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { projectsApi } from '$lib/api/projects';
	import { governanceExtApi, type AuditReport } from '$lib/api/governance-ext';
	import { toast } from '$lib/stores/toast';

	const projectId = $derived($page.params.pid || '');

	let loading = $state(true);
	let generating = $state(false);
	let projectName = $state('');
	let reports = $state<AuditReport[]>([]);
	let periodStart = $state('');
	let periodEnd = $state('');
	let pageNum = $state(1);
	let perPage = $state(20);
	let total = $state(0);
	let totalPages = $state(0);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		const projectRes = await projectsApi.get(projectId);
		if (projectRes.code === 0 && projectRes.data) {
			projectName = projectRes.data.name;
		}
		await loadReports();
		loading = false;
	}

	async function loadReports() {
		const res = await governanceExtApi.listProjectAuditReports(projectId, {
			page: pageNum,
			per_page: perPage
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.auditLoadFailed'));
			reports = [];
			total = 0;
			totalPages = 0;
			return;
		}
		reports = res.data.items;
		total = res.data.total;
		totalPages = res.data.total_pages;
	}

	async function generateReport() {
		generating = true;
		const res = await governanceExtApi.createProjectAuditReport(projectId, {
			period_start: periodStart || undefined,
			period_end: periodEnd || undefined
		});
		generating = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.auditGenerateFailed'));
			return;
		}
		toast.success(get(t)('governanceExt.auditGenerateSuccess'));
		pageNum = 1;
		await loadReports();
	}

	async function goPage(next: number) {
		pageNum = Math.max(1, Math.min(next, totalPages || 1));
		await loadReports();
	}
 </script>

<svelte:head>
	<title>{$t('pageTitle.auditReports')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.auditTitle')}</h1>
		<p class="mt-1 text-sm text-slate-500">
			{$t('governanceExt.auditSubtitle', { values: { project: projectName || projectId } })}
		</p>
	</div>

	<div class="grid grid-cols-1 gap-3 rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800 md:grid-cols-4">
		<div>
			<label for="audit-period-start" class="mb-1 block text-sm text-slate-600">{$t('governanceExt.periodStart')}</label>
			<input id="audit-period-start" bind:value={periodStart} type="date" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div>
			<label for="audit-period-end" class="mb-1 block text-sm text-slate-600">{$t('governanceExt.periodEnd')}</label>
			<input id="audit-period-end" bind:value={periodEnd} type="date" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div class="md:col-span-2 flex items-end">
			<button type="button" onclick={generateReport} disabled={generating} class="w-full rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700 disabled:opacity-60">
				{generating ? $t('common.processing') : $t('governanceExt.generateReport')}
			</button>
		</div>
	</div>

	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
		{#if loading}
			<p class="text-sm text-slate-500">{$t('common.loading')}</p>
		{:else if reports.length === 0}
			<p class="text-sm text-slate-500">{$t('common.noData')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full text-left text-sm">
					<thead>
						<tr class="border-b border-slate-200 text-xs text-slate-500 dark:border-slate-700">
							<th class="px-2 py-2">{$t('common.id')}</th>
							<th class="px-2 py-2">{$t('governanceExt.period')}</th>
							<th class="px-2 py-2">{$t('governanceExt.totalDecisions')}</th>
							<th class="px-2 py-2">{$t('governanceExt.passRate')}</th>
							<th class="px-2 py-2">{$t('common.createdAt')}</th>
							<th class="px-2 py-2">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each reports as report}
							<tr class="border-b border-slate-100 dark:border-slate-800">
								<td class="px-2 py-2 text-xs text-slate-500">{report.id}</td>
								<td class="px-2 py-2 text-xs">
									{report.period_start} ~ {report.period_end}
								</td>
								<td class="px-2 py-2 text-xs">{report.total_proposals}</td>
								<td class="px-2 py-2 text-xs">
									{report.total_proposals > 0
										? `${((report.approved_proposals / report.total_proposals) * 100).toFixed(1)}%`
										: '0.0%'}
								</td>
								<td class="px-2 py-2 text-xs text-slate-500">{new Date(report.generated_at).toLocaleString()}</td>
								<td class="px-2 py-2 text-xs">
									<a href={`/projects/${projectId}/audit-reports/${report.id}`} class="text-blue-600 hover:underline">
										{$t('governanceExt.viewDetail')}
									</a>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		<div class="mt-4 flex items-center justify-between border-t border-slate-200 pt-3 text-sm dark:border-slate-700">
			<p class="text-slate-500">{$t('governance.pagination', { values: { total, page: pageNum, pages: totalPages || 1 } })}</p>
			<div class="flex gap-2">
				<button type="button" onclick={() => goPage(pageNum - 1)} disabled={pageNum <= 1} class="rounded border border-slate-300 px-3 py-1.5 text-xs hover:bg-slate-50 disabled:opacity-60 dark:border-slate-600 dark:hover:bg-slate-900">
					{$t('governance.prevPage')}
				</button>
				<button type="button" onclick={() => goPage(pageNum + 1)} disabled={totalPages > 0 && pageNum >= totalPages} class="rounded border border-slate-300 px-3 py-1.5 text-xs hover:bg-slate-50 disabled:opacity-60 dark:border-slate-600 dark:hover:bg-slate-900">
					{$t('governance.nextPage')}
				</button>
			</div>
		</div>
	</div>
</div>

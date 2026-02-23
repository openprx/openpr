<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { governanceExtApi, type AuditReport } from '$lib/api/governance-ext';
	import { toast } from '$lib/stores/toast';

	const projectId = $derived($page.params.pid || '');
	const reportId = $derived($page.params.id || '');

	let loading = $state(true);
	let report = $state<AuditReport | null>(null);

	const reportTitle = $derived.by(() => {
		if (report?.id) {
			return $t('pageTitle.auditReportDetailWithId', { values: { id: report.id } });
		}
		return $t('pageTitle.auditReportDetail');
	});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		const res = await governanceExtApi.getProjectAuditReport(projectId, reportId);
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.auditLoadFailed'));
			loading = false;
			return;
		}
		report = res.data;
		loading = false;
	}

	function jsonText(value: unknown): string {
		if (value === null || value === undefined) {
			return '-';
		}
		try {
			return JSON.stringify(value, null, 2);
		} catch {
			return '-';
		}
	}
 </script>

<svelte:head>
	<title>{reportTitle}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<div class="flex items-center justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.auditDetailTitle')}</h1>
				<p class="mt-1 text-sm text-slate-500">{reportId}</p>
			</div>
			<a href={`/projects/${projectId}/audit-reports`} class="rounded-md border border-slate-300 px-3 py-2 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900">
				{$t('governanceExt.backToList')}
			</a>
		</div>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-800">
			{$t('common.loading')}
		</div>
	{:else if report}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-4">
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.totalDecisions')}</p>
				<p class="mt-1 text-2xl font-semibold text-slate-900 dark:text-slate-100">{report.total_proposals}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governance.status.approved')}</p>
				<p class="mt-1 text-2xl font-semibold text-emerald-700">{report.approved_proposals}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governance.status.rejected')}</p>
				<p class="mt-1 text-2xl font-semibold text-red-700">{report.rejected_proposals}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governance.status.vetoed')}</p>
				<p class="mt-1 text-2xl font-semibold text-rose-700">{report.vetoed_proposals}</p>
			</div>
		</div>

		<div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h2 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.ratingDistribution')}</h2>
				<pre class="overflow-auto rounded-md bg-slate-50 p-3 text-xs dark:bg-slate-900">{jsonText(report.rating_distribution)}</pre>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h2 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.aiParticipation')}</h2>
				<pre class="overflow-auto rounded-md bg-slate-50 p-3 text-xs dark:bg-slate-900">{jsonText(report.ai_participation_stats)}</pre>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h2 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.domainStats')}</h2>
				<pre class="overflow-auto rounded-md bg-slate-50 p-3 text-xs dark:bg-slate-900">{jsonText(report.domain_stats)}</pre>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h2 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.keyInsights')}</h2>
				<pre class="overflow-auto rounded-md bg-slate-50 p-3 text-xs dark:bg-slate-900">{jsonText(report.key_insights)}</pre>
			</div>
		</div>
	{/if}
</div>

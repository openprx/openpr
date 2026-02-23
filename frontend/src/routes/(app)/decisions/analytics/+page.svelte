<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { governanceExtApi, type DecisionAnalytics } from '$lib/api/governance-ext';
	import { projectOptionsStore } from '$lib/stores/project-options';
	import { toast } from '$lib/stores/toast';
	import { groupProjectOptionsByWorkspace } from '$lib/utils/project-options';

	let loading = $state(true);
	const projects = $derived($projectOptionsStore.items);
	const groupedProjects = $derived.by(() => groupProjectOptionsByWorkspace(projects));
	let selectedProjectId = $state('');
	let startAt = $state('');
	let endAt = $state('');
	let analytics = $state<DecisionAnalytics | null>(null);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		await projectOptionsStore.ensureLoaded();
		if (projects.length > 0) {
			selectedProjectId = projects[0].id;
		}
		await loadAnalytics();
		loading = false;
	}

	async function loadAnalytics() {
		const res = await governanceExtApi.getDecisionAnalytics({
			project_id: selectedProjectId || undefined,
			start_at: startAt ? new Date(startAt).toISOString() : undefined,
			end_at: endAt ? new Date(endAt).toISOString() : undefined
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.analyticsLoadFailed'));
			analytics = null;
			return;
		}
		analytics = res.data;
	}

	function pct(value: number): string {
		return `${(value * 100).toFixed(1)}%`;
	}

	function bar(value: number): string {
		return `width: ${Math.max(0, Math.min(100, value * 100))}%`;
	}
 </script>

<svelte:head>
	<title>{$t('pageTitle.decisionAnalytics')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.analyticsTitle')}</h1>
		<p class="mt-1 text-sm text-slate-500">{$t('governanceExt.analyticsSubtitle')}</p>
	</div>

	<div class="grid grid-cols-1 gap-3 rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800 md:grid-cols-4">
		<div>
			<label for="analytics-project" class="mb-1 block text-sm text-slate-600">{$t('impactReview.project')}</label>
			<select id="analytics-project" bind:value={selectedProjectId} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
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
			<label for="analytics-start" class="mb-1 block text-sm text-slate-600">{$t('governanceAuditLogs.startAt')}</label>
			<input id="analytics-start" bind:value={startAt} type="datetime-local" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div>
			<label for="analytics-end" class="mb-1 block text-sm text-slate-600">{$t('governanceAuditLogs.endAt')}</label>
			<input id="analytics-end" bind:value={endAt} type="datetime-local" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div class="flex items-end">
			<button type="button" onclick={loadAnalytics} class="w-full rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700">
				{$t('common.search')}
			</button>
		</div>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-800">
			{$t('common.loading')}
		</div>
	{:else if analytics}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-4">
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.totalDecisions')}</p>
				<p class="mt-1 text-2xl font-semibold text-slate-900 dark:text-slate-100">{analytics.overview.total_decisions}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.passRate')}</p>
				<p class="mt-1 text-2xl font-semibold text-emerald-700">{pct(analytics.overview.pass_rate)}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.avgCycleHours')}</p>
				<p class="mt-1 text-2xl font-semibold text-slate-900 dark:text-slate-100">{(analytics.overview.avg_cycle_hours ?? 0).toFixed(1)}h</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.vetoedCount')}</p>
				<p class="mt-1 text-2xl font-semibold text-rose-700">{analytics.overview.vetoed_count}</p>
			</div>
		</div>

		<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
			<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.approvalChart')}</h2>
			<div class="space-y-2 text-xs">
				<div>
					<div class="mb-1 flex items-center justify-between">
						<span>{$t('governance.status.approved')}</span>
						<span>{analytics.overview.approved_count}</span>
					</div>
					<div class="h-2 rounded bg-slate-100">
						<div class="h-2 rounded bg-emerald-500" style={bar(analytics.overview.total_decisions > 0 ? analytics.overview.approved_count / analytics.overview.total_decisions : 0)}></div>
					</div>
				</div>
				<div>
					<div class="mb-1 flex items-center justify-between">
						<span>{$t('governance.status.rejected')}</span>
						<span>{analytics.overview.rejected_count}</span>
					</div>
					<div class="h-2 rounded bg-slate-100">
						<div class="h-2 rounded bg-red-500" style={bar(analytics.overview.total_decisions > 0 ? analytics.overview.rejected_count / analytics.overview.total_decisions : 0)}></div>
					</div>
				</div>
				<div>
					<div class="mb-1 flex items-center justify-between">
						<span>{$t('governance.status.vetoed')}</span>
						<span>{analytics.overview.vetoed_count}</span>
					</div>
					<div class="h-2 rounded bg-slate-100">
						<div class="h-2 rounded bg-rose-500" style={bar(analytics.overview.total_decisions > 0 ? analytics.overview.vetoed_count / analytics.overview.total_decisions : 0)}></div>
					</div>
				</div>
			</div>
		</div>

		<div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.byType')}</h2>
				<div class="overflow-x-auto">
					<table class="min-w-full text-left text-xs">
						<thead>
							<tr class="border-b border-slate-200 text-slate-500 dark:border-slate-700">
								<th class="px-2 py-2">{$t('governance.typeLabel')}</th>
								<th class="px-2 py-2">{$t('common.total')}</th>
								<th class="px-2 py-2">{$t('governanceExt.passRate')}</th>
								<th class="px-2 py-2">{$t('governanceExt.avgCycleHours')}</th>
							</tr>
						</thead>
						<tbody>
							{#each analytics.by_type as row}
								<tr class="border-b border-slate-100 dark:border-slate-800">
									<td class="px-2 py-2">{row.proposal_type}</td>
									<td class="px-2 py-2">{row.total_decisions}</td>
									<td class="px-2 py-2">{pct(row.pass_rate)}</td>
									<td class="px-2 py-2">{(row.avg_cycle_hours ?? 0).toFixed(1)}h</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</div>

			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.byDomain')}</h2>
				<div class="overflow-x-auto">
					<table class="min-w-full text-left text-xs">
						<thead>
							<tr class="border-b border-slate-200 text-slate-500 dark:border-slate-700">
								<th class="px-2 py-2">{$t('governance.domainLabel')}</th>
								<th class="px-2 py-2">{$t('common.total')}</th>
								<th class="px-2 py-2">{$t('governanceExt.passRate')}</th>
								<th class="px-2 py-2">{$t('governanceExt.avgCycleHours')}</th>
							</tr>
						</thead>
						<tbody>
							{#each analytics.by_domain as row}
								<tr class="border-b border-slate-100 dark:border-slate-800">
									<td class="px-2 py-2">{row.domain}</td>
									<td class="px-2 py-2">{row.total_decisions}</td>
									<td class="px-2 py-2">{pct(row.pass_rate)}</td>
									<td class="px-2 py-2">{(row.avg_cycle_hours ?? 0).toFixed(1)}h</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</div>
		</div>
	{/if}
</div>

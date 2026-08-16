<script lang="ts">
	import { Activity, RefreshCw } from '@lucide/svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import {
		operationLogsApi,
		type BotOperationLog,
		type BotOperationOutcome
	} from '$lib/api/operation-logs';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import { locale, t } from 'svelte-i18n';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const pageSize = 50;

	let logs = $state<BotOperationLog[]>([]);
	let loading = $state(true);
	let botIdFilter = $state('');
	let toolNameFilter = $state('');
	let outcomeFilter = $state<BotOperationOutcome | ''>('');
	let cursorHistory = $state<Array<string | null>>([null]);
	let pageIndex = $state(0);
	let nextCursor = $state<string | null>(null);

	onMount(() => {
		void loadLogs();
	});

	async function loadLogs(cursor: string | null = cursorHistory[pageIndex] ?? null) {
		loading = true;
		const response = await operationLogsApi.list(workspaceId, {
			bot_id: botIdFilter.trim() || undefined,
			tool_name: toolNameFilter.trim() || undefined,
			outcome: outcomeFilter || undefined,
			cursor: cursor || undefined,
			limit: pageSize
		});
		if (response.code !== 0 || !response.data) {
			logs = [];
			nextCursor = null;
			toast.error(response.message);
		} else {
			logs = response.data.items;
			nextCursor = response.data.next_cursor;
		}
		loading = false;
	}

	function applyFilters() {
		cursorHistory = [null];
		pageIndex = 0;
		void loadLogs(null);
	}

	function resetFilters() {
		botIdFilter = '';
		toolNameFilter = '';
		outcomeFilter = '';
		applyFilters();
	}

	function previousPage() {
		if (pageIndex === 0) return;
		pageIndex -= 1;
		void loadLogs(cursorHistory[pageIndex] ?? null);
	}

	function nextPage() {
		if (!nextCursor) return;
		cursorHistory = [...cursorHistory.slice(0, pageIndex + 1), nextCursor];
		pageIndex += 1;
		void loadLogs(nextCursor);
	}

	function formatDate(value: string): string {
		return new Date(value).toLocaleString($locale === 'en' ? 'en-US' : 'zh-CN');
	}

	function botLabel(log: BotOperationLog): string {
		return log.bot_name || log.bot_id.slice(0, 8);
	}
</script>

<svelte:head>
	<title>{$t('connections.title')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-6">
	<header class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<div class="flex items-center gap-2">
				<Activity class="h-6 w-6 text-cyan-700 dark:text-cyan-300" aria-hidden="true" />
				<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('connections.title')}</h1>
			</div>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-400">{$t('connections.subtitle')}</p>
		</div>
		<Button variant="secondary" onclick={() => loadLogs()} disabled={loading}>
			<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('connections.refresh')}
		</Button>
	</header>

	<section class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm dark:border-slate-700 dark:bg-slate-900">
		<div class="grid gap-3 md:grid-cols-[1fr_1fr_180px_auto] md:items-end">
			<div>
				<label for="operation-bot-filter" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('connections.bot')}</label>
				<Input id="operation-bot-filter" bind:value={botIdFilter} placeholder={$t('connections.botFilterPlaceholder')} />
			</div>
			<div>
				<label for="operation-tool-filter" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('connections.tool')}</label>
				<Input id="operation-tool-filter" bind:value={toolNameFilter} placeholder="form_records.list" />
			</div>
			<div>
				<label for="operation-outcome-filter" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('connections.result')}</label>
				<select id="operation-outcome-filter" bind:value={outcomeFilter} class="h-10 w-full rounded-md border border-slate-300 bg-white px-3 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
					<option value="">{$t('connections.allResults')}</option>
					<option value="ok">{$t('connections.resultOk')}</option>
					<option value="error">{$t('connections.resultError')}</option>
				</select>
			</div>
			<div class="flex gap-2">
				<Button onclick={applyFilters} disabled={loading}>{$t('connections.applyFilters')}</Button>
				<Button variant="secondary" onclick={resetFilters} disabled={loading}>{$t('connections.resetFilters')}</Button>
			</div>
		</div>
	</section>

	<section class="overflow-hidden rounded-lg border border-slate-200 bg-white shadow-sm dark:border-slate-700 dark:bg-slate-900">
		{#if loading}
			<div class="p-8 text-center text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		{:else if logs.length === 0}
			<div class="p-8 text-center text-sm text-slate-500 dark:text-slate-400">{$t('connections.noOperationLogs')}</div>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-slate-200 text-sm dark:divide-slate-700">
					<thead class="bg-slate-50 text-left text-xs font-semibold uppercase tracking-wide text-slate-500 dark:bg-slate-800 dark:text-slate-400">
						<tr>
							<th class="px-4 py-3">{$t('connections.time')}</th>
							<th class="px-4 py-3">{$t('connections.bot')}</th>
							<th class="px-4 py-3">{$t('connections.tool')}</th>
							<th class="px-4 py-3">{$t('connections.surface')}</th>
							<th class="px-4 py-3">{$t('connections.result')}</th>
							<th class="px-4 py-3 text-right">{$t('connections.duration')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-200 dark:divide-slate-700">
						{#each logs as log (log.id)}
							<tr class="align-top hover:bg-slate-50/70 dark:hover:bg-slate-800/60">
								<td class="whitespace-nowrap px-4 py-3 text-slate-600 dark:text-slate-300">{formatDate(log.created_at)}</td>
								<td class="px-4 py-3">
									<div class="font-medium text-slate-900 dark:text-slate-100">{botLabel(log)}</div>
									<div class="font-mono text-xs text-slate-400">{log.bot_id.slice(0, 8)}</div>
								</td>
								<td class="px-4 py-3">
									<div class="font-mono text-xs font-medium text-slate-800 dark:text-slate-200">{log.tool_name || $t('connections.directRest')}</div>
									<div class="mt-1 max-w-md truncate font-mono text-xs text-slate-400">{log.method} {log.path}</div>
								</td>
								<td class="px-4 py-3"><span class="rounded bg-slate-100 px-2 py-1 font-mono text-xs text-slate-700 dark:bg-slate-800 dark:text-slate-300">{log.surface}</span></td>
								<td class="px-4 py-3">
									<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {log.outcome === 'ok' ? 'bg-emerald-50 text-emerald-700 ring-emerald-200' : 'bg-red-50 text-red-700 ring-red-200'}">
										{log.outcome === 'ok' ? $t('connections.resultOk') : $t('connections.resultError')}
									</span>
									{#if log.business_code !== 0}<div class="mt-1 text-xs text-slate-500">{log.business_code} · {log.error_message || '-'}</div>{/if}
								</td>
								<td class="whitespace-nowrap px-4 py-3 text-right tabular-nums text-slate-700 dark:text-slate-300">{log.duration_ms} ms</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>

	<div class="flex items-center justify-between">
		<span class="text-sm text-slate-500 dark:text-slate-400">{$t('connections.page', { values: { page: pageIndex + 1 } })}</span>
		<div class="flex gap-2">
			<Button variant="secondary" onclick={previousPage} disabled={loading || pageIndex === 0}>{$t('connections.previous')}</Button>
			<Button variant="secondary" onclick={nextPage} disabled={loading || !nextCursor}>{$t('connections.next')}</Button>
		</div>
	</div>
</div>

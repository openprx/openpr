<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import {
		governanceExtApi,
		type AiAlignmentStats,
		type AiLearningRecord
	} from '$lib/api/governance-ext';
	import { toast } from '$lib/stores/toast';

	const participantId = $derived($page.params.id || '');

	let loading = $state(true);
	let loadingFeedback = $state(false);
	let records = $state<AiLearningRecord[]>([]);
	let stats = $state<AiAlignmentStats | null>(null);
	let domain = $state('');
	let pageNum = $state(1);
	let perPage = $state(20);
	let total = $state(0);
	let totalPages = $state(0);
	let feedbackReviewId = $state('');
	let feedbackItems = $state<AiLearningRecord[]>([]);

	onMount(() => {
		void loadAll();
	});

	async function loadAll() {
		loading = true;
		const [learningRes, statsRes] = await Promise.all([
			governanceExtApi.getAiParticipantLearning(participantId, {
				domain: domain.trim() || undefined,
				page: pageNum,
				per_page: perPage
			}),
			governanceExtApi.getAiParticipantAlignmentStats(participantId)
		]);

		if (learningRes.code !== 0 || !learningRes.data) {
			toast.error(learningRes.message || get(t)('governanceExt.learningLoadFailed'));
			records = [];
			total = 0;
			totalPages = 0;
		} else {
			records = learningRes.data.items;
			total = learningRes.data.total;
			totalPages = learningRes.data.total_pages;
		}

		if (statsRes.code !== 0 || !statsRes.data) {
			toast.error(statsRes.message || get(t)('governanceExt.learningLoadFailed'));
			stats = null;
		} else {
			stats = statsRes.data;
		}
		loading = false;
	}

	async function loadFeedback(reviewId: string) {
		loadingFeedback = true;
		feedbackReviewId = reviewId;
		const res = await governanceExtApi.getAiReviewFeedback(reviewId);
		loadingFeedback = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.learningLoadFailed'));
			feedbackItems = [];
			return;
		}
		feedbackItems = res.data.items;
	}

	async function search() {
		pageNum = 1;
		await loadAll();
	}

	async function goPage(next: number) {
		pageNum = Math.max(1, Math.min(next, totalPages || 1));
		await loadAll();
	}

	function pct(value: number): string {
		return `${(value * 100).toFixed(1)}%`;
	}

	function tone(value: string): string {
		if (value === 'aligned') return 'bg-emerald-100 text-emerald-700';
		if (value === 'misaligned') return 'bg-red-100 text-red-700';
		return 'bg-slate-100 text-slate-700';
	}
 </script>

<svelte:head>
	<title>{$t('pageTitle.aiLearning')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.learningTitle')}</h1>
		<p class="mt-1 text-sm text-slate-500">{$t('governanceExt.learningSubtitle', { values: { id: participantId } })}</p>
	</div>

	{#if stats}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-4">
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('common.total')}</p>
				<p class="mt-1 text-2xl font-semibold text-slate-900 dark:text-slate-100">{stats.total}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.overallAlignmentRate')}</p>
				<p class="mt-1 text-2xl font-semibold text-emerald-700">{pct(stats.overall_alignment_rate)}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.recentAlignmentRate')}</p>
				<p class="mt-1 text-2xl font-semibold text-blue-700">{pct(stats.recent_alignment_rate)}</p>
			</div>
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<p class="text-xs text-slate-500">{$t('governanceExt.improvementTrend')}</p>
				<p class={`mt-1 text-2xl font-semibold ${stats.improvement_trend >= 0 ? 'text-emerald-700' : 'text-red-700'}`}>
					{stats.improvement_trend >= 0 ? '+' : ''}{pct(stats.improvement_trend)}
				</p>
			</div>
		</div>
	{/if}

	<div class="grid grid-cols-1 gap-3 rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800 md:grid-cols-4">
		<div>
			<label for="learning-domain" class="mb-1 block text-sm text-slate-600">{$t('governance.domainLabel')}</label>
			<input id="learning-domain" bind:value={domain} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
		</div>
		<div class="md:col-span-3 flex items-end">
			<button type="button" onclick={search} class="w-full rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700">
				{$t('common.search')}
			</button>
		</div>
	</div>

	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
		{#if loading}
			<p class="text-sm text-slate-500">{$t('common.loading')}</p>
		{:else if records.length === 0}
			<p class="text-sm text-slate-500">{$t('common.noData')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full text-left text-sm">
					<thead>
						<tr class="border-b border-slate-200 text-xs text-slate-500 dark:border-slate-700">
							<th class="px-2 py-2">Review</th>
							<th class="px-2 py-2">{$t('governance.domainLabel')}</th>
							<th class="px-2 py-2">{$t('governanceExt.alignment')}</th>
							<th class="px-2 py-2">Rating</th>
							<th class="px-2 py-2">{$t('common.createdAt')}</th>
							<th class="px-2 py-2">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each records as record}
							<tr class="border-b border-slate-100 dark:border-slate-800">
								<td class="px-2 py-2 text-xs">
									<a href={`/proposals/${record.proposal_id}`} class="text-blue-600 hover:underline">{record.review_id}</a>
								</td>
								<td class="px-2 py-2 text-xs">{record.domain}</td>
								<td class="px-2 py-2 text-xs">
									<span class={`rounded px-2 py-0.5 ${tone(record.outcome_alignment)}`}>{record.outcome_alignment}</span>
								</td>
								<td class="px-2 py-2 text-xs">{record.review_rating}</td>
								<td class="px-2 py-2 text-xs text-slate-500">{new Date(record.created_at).toLocaleString()}</td>
								<td class="px-2 py-2 text-xs">
									<button type="button" class="text-blue-600 hover:underline" onclick={() => loadFeedback(record.review_id)}>
										{$t('governanceExt.viewFeedback')}
									</button>
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

	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
		<div class="mb-3 flex items-center justify-between">
			<h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
				{$t('governanceExt.feedbackForReview', { values: { id: feedbackReviewId || '-' } })}
			</h2>
			{#if loadingFeedback}
				<span class="text-xs text-slate-500">{$t('common.loading')}</span>
			{/if}
		</div>
		{#if feedbackItems.length === 0}
			<p class="text-sm text-slate-500">{$t('common.noData')}</p>
		{:else}
			<div class="space-y-2">
				{#each feedbackItems as item}
					<div class="rounded border border-slate-200 p-3 text-xs dark:border-slate-700">
						<p class="text-slate-700 dark:text-slate-200">{item.lesson_learned || '-'}</p>
						<p class="mt-1 text-slate-500">{item.follow_up_improvement || '-'}</p>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { proposalsApi, type Proposal } from '$lib/api/proposals';
	import {
		impactReviewsApi,
		type ImpactReviewDetail,
		type ReviewRating,
		type ReviewStatus
	} from '$lib/api/impact-reviews';
	import { toast } from '$lib/stores/toast';

	const proposalId = $derived($page.params.id || '');

	let loading = $state(true);
	let proposal = $state<Proposal | null>(null);
	let impactReviewDetail = $state<ImpactReviewDetail | null>(null);
	let loadingImpactReview = $state(false);
	let impactRating = $state<ReviewRating | ''>('');
	let impactStatus = $state<ReviewStatus>('collecting');
	let impactAchievements = $state('');
	let impactLessons = $state('');
	let impactGoalAchievementsText = $state('[]');
	let impactSaving = $state(false);

	const pageTitle = $derived.by(() => {
		if (proposal?.title?.trim()) {
			return $t('pageTitle.proposalImpactReviewWithTitle', { values: { title: proposal.title.trim() } });
		}
		return $t('pageTitle.proposalImpactReview');
	});

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		if (!proposalId) {
			loading = false;
			return;
		}

		const proposalRes = await proposalsApi.get(proposalId);
		if (proposalRes.code !== 0 || !proposalRes.data) {
			toast.error(proposalRes.message || get(t)('governance.notFound'));
			loading = false;
			return;
		}

		proposal = proposalRes.data.proposal;
		await loadImpactReview();
		loading = false;
	}

	function syncImpactForm() {
		impactRating = impactReviewDetail?.review.rating || '';
		impactStatus = impactReviewDetail?.review.status || 'collecting';
		impactAchievements = impactReviewDetail?.review.achievements || '';
		impactLessons = impactReviewDetail?.review.lessons || '';
		impactGoalAchievementsText = JSON.stringify(impactReviewDetail?.review.goal_achievements || [], null, 2);
	}

	async function loadImpactReview() {
		if (!proposalId) {
			impactReviewDetail = null;
			return;
		}
		loadingImpactReview = true;
		const res = await impactReviewsApi.getByProposal(proposalId);
		loadingImpactReview = false;
		if (res.code === 0 && res.data) {
			impactReviewDetail = res.data;
			syncImpactForm();
			return;
		}
		impactReviewDetail = null;
	}

	async function createImpactReview() {
		if (!proposalId) return;
		const res = await impactReviewsApi.createForProposal(proposalId);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('impactReview.createFailed'));
			return;
		}
		toast.success(get(t)('impactReview.createSuccess'));
		await loadImpactReview();
	}

	async function saveImpactReview() {
		if (!proposalId || !impactReviewDetail) return;
		let parsedGoalAchievements: unknown = [];
		try {
			parsedGoalAchievements = JSON.parse(impactGoalAchievementsText || '[]');
		} catch {
			toast.error(get(t)('impactReview.goalAchievementsInvalid'));
			return;
		}

		impactSaving = true;
		const res = await impactReviewsApi.updateByProposal(proposalId, {
			status: impactStatus,
			rating: impactRating || undefined,
			goal_achievements: parsedGoalAchievements,
			achievements: impactAchievements.trim() || undefined,
			lessons: impactLessons.trim() || undefined
		});
		impactSaving = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('impactReview.updateFailed'));
			return;
		}
		impactReviewDetail = res.data;
		syncImpactForm();
		toast.success(get(t)('impactReview.updateSuccess'));
	}
</script>

<svelte:head>
	<title>{pageTitle}</title>
</svelte:head>

<div class="mx-auto max-w-4xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<div class="mb-2 flex items-center justify-between gap-3">
			<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('impactReview.title')}</h1>
			<a href={`/proposals/${proposalId}`} class="text-sm text-blue-600 hover:underline">{$t('impactReview.backToProposal')}</a>
		</div>
		<p class="text-sm text-slate-500">{$t('impactReview.detailSubtitle')}</p>
	</div>

	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		{#if loading}
			<p class="text-sm text-slate-500">{$t('common.loading')}</p>
		{:else if !proposal}
			<p class="text-sm text-slate-500">{$t('governance.notFound')}</p>
		{:else}
			<div class="mb-3 text-xs text-slate-500">
				<p>{$t('impactReview.proposalId')}: {proposal.id}</p>
				<p>{$t('governance.createdAtLabel')}: {new Date(proposal.created_at).toLocaleString()}</p>
			</div>

			{#if loadingImpactReview}
				<p class="text-sm text-slate-500">{$t('common.loading')}</p>
			{:else if !impactReviewDetail}
				<p class="text-sm text-slate-500">{$t('impactReview.notCreated')}</p>
				{#if proposal.status === 'approved'}
					<button type="button" onclick={createImpactReview} class="mt-3 rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700">
						{$t('impactReview.create')}
					</button>
				{/if}
			{:else}
				<div class="mb-3 grid grid-cols-1 gap-2 text-xs text-slate-500 md:grid-cols-4">
					<p>{$t('impactReview.reviewId')}: {impactReviewDetail.review.id}</p>
					<p>{$t('impactReview.scheduledAt')}: {impactReviewDetail.review.scheduled_at ? new Date(impactReviewDetail.review.scheduled_at).toLocaleString() : '-'}</p>
					<p>{$t('impactReview.conductedAt')}: {impactReviewDetail.review.conducted_at ? new Date(impactReviewDetail.review.conducted_at).toLocaleString() : '-'}</p>
					<p>{$t('impactReview.trustApplied')}: {impactReviewDetail.review.trust_score_applied ? $t('impactReview.yes') : $t('impactReview.no')}</p>
				</div>
				<div class="space-y-3">
					<div class="grid grid-cols-1 gap-3 md:grid-cols-2">
						<div>
							<label for="proposal-review-status" class="mb-1 block text-sm text-slate-600">{$t('common.status')}</label>
							<select id="proposal-review-status" bind:value={impactStatus} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
								<option value="pending">{$t('impactReview.status.pending')}</option>
								<option value="collecting">{$t('impactReview.status.collecting')}</option>
								<option value="completed">{$t('impactReview.status.completed')}</option>
								<option value="skipped">{$t('impactReview.status.skipped')}</option>
							</select>
						</div>
						<div>
							<label for="proposal-review-rating" class="mb-1 block text-sm text-slate-600">{$t('impactReview.rating')}</label>
							<select id="proposal-review-rating" bind:value={impactRating} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
								<option value="">{`- ${$t('impactReview.ratingAll')} -`}</option>
								<option value="S">S</option>
								<option value="A">A</option>
								<option value="B">B</option>
								<option value="C">C</option>
								<option value="F">F</option>
							</select>
						</div>
					</div>
					<div>
						<label for="proposal-review-goals" class="mb-1 block text-sm text-slate-600">{$t('impactReview.goalAchievements')}</label>
						<textarea id="proposal-review-goals" bind:value={impactGoalAchievementsText} rows="6" class="w-full rounded-md border border-slate-300 px-3 py-2 font-mono text-xs dark:border-slate-600 dark:bg-slate-900"></textarea>
					</div>
					<div>
						<label for="proposal-review-achievements" class="mb-1 block text-sm text-slate-600">{$t('impactReview.achievements')}</label>
						<textarea id="proposal-review-achievements" bind:value={impactAchievements} rows="4" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"></textarea>
					</div>
					<div>
						<label for="proposal-review-lessons" class="mb-1 block text-sm text-slate-600">{$t('impactReview.lessons')}</label>
						<textarea id="proposal-review-lessons" bind:value={impactLessons} rows="4" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"></textarea>
					</div>
					<div class="flex justify-end">
						<button type="button" onclick={saveImpactReview} class="rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700" disabled={impactSaving}>
							{impactSaving ? $t('common.saving') : $t('common.save')}
						</button>
					</div>
				</div>
			{/if}
		{/if}
	</div>
</div>

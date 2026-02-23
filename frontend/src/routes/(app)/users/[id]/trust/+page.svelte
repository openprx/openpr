<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { trustApi, type Appeal, type UserTrustScore } from '$lib/api/trust';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';

	const userId = $derived($page.params.id || '');

	let loading = $state(false);
	let loadingAppeals = $state(false);
	let projectId = $state('');
	let scores = $state<UserTrustScore[]>([]);
	let appeals = $state<Appeal[]>([]);
	let userType = $state<'human' | 'ai'>('human');

	const globalScore = $derived(scores.find((item) => item.domain === 'global'));
	const domainScores = $derived(scores.filter((item) => item.domain !== 'global').sort((a, b) => b.score - a.score));

	const totalDomainScore = $derived(
		domainScores.reduce((sum, item) => sum + Math.max(item.score, 0), 0)
	);

	const timeline = $derived(
		scores
			.slice()
			.sort((a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime())
			.slice(0, 30)
	);

	const relatedAppeals = $derived(
		appeals
			.filter((item) => item.appellant_id === userId)
			.sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
	);

	onMount(() => {
		void loadAll();
	});

	async function loadAll() {
		await Promise.all([loadTrust(), loadAppeals()]);
	}

	async function loadTrust() {
		loading = true;
		const res = await trustApi.getUserTrust(userId, projectId.trim() || undefined);
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('trustProfile.loadFailed'));
			scores = [];
			loading = false;
			return;
		}
		scores = res.data.scores;
		userType = res.data.user_type;
		loading = false;
	}

	async function loadAppeals() {
		loadingAppeals = true;
		const res = await trustApi.listAppeals({ mine: false });
		if (res.code !== 0 || !res.data) {
			loadingAppeals = false;
			return;
		}
		appeals = res.data.items;
		loadingAppeals = false;
	}

	function levelTone(level: string): string {
		switch (level) {
			case 'observer':
				return 'bg-slate-100 text-slate-700';
			case 'advisor':
				return 'bg-sky-100 text-sky-700';
			case 'voter':
				return 'bg-emerald-100 text-emerald-700';
			case 'vetoer':
				return 'bg-amber-100 text-amber-700';
			case 'autonomous':
				return 'bg-fuchsia-100 text-fuchsia-700';
			default:
				return 'bg-slate-100 text-slate-700';
		}
	}

	function appealTone(status: string): string {
		switch (status) {
			case 'accepted':
				return 'bg-emerald-100 text-emerald-700';
			case 'rejected':
				return 'bg-red-100 text-red-700';
			default:
				return 'bg-amber-100 text-amber-700';
		}
	}

	function barWidth(score: number): string {
		if (totalDomainScore <= 0) {
			return '0%';
		}
		const pct = Math.max(0, Math.min(100, (score / totalDomainScore) * 100));
		return `${pct.toFixed(1)}%`;
	}

	function levelLabel(level: string): string {
		return get(t)(`trustCommon.level.${level}`) || level;
	}

	function appealStatusLabel(status: Appeal['status']): string {
		return get(t)(`appeals.status.${status}`) || status;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.userTrust', { values: { id: userId } })}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
		<div class="mb-4 flex flex-wrap items-end justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('trustProfile.title')}</h1>
				<p class="text-sm text-slate-500">{$t('trustProfile.subtitle', { values: { id: userId } })}</p>
			</div>
			<div class="flex gap-2">
				<input
					type="text"
					bind:value={projectId}
					placeholder={$t('trustProfile.projectIdPlaceholder')}
					class="rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
				/>
				<Button onclick={loadAll} loading={loading || loadingAppeals}>{$t('common.search')}</Button>
			</div>
		</div>

		{#if loading}
			<div class="py-8 text-center text-slate-500">{$t('common.loading')}</div>
		{:else if scores.length === 0}
			<div class="py-8 text-center text-slate-500">{$t('trustProfile.empty')}</div>
		{:else}
			<div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
				<div class="rounded-lg border border-slate-200 p-4 dark:border-slate-700 lg:col-span-1">
					<div class="text-xs uppercase tracking-wide text-slate-500">{$t('trustProfile.globalSummary')}</div>
					<div class="mt-2 text-sm text-slate-600 dark:text-slate-300">{$t('trustProfile.userType')}: {userType === 'ai' ? $t('trustBoard.typeAi') : $t('trustBoard.typeHuman')}</div>
					<div class="mt-3 text-3xl font-bold text-slate-900 dark:text-slate-100">{globalScore?.score ?? 0}</div>
					<div class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('trustProfile.weightLabel')}: {(globalScore?.vote_weight ?? 1).toFixed(2)}x</div>
					<div class="mt-2">
						<span class={`rounded px-2 py-0.5 text-xs font-medium ${levelTone(globalScore?.level ?? 'observer')}`}>{levelLabel(globalScore?.level ?? 'observer')}</span>
					</div>
				</div>

				<div class="rounded-lg border border-slate-200 p-4 dark:border-slate-700 lg:col-span-2">
					<div class="mb-3 text-xs uppercase tracking-wide text-slate-500">{$t('trustProfile.domainDistribution')}</div>
					{#if domainScores.length === 0}
						<div class="text-sm text-slate-500">{$t('trustProfile.noDomainData')}</div>
					{:else}
						<div class="space-y-2">
							{#each domainScores as item (item.project_id + item.domain)}
								<div>
									<div class="mb-1 flex items-center justify-between text-xs text-slate-600 dark:text-slate-300">
										<span>{item.domain}</span>
										<span>{item.score}</span>
									</div>
									<div class="h-2 rounded bg-slate-100 dark:bg-slate-700">
										<div class="h-2 rounded bg-blue-500" style={`width:${barWidth(item.score)}`}></div>
									</div>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</div>

			<div class="mt-4 grid grid-cols-1 gap-4 lg:grid-cols-2">
				<div class="rounded-lg border border-slate-200 p-4 dark:border-slate-700">
					<h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('trustProfile.historyTitle')}</h2>
					<div class="mt-3 space-y-2">
						{#each timeline as item (item.project_id + item.domain + item.updated_at)}
							<div class="rounded border border-slate-200 px-3 py-2 text-sm dark:border-slate-700">
								<div class="flex items-center justify-between gap-2">
									<div class="font-medium text-slate-900 dark:text-slate-100">{item.domain}</div>
									<div class="text-xs text-slate-500">{new Date(item.updated_at).toLocaleString()}</div>
								</div>
								<div class="mt-1 text-xs text-slate-600 dark:text-slate-300">
								{$t('trustProfile.historyMeta', { values: { score: item.score, level: levelLabel(item.level), weight: item.vote_weight.toFixed(2) } })}
								</div>
							</div>
						{/each}
					</div>
				</div>

				<div class="rounded-lg border border-slate-200 p-4 dark:border-slate-700">
					<h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('trustProfile.changeLogTitle')}</h2>
					{#if loadingAppeals}
						<div class="mt-3 text-sm text-slate-500">{$t('common.loading')}</div>
					{:else if relatedAppeals.length === 0}
						<div class="mt-3 text-sm text-slate-500">{$t('trustProfile.noChangeLog')}</div>
					{:else}
						<div class="mt-3 space-y-2">
							{#each relatedAppeals as item (item.id)}
								<div class="rounded border border-slate-200 px-3 py-2 text-sm dark:border-slate-700">
									<div class="flex items-center justify-between gap-2">
										<div class="font-medium text-slate-900 dark:text-slate-100">{$t('trustProfile.appealWithLog', { values: { id: item.log_id } })}</div>
										<span class={`rounded px-2 py-0.5 text-xs ${appealTone(item.status)}`}>{appealStatusLabel(item.status)}</span>
									</div>
									<p class="mt-1 text-xs text-slate-600 dark:text-slate-300">{item.reason}</p>
									<p class="mt-1 text-xs text-slate-500">{new Date(item.created_at).toLocaleString()}</p>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		{/if}
	</div>
</div>

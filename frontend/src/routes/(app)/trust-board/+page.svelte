<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { trustApi, type TrustScoreRank } from '$lib/api/trust';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';

	let loading = $state(false);
	let items = $state<TrustScoreRank[]>([]);
	let selectedDomain = $state('global');
	let projectId = $state('');
	let page = $state(1);
	let totalPages = $state(1);
	let total = $state(0);

	const domains = $derived.by(() => {
		const set = new Set(items.map((item) => item.domain));
		const ordered = Array.from(set).sort((a, b) => a.localeCompare(b));
		if (!ordered.includes('global')) {
			ordered.unshift('global');
		}
		return ordered;
	});

	const filteredItems = $derived.by(() => {
		const list = items
			.filter((item) => item.domain === selectedDomain)
			.sort((a, b) => b.score - a.score || b.vote_weight - a.vote_weight);
		return list;
	});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		const res = await trustApi.listTrustScores({
			project_id: projectId.trim() || undefined,
			domain: 'all',
			page,
			per_page: 100
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('trustBoard.loadFailed'));
			items = [];
			totalPages = 1;
			total = 0;
			loading = false;
			return;
		}
		items = res.data.items;
		totalPages = Math.max(1, res.data.total_pages || 1);
		total = res.data.total || items.length;
		if (domains.length > 0 && !domains.includes(selectedDomain)) {
			selectedDomain = domains[0];
		}
		loading = false;
	}

	async function search() {
		page = 1;
		await load();
	}

	async function prevPage() {
		if (page <= 1 || loading) return;
		page -= 1;
		await load();
	}

	async function nextPage() {
		if (page >= totalPages || loading) return;
		page += 1;
		await load();
	}

	function scoreTone(rank: number): string {
		if (rank === 1) return 'bg-amber-50 border-amber-200';
		if (rank === 2) return 'bg-slate-50 border-slate-300';
		if (rank === 3) return 'bg-orange-50 border-orange-200';
		return 'bg-white border-slate-200 dark:bg-slate-900 dark:border-slate-700';
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

	function trendMeta(item: TrustScoreRank): { label: string; className: string } {
		if (item.consecutive_rejections >= 2) {
			return {
				label: get(t)('trustBoard.trend.down'),
				className: 'text-red-600'
			};
		}
		const updated = new Date(item.updated_at).getTime();
		if (Number.isFinite(updated) && Date.now() - updated <= 7 * 24 * 3600 * 1000) {
			return {
				label: get(t)('trustBoard.trend.up'),
				className: 'text-emerald-600'
			};
		}
		return {
			label: get(t)('trustBoard.trend.stable'),
			className: 'text-slate-500'
		};
	}

	function levelLabel(level: string): string {
		return get(t)(`trustCommon.level.${level}`) || level;
	}

	function shortId(value: string): string {
		if (value.length <= 12) return value;
		return `${value.slice(0, 8)}...`;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.trustBoard')}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
		<div class="mb-4 flex flex-wrap items-end justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('trustBoard.title')}</h1>
				<p class="text-sm text-slate-500">{$t('trustBoard.subtitle')}</p>
			</div>
			<div class="flex gap-2">
				<input
					type="text"
					bind:value={projectId}
					placeholder={$t('trustBoard.projectIdPlaceholder')}
					class="rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
				/>
				<Button onclick={search} loading={loading}>{$t('common.search')}</Button>
			</div>
		</div>

		<div class="mb-4 flex flex-wrap gap-2">
			{#each domains as domain}
				<button
					type="button"
					onclick={() => (selectedDomain = domain)}
					class={`rounded-md px-3 py-1.5 text-sm ${selectedDomain === domain
						? 'bg-slate-900 text-white dark:bg-slate-100 dark:text-slate-900'
						: 'bg-slate-100 text-slate-700 hover:bg-slate-200 dark:bg-slate-700 dark:text-slate-100'}`}
				>
					{domain}
				</button>
			{/each}
		</div>

		{#if loading}
			<div class="py-8 text-center text-slate-500">{$t('common.loading')}</div>
		{:else if filteredItems.length === 0}
			<div class="py-8 text-center text-slate-500">{$t('trustBoard.empty')}</div>
		{:else}
			<div class="space-y-2">
				{#each filteredItems as item, index (item.user_id + item.project_id + item.domain)}
					<div class={`rounded-lg border p-3 ${scoreTone(index + 1)}`}>
						<div class="flex flex-wrap items-center justify-between gap-3">
							<div class="min-w-0 flex-1">
								<div class="flex items-center gap-2">
									<span class="inline-flex h-6 min-w-6 items-center justify-center rounded-full bg-slate-900 px-1.5 text-xs font-semibold text-white dark:bg-slate-100 dark:text-slate-900">
										#{index + 1}
									</span>
									<a href={`/users/${item.user_id}/trust`} class="truncate font-medium text-blue-600 hover:underline dark:text-blue-400" title={item.user_id}>
										{shortId(item.user_id)}
									</a>
									<span class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600 dark:bg-slate-700 dark:text-slate-200">{item.user_type === 'ai' ? $t('trustBoard.typeAi') : $t('trustBoard.typeHuman')}</span>
									{#if item.level === 'vetoer' || item.level === 'autonomous'}
										<span title={$t('trustBoard.hasVeto')} class="text-amber-500">⚡</span>
									{/if}
								</div>
								<div class="mt-1 text-xs text-slate-500">{$t('trustBoard.projectLabel')}: {item.project_id}</div>
							</div>

							<div class="flex items-center gap-4 text-sm">
								<div class="text-right">
									<div class="text-xs text-slate-500">{$t('trustBoard.score')}</div>
									<div class="font-semibold text-slate-900 dark:text-slate-100">{item.score}</div>
								</div>
								<div class="text-right">
									<div class="text-xs text-slate-500">{$t('trustBoard.weight')}</div>
									<div class="font-semibold text-slate-900 dark:text-slate-100">{item.vote_weight.toFixed(2)}x</div>
								</div>
								<div class="text-right">
								<div class="text-xs text-slate-500">{$t('trustBoard.level')}</div>
								<span class={`rounded px-2 py-0.5 text-xs font-medium ${levelTone(item.level)}`}>{levelLabel(item.level)}</span>
								</div>
								<div class="text-right">
									<div class="text-xs text-slate-500">{$t('trustBoard.trendLabel')}</div>
									{#key item.updated_at + item.consecutive_rejections}
										{@const trend = trendMeta(item)}
										<div class={`font-medium ${trend.className}`}>{trend.label}</div>
									{/key}
								</div>
							</div>
						</div>
					</div>
				{/each}
			</div>
		{/if}

		<div class="mt-4 flex items-center justify-end gap-2 text-sm text-slate-500 dark:text-slate-300">
			<span>{$t('trustBoard.pagination', { values: { total, page, pages: totalPages } })}</span>
			<Button size="sm" variant="secondary" onclick={prevPage} disabled={loading || page <= 1}>{$t('trustBoard.prevPage')}</Button>
			<Button size="sm" variant="secondary" onclick={nextPage} disabled={loading || page >= totalPages}>{$t('trustBoard.nextPage')}</Button>
		</div>
	</div>
</div>

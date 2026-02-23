<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { proposalsApi, type Proposal, type ProposalStatus, type ProposalType } from '$lib/api/proposals';
	import { toast } from '$lib/stores/toast';

	let loading = $state(true);
	let items = $state<Proposal[]>([]);
	let statusFilter = $state<'all' | ProposalStatus>('all');
	let typeFilter = $state<'all' | ProposalType>('all');
	let domainFilter = $state('');
	let sort = $state('created_at:desc');
	let page = $state(1);
	let perPage = $state(20);
	let totalPages = $state(1);
	let total = $state(0);
	let nowTs = $state(Date.now());

	onMount(() => {
		void load();
		const timer = window.setInterval(() => {
			nowTs = Date.now();
		}, 1000);
		return () => window.clearInterval(timer);
	});

	async function load() {
		loading = true;
		const res = await proposalsApi.list({
			status: statusFilter === 'all' ? undefined : statusFilter,
			proposal_type: typeFilter === 'all' ? undefined : typeFilter,
			domain: domainFilter.trim() || undefined,
			sort,
			page,
			per_page: perPage
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governance.loadFailed'));
			items = [];
			total = 0;
			totalPages = 1;
		} else {
			items = res.data.items;
			total = res.data.total;
			totalPages = Math.max(1, res.data.total_pages);
		}
		loading = false;
	}

	async function applyFilters() {
		page = 1;
		await load();
	}

	async function goPage(next: number) {
		page = Math.max(1, Math.min(totalPages, next));
		await load();
	}

	function statusTone(status: ProposalStatus): string {
		switch (status) {
			case 'draft':
				return 'bg-slate-100 text-slate-700';
			case 'open':
				return 'bg-blue-100 text-blue-700';
			case 'voting':
				return 'bg-amber-100 text-amber-700';
			case 'approved':
				return 'bg-emerald-100 text-emerald-700';
			case 'rejected':
				return 'bg-red-100 text-red-700';
			case 'vetoed':
				return 'bg-rose-100 text-rose-700';
			case 'archived':
				return 'bg-slate-200 text-slate-700';
		}
	}

	function statusLabel(status: ProposalStatus): string {
		return get(t)(`governance.status.${status}`);
	}

	function typeLabel(type: ProposalType): string {
		return get(t)(`governance.type.${type}`);
	}

	function formatCountdown(target?: string): string {
		if (!target) return '-';
		const end = new Date(target).getTime();
		if (!Number.isFinite(end)) return '-';
		const diff = end - nowTs;
		if (diff <= 0) return get(t)('governance.countdown.ended');

		const totalSeconds = Math.floor(diff / 1000);
		const days = Math.floor(totalSeconds / 86400);
		const hours = Math.floor((totalSeconds % 86400) / 3600);
		const minutes = Math.floor((totalSeconds % 3600) / 60);

		const chunks: string[] = [];
		if (days > 0) chunks.push(`${days}d`);
		if (hours > 0 || days > 0) chunks.push(`${hours}h`);
		chunks.push(`${minutes}m`);
		return get(t)('governance.countdown.remaining', { values: { time: chunks.join(' ') } });
	}
</script>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
		<div class="mb-4 flex flex-wrap items-center justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governance.title')}</h1>
				<p class="text-sm text-slate-500">{$t('governance.subtitle')}</p>
			</div>
			<a
				href="/proposals/new"
				class="inline-flex items-center rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700"
			>
				{$t('governance.createAction')}
			</a>
		</div>

		<div class="mb-4 grid grid-cols-1 gap-2 md:grid-cols-5">
			<select bind:value={statusFilter} onchange={applyFilters} class="rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100">
				<option value="all">{$t('governance.status.all')}</option>
				<option value="draft">{$t('governance.status.draft')}</option>
				<option value="open">{$t('governance.status.open')}</option>
				<option value="voting">{$t('governance.status.voting')}</option>
				<option value="approved">{$t('governance.status.approved')}</option>
				<option value="rejected">{$t('governance.status.rejected')}</option>
				<option value="vetoed">{$t('governance.status.vetoed')}</option>
				<option value="archived">{$t('governance.status.archived')}</option>
			</select>
			<select bind:value={typeFilter} onchange={applyFilters} class="rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100">
				<option value="all">{$t('governance.type.all')}</option>
				<option value="feature">{$t('governance.type.feature')}</option>
				<option value="architecture">{$t('governance.type.architecture')}</option>
				<option value="priority">{$t('governance.type.priority')}</option>
				<option value="resource">{$t('governance.type.resource')}</option>
				<option value="governance">{$t('governance.type.governance')}</option>
				<option value="bugfix">{$t('governance.type.bugfix')}</option>
			</select>
			<input bind:value={domainFilter} onkeydown={(e) => e.key === 'Enter' && applyFilters()} placeholder={$t('governance.domainPlaceholder')} class="rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100" />
			<select bind:value={sort} onchange={applyFilters} class="rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100">
				<option value="created_at:desc">{$t('governance.sort.createdDesc')}</option>
				<option value="created_at:asc">{$t('governance.sort.createdAsc')}</option>
				<option value="title:asc">{$t('governance.sort.titleAsc')}</option>
				<option value="title:desc">{$t('governance.sort.titleDesc')}</option>
				<option value="status:asc">{$t('governance.sort.statusAsc')}</option>
				<option value="status:desc">{$t('governance.sort.statusDesc')}</option>
			</select>
			<button type="button" onclick={applyFilters} class="rounded-md bg-slate-800 px-3 py-2 text-sm text-white hover:bg-slate-900">{$t('governance.filter')}</button>
		</div>

		{#if loading}
			<div class="py-8 text-center text-slate-500">{$t('governance.loading')}</div>
		{:else if items.length === 0}
			<div class="py-8 text-center text-slate-500">{$t('governance.empty')}</div>
		{:else}
			<div class="mb-4 grid grid-cols-1 gap-3 md:grid-cols-2">
				{#each items as item}
					<button
						type="button"
						onclick={() => goto(`/proposals/${item.id}`)}
						class="rounded-lg border border-slate-200 bg-white p-4 text-left transition hover:shadow-md dark:border-slate-700 dark:bg-slate-900"
					>
						<div class="mb-2 flex items-start justify-between gap-2">
							<h2 class="line-clamp-2 text-base font-semibold text-slate-900 dark:text-slate-100">{item.title}</h2>
							<span class={`rounded px-2 py-0.5 text-xs font-medium ${statusTone(item.status)}`}>{statusLabel(item.status)}</span>
						</div>
						<div class="space-y-1 text-xs text-slate-500">
							<p>{$t('governance.typeLabel')}: {typeLabel(item.proposal_type)}</p>
							<p>{$t('governance.authorLabel')}: {item.author_id}</p>
							<p>{$t('governance.domainLabel')}: {item.domains.join(', ') || '-'}</p>
							{#if item.status === 'voting'}
								<p>{$t('governance.votingDeadlineLabel')}: {formatCountdown(item.voting_ended_at)}</p>
							{/if}
							<p>{$t('governance.createdAtLabel')}: {new Date(item.created_at).toLocaleString()}</p>
						</div>
					</button>
				{/each}
			</div>
		{/if}

		<div class="flex items-center justify-between border-t border-slate-200 pt-3 text-sm dark:border-slate-700">
			<p class="text-slate-500">{$t('governance.pagination', { values: { total, page, pages: totalPages } })}</p>
			<div class="flex gap-2">
				<button type="button" onclick={() => goPage(page - 1)} disabled={page <= 1 || loading} class="rounded-md border border-slate-300 px-3 py-1 text-slate-700 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200">{$t('governance.prevPage')}</button>
				<button type="button" onclick={() => goPage(page + 1)} disabled={page >= totalPages || loading} class="rounded-md border border-slate-300 px-3 py-1 text-slate-700 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200">{$t('governance.nextPage')}</button>
			</div>
		</div>
	</div>
</div>

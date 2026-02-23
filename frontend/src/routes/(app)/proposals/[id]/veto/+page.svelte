<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { authStore } from '$lib/stores/auth';
	import { proposalsApi, type Proposal } from '$lib/api/proposals';
	import { vetoApi, type VetoEvent } from '$lib/api/veto';
	import { issuesApi } from '$lib/api/issues';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';

	const proposalId = $derived($page.params.id || '');

	let loading = $state(false);
	let actionLoading = $state(false);
	let proposal = $state<Proposal | null>(null);
	let vetoEvent = $state<VetoEvent | null>(null);
	let vetoMissing = $state(false);
	let totalVetoers = $state(0);
	let projectId = $state('');

	const meId = $derived($authStore.user?.id || '');
	const isAuthor = $derived(Boolean(proposal && meId && proposal.author_id === meId));
	const hasEscalation = $derived(Boolean(vetoEvent?.escalation_started_at));
	const canEscalate = $derived(Boolean(isAuthor && vetoEvent?.status === 'active' && !hasEscalation));
	const hasMyBallot = $derived(Boolean(vetoEvent?.escalation_votes.ballots[meId] !== undefined));
	const canVoteEscalation = $derived(Boolean(vetoEvent?.status === 'active' && hasEscalation && !hasMyBallot));

	const escalationDeadlineText = $derived.by(() => {
		if (!vetoEvent?.escalation_started_at) {
			return '-';
		}
		const deadline = new Date(vetoEvent.escalation_started_at).getTime() + 48 * 3600 * 1000;
		return new Date(deadline).toLocaleString();
	});

	const requiredVotes = $derived(Math.ceil((totalVetoers * 2) / 3));

	onMount(() => {
		void loadAll();
	});

	function isNotFound(code: number): boolean {
		return code === 404;
	}

	async function loadAll() {
		loading = true;
		vetoMissing = false;
		const proposalRes = await proposalsApi.get(proposalId);
		if (proposalRes.code !== 0 || !proposalRes.data) {
			toast.error(proposalRes.message || get(t)('vetoDetail.loadProposalFailed'));
			loading = false;
			return;
		}
		proposal = proposalRes.data.proposal;

		const vetoRes = await vetoApi.get(proposalId);
		if (isNotFound(vetoRes.code)) {
			vetoEvent = null;
			vetoMissing = true;
			loading = false;
			return;
		}
		if (vetoRes.code !== 0 || !vetoRes.data) {
			toast.error(vetoRes.message || get(t)('vetoDetail.loadFailed'));
			loading = false;
			return;
		}
		vetoEvent = vetoRes.data;

		await resolveProjectId();
		await loadVetoerCount();
		loading = false;
	}

	async function resolveProjectId() {
		const queryProject = $page.url.searchParams.get('project_id');
		if (queryProject) {
			projectId = queryProject;
			return;
		}
		const linksRes = await proposalsApi.listIssues(proposalId);
		const issueId = linksRes.data?.items?.[0]?.issue_id;
		if (!issueId) {
			return;
		}
		const issueRes = await issuesApi.get(issueId);
		if (issueRes.code === 0 && issueRes.data?.project_id) {
			projectId = issueRes.data.project_id;
		}
	}

	async function loadVetoerCount() {
		if (!vetoEvent?.domain) {
			totalVetoers = 0;
			return;
		}
		const res = await vetoApi.listVetoers({
			project_id: projectId || undefined,
			domain: vetoEvent.domain
		});
		if (isNotFound(res.code)) {
			totalVetoers = 0;
			return;
		}
		totalVetoers = res.code === 0 && res.data ? res.data.items.length : 0;
	}

	function statusTone(status: string): string {
		switch (status) {
			case 'active':
				return 'bg-amber-100 text-amber-700';
			case 'overturned':
				return 'bg-emerald-100 text-emerald-700';
			case 'upheld':
				return 'bg-red-100 text-red-700';
			case 'withdrawn':
				return 'bg-slate-100 text-slate-700';
			default:
				return 'bg-slate-100 text-slate-700';
		}
	}

	function statusLabel(status: string): string {
		return get(t)(`trustCommon.vetoStatus.${status}`) || status;
	}

	async function startEscalation() {
		actionLoading = true;
		const res = await vetoApi.startEscalation(proposalId);
		actionLoading = false;
		if (isNotFound(res.code)) {
			vetoEvent = null;
			vetoMissing = true;
			return;
		}
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('vetoDetail.startEscalationFailed'));
			return;
		}
		vetoEvent = res.data;
		vetoMissing = false;
		toast.success(get(t)('vetoDetail.startEscalationSuccess'));
	}

	async function voteEscalation(overturn: boolean) {
		actionLoading = true;
		const res = await vetoApi.voteEscalation(proposalId, overturn);
		actionLoading = false;
		if (isNotFound(res.code)) {
			vetoEvent = null;
			vetoMissing = true;
			return;
		}
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('vetoDetail.voteEscalationFailed'));
			return;
		}
		vetoEvent = res.data;
		vetoMissing = false;
		toast.success(get(t)('vetoDetail.voteEscalationSuccess'));
	}

	async function withdrawVeto() {
		actionLoading = true;
		const res = await vetoApi.withdraw(proposalId);
		actionLoading = false;
		if (isNotFound(res.code)) {
			vetoEvent = null;
			vetoMissing = true;
			return;
		}
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('vetoDetail.withdrawFailed'));
			return;
		}
		vetoEvent = res.data;
		vetoMissing = false;
		toast.success(get(t)('vetoDetail.withdrawSuccess'));
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.vetoDetail')}</title>
</svelte:head>

<div class="mx-auto max-w-5xl space-y-4">
	{#if loading}
		<div class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-900">{$t('common.loading')}</div>
	{:else if proposal && vetoMissing}
		<div class="rounded-lg border border-slate-200 bg-white px-6 py-14 dark:border-slate-700 dark:bg-slate-900">
			<div class="mx-auto flex max-w-md flex-col items-center text-center">
				<div class="flex h-14 w-14 items-center justify-center rounded-full bg-slate-100 text-slate-500 dark:bg-slate-800 dark:text-slate-300">
					<svg class="h-7 w-7" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
						<path d="M9 12h6" />
						<path d="M12 9v6" />
						<path d="M4 6.5A2.5 2.5 0 0 1 6.5 4h11A2.5 2.5 0 0 1 20 6.5v11a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 17.5v-11Z" />
					</svg>
				</div>
				<p class="mt-4 text-base font-medium text-slate-900 dark:text-slate-100">{$t('vetoDetail.empty')}</p>
				<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">{$t('vetoDetail.emptyHint')}</p>
				<div class="mt-5">
					<Button variant="secondary" onclick={() => goto(`/proposals/${proposalId}`)}>{$t('vetoDetail.backToProposal')}</Button>
				</div>
			</div>
		</div>
	{:else if !proposal || !vetoEvent}
		<div class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-900">{$t('vetoDetail.empty')}</div>
	{:else}
		<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900">
			<div class="mb-3 flex flex-wrap items-center justify-between gap-2">
				<div>
					<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('vetoDetail.title')}</h1>
					<p class="text-sm text-slate-500">{proposal.title}</p>
				</div>
				<span class={`rounded px-2 py-1 text-xs font-medium ${statusTone(vetoEvent.status)}`}>{statusLabel(vetoEvent.status)}</span>
			</div>

			<div class="grid grid-cols-1 gap-3 text-sm text-slate-700 dark:text-slate-300 md:grid-cols-2">
				<div>
					<p><span class="text-slate-500">{$t('vetoDetail.vetoer')}:</span> {vetoEvent.vetoer_id}</p>
					<p><span class="text-slate-500">{$t('vetoDetail.domain')}:</span> {vetoEvent.domain}</p>
					<p><span class="text-slate-500">{$t('vetoDetail.createdAt')}:</span> {new Date(vetoEvent.created_at).toLocaleString()}</p>
				</div>
				<div>
					<p><span class="text-slate-500">{$t('vetoDetail.totalVetoers')}:</span> {totalVetoers || '-'}</p>
					<p><span class="text-slate-500">{$t('vetoDetail.requiredVotes')}:</span> {requiredVotes || '-'}</p>
					<p><span class="text-slate-500">{$t('vetoDetail.escalationDeadline')}:</span> {hasEscalation ? escalationDeadlineText : '-'}</p>
				</div>
			</div>

			<div class="mt-3 rounded border border-slate-200 bg-slate-50 p-3 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300">
				<div class="font-medium text-slate-900 dark:text-slate-100">{$t('vetoDetail.reason')}</div>
				<p class="mt-1 whitespace-pre-wrap">{vetoEvent.reason}</p>
			</div>
		</div>

		<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900">
			<h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('vetoDetail.escalationTitle')}</h2>
			{#if hasEscalation}
				<div class="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-3">
					<div class="rounded border border-emerald-200 bg-emerald-50 p-3 text-sm text-emerald-700">
						<div class="text-xs uppercase tracking-wide">{$t('vetoDetail.overturnVotes')}</div>
						<div class="mt-1 text-xl font-semibold">{vetoEvent.escalation_votes.overturned}</div>
					</div>
					<div class="rounded border border-red-200 bg-red-50 p-3 text-sm text-red-700">
						<div class="text-xs uppercase tracking-wide">{$t('vetoDetail.upholdVotes')}</div>
						<div class="mt-1 text-xl font-semibold">{vetoEvent.escalation_votes.upheld}</div>
					</div>
					<div class="rounded border border-slate-200 bg-slate-50 p-3 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300">
						<div class="text-xs uppercase tracking-wide">{$t('vetoDetail.voteProgress')}</div>
						<div class="mt-1 text-xl font-semibold">{Object.keys(vetoEvent.escalation_votes.ballots).length}/{totalVetoers || '-'}</div>
					</div>
				</div>

				{#if canVoteEscalation}
					<div class="mt-4 flex flex-wrap gap-2">
						<Button variant="danger" onclick={() => voteEscalation(false)} loading={actionLoading}>{$t('vetoDetail.voteUphold')}</Button>
						<Button onclick={() => voteEscalation(true)} loading={actionLoading}>{$t('vetoDetail.voteOverturn')}</Button>
					</div>
				{/if}
			{:else if canEscalate}
				<div class="mt-3 rounded border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800">
					<p>{$t('vetoDetail.canEscalateHint')}</p>
					<div class="mt-2">
						<Button onclick={startEscalation} loading={actionLoading}>{$t('vetoDetail.startEscalation')}</Button>
					</div>
				</div>
			{:else}
				<div class="mt-3 text-sm text-slate-500">{$t('vetoDetail.noEscalationAction')}</div>
			{/if}
		</div>

		<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900">
			<h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('vetoDetail.authorActions')}</h2>
			<div class="mt-3 flex flex-wrap gap-2">
				<Button variant="secondary" onclick={() => goto(`/proposals/${proposalId}/edit`)}>{$t('vetoDetail.editProposal')}</Button>
				{#if meId === vetoEvent.vetoer_id && vetoEvent.status === 'active'}
					<Button variant="danger" onclick={withdrawVeto} loading={actionLoading}>{$t('vetoDetail.withdrawVeto')}</Button>
				{/if}
			</div>
		</div>
	{/if}
</div>

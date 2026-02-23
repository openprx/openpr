<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { governanceExtApi, type ProposalChain, type TimelineEvent } from '$lib/api/governance-ext';
	import { toast } from '$lib/stores/toast';

	const proposalId = $derived($page.params.id || '');

	let loading = $state(true);
	let data = $state<ProposalChain | null>(null);
	let events = $state<TimelineEvent[]>([]);
	let noLinkedIssues = $state(false);

	const pageTitle = $derived.by(() => {
		if (data?.proposal.title) {
			return $t('pageTitle.proposalChainWithTitle', { values: { title: data.proposal.title } });
		}
		return $t('pageTitle.proposalChain');
	});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		noLinkedIssues = false;
		const [chainRes, timelineRes] = await Promise.all([
			governanceExtApi.getProposalChain(proposalId),
			governanceExtApi.getProposalTimeline(proposalId)
		]);
		if (chainRes.code !== 0 || !chainRes.data) {
			const message = (chainRes.message || '').toLowerCase();
			if (message.includes('proposal has no linked issues')) {
				noLinkedIssues = true;
				loading = false;
				return;
			}
			toast.error(chainRes.message || get(t)('governanceExt.loadFailed'));
			loading = false;
			return;
		}

		data = chainRes.data;
		noLinkedIssues = chainRes.data.issues.length === 0;
		events =
			timelineRes.code === 0 && timelineRes.data
				? timelineRes.data.events
				: [...chainRes.data.timeline].sort((a, b) => +new Date(a.timestamp) - +new Date(b.timestamp));
		loading = false;
	}

	function eventTone(eventType: string): string {
		if (eventType.includes('decision_approved')) return 'bg-emerald-100 text-emerald-700';
		if (eventType.includes('decision_rejected') || eventType.includes('decision_vetoed')) return 'bg-red-100 text-red-700';
		if (eventType.includes('review')) return 'bg-indigo-100 text-indigo-700';
		if (eventType.includes('vote')) return 'bg-amber-100 text-amber-700';
		return 'bg-slate-100 text-slate-700';
	}
 </script>

<svelte:head>
	<title>{pageTitle}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.chainTitle')}</h1>
		<p class="mt-1 text-sm text-slate-500">{$t('governanceExt.chainSubtitle')}</p>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-800">
			{$t('common.loading')}
		</div>
	{:else if noLinkedIssues}
		<div class="rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-700 dark:bg-slate-800">
			<p class="text-sm text-slate-600 dark:text-slate-300">{$t('governanceExt.chainEmptyNoLinkedIssues')}</p>
			<div class="mt-4">
				<a href={`/proposals/${proposalId}`} class="inline-flex rounded-md border border-slate-300 px-3 py-2 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900">
					{$t('governanceExt.backToProposal')}
				</a>
			</div>
		</div>
	{:else if data}
		<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
			<div class="mb-4 flex items-center justify-between gap-3">
				<div>
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{data.proposal.title}</h2>
					<p class="mt-1 text-xs text-slate-500">{data.proposal.id}</p>
				</div>
				<a href={`/proposals/${data.proposal.id}`} class="rounded-md border border-slate-300 px-3 py-2 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900">
					{$t('governanceExt.backToProposal')}
				</a>
			</div>

			<div class="flex flex-wrap items-center gap-2 text-xs">
				<div class="rounded border border-blue-200 bg-blue-50 px-3 py-2 text-blue-700">{data.proposal.status}</div>
				<span class="text-slate-400">→</span>
				<div class="rounded border border-slate-200 bg-slate-50 px-3 py-2 text-slate-700">
					{$t('governanceExt.graphIssueCount', { values: { count: data.issues.length } })}
				</div>
				<span class="text-slate-400">→</span>
				<div class="rounded border border-amber-200 bg-amber-50 px-3 py-2 text-amber-700">
					{data.decision
						? $t('governanceExt.graphDecision', { values: { result: data.decision.result } })
						: $t('governanceExt.graphNoDecision')}
				</div>
				<span class="text-slate-400">→</span>
				<div class="rounded border border-indigo-200 bg-indigo-50 px-3 py-2 text-indigo-700">
					{data.impact_review
						? $t('governanceExt.graphReview', { values: { status: data.impact_review.status } })
						: $t('governanceExt.graphNoReview')}
				</div>
			</div>
		</div>

		<div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
			<div class="space-y-4 lg:col-span-2">
				<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
					<h3 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.timeline')}</h3>
					<div class="space-y-2">
						{#if events.length === 0}
							<p class="text-sm text-slate-500">{$t('common.noData')}</p>
						{:else}
							{#each events as event}
								<div class="flex gap-2 rounded-md border border-slate-200 p-3 dark:border-slate-700">
									<span class={`rounded px-2 py-0.5 text-xs ${eventTone(event.event_type)}`}>{event.event_type}</span>
									<div class="min-w-0 flex-1 text-xs">
										<p class="text-slate-700 dark:text-slate-200">{event.description}</p>
										<p class="mt-1 text-slate-500">
											{new Date(event.timestamp).toLocaleString()} · {event.actor || '-'}
										</p>
									</div>
								</div>
							{/each}
						{/if}
					</div>
				</div>
			</div>

			<div class="space-y-4">
				<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
					<h3 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.votes')}</h3>
					<p class="text-xs text-slate-500">{$t('governanceExt.voteCount', { values: { count: data.votes.length } })}</p>
					<div class="mt-2 max-h-72 space-y-2 overflow-auto">
						{#each data.votes as vote}
							<div class="rounded border border-slate-200 px-2 py-1 text-xs dark:border-slate-700">
								<p>{vote.voter_id} · {vote.choice}</p>
								<p class="text-slate-500">{new Date(vote.voted_at).toLocaleString()}</p>
							</div>
						{/each}
					</div>
				</div>

				<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
					<h3 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governanceExt.feedbackProposals')}</h3>
					{#if data.feedback_proposals.length === 0}
						<p class="text-xs text-slate-500">{$t('common.noData')}</p>
					{:else}
						<div class="space-y-2">
							{#each data.feedback_proposals as feedback}
								<a href={`/proposals/${feedback.proposal_id}`} class="block rounded border border-slate-200 px-2 py-1 text-xs hover:bg-slate-50 dark:border-slate-700 dark:hover:bg-slate-900">
									<p class="font-medium text-slate-700 dark:text-slate-200">{feedback.title}</p>
									<p class="text-slate-500">{feedback.link_type}</p>
								</a>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		</div>
	{/if}
</div>

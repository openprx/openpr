<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { FlowHistoryEntry, FlowObjectView } from '$lib/api/flow';
	import { FlowCommandService } from '$lib/flow/command-service';
	import { toast } from '$lib/stores/toast';

	const commandService = new FlowCommandService();

	interface Props {
		object: FlowObjectView;
	}

	let { object }: Props = $props();

	let history = $state<FlowHistoryEntry[]>([]);
	let historyLoading = $state(true);
	let nextBeforeSeq = $state<number | undefined>(undefined);

	async function loadHistory(before?: number) {
		historyLoading = true;
		const result = await commandService.getHistory(object.id, { before_seq: before, limit: 20 });
		if (result.code === 0 && result.data) {
			history = before ? [...history, ...result.data.items] : result.data.items;
			nextBeforeSeq = result.data.next_before_seq;
		}
		historyLoading = false;
	}

	onMount(() => {
		void loadHistory();
	});

	$effect(() => {
		// Re-load whenever the panel is pointed at a different object.
		void object.id;
		void loadHistory();
	});

	async function copyReference() {
		try {
			await navigator.clipboard.writeText(object.id);
			toast.success($t('flow.panel.copied'));
		} catch {
			toast.error($t('flow.panel.copyReference'));
		}
	}

	function formatTime(value: string): string {
		try {
			return new Date(value).toLocaleString();
		} catch {
			return value;
		}
	}

	/** `FlowHistoryEntry.semantic_summary` is a structured JSON object server-side (`accepted_seq`,
	 * `changed_block_ids`, ...), not a rendered string -- render the fields a human can act on. */
	function formatSummary(summary: unknown): string {
		if (summary && typeof summary === 'object' && 'accepted_seq' in summary) {
			const s = summary as { accepted_seq: number; changed_block_ids?: unknown[] };
			const blocks = Array.isArray(s.changed_block_ids) ? s.changed_block_ids.length : 0;
			return `seq ${s.accepted_seq}${blocks > 0 ? ` · ${blocks}` : ''}`;
		}
		return typeof summary === 'string' ? summary : '';
	}
</script>

<aside
	class="flex h-full w-80 shrink-0 flex-col gap-6 overflow-y-auto border-l border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900"
	aria-label={$t('flow.panel.title')}
>
	<section>
		<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('flow.panel.title')}</h2>
		<dl class="space-y-2 text-sm">
			<div class="flex items-center justify-between gap-2">
				<dt class="text-slate-500 dark:text-slate-400">{$t('flow.panel.objectId')}</dt>
				<dd class="flex items-center gap-2">
					<code class="truncate text-xs text-slate-600 dark:text-slate-300" title={object.id}>
						{object.id.slice(0, 8)}…
					</code>
					<button
						type="button"
						class="text-xs font-medium text-blue-600 hover:underline dark:text-blue-400"
						onclick={copyReference}
					>
						{$t('flow.panel.copyReference')}
					</button>
				</dd>
			</div>
			<div class="flex items-center justify-between">
				<dt class="text-slate-500 dark:text-slate-400">{$t('flow.panel.created')}</dt>
				<dd class="text-slate-700 dark:text-slate-200">{formatTime(object.created_at)}</dd>
			</div>
			<div class="flex items-center justify-between">
				<dt class="text-slate-500 dark:text-slate-400">{$t('flow.panel.updated')}</dt>
				<dd class="text-slate-700 dark:text-slate-200">{formatTime(object.updated_at)}</dd>
			</div>
		</dl>
	</section>

	<section>
		<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('flow.history.title')}</h2>
		{#if historyLoading && history.length === 0}
			<p class="text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</p>
		{:else if history.length === 0}
			<p class="text-sm text-slate-500 dark:text-slate-400">{$t('flow.history.empty')}</p>
		{:else}
			<ul class="space-y-3">
				{#each history as entry (entry.seq)}
					<li class="border-l-2 border-slate-200 pl-3 text-sm dark:border-slate-700">
						<p class="text-slate-700 dark:text-slate-200">{formatSummary(entry.semantic_summary)}</p>
						<p class="text-xs text-slate-400 dark:text-slate-500">
							{$t('flow.history.entry', { values: { actor: entry.actor, time: formatTime(entry.created_at) } })}
						</p>
					</li>
				{/each}
			</ul>
			{#if nextBeforeSeq}
				<button
					type="button"
					class="mt-3 text-xs font-medium text-blue-600 hover:underline dark:text-blue-400"
					onclick={() => loadHistory(nextBeforeSeq)}
				>
					{$t('flow.history.loadMore')}
				</button>
			{/if}
		{/if}
	</section>

	<!-- `flow.relations.*`: the right-panel Relations area is v0.5 scope
		 (`ui-surface-v1.md` "后续版本 UI 派生" v0.5: "右 panel 增加 lazy-loaded Relations 区"). The
		 section exists rather than the key sitting unused/hidden, honestly labelled unavailable
		 instead of a placeholder that could be mistaken for real content. -->
	<section>
		<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('flow.relations.title')}</h2>
		<p class="text-sm text-slate-500 dark:text-slate-400">{$t('flow.relations.comingSoon')}</p>
	</section>
</aside>

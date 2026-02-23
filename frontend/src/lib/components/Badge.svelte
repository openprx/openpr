<script lang="ts">
	import { t } from 'svelte-i18n';
	import { get } from 'svelte/store';
	import type { IssuePriority, IssueStatus } from '$lib/api/issues';

	type BadgeValue = IssuePriority | IssueStatus | string;

	interface Props {
		value: BadgeValue;
		kind?: 'priority' | 'status';
		size?: 'sm' | 'md';
	}

	let { value, kind = 'status', size = 'sm' }: Props = $props();

	const priorityStyles: Record<string, string> = {
		low: 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300 border-slate-200 dark:border-slate-700',
		medium: 'bg-blue-500/20 dark:bg-blue-500/30 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-500/40',
		high: 'bg-orange-500/20 dark:bg-orange-500/30 text-orange-700 dark:text-orange-300 border-orange-200 dark:border-orange-500/40',
		urgent: 'bg-red-500/20 dark:bg-red-500/30 text-red-700 dark:text-red-300 border-red-200 dark:border-red-500/40'
	};

	const statusStyles: Record<string, string> = {
		backlog: 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300 border-slate-200 dark:border-slate-700',
		todo: 'bg-blue-500/20 dark:bg-blue-500/30 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-500/40',
		in_progress: 'bg-amber-500/20 dark:bg-amber-500/30 text-amber-700 dark:text-amber-300 border-amber-200 dark:border-amber-500/40',
		done: 'bg-emerald-500/20 dark:bg-emerald-500/30 text-emerald-700 dark:text-emerald-300 border-emerald-200 dark:border-emerald-500/40'
	};

	function getLabel(val: string): string {
		// Map internal values to i18n keys
		const i18nKeys: Record<string, string> = {
			backlog: 'issue.backlog',
			todo: 'issue.todo',
			in_progress: 'issue.inProgress',
			done: 'issue.done',
			low: 'issue.low',
			medium: 'issue.medium',
			high: 'issue.high',
			urgent: 'issue.urgent'
		};
		
		const key = i18nKeys[val];
		return key ? get(t)(key) : val;
	}

	const sizeClass = $derived(size === 'md' ? 'px-2.5 py-1 text-xs' : 'px-2 py-0.5 text-[11px]');
	const toneClass = $derived(
		kind === 'priority'
			? (priorityStyles[value] ?? 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300 border-slate-200 dark:border-slate-700')
			: (statusStyles[value] ?? 'bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300 border-slate-200 dark:border-slate-700')
	);
	const text = $derived(getLabel(value));
</script>

<span class={`inline-flex items-center rounded-md border font-medium ${sizeClass} ${toneClass}`}>
	{text}
</span>

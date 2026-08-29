<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { SyncState } from '$lib/flow/types';

	interface Props {
		state: SyncState;
	}

	let { state }: Props = $props();

	const DOT_CLASS: Record<SyncState, string> = {
		local: 'bg-slate-400',
		saving: 'bg-amber-500 animate-pulse',
		saved: 'bg-emerald-500',
		offline: 'bg-slate-400',
		reconnecting: 'bg-amber-500 animate-pulse',
		resyncing: 'bg-amber-500 animate-pulse',
		auth_required: 'bg-red-500',
		read_only: 'bg-slate-400',
		error: 'bg-red-500'
	};

	const dotClass = $derived(DOT_CLASS[state]);
	const label = $derived($t(`flow.sync.${state}`));
</script>

<div
	class="flex items-center gap-2 rounded-full border border-slate-200 bg-white px-3 py-1 text-xs font-medium text-slate-600 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300"
	role="status"
	aria-live="polite"
>
	<span class={`h-2 w-2 rounded-full ${dotClass}`} aria-hidden="true"></span>
	<span>{label}</span>
</div>

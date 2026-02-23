<script lang="ts">
	import { t } from 'svelte-i18n';
	interface Props {
		label: string;
		color?: string;
		removable?: boolean;
		onremove?: () => void;
	}

	let { label, color = '#64748b', removable = false, onremove }: Props = $props();
</script>

<span class="inline-flex items-center gap-1 rounded-full border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 px-2.5 py-1 text-xs text-slate-700 dark:text-slate-300">
	<span class="h-2 w-2 rounded-full" style={`background-color: ${color};`}></span>
	<span>{label}</span>
	{#if removable}
		<button
			type="button"
			onclick={onremove}
			class="rounded p-0.5 text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800 dark:bg-slate-800 hover:text-slate-700 dark:text-slate-300"
			aria-label={$t('activity.labelRemoved', { values: { name: label } })}
		>
			x
		</button>
	{/if}
</span>

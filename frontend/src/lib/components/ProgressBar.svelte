<script lang="ts">
	import { t } from 'svelte-i18n';
	interface Props {
		value: number;
		label?: string;
		showPercentage?: boolean;
	}

	let { value, label, showPercentage = true }: Props = $props();

	const safeValue = $derived(Math.min(100, Math.max(0, Math.round(value))));
</script>

<div class="space-y-1">
	{#if label || showPercentage}
		<div class="flex items-center justify-between text-xs text-slate-600 dark:text-slate-300">
			<span>{label ?? $t('common.status')}</span>
			{#if showPercentage}
				<span>{safeValue}%</span>
			{/if}
		</div>
	{/if}
	<div class="h-2 w-full overflow-hidden rounded-full bg-slate-200">
		<div class="h-full rounded-full bg-blue-600 transition-all" style={`width:${safeValue}%`}></div>
	</div>
</div>

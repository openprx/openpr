<script lang="ts">
	import { Bot, FileText, LayoutPanelTop, Shield, Upload } from '@lucide/svelte';
	import { t } from 'svelte-i18n';

	export type FormWorkflowMode =
		| 'data'
		| 'detail'
		| 'design'
		| 'layout'
		| 'permissions'
		| 'importExport'
		| 'automation';

	interface Props {
		activeMode: FormWorkflowMode;
		onModeChange?: (mode: FormWorkflowMode) => void;
	}

	let { activeMode, onModeChange }: Props = $props();

	const modes: Array<{ key: FormWorkflowMode; icon: typeof LayoutPanelTop }> = [
		{ key: 'design', icon: LayoutPanelTop },
		{ key: 'layout', icon: FileText },
		{ key: 'permissions', icon: Shield },
		{ key: 'importExport', icon: Upload },
		{ key: 'automation', icon: Bot }
	];
</script>

<div class="border-b border-slate-200 dark:border-slate-700">
	<div class="flex flex-wrap gap-1 px-1">
		{#each modes as mode}
			{@const Icon = mode.icon}
			<button
				type="button"
				class="inline-flex h-10 items-center gap-2 border-b-2 px-3 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 {activeMode ===
				mode.key
					? 'border-blue-600 text-blue-700 dark:border-blue-300 dark:text-blue-200'
					: 'border-transparent text-slate-600 hover:border-slate-300 hover:text-slate-950 dark:text-slate-400 dark:hover:border-slate-600 dark:hover:text-slate-100'}"
				aria-current={activeMode === mode.key ? 'page' : undefined}
				onclick={() => onModeChange?.(mode.key)}
			>
				<Icon class="h-4 w-4" aria-hidden="true" />
				<span>{$t(`forms.modes.${mode.key}`)}</span>
			</button>
		{/each}
	</div>
</div>

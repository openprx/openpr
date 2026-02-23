<script lang="ts">
	type EmptyStateIcon = 'clipboard' | 'box' | 'search' | 'office';

	interface Props {
		icon?: EmptyStateIcon;
		title: string;
		description?: string;
		action?: import('svelte').Snippet;
	}

	let { icon = 'clipboard', title, description, action }: Props = $props();
</script>

<div class="bg-white dark:bg-slate-900 rounded-lg border border-slate-200 dark:border-slate-700 p-12 text-center">
	<div class="mb-4 flex justify-center text-slate-400">
		{#if icon === 'box'}
			<svg class="h-12 w-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M20 7l-8-4-8 4m16 0l-8 4m8-4v10l-8 4m0-10L4 7m8 4v10M4 7v10l8 4"></path>
			</svg>
		{:else if icon === 'search'}
			<svg class="h-12 w-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-4.35-4.35m1.35-5.65a7 7 0 11-14 0 7 7 0 0114 0z"></path>
			</svg>
		{:else if icon === 'office'}
			<svg class="h-12 w-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 21h18M5 21V7l7-4 7 4v14M9 9h1m4 0h1m-6 4h1m4 0h1"></path>
			</svg>
		{:else}
			<svg class="h-12 w-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h7l5 5v11a2 2 0 01-2 2z"></path>
			</svg>
		{/if}
	</div>
	<h3 class="text-lg font-semibold text-slate-900 dark:text-slate-100 mb-2">{title}</h3>
	{#if description}
		<p class="text-slate-600 dark:text-slate-300 mb-6 max-w-md mx-auto">{description}</p>
	{/if}
	{#if action}
		<div class="flex justify-center">
			{@render action()}
		</div>
	{/if}
</div>

<script lang="ts">
	interface Props {
		hover?: boolean;
		clickable?: boolean;
		padding?: 'none' | 'sm' | 'md' | 'lg';
		onclick?: () => void;
		children?: import('svelte').Snippet;
	}

	let { hover = false, clickable = false, padding = 'md', onclick, children }: Props = $props();

	const paddingClasses = {
		none: '',
		sm: 'p-4',
		md: 'p-6',
		lg: 'p-8'
	};

	const className = $derived(
		`rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-800 dark:shadow-slate-900/50 ${paddingClasses[padding]} ${
			hover || clickable ? 'hover:shadow-lg dark:shadow-slate-900/50 dark:hover:shadow-slate-900/50 transition-shadow' : ''
		} ${clickable ? 'cursor-pointer' : ''}`
	);
</script>

{#if clickable && onclick}
	<button {onclick} class={`${className} text-left w-full`}>
		{#if children}
			{@render children()}
		{/if}
	</button>
{:else}
	<div class={className}>
		{#if children}
			{@render children()}
		{/if}
	</div>
{/if}

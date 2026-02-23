<script lang="ts">
	interface Props {
		variant?: 'primary' | 'secondary' | 'danger' | 'dangerSolid' | 'ghost';
		size?: 'sm' | 'md' | 'lg';
		disabled?: boolean;
		loading?: boolean;
		type?: 'button' | 'submit' | 'reset';
		onclick?: () => void;
		children?: import('svelte').Snippet;
	}

	let {
		variant = 'primary',
		size = 'md',
		disabled = false,
		loading = false,
		type = 'button',
		onclick,
		children
	}: Props = $props();

	const baseClasses =
		'inline-flex items-center justify-center font-medium rounded-md transition-colors focus:outline-none focus:ring-2 focus:ring-offset-2 dark:focus:ring-offset-slate-900 disabled:opacity-50 disabled:cursor-not-allowed';

	const variantClasses = {
		primary:
			'bg-blue-600 text-white hover:bg-blue-700 focus:ring-blue-500 dark:focus:ring-blue-400 disabled:hover:bg-blue-600',
		secondary:
			'bg-transparent text-slate-600 hover:text-slate-900 hover:bg-slate-100 focus:ring-slate-500 dark:text-slate-400 dark:hover:text-slate-200 dark:hover:bg-slate-800 dark:focus:ring-slate-400 disabled:hover:bg-transparent dark:disabled:hover:bg-transparent',
		danger:
			'bg-transparent border border-slate-300 text-slate-600 hover:border-red-300 hover:text-red-600 hover:bg-red-50 focus:ring-red-500 dark:border-slate-700 dark:text-slate-400 dark:hover:border-red-700 dark:hover:text-red-400 dark:hover:bg-red-950/30 dark:focus:ring-red-400',
		dangerSolid: 'bg-red-600 text-white hover:bg-red-700 focus:ring-red-500 dark:focus:ring-red-400 disabled:hover:bg-red-600',
		ghost:
			'bg-transparent text-slate-700 hover:bg-slate-100 dark:bg-slate-800 focus:ring-slate-500 dark:focus:ring-slate-400 disabled:hover:bg-transparent dark:text-slate-200 dark:hover:bg-slate-700'
	};

	const sizeClasses = {
		sm: 'px-3 py-1.5 text-sm',
		md: 'px-4 py-2 text-sm',
		lg: 'px-6 py-3 text-base'
	};

	const className = $derived(`${baseClasses} ${variantClasses[variant]} ${sizeClasses[size]}`);
</script>

<button {type} class={className} disabled={disabled || loading} {onclick}>
	{#if loading}
		<svg class="animate-spin -ml-1 mr-2 h-4 w-4" fill="none" viewBox="0 0 24 24">
			<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"
			></circle>
			<path
				class="opacity-75"
				fill="currentColor"
				d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
			></path>
		</svg>
	{/if}
	{#if children}
		{@render children()}
	{/if}
</button>

<script lang="ts">
	import type { HTMLSelectAttributes } from 'svelte/elements';

	type SelectEventHandler = NonNullable<HTMLSelectAttributes['onchange']>;

	export interface SelectOption {
		label: string;
		value: string;
		disabled?: boolean;
	}

	interface Props {
		label?: string;
		error?: string;
		hint?: string;
		helperText?: string;
		value?: string;
		options: SelectOption[];
		required?: boolean;
		disabled?: boolean;
		id?: string;
		name?: string;
		placeholder?: string;
		onchange?: SelectEventHandler;
	}

	let {
		label,
		error,
		hint,
		helperText,
		value = $bindable(''),
		options,
		required = false,
		disabled = false,
		id,
		name,
		placeholder,
		onchange
	}: Props = $props();

	const selectId = $derived(id || `select-${Math.random().toString(36).substring(7)}`);

	const selectClasses = $derived(
		`block w-full px-3 py-2 border rounded-md shadow-sm dark:shadow-slate-900/50 transition-colors focus:outline-none focus:ring-2 focus:ring-offset-0 dark:focus:ring-offset-slate-900 ${
			error
				? 'border-red-300 focus:border-red-500 focus:ring-red-500 dark:focus:ring-red-400'
				: 'border-slate-300 dark:border-slate-600 focus:border-blue-500 focus:ring-blue-500 dark:focus:ring-blue-400'
		} ${disabled ? 'bg-slate-50 dark:bg-slate-700 cursor-not-allowed' : 'bg-white dark:bg-slate-800'}`
	);
</script>

<div class="space-y-1">
	{#if label}
		<label for={selectId} class="block text-sm font-medium text-slate-700 dark:text-slate-300">
			{label}
			{#if required}
				<span class="text-red-500">*</span>
			{/if}
		</label>
	{/if}

	<select
		id={selectId}
		{name}
		bind:value
		{required}
		{disabled}
		{onchange}
		class={`${selectClasses} text-slate-900 dark:text-slate-100`}
	>
		{#if placeholder}
			<option value="" disabled>{placeholder}</option>
		{/if}
		{#each options as option (option.value)}
			<option value={option.value} disabled={option.disabled}>{option.label}</option>
		{/each}
	</select>

	{#if error}
		<p class="text-sm text-red-600">{error}</p>
	{:else if helperText || hint}
		<p class="text-sm text-slate-500 dark:text-slate-400">{helperText ?? hint}</p>
	{/if}
</div>

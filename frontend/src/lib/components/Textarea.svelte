<script lang="ts">
	import type { HTMLTextareaAttributes } from 'svelte/elements';

type TextareaEventHandler = NonNullable<HTMLTextareaAttributes['oninput']>;
type TextareaPasteEventHandler = NonNullable<HTMLTextareaAttributes['onpaste']>;

	interface Props {
		label?: string;
		error?: string;
		hint?: string;
		helperText?: string;
		placeholder?: string;
		value?: string;
		rows?: number;
		required?: boolean;
		disabled?: boolean;
		readonly?: boolean;
		maxlength?: number;
		minlength?: number;
		id?: string;
		name?: string;
		oninput?: TextareaEventHandler;
		onpaste?: TextareaPasteEventHandler;
		element?: HTMLTextAreaElement | null;
	}

	let {
		label,
		error,
		hint,
		helperText,
		placeholder,
		value = $bindable(''),
		rows = 4,
		required = false,
		disabled = false,
		readonly = false,
		maxlength,
		minlength,
		id,
		name,
		oninput,
		onpaste,
		element = $bindable(null)
	}: Props = $props();

	const textareaId = $derived(id || `textarea-${Math.random().toString(36).substring(7)}`);

	const textareaClasses = $derived(
		`block w-full px-3 py-2 border rounded-md shadow-sm dark:shadow-slate-900/50 transition-colors focus:outline-none focus:ring-2 focus:ring-offset-0 dark:focus:ring-offset-slate-900 ${
			error
				? 'border-red-300 focus:border-red-500 focus:ring-red-500 dark:focus:ring-red-400'
				: 'border-slate-300 dark:border-slate-600 focus:border-blue-500 focus:ring-blue-500 dark:focus:ring-blue-400'
		} ${disabled ? 'bg-slate-50 dark:bg-slate-700 cursor-not-allowed' : 'bg-white dark:bg-slate-800'} ${readonly ? 'bg-slate-50 dark:bg-slate-700' : ''}`
	);
</script>

<div class="space-y-1">
	{#if label}
		<label for={textareaId} class="block text-sm font-medium text-slate-700 dark:text-slate-300">
			{label}
			{#if required}
				<span class="text-red-500">*</span>
			{/if}
		</label>
	{/if}

	<textarea
		id={textareaId}
		{name}
		{placeholder}
		bind:value
		{rows}
		{required}
		{disabled}
		{readonly}
		{maxlength}
		{minlength}
		{oninput}
		{onpaste}
		bind:this={element}
		class={`${textareaClasses} text-slate-900 placeholder:text-slate-400 dark:text-slate-100 dark:placeholder-slate-500`}
	></textarea>

	{#if error}
		<p class="text-sm text-red-600">{error}</p>
	{:else if helperText || hint}
		<p class="text-sm text-slate-500 dark:text-slate-400">{helperText ?? hint}</p>
	{/if}
</div>

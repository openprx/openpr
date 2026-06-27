<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { FormField } from '$lib/api/forms';

	interface Props {
		field: FormField | null;
		value: unknown;
		empty?: string;
	}

	let { field, value, empty = '-' }: Props = $props();

	function isRecord(input: unknown): input is Record<string, unknown> {
		return typeof input === 'object' && input !== null && !Array.isArray(input);
	}

	function sanitizeOptionColor(input: unknown): string {
		if (typeof input !== 'string') return '';
		const trimmed = input.trim();
		if (/^#[0-9a-f]{6}$/i.test(trimmed) || /^#[0-9a-f]{3}$/i.test(trimmed)) return trimmed;
		return '';
	}

	function optionColor(option: string): string {
		if (!field) return '';
		return sanitizeOptionColor(field.option_colors?.[option] ?? field.option_colors?.[option.trim()]);
	}

	function optionDescription(option: string): string {
		return field?.option_descriptions?.[option]?.trim() ?? field?.option_descriptions?.[option.trim()]?.trim() ?? '';
	}

	function optionGroup(option: string): string {
		return field?.option_groups?.[option]?.trim() ?? field?.option_groups?.[option.trim()]?.trim() ?? '';
	}

	function optionArchived(option: string): boolean {
		return Boolean(field?.option_archived?.includes(option) || field?.option_archived?.includes(option.trim()));
	}

	function optionDisplayLabel(option: string): string {
		const group = optionGroup(option);
		return group ? `${group} / ${option}` : option;
	}

	function optionStyle(option: string): string {
		const color = optionColor(option);
		return color ? `--option-color: ${color};` : '';
	}

	function optionItems(): string[] {
		if (!field || !['single_select', 'multi_select'].includes(field.type ?? 'text')) return [];
		if (field.type === 'multi_select') {
			return Array.isArray(value)
				? value
						.map((item) => (typeof item === 'string' || typeof item === 'number' ? String(item).trim() : ''))
						.filter(Boolean)
				: [];
		}
		if (typeof value === 'string' || typeof value === 'number') {
			const item = String(value).trim();
			return item ? [item] : [];
		}
		return [];
	}

	function displayValue(input: unknown): string {
		if (input === null || input === undefined || input === '') return empty;
		if (typeof input === 'boolean') return input ? $t('forms.yes') : $t('forms.no');
		if (typeof input === 'string' || typeof input === 'number') return String(input);
		if (Array.isArray(input)) return input.map(displayValue).join(', ');
		if (isRecord(input)) {
			if (typeof input.display === 'string') return input.display;
			if (typeof input.title === 'string') return input.title;
			if (typeof input.decimal === 'string') {
				return `${input.decimal}${typeof input.currency === 'string' ? ` ${input.currency}` : ''}`;
			}
			if (typeof input.value === 'number') return String(input.value);
			return JSON.stringify(input);
		}
		return String(input);
	}

	const items = $derived(optionItems());
</script>

{#if items.length > 0}
	<span class="inline-flex max-w-full flex-wrap gap-1 align-middle">
		{#each items as item}
			<span
				class={optionColor(item)
					? 'inline-flex max-w-full items-center gap-1 rounded px-1.5 py-0.5 text-xs font-medium text-slate-800 ring-1 ring-[color:var(--option-color)]/30 dark:text-slate-100'
					: 'inline-flex max-w-full items-center gap-1 rounded bg-slate-100 px-1.5 py-0.5 text-xs font-medium text-slate-700 dark:bg-slate-800 dark:text-slate-200'}
				style={optionColor(item)
					? `${optionStyle(item)} background-color: color-mix(in srgb, var(--option-color) 16%, transparent);`
					: undefined}
				data-option-color={field ? `${field.key}:${item}` : item}
				data-option-color-hex={optionColor(item) || undefined}
				data-option-archived={optionArchived(item) ? `${field?.key}:${item}` : undefined}
				data-option-description={optionDescription(item) || undefined}
				data-option-group={optionGroup(item) || undefined}
				title={optionDescription(item) || undefined}
			>
				{#if optionColor(item)}
					<span
						class="h-2 w-2 shrink-0 rounded-full"
						style={`background-color: ${optionColor(item)}`}
						aria-hidden="true"
					></span>
				{/if}
				<span class="min-w-0 truncate">{optionDisplayLabel(item)}</span>
			</span>
		{/each}
	</span>
{:else}
	{displayValue(value)}
{/if}

<script lang="ts">
	interface Props {
		name?: string;
		userId?: string;
		size?: 'sm' | 'md';
	}

	let { name, userId, size = 'sm' }: Props = $props();

	const source = $derived((name?.trim() || userId?.trim() || '?').toUpperCase());
	const initials = $derived(source.split(/\s+/).filter(Boolean).slice(0, 2).map((item) => item[0]).join(''));
	const sizeClass = $derived(size === 'md' ? 'h-8 w-8 text-xs' : 'h-6 w-6 text-[10px]');
</script>

<div
	class={`inline-flex items-center justify-center rounded-full bg-blue-100 text-blue-700 font-semibold ${sizeClass}`}
	title={name || userId || 'Unassigned'}
>
	{initials}
</div>

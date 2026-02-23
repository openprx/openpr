<script lang="ts">
	interface Props {
		class?: string;
		title?: string;
	}

	let { class: className = 'h-4 w-4', title = 'Bot' }: Props = $props();
</script>

<svg
	xmlns="http://www.w3.org/2000/svg"
	viewBox="0 0 24 24"
	fill="none"
	stroke="currentColor"
	stroke-width="1.8"
	stroke-linecap="round"
	stroke-linejoin="round"
	class={className}
	aria-label={title}
>
	<title>{title}</title>
	<path d="M12 2v3" />
	<rect x="5" y="7" width="14" height="11" rx="3" />
	<circle cx="10" cy="12" r="1" />
	<circle cx="14" cy="12" r="1" />
	<path d="M9 15h6" />
	<path d="M5 11H3" />
	<path d="M21 11h-2" />
</svg>

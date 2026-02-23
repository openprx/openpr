<script lang="ts">
	import '$lib/i18n';
	import { onMount } from 'svelte';
	import '../app.css';
	import favicon from '$lib/assets/favicon.svg';
	import Toast from '$lib/components/Toast.svelte';
	import { applyTheme } from '$lib/theme';

	let { children } = $props();

	onMount(() => {
		const saved = localStorage.getItem('theme');
		const nextTheme = saved === 'light' || saved === 'dark' || saved === 'system' ? saved : 'light';
		applyTheme(nextTheme);
	});
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

{@render children()}
<Toast />

<script lang="ts">
	import { setContext } from 'svelte';
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import FlowNavigator from '$lib/components/flow/FlowNavigator.svelte';
	import { FlowObjectRepository } from '$lib/flow/object-repository';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';

	let { data, children } = $props();

	// One repository per Flow route-tree mount, shared by the navigator and whichever
	// `[objectId]/+page.svelte` is currently rendered underneath -- this is what lets navigating
	// between two open Page tabs reuse the same engine-doc lifecycle rules instead of each page
	// route reinventing "how do I open an object".
	const repository = new FlowObjectRepository();
	setContext(FLOW_REPOSITORY_CONTEXT, repository);

	const selectedObjectId = $derived(($page.params as { objectId?: string }).objectId ?? null);
</script>

{#if !data.flowEnabled}
	<div class="flex h-full min-h-[60vh] flex-col items-center justify-center gap-2 p-8 text-center">
		<h1 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('flow.route.disabledTitle')}</h1>
		<p class="max-w-md text-sm text-slate-500 dark:text-slate-400">{$t('flow.route.disabledBody')}</p>
	</div>
{:else}
	<div class="flex h-[calc(100vh-4rem)] overflow-hidden">
		<FlowNavigator workspaceId={data.workspaceId} {selectedObjectId} />
		<div class="flex flex-1 flex-col overflow-hidden">
			{@render children()}
		</div>
	</div>
{/if}

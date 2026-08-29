<script lang="ts">
	// The Flow object route. Opens exactly one `FlowObjectRepository` entry for this `objectId`
	// (ref-counted; closed on navigation away) and hands its `doc`/`projection`/`session.state`
	// down to the canvas, sync indicator, and context panel -- none of which ever touch the
	// network or the CRDT doc directly themselves.

	import { getContext, onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { requireRouteParam } from '$lib/utils/route-params';
	import type { FlowObjectRepository, OpenFlowObjectResult } from '$lib/flow/object-repository';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import FlowCanvas from '$lib/components/flow/FlowCanvas.svelte';
	import FlowContextPanel from '$lib/components/flow/FlowContextPanel.svelte';
	import FlowSyncIndicator from '$lib/components/flow/FlowSyncIndicator.svelte';
	import type { FlowError, SyncState } from '$lib/flow/types';

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);

	let entry = $state<OpenFlowObjectResult | null>(null);
	let loadError = $state<FlowError | null>(null);
	let syncState = $state<SyncState>('local');
	let title = $state('');
	let titleSaveTimer: ReturnType<typeof setTimeout> | null = null;

	let openedObjectId: string | null = null;

	async function openCurrent(): Promise<void> {
		const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
		const objectId = requireRouteParam($page.params.objectId, 'objectId');
		if (openedObjectId === objectId) return;

		if (openedObjectId) {
			await repository.close(openedObjectId);
			entry = null;
		}
		openedObjectId = objectId;
		loadError = null;

		const controller = new AbortController();
		try {
			const opened = await repository.open({ workspaceId, objectId, signal: controller.signal });
			if (openedObjectId !== objectId) return; // navigated away again before this resolved
			entry = opened;
			title = opened.handle.object.title;
			opened.session.state.subscribe((value) => (syncState = value));
			opened.flowError.subscribe((value) => {
				if (value) loadError = value;
			});
		} catch (error) {
			loadError = error as FlowError;
		}
	}

	onMount(() => {
		void openCurrent();
	});

	$effect(() => {
		void $page.params.objectId;
		void openCurrent();
	});

	onDestroy(() => {
		if (openedObjectId) void repository.close(openedObjectId);
	});

	function onTitleInput(): void {
		if (!entry) return;
		if (titleSaveTimer) clearTimeout(titleSaveTimer);
		titleSaveTimer = setTimeout(() => {
			if (entry) repository.setTitle(entry.handle.objectId, title);
		}, 400);
	}
</script>

{#if loadError}
	<div class="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center">
		<h1 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{loadError.code === 'not_found' ? $t('flow.route.notFoundTitle') : $t('flow.route.disabledTitle')}
		</h1>
		<p class="max-w-md text-sm text-slate-500 dark:text-slate-400">{$t(`flow.error.${loadError.code}`)}</p>
	</div>
{:else if !entry}
	<div class="flex flex-1 items-center justify-center p-8">
		<p class="text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</p>
	</div>
{:else}
	<div class="flex items-center justify-between border-b border-slate-200 px-6 py-3 dark:border-slate-800">
		<input
			bind:value={title}
			oninput={onTitleInput}
			placeholder={$t('flow.titlePlaceholder')}
			aria-label={$t('flow.panel.title')}
			class="flex-1 truncate border-none bg-transparent text-lg font-semibold text-slate-900 outline-none dark:text-slate-100"
		/>
		<FlowSyncIndicator state={syncState} />
	</div>
	<div class="flex flex-1 overflow-hidden">
		<FlowCanvas doc={entry.doc} />
		<FlowContextPanel object={entry.handle.object} />
	</div>
{/if}

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
	import { resolveRenderer } from '$lib/flow/renderer-registry';
	import { toast } from '$lib/stores/toast';
	import FlowCanvas from '$lib/components/flow/FlowCanvas.svelte';
	import FlowContextPanel from '$lib/components/flow/FlowContextPanel.svelte';
	import FlowSyncIndicator from '$lib/components/flow/FlowSyncIndicator.svelte';
	import type { FlowError, SyncState } from '$lib/flow/types';

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);

	let entry = $state<OpenFlowObjectResult | null>(null);
	// Set only from `openCurrent`'s catch, i.e. the object itself could not be opened at all --
	// this is the ONLY error that replaces the whole route body.
	let loadError = $state<FlowError | null>(null);
	// Set from `flowError` AFTER `entry` exists -- a rejected update, a limit violation, a policy
	// rejection, etc. mid-session. Shown as a dismissible inline banner; it must never unmount the
	// already-open editor/canvas, which would silently discard whatever the user was mid-edit on.
	// (Previously this bug existed: both cases wrote the same `loadError`, so a `policy_rejected`
	// on an open document flipped the `{#if loadError}` branch and replaced `FlowCanvas` with the
	// full-page error view.)
	let runtimeError = $state<FlowError | null>(null);
	let syncState = $state<SyncState>('local');
	let title = $state('');
	let titleSaveTimer: ReturnType<typeof setTimeout> | null = null;
	let reloading = $state(false);

	let openedObjectId: string | null = null;
	let unsubscribeState: (() => void) | null = null;
	let unsubscribeFlowError: (() => void) | null = null;

	function teardownSubscriptions(): void {
		unsubscribeState?.();
		unsubscribeState = null;
		unsubscribeFlowError?.();
		unsubscribeFlowError = null;
	}

	async function openCurrent(): Promise<void> {
		const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
		const objectId = requireRouteParam($page.params.objectId, 'objectId');
		if (openedObjectId === objectId) return;

		teardownSubscriptions();
		if (openedObjectId) {
			await repository.close(openedObjectId);
			entry = null;
		}
		openedObjectId = objectId;
		loadError = null;
		runtimeError = null;

		const controller = new AbortController();
		try {
			const opened = await repository.open({ workspaceId, objectId, signal: controller.signal });
			if (openedObjectId !== objectId) return; // navigated away again before this resolved
			entry = opened;
			title = opened.handle.object.title;
			unsubscribeState = opened.session.state.subscribe((value) => (syncState = value));
			unsubscribeFlowError = opened.flowError.subscribe((value) => {
				if (value) runtimeError = value;
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
		teardownSubscriptions();
		if (openedObjectId) void repository.close(openedObjectId);
	});

	function onTitleInput(): void {
		if (!entry) return;
		if (titleSaveTimer) clearTimeout(titleSaveTimer);
		titleSaveTimer = setTimeout(() => {
			if (entry) repository.setTitle(entry.handle.objectId, title);
		}, 400);
	}

	function dismissRuntimeError(): void {
		runtimeError = null;
	}

	// `policy_rejected`/`invalid_update`/`stale_frontier`: the rejected intent cannot be replayed
	// (no outbox this round -- see `object-session.ts`'s header comment), so offer a local
	// snapshot download instead of silently losing it (`ui-surface-v1.md` step 5: "不能重放的
	// intent 进入 recovery draft，并提供下载/复制动作").
	const canDownloadDraft = $derived(
		runtimeError !== null &&
			(runtimeError.code === 'policy_rejected' || runtimeError.code === 'invalid_update' || runtimeError.code === 'stale_frontier')
	);

	function downloadRecoveryDraft(): void {
		if (!entry) return;
		try {
			const blob = repository.exportRecoveryDraft(entry.handle.objectId);
			const url = URL.createObjectURL(blob);
			const link = document.createElement('a');
			link.href = url;
			link.download = `${entry.handle.objectId}-recovery-draft.bin`;
			document.body.appendChild(link);
			link.click();
			link.remove();
			URL.revokeObjectURL(url);
			toast.success($t('flow.recovery.downloaded'));
		} catch {
			toast.error($t('flow.recovery.downloadFailed'));
		}
	}

	// `stale_frontier`: this connection's local doc is behind the head the server just told it
	// about but did not push a resync for (only `resync_required` auto-resyncs). `bootstrap` +
	// `replaceWithAccepted` fetch a fresh accepted snapshot+tail over REST -- independent of the
	// live WebSocket session -- without discarding the rejected edit the user can still download.
	async function reloadFromServer(): Promise<void> {
		if (!entry) return;
		reloading = true;
		try {
			const bootstrap = await repository.bootstrap(entry.handle.objectId);
			await repository.replaceWithAccepted(bootstrap);
			runtimeError = null;
			toast.success($t('flow.recovery.reloaded'));
		} catch (error) {
			runtimeError = error as FlowError;
		} finally {
			reloading = false;
		}
	}

	// `contracts/ui-surface-v1.md` "Renderer registry": an unregistered `${objectType}:${viewType}`
	// must fail to read-only/unsupported, never reach the CRDT-backed canvas.
	const canvasRenderer = $derived(entry ? resolveRenderer(entry.handle.objectType, 'canvas') : null);
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

	{#if runtimeError}
		<div
			class="flex flex-wrap items-center gap-3 border-b border-amber-200 bg-amber-50 px-6 py-2 text-sm text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-100"
			role="alert"
		>
			<span class="flex-1">
				{runtimeError.code === 'server_draining'
					? $t(`flow.error.server_draining.${(runtimeError.details as { reason?: string } | undefined)?.reason === 'drain' ? 'drain' : 'contention'}`)
					: $t(`flow.error.${runtimeError.code}`)}
			</span>
			{#if canDownloadDraft}
				<button
					type="button"
					class="font-medium underline underline-offset-2 hover:no-underline"
					onclick={downloadRecoveryDraft}
				>
					{$t('flow.recovery.download')}
				</button>
				<button
					type="button"
					class="font-medium underline underline-offset-2 hover:no-underline disabled:opacity-50"
					disabled={reloading}
					onclick={reloadFromServer}
				>
					{$t('flow.recovery.reload')}
				</button>
			{/if}
			<button type="button" class="font-medium underline underline-offset-2 hover:no-underline" onclick={dismissRuntimeError}>
				{$t('flow.recovery.dismiss')}
			</button>
		</div>
	{/if}

	<div class="flex flex-1 overflow-hidden">
		{#if canvasRenderer}
			<FlowCanvas doc={entry.doc} />
		{:else}
			<div class="flex flex-1 items-center justify-center p-8 text-center">
				<p class="text-sm text-slate-500 dark:text-slate-400">{$t('flow.canvas.unsupported')}</p>
			</div>
		{/if}
		<FlowContextPanel object={entry.handle.object} />
	</div>
{/if}

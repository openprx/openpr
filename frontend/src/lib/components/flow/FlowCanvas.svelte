<script lang="ts">
	// Center canvas: mounts `EditorAdapter` (dynamic import, engine chunk only loads here) into a
	// host div, renders the slash menu overlay, and exposes nest/outdent via a per-block hover
	// handle in addition to the Tab/Shift-Tab keyboard shortcut `EditorAdapter` already wires up.
	//
	// This component owns zero content state itself: `FlowEditorAdapter` writes straight into the
	// Loro doc (owned by the route's `FlowObjectRepository` entry), and the DOM the browser shows
	// is ProseMirror's own render of that doc via `LoroSyncPlugin` -- there is no second copy of
	// block text sitting in this component's `$state`.

	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { FlowEditorAdapter, type FlowBlockType, type SlashMenuState } from '$lib/flow/editor-adapter';
	import type { LoroDocType } from 'loro-prosemirror';

	interface Props {
		doc: LoroDocType;
	}

	let { doc }: Props = $props();

	let host: HTMLDivElement | undefined = $state();
	let adapter: FlowEditorAdapter | null = null;
	let slashMenu = $state<SlashMenuState>({ active: false, query: '', coords: null });
	let hoverHandle = $state<{ top: number; pos: number } | null>(null);

	const SLASH_OPTIONS: ReadonlyArray<{ type: FlowBlockType; level?: number; labelKey: string }> = [
		{ type: 'paragraph', labelKey: 'flow.canvas.slashMenu.paragraph' },
		{ type: 'heading', level: 1, labelKey: 'flow.canvas.slashMenu.heading1' },
		{ type: 'heading', level: 2, labelKey: 'flow.canvas.slashMenu.heading2' },
		{ type: 'heading', level: 3, labelKey: 'flow.canvas.slashMenu.heading3' },
		{ type: 'bulletItem', labelKey: 'flow.canvas.slashMenu.bullet' },
		{ type: 'codeBlock', labelKey: 'flow.canvas.slashMenu.code' }
	];

	const filteredOptions = $derived(
		SLASH_OPTIONS.filter((option) => option.labelKey.toLowerCase().includes(slashMenu.query.toLowerCase()) || slashMenu.query === '')
	);

	onMount(() => {
		adapter = new FlowEditorAdapter(doc, {
			onSlashMenu: (state) => {
				slashMenu = state;
			}
		});
		if (host) void adapter.mount(host, 'root');
		return () => {
			void adapter?.destroy();
		};
	});

	onDestroy(() => {
		void adapter?.destroy();
	});

	function pick(option: (typeof SLASH_OPTIONS)[number]): void {
		adapter?.setBlockType(option.type, option.level);
		slashMenu = { active: false, query: '', coords: null };
	}

	function onHostMouseMove(event: MouseEvent): void {
		if (!adapter || !host) return;
		const pos = adapter.posAtClientPoint(event.clientX, event.clientY);
		if (pos === null) {
			hoverHandle = null;
			return;
		}
		const hostRect = host.getBoundingClientRect();
		hoverHandle = { pos, top: event.clientY - hostRect.top };
	}

	function onHostMouseLeave(): void {
		hoverHandle = null;
	}

	function adjustHoveredIndent(delta: number): void {
		if (!adapter || hoverHandle === null) return;
		adapter.adjustIndentAtPos(hoverHandle.pos, delta);
	}
</script>

<div
	class="relative flex-1 overflow-y-auto bg-white px-16 py-10 dark:bg-slate-950"
	role="presentation"
	onmousemove={onHostMouseMove}
	onmouseleave={onHostMouseLeave}
>
	{#if hoverHandle}
		<div
			class="absolute -left-1 flex -translate-x-full items-center gap-0.5 opacity-70 hover:opacity-100"
			style={`top: ${hoverHandle.top}px`}
		>
			<button
				type="button"
				class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
				onclick={() => adjustHoveredIndent(-1)}
				aria-label={$t('flow.block.outdent')}
				title={$t('flow.block.outdent')}
			>
				⇤
			</button>
			<button
				type="button"
				class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
				onclick={() => adjustHoveredIndent(1)}
				aria-label={$t('flow.block.nest')}
				title={$t('flow.block.nest')}
			>
				⇥
			</button>
			<span class="cursor-grab select-none px-1 text-slate-300 dark:text-slate-600" aria-label={$t('flow.block.handleLabel')}>
				⋮⋮
			</span>
		</div>
	{/if}

	<div
		bind:this={host}
		class="flow-canvas prose prose-slate mx-auto max-w-3xl dark:prose-invert focus:outline-none"
		data-placeholder={$t('flow.canvas.placeholder')}
	></div>

	{#if slashMenu.active && slashMenu.coords}
		<div
			class="fixed z-50 w-56 rounded-md border border-slate-200 bg-white py-1 shadow-lg dark:border-slate-700 dark:bg-slate-900"
			style={`left: ${slashMenu.coords.left}px; top: ${slashMenu.coords.top + 4}px`}
			role="listbox"
			aria-label={$t('flow.canvas.slashMenu.paragraph')}
		>
			{#if filteredOptions.length === 0}
				<p class="px-3 py-2 text-sm text-slate-400">{$t('flow.canvas.slashMenu.empty')}</p>
			{:else}
				{#each filteredOptions as option (option.labelKey + String(option.level ?? ''))}
					<button
						type="button"
						class="block w-full px-3 py-1.5 text-left text-sm text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
						onclick={() => pick(option)}
						role="option"
						aria-selected="false"
					>
						{$t(option.labelKey)}
					</button>
				{/each}
			{/if}
		</div>
	{/if}
</div>

<style>
	:global(.flow-canvas .ProseMirror) {
		min-height: 60vh;
		outline: none;
	}
	:global(.flow-canvas .ProseMirror p:first-child:last-child:empty::before) {
		content: attr(data-placeholder);
		color: theme('colors.slate.400');
		pointer-events: none;
	}
	:global(.flow-canvas [data-indent]) {
		margin-left: calc(var(--flow-indent, 0) * 1.5rem);
	}
	:global(.flow-canvas .flow-bullet-item) {
		position: relative;
		padding-left: 1.25rem;
	}
	:global(.flow-canvas .flow-bullet-item::before) {
		content: '•';
		position: absolute;
		left: 0.25rem;
	}
	:global(.flow-canvas pre) {
		background: theme('colors.slate.100');
		border-radius: 0.375rem;
		padding: 0.75rem;
		overflow-x: auto;
	}
	:global(.dark .flow-canvas pre) {
		background: theme('colors.slate.900');
	}
</style>

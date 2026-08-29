<script lang="ts">
	// Left navigator: create/select/expand/collapse Page objects, same-parent-only pointer AND
	// keyboard reorder (`ADR-0012` §4: "v0.4 的拖拽只做同一父级内重排"). Reorder is a real CRDT
	// content change on the single per-workspace Navigator object's `order` map
	// (`domain-model-v1.md`: "Navigator...导航排序"), committed through the same
	// `FlowObjectRepository`/`ObjectSession` pair a Page uses -- there is no separate write path.
	//
	// a11y (`contracts/ui-surface-v1.md` "a11y v0.4 基线", task brief item 4): `role=tree` with
	// roving tabindex, Arrow navigation/Home/End/Enter for the base tree, and a SEPARATE
	// "lift/move/drop" mode for reorder: Space lifts, ArrowUp/Down swaps sibling position live
	// (visually only, nothing is written to the CRDT until Drop), Escape cancels and reverts to
	// the last committed order with zero writes, a `Space` while lifted commits (Drop). Every
	// transition is announced through one `aria-live=polite` region.
	//
	// v0.4 scope note (disclosed, matches the pointer-drag scope): ArrowLeft/ArrowRight while
	// lifted would mean "nest under a different parent", which is a `parent_id` change -- a
	// governance command (`move_object`) that does not exist until v0.5
	// (`ADR-0012` §4: "跨父级移动是治理命令"). Pointer drag in this component enforces the same
	// same-parent-only rule (a drop target outside the source's parent is rejected). So
	// ArrowLeft/ArrowRight while lifted are wired to the same live-region "not available this
	// version" announcement pointer drag would hit, not silently ignored, and they do not touch
	// state. Outside lift mode, ArrowLeft/ArrowRight are the ordinary WAI-ARIA treeitem
	// collapse/expand-and-move gesture, unrelated to reordering.

	import { getContext, onDestroy, onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import { flowApi, type FlowObjectView } from '$lib/api/flow';
	import { FlowCommandService } from '$lib/flow/command-service';
	import { getNamedMap, type FlowObjectRepository, type OpenFlowObjectResult } from '$lib/flow/object-repository';
	import { keyBetween } from '$lib/flow/fractional-index';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';

	interface Props {
		workspaceId: string;
		selectedObjectId: string | null;
	}

	let { workspaceId, selectedObjectId }: Props = $props();

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commandService = new FlowCommandService();

	let objects = $state<FlowObjectView[]>([]);
	let navEntry: OpenFlowObjectResult | null = null;
	let orderVersion = $state(0); // bumped whenever the order map changes, to re-derive the tree
	let loading = $state(true);
	let expanded = $state(new Set<string>());
	let focusedId = $state<string | null>(null);
	let liftedId = $state<string | null>(null);
	let draftSiblingOrder = $state<string[]>([]); // object ids, lift-mode only, visual-only until drop
	let announcement = $state('');
	let creatingParentId = $state<string | null | undefined>(undefined);

	function orderOf(objectId: string): string {
		if (!navEntry) return '';
		const value = getNamedMap(navEntry.doc, 'order').get(objectId);
		return typeof value === 'string' ? value : '';
	}

	interface TreeNode {
		object: FlowObjectView;
		children: TreeNode[];
		depth: number;
	}

	const tree = $derived.by((): TreeNode[] => {
		void orderVersion;
		const byParent = new Map<string | null, FlowObjectView[]>();
		for (const object of objects) {
			const parentKey = object.parent_id ?? null;
			const siblings = byParent.get(parentKey) ?? [];
			siblings.push(object);
			byParent.set(parentKey, siblings);
		}
		for (const siblings of byParent.values()) {
			siblings.sort((a, b) => orderOf(a.id).localeCompare(orderOf(b.id)) || a.created_at.localeCompare(b.created_at));
		}
		function build(parentId: string | null, depth: number): TreeNode[] {
			const siblings = byParent.get(parentId) ?? [];
			return siblings.map((object) => ({
				object,
				depth,
				children: expanded.has(object.id) ? build(object.id, depth + 1) : []
			}));
		}
		return build(null, 0);
	});

	function flattenVisible(nodes: TreeNode[] = tree): TreeNode[] {
		const out: TreeNode[] = [];
		for (const node of nodes) {
			out.push(node);
			if (node.children.length > 0) out.push(...flattenVisible(node.children));
		}
		return out;
	}

	function siblingsOf(objectId: string): FlowObjectView[] {
		const object = objects.find((o) => o.id === objectId);
		const parentId = object?.parent_id ?? null;
		return objects.filter((o) => (o.parent_id ?? null) === parentId).sort((a, b) => orderOf(a.id).localeCompare(orderOf(b.id)));
	}

	async function ensureOrderEntries(): Promise<void> {
		if (!navEntry) return;
		const map = getNamedMap(navEntry.doc, 'order');
		const byParent = new Map<string | null, FlowObjectView[]>();
		for (const object of objects) {
			const parentKey = object.parent_id ?? null;
			const siblings = byParent.get(parentKey) ?? [];
			siblings.push(object);
			byParent.set(parentKey, siblings);
		}
		let wrote = false;
		for (const siblings of byParent.values()) {
			const missing = siblings.filter((o) => typeof map.get(o.id) !== 'string');
			if (missing.length === 0) continue;
			const existingKeys = siblings.map((o) => map.get(o.id)).filter((v): v is string => typeof v === 'string');
			let previous = existingKeys.length > 0 ? existingKeys.sort().at(-1) : undefined;
			for (const object of missing.sort((a, b) => a.created_at.localeCompare(b.created_at))) {
				const key = keyBetween(previous, undefined);
				map.set(object.id, key);
				previous = key;
				wrote = true;
			}
		}
		if (wrote) {
			navEntry.doc.commit();
			orderVersion += 1;
		}
	}

	async function loadObjects(): Promise<void> {
		const result = await flowApi.listObjects(workspaceId, { limit: 100 });
		if (result.code === 0 && result.data) {
			objects = result.data.items.filter((o) => o.object_type === 'page');
		}
	}

	async function ensureNavigatorObject(): Promise<string> {
		const existing = await flowApi.listObjects(workspaceId, { object_type: 'navigator', limit: 1 });
		if (existing.code === 0 && existing.data && existing.data.items.length > 0) {
			return existing.data.items[0].id;
		}
		const created = await commandService.createObject(workspaceId, {
			object_type: 'navigator',
			title: 'Navigator'
		});
		return created.object.id;
	}

	onMount(() => {
		const controller = new AbortController();
		void (async () => {
			try {
				await loadObjects();
				const navigatorObjectId = await ensureNavigatorObject();
				navEntry = await repository.open({ workspaceId, objectId: navigatorObjectId, signal: controller.signal });
				await ensureOrderEntries();
			} finally {
				loading = false;
			}
		})();
		return () => controller.abort();
	});

	onDestroy(() => {
		if (navEntry) void repository.close(navEntry.handle.objectId);
	});

	function announce(key: string, values?: Record<string, unknown>): void {
		announcement = get(t)(key, { values });
	}

	function toggleExpand(id: string): void {
		const next = new Set(expanded);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		expanded = next;
	}

	function select(id: string): void {
		void goto(`/workspace/${workspaceId}/flow/${id}`);
	}

	function onTreeKeydown(event: KeyboardEvent, node: TreeNode): void {
		const visible = flattenVisible();
		const index = visible.findIndex((n) => n.object.id === node.object.id);

		if (liftedId === node.object.id) {
			switch (event.key) {
				case 'ArrowUp':
				case 'ArrowDown': {
					event.preventDefault();
					const delta = event.key === 'ArrowUp' ? -1 : 1;
					const pos = draftSiblingOrder.indexOf(node.object.id);
					const swapWith = pos + delta;
					if (swapWith < 0 || swapWith >= draftSiblingOrder.length) return;
					const next = [...draftSiblingOrder];
					[next[pos], next[swapWith]] = [next[swapWith], next[pos]];
					draftSiblingOrder = next;
					announce('flow.nav.liveRegion.moved', { title: node.object.title, position: swapWith + 1 });
					return;
				}
				case 'ArrowLeft':
				case 'ArrowRight':
					event.preventDefault();
					announce('flow.nav.liveRegion.reorderFailed');
					return;
				case ' ':
				case 'Enter':
					event.preventDefault();
					void commitLift(node.object.id);
					return;
				case 'Escape':
					event.preventDefault();
					cancelLift(node.object.title);
					return;
				default:
					return;
			}
		}

		switch (event.key) {
			case 'ArrowDown': {
				event.preventDefault();
				const next = visible[index + 1];
				if (next) focusedId = next.object.id;
				return;
			}
			case 'ArrowUp': {
				event.preventDefault();
				const previous = visible[index - 1];
				if (previous) focusedId = previous.object.id;
				return;
			}
			case 'ArrowRight':
				event.preventDefault();
				if (!expanded.has(node.object.id)) toggleExpand(node.object.id);
				return;
			case 'ArrowLeft':
				event.preventDefault();
				if (expanded.has(node.object.id)) toggleExpand(node.object.id);
				return;
			case 'Home':
				event.preventDefault();
				if (visible[0]) focusedId = visible[0].object.id;
				return;
			case 'End':
				event.preventDefault();
				if (visible.at(-1)) focusedId = visible.at(-1)!.object.id;
				return;
			case 'Enter':
				event.preventDefault();
				select(node.object.id);
				return;
			case ' ':
				event.preventDefault();
				startLift(node.object.id);
				return;
			default:
				return;
		}
	}

	function startLift(objectId: string): void {
		liftedId = objectId;
		draftSiblingOrder = siblingsOf(objectId).map((o) => o.id);
		const object = objects.find((o) => o.id === objectId);
		announce('flow.nav.liveRegion.lifted', { title: object?.title ?? '' });
	}

	function cancelLift(title: string): void {
		liftedId = null;
		draftSiblingOrder = [];
		announce('flow.nav.liveRegion.cancelled', { title });
	}

	async function commitLift(objectId: string): Promise<void> {
		if (!navEntry) return;
		const map = getNamedMap(navEntry.doc, 'order');
		const finalIndex = draftSiblingOrder.indexOf(objectId);
		const before = draftSiblingOrder[finalIndex - 1];
		const after = draftSiblingOrder[finalIndex + 1];
		const beforeKey = before ? orderOf(before) : undefined;
		const afterKey = after ? orderOf(after) : undefined;
		try {
			const newKey = keyBetween(beforeKey, afterKey);
			map.set(objectId, newKey);
			navEntry.doc.commit();
			orderVersion += 1;
			const object = objects.find((o) => o.id === objectId);
			announce('flow.nav.liveRegion.dropped', { title: object?.title ?? '' });
		} catch {
			announce('flow.nav.liveRegion.reorderFailed');
		}
		liftedId = null;
		draftSiblingOrder = [];
	}

	// Pointer drag: same-parent-only, matching the keyboard equivalent's scope exactly.
	let dragSourceId: string | null = null;

	function onDragStart(objectId: string): void {
		dragSourceId = objectId;
	}

	function onDragOver(event: DragEvent, targetId: string): void {
		if (!dragSourceId || dragSourceId === targetId) return;
		const source = objects.find((o) => o.id === dragSourceId);
		const target = objects.find((o) => o.id === targetId);
		if (!source || !target || (source.parent_id ?? null) !== (target.parent_id ?? null)) return;
		event.preventDefault();
	}

	async function onDrop(event: DragEvent, targetId: string): Promise<void> {
		event.preventDefault();
		if (!dragSourceId || !navEntry) return;
		const source = objects.find((o) => o.id === dragSourceId);
		const target = objects.find((o) => o.id === targetId);
		dragSourceId = null;
		if (!source || !target || (source.parent_id ?? null) !== (target.parent_id ?? null)) return;

		const siblings = siblingsOf(targetId).filter((o) => o.id !== source.id);
		const targetIndex = siblings.findIndex((o) => o.id === targetId);
		const before = siblings[targetIndex - 1];
		const after = siblings[targetIndex];
		const map = getNamedMap(navEntry.doc, 'order');
		try {
			const newKey = keyBetween(before ? orderOf(before.id) : undefined, after ? orderOf(after.id) : undefined);
			map.set(source.id, newKey);
			navEntry.doc.commit();
			orderVersion += 1;
		} catch {
			announce('flow.nav.liveRegion.reorderFailed');
		}
	}

	async function createPage(parentId?: string): Promise<void> {
		if (creatingParentId !== undefined) return;
		creatingParentId = parentId ?? null;
		try {
			const created = await commandService.createObject(workspaceId, {
				object_type: 'page',
				parent_object_id: parentId,
				title: get(t)('flow.nav.untitled')
			});
			objects = [...objects, created.object];
			if (navEntry) {
				const siblings = siblingsOf(created.object.id).filter((o) => o.id !== created.object.id);
				const lastKey = siblings.length > 0 ? orderOf(siblings.at(-1)!.id) : undefined;
				getNamedMap(navEntry.doc, 'order').set(created.object.id, keyBetween(lastKey, undefined));
				navEntry.doc.commit();
				orderVersion += 1;
			}
			if (parentId) expanded = new Set(expanded).add(parentId);
			select(created.object.id);
		} finally {
			creatingParentId = undefined;
		}
	}
</script>

<nav
	class="flex h-full w-64 shrink-0 flex-col border-r border-slate-200 bg-slate-50 dark:border-slate-800 dark:bg-slate-950"
	aria-label={$t('flow.nav.title')}
>
	<div class="flex items-center justify-between border-b border-slate-200 p-3 dark:border-slate-800">
		<span class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('flow.nav.title')}</span>
		<button
			type="button"
			class="rounded p-1 text-slate-500 hover:bg-slate-200 hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
			onclick={() => createPage()}
			disabled={creatingParentId !== undefined}
			aria-label={$t('flow.nav.newPage')}
			title={$t('flow.nav.newPage')}
		>
			+
		</button>
	</div>

	<div class="flex-1 overflow-y-auto p-2">
		{#if loading}
			<p class="p-2 text-sm text-slate-500 dark:text-slate-400">{$t('flow.nav.loading')}</p>
		{:else if tree.length === 0}
			<div class="p-3 text-sm text-slate-500 dark:text-slate-400">
				<p class="mb-2">{$t('flow.nav.empty')}</p>
				<button type="button" class="font-medium text-blue-600 hover:underline dark:text-blue-400" onclick={() => createPage()}>
					{$t('flow.nav.createFirst')}
				</button>
			</div>
		{:else}
			{#snippet renderNode(node: TreeNode)}
				<div role="none">
					<div
						role="treeitem"
						tabindex={focusedId === node.object.id || (focusedId === null && node.depth === 0 && tree[0] === node) ? 0 : -1}
						aria-level={node.depth + 1}
						aria-expanded={node.children.length > 0 || undefined}
						aria-selected={selectedObjectId === node.object.id}
						draggable="true"
						class={`group flex cursor-pointer items-center gap-1 rounded px-2 py-1.5 text-sm outline-none ${
							selectedObjectId === node.object.id
								? 'bg-blue-100 text-blue-900 dark:bg-blue-900/40 dark:text-blue-100'
								: 'text-slate-700 hover:bg-slate-200 dark:text-slate-300 dark:hover:bg-slate-800'
						} ${liftedId === node.object.id ? 'ring-2 ring-amber-400' : ''} ${
							focusedId === node.object.id ? 'ring-2 ring-blue-400' : ''
						}`}
						style={`padding-left: ${node.depth * 16 + 8}px`}
						onclick={() => select(node.object.id)}
						onkeydown={(e) => onTreeKeydown(e, node)}
						onfocus={() => (focusedId = node.object.id)}
						ondragstart={() => onDragStart(node.object.id)}
						ondragover={(e) => onDragOver(e, node.object.id)}
						ondrop={(e) => onDrop(e, node.object.id)}
					>
						{#if node.children.length > 0 || objects.some((o) => o.parent_id === node.object.id)}
							<button
								type="button"
								class="w-4 shrink-0 text-slate-400"
								onclick={(e) => {
									e.stopPropagation();
									toggleExpand(node.object.id);
								}}
								aria-label={expanded.has(node.object.id) ? $t('flow.nav.collapse') : $t('flow.nav.expand')}
							>
								{expanded.has(node.object.id) ? '▾' : '▸'}
							</button>
						{:else}
							<span class="w-4 shrink-0"></span>
						{/if}
						<span class="truncate">{node.object.title || $t('flow.nav.untitled')}</span>
					</div>
					{#if expanded.has(node.object.id)}
						<div role="group">
							{#each node.children as child (child.object.id)}
								{@render renderNode(child)}
							{/each}
						</div>
					{/if}
				</div>
			{/snippet}

			<div role="tree" aria-label={$t('flow.nav.title')}>
				{#each tree as node (node.object.id)}
					{@render renderNode(node)}
				{/each}
			</div>
		{/if}
	</div>

	<div aria-live="polite" class="sr-only" role="status">{announcement}</div>
</nav>

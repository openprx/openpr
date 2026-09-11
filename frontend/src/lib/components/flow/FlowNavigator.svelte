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
	import type { FlowObjectView } from '$lib/api/flow';
	import { FlowCommandService } from '$lib/flow/command-service';
	import {
		getNamedMap,
		type FlowObjectRepository,
		type OpenFlowObjectResult
	} from '$lib/flow/object-repository';
	import {
		appendOrderKey,
		applyDestination,
		contextMenuDestination,
		keyboardStepDestination,
		orderKeyForDestination,
		pointerDropDestination,
		seedOrderKeys,
		type DropSide,
		type OrderedSibling
	} from '$lib/flow/navigator-reorder';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import { resolveRenderer } from '$lib/flow/renderer-registry';

	// `contracts/ui-surface-v1.md` "Renderer registry": an unregistered `${objectType}:${viewType}`
	// must fail to read-only/unsupported rather than the route always assuming a tree renders.
	const navRenderer = resolveRenderer('navigator', 'tree');

	interface Props {
		workspaceId: string;
		selectedObjectId: string | null;
	}

	let { workspaceId, selectedObjectId }: Props = $props();

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commandService = new FlowCommandService();

	let objects = $state<FlowObjectView[]>([]);
	let rootObjectId = $state<string | null>(null);
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
			siblings.sort(
				(a, b) =>
					orderOf(a.id).localeCompare(orderOf(b.id)) || a.created_at.localeCompare(b.created_at)
			);
		}
		function build(parentId: string | null, depth: number): TreeNode[] {
			const siblings = byParent.get(parentId) ?? [];
			return siblings.map((object) => ({
				object,
				depth,
				children: expanded.has(object.id) ? build(object.id, depth + 1) : []
			}));
		}
		return rootObjectId ? build(rootObjectId, 0) : [];
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
		return objects
			.filter((o) => (o.parent_id ?? null) === parentId)
			.sort((a, b) => orderOf(a.id).localeCompare(orderOf(b.id)));
	}

	/** The same sibling list in the shape `navigator-reorder` works in: committed order, each
	 * entry carrying its current fractional-index key. Every gesture below builds its input with
	 * this one function, so no gesture can be reasoning about a differently-sorted list. */
	function orderedSiblingsOf(objectId: string): OrderedSibling[] {
		return siblingsOf(objectId).map((o) => ({ id: o.id, orderKey: orderOf(o.id) }));
	}

	/** Writes one reorder. The destination always comes from `navigator-reorder`; this function
	 * owns the CRDT write and the announcement, and never computes bounds itself.
	 *
	 * `destination === null` means the gesture could not express a move at all (end of list,
	 * cross-parent drop, unknown id) -- that is the announced failure. A destination that resolves
	 * to no key change means the object is already there: nothing is written and nothing is
	 * announced as a failure, because landing where you started is a no-op, not an error. */
	function commitReorder(objectId: string, destination: number | null): boolean {
		if (!navEntry || destination === null) {
			announce('flow.nav.liveRegion.reorderFailed');
			return false;
		}
		const newKey = orderKeyForDestination(orderedSiblingsOf(objectId), objectId, destination);
		if (newKey === null) return false;
		try {
			getNamedMap(navEntry.doc, 'order').set(objectId, newKey);
			navEntry.doc.commit();
			orderVersion += 1;
		} catch {
			announce('flow.nav.liveRegion.reorderFailed');
			return false;
		}
		const object = objects.find((o) => o.id === objectId);
		announce('flow.nav.liveRegion.dropped', { title: object?.title ?? '' });
		return true;
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
			const existingKeys = siblings
				.map((o) => map.get(o.id))
				.filter((v): v is string => typeof v === 'string');
			const ordered = missing.sort((a, b) => a.created_at.localeCompare(b.created_at));
			const keys = seedOrderKeys(existingKeys, ordered.length);
			ordered.forEach((object, index) => {
				map.set(object.id, keys[index]);
				wrote = true;
			});
		}
		if (wrote) {
			navEntry.doc.commit();
			orderVersion += 1;
		}
	}

	async function loadObjects(): Promise<void> {
		const result = await commandService.listObjects(workspaceId, { limit: 100 });
		if (result.code === 0 && result.data) {
			objects = result.data.items.filter((o) => o.object_type === 'page');
		}
	}

	async function loadNavigatorRoot(): Promise<string> {
		const result = await commandService.getNavigator(workspaceId);
		if (result.code !== 0 || !result.data) {
			throw new Error('workspace navigator root is unavailable');
		}
		rootObjectId = result.data.root_object_id;
		return result.data.root_object_id;
	}

	onMount(() => {
		const controller = new AbortController();
		void (async () => {
			try {
				const navigatorObjectId = await loadNavigatorRoot();
				await loadObjects();
				navEntry = await repository.open({
					workspaceId,
					objectId: navigatorObjectId,
					signal: controller.signal
				});
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
					// The draft is a preview of the SAME destination the drop will commit -- both go
					// through `navigator-reorder`, so what the live region announces during a lift
					// cannot drift from what lands in the CRDT on Drop.
					const draft = draftSiblingOrder.map((id) => ({ id, orderKey: orderOf(id) }));
					const destination = keyboardStepDestination(draft, node.object.id, delta);
					if (destination === null) return;
					draftSiblingOrder = applyDestination(draftSiblingOrder, node.object.id, destination);
					announce('flow.nav.liveRegion.moved', {
						title: node.object.title,
						position: destination + 1
					});
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
			case 'ContextMenu': {
				event.preventDefault();
				const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
				openContextMenu(node.object.id, rect.left, rect.bottom);
				return;
			}
			case 'F10': {
				if (!event.shiftKey) return;
				event.preventDefault();
				const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
				openContextMenu(node.object.id, rect.left, rect.bottom);
				return;
			}
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
		// The draft already holds the object at its intended position, and `applyDestination` puts
		// it at exactly index `destination` -- so the draft index IS the destination, in the same
		// coordinate system every other gesture uses.
		const index = draftSiblingOrder.indexOf(objectId);
		commitReorder(objectId, index < 0 ? null : index);
		liftedId = null;
		draftSiblingOrder = [];
	}

	// Context menu (`contracts/ui-surface-v1.md` a11y baseline: "context menu 提供 Move before/
	// after/inside/outside，功能与 pointer drag 相同"). "before"/"after" reuse the exact
	// same-parent CRDT order write the lift/drop and pointer-drag paths already use above -- there
	// is still only one write path for a reorder. "inside"/"outside" would nest under a different
	// parent, which -- matching the same documented v0.4 scope note at the top of this file for
	// ArrowLeft/ArrowRight while lifted -- is a governance `move_object` command that does not
	// exist until v0.5, so those two items announce "not available this version" and touch no
	// state, exactly like the keyboard path already does.
	let contextMenuFor = $state<string | null>(null);
	let contextMenuPos = $state<{ x: number; y: number } | null>(null);

	function openContextMenu(objectId: string, x: number, y: number): void {
		contextMenuFor = objectId;
		contextMenuPos = { x, y };
	}

	function closeContextMenu(): void {
		contextMenuFor = null;
		contextMenuPos = null;
	}

	function onTreeContextMenu(event: MouseEvent, objectId: string): void {
		event.preventDefault();
		openContextMenu(objectId, event.clientX, event.clientY);
	}

	async function moveRelative(objectId: string, direction: DropSide): Promise<void> {
		closeContextMenu();
		commitReorder(
			objectId,
			contextMenuDestination(orderedSiblingsOf(objectId), objectId, direction)
		);
	}

	function moveUnavailable(): void {
		closeContextMenu();
		announce('flow.nav.liveRegion.reorderFailed');
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

		// Pointer drag resolves to the same destination-then-key pipeline the keyboard and context
		// menu use, and produces the same live-region announcement: the a11y baseline treats them
		// as one operation with three input methods, not three operations.
		const siblings = orderedSiblingsOf(source.id);
		commitReorder(source.id, pointerDropDestination(siblings, source.id, targetId));
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
				const siblings = orderedSiblingsOf(created.object.id).filter(
					(o) => o.id !== created.object.id
				);
				getNamedMap(navEntry.doc, 'order').set(created.object.id, appendOrderKey(siblings));
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
	<div
		class="flex items-center justify-between border-b border-slate-200 p-3 dark:border-slate-800"
	>
		<span class="text-sm font-semibold text-slate-900 dark:text-slate-100"
			>{$t('flow.nav.title')}</span
		>
		<div class="flex items-center gap-1">
			<!-- `flow.search.*` (`i18n v0.4 基线`): Flow search is v0.5 scope
				 (`ui-surface-v1.md` "后续版本 UI 派生" v0.5: "Navigator/command palette 增加 Flow
				 search"). The entry point exists and is disabled/announced rather than the key
				 sitting unused, matching the "禁止...缺 key 时把 key string 当发布文案" rule by
				 never rendering the key as anything other than real, honest UI copy. -->
			<button
				type="button"
				class="rounded p-1 text-slate-400 dark:text-slate-500"
				disabled
				aria-disabled="true"
				aria-label={$t('flow.search.comingSoon')}
				title={$t('flow.search.comingSoon')}
			>
				🔍
			</button>
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
	</div>

	<div class="flex-1 overflow-y-auto p-2">
		{#if !navRenderer}
			<p class="p-2 text-sm text-slate-500 dark:text-slate-400">{$t('flow.canvas.unsupported')}</p>
		{:else if loading}
			<p class="p-2 text-sm text-slate-500 dark:text-slate-400">{$t('flow.nav.loading')}</p>
		{:else if tree.length === 0}
			<div class="p-3 text-sm text-slate-500 dark:text-slate-400">
				<p class="mb-2">{$t('flow.nav.empty')}</p>
				<button
					type="button"
					class="font-medium text-blue-600 hover:underline dark:text-blue-400"
					onclick={() => createPage()}
				>
					{$t('flow.nav.createFirst')}
				</button>
			</div>
		{:else}
			{#snippet renderNode(node: TreeNode)}
				<div role="none">
					<div
						role="treeitem"
						tabindex={focusedId === node.object.id ||
						(focusedId === null && node.depth === 0 && tree[0] === node)
							? 0
							: -1}
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
						oncontextmenu={(e) => onTreeContextMenu(e, node.object.id)}
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
								aria-label={expanded.has(node.object.id)
									? $t('flow.nav.collapse')
									: $t('flow.nav.expand')}
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

{#if contextMenuFor && contextMenuPos}
	<div
		class="fixed inset-0 z-40"
		role="presentation"
		onclick={closeContextMenu}
		oncontextmenu={(e) => {
			e.preventDefault();
			closeContextMenu();
		}}
	></div>
	<div
		class="fixed z-50 w-48 rounded-md border border-slate-200 bg-white py-1 text-sm shadow-lg dark:border-slate-700 dark:bg-slate-900"
		style={`left: ${contextMenuPos.x}px; top: ${contextMenuPos.y}px`}
		role="menu"
		tabindex="-1"
		aria-label={$t('flow.nav.contextMenu.label')}
		onkeydown={(e) => {
			if (e.key === 'Escape') {
				e.preventDefault();
				closeContextMenu();
			}
		}}
	>
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
			onclick={() => moveRelative(contextMenuFor!, 'before')}
		>
			{$t('flow.nav.contextMenu.moveBefore')}
		</button>
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
			onclick={() => moveRelative(contextMenuFor!, 'after')}
		>
			{$t('flow.nav.contextMenu.moveAfter')}
		</button>
		<!-- v0.4 scope note: "inside"/"outside" need the `move_object` governance command, which
			 does not exist until v0.5 -- see the block comment above `contextMenuFor`'s declaration. -->
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-slate-400 hover:bg-slate-100 dark:text-slate-500 dark:hover:bg-slate-800"
			onclick={moveUnavailable}
		>
			{$t('flow.nav.contextMenu.moveInside')}
		</button>
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-slate-400 hover:bg-slate-100 dark:text-slate-500 dark:hover:bg-slate-800"
			onclick={moveUnavailable}
		>
			{$t('flow.nav.contextMenu.moveOutside')}
		</button>
	</div>
{/if}

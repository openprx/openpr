// `EditorAdapter`: ProseMirror bound to a single rich-text container via `loro-prosemirror`
// (ADR-0006: Svelte 5 + ProseMirror core + `loro-prosemirror`). Owns no Page tree, network, auth
// or business policy (`contracts/ui-surface-v1.md` "职责边界").
//
// v0.4 alpha block model (documented simplification, not hidden): Page content is a FLAT sequence
// of typed blocks, each carrying an `indent` attribute (0..7) instead of true structural nesting
// (bulletList > listItem > paragraph). This mirrors how Notion's own block model actually works
// (a flat list of blocks + an indent/parent reference, not nested-container prose) and keeps
// `nest`/`outdent` a single uniform command across every registered block type
// (`ui-surface-v1.md`'s "Renderer registry": paragraph/heading/list/code). No inline marks
// (bold/italic/etc.) in this round -- plain text per block only.
//
// ALL loro-crdt / loro-prosemirror / prosemirror-* imports are dynamic `import()` calls inside
// this module's functions, never static top-level imports. That is what keeps the engine chunk
// out of every bundle that does not reach the Flow route
// (`contracts/ui-surface-v1.md` "engine chunk 只从 `(app)/flow` 动态 import").

import type { EditorAdapterContract, EngineDiff, RelativeSelection } from './types';
// Type-only import: erased at compile time, so this does not pull `loro-prosemirror` into the
// module graph -- only its exported *type* `LoroDocType` (the specific `{doc, data}` container
// shape it expects) is used here, to type the doc reference the caller constructs at runtime via
// dynamic `import('loro-crdt')`.
import type { LoroDocType } from 'loro-prosemirror';

export const MAX_INDENT = 7;

type PmModule = typeof import('prosemirror-model');
type PmStateModule = typeof import('prosemirror-state');
type PmViewModule = typeof import('prosemirror-view');
type PmCommandsModule = typeof import('prosemirror-commands');
type PmKeymapModule = typeof import('prosemirror-keymap');
type LoroPmModule = typeof import('loro-prosemirror');

export type FlowBlockType = 'paragraph' | 'heading' | 'bulletItem' | 'codeBlock';

export interface SlashMenuState {
	readonly active: boolean;
	readonly query: string;
	readonly coords: { left: number; top: number } | null;
}

export interface EditorAdapterHooks {
	onSlashMenu?(state: SlashMenuState): void;
	onSelectionChange?(): void;
}

let modulesPromise: Promise<{
	pm: PmModule;
	pmState: PmStateModule;
	pmView: PmViewModule;
	pmCommands: PmCommandsModule;
	pmKeymap: PmKeymapModule;
	loroPm: LoroPmModule;
}> | null = null;

/** Loads the whole ProseMirror + Loro-binding engine bundle exactly once, lazily. */
function loadEngineModules() {
	if (!modulesPromise) {
		modulesPromise = Promise.all([
			import('prosemirror-model'),
			import('prosemirror-state'),
			import('prosemirror-view'),
			import('prosemirror-commands'),
			import('prosemirror-keymap'),
			import('loro-prosemirror')
		]).then(([pm, pmState, pmView, pmCommands, pmKeymap, loroPm]) => ({
			pm,
			pmState,
			pmView,
			pmCommands,
			pmKeymap,
			loroPm
		}));
	}
	return modulesPromise;
}

function buildSchema(pm: PmModule): InstanceType<PmModule['Schema']> {
	const indentAttr = { default: 0 };
	return new pm.Schema({
		nodes: {
			doc: { content: 'block+' },
			paragraph: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr },
				toDOM: (node) => ['p', { 'data-indent': node.attrs.indent }, 0],
				parseDOM: [{ tag: 'p', getAttrs: (dom) => ({ indent: Number((dom as HTMLElement).dataset.indent ?? 0) }) }]
			},
			heading: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr, level: { default: 1 } },
				toDOM: (node) => [`h${node.attrs.level}`, { 'data-indent': node.attrs.indent }, 0],
				parseDOM: [1, 2, 3].map((level) => ({
					tag: `h${level}`,
					getAttrs: (dom) => ({ level, indent: Number((dom as HTMLElement).dataset.indent ?? 0) })
				}))
			},
			bulletItem: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr },
				toDOM: (node) => ['div', { class: 'flow-bullet-item', 'data-indent': node.attrs.indent }, 0],
				parseDOM: [{ tag: 'div.flow-bullet-item' }]
			},
			codeBlock: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr },
				marks: '',
				code: true,
				toDOM: (node) => ['pre', { 'data-indent': node.attrs.indent }, ['code', 0]],
				parseDOM: [{ tag: 'pre', preserveWhitespace: 'full' as const }]
			},
			text: { group: 'inline' }
		},
		marks: {}
	});
}

function emptyDoc(schema: InstanceType<PmModule['Schema']>) {
	return schema.node('doc', null, [schema.node('paragraph', { indent: 0 }, [])]);
}

export class FlowEditorAdapter implements EditorAdapterContract {
	private readonly doc: LoroDocType;
	private readonly hooks: EditorAdapterHooks;
	private view: import('prosemirror-view').EditorView | null = null;
	private schema: InstanceType<PmModule['Schema']> | null = null;
	private mods: Awaited<ReturnType<typeof loadEngineModules>> | null = null;

	constructor(doc: LoroDocType, hooks: EditorAdapterHooks = {}) {
		this.doc = doc;
		this.hooks = hooks;
	}

	async mount(host: HTMLElement, _blockId: string): Promise<void> {
		const mods = await loadEngineModules();
		this.mods = mods;
		const { pm, pmState, pmView, pmCommands, pmKeymap, loroPm } = mods;
		const schema = buildSchema(pm);
		this.schema = schema;

		const nestOutdent = (delta: number): import('prosemirror-state').Command => (state, dispatch) => {
			const { $from } = state.selection;
			const depth = $from.parent.attrs.indent as number;
			const next = Math.max(0, Math.min(MAX_INDENT, depth + delta));
			if (next === depth) return false;
			if (dispatch) {
				const pos = $from.before($from.depth);
				dispatch(state.tr.setNodeAttribute(pos, 'indent', next));
			}
			return true;
		};

		const customKeymap = pmKeymap.keymap({
			'Mod-z': loroPm.undo,
			'Mod-y': loroPm.redo,
			'Mod-Shift-z': loroPm.redo,
			Tab: nestOutdent(1),
			'Shift-Tab': nestOutdent(-1)
		});
		const baseKeymapPlugin = pmKeymap.keymap(pmCommands.baseKeymap);

		const state = pmState.EditorState.create({
			schema,
			doc: emptyDoc(schema),
			plugins: [
				loroPm.LoroSyncPlugin({ doc: this.doc }),
				loroPm.LoroUndoPlugin({ doc: this.doc }),
				customKeymap,
				baseKeymapPlugin
			]
		});

		this.view = new pmView.EditorView(host, {
			state,
			dispatchTransaction: (tr) => {
				if (!this.view) return;
				const newState = this.view.state.apply(tr);
				this.view.updateState(newState);
				this.detectSlashMenu(newState);
				this.hooks.onSelectionChange?.();
			}
		});
	}

	private detectSlashMenu(state: import('prosemirror-state').EditorState): void {
		if (!this.hooks.onSlashMenu || !this.view) return;
		const { $from } = state.selection;
		const textBefore = $from.parent.textBetween(0, $from.parentOffset, undefined, '￼');
		const match = /(?:^|\s)\/(\w*)$/.exec(textBefore);
		if (match) {
			const coords = this.view.coordsAtPos($from.pos);
			this.hooks.onSlashMenu({ active: true, query: match[1], coords: { left: coords.left, top: coords.bottom } });
		} else {
			this.hooks.onSlashMenu({ active: false, query: '', coords: null });
		}
	}

	/** Called by the slash menu UI when the user picks a block type; replaces the current block. */
	setBlockType(type: FlowBlockType, level?: number): void {
		if (!this.view || !this.schema) return;
		const { state, dispatch } = this.view;
		const { $from } = state.selection;
		const pos = $from.before($from.depth);
		const nodeType = this.schema.nodes[type];
		const indent = $from.parent.attrs.indent as number;
		const attrs = type === 'heading' ? { indent, level: level ?? 1 } : { indent };
		const tr = state.tr.setNodeMarkup(pos, nodeType, attrs);
		// Strip the trailing "/query" the user typed to open the menu.
		const parentEnd = $from.after($from.depth) - 1;
		const parentStart = $from.before($from.depth) + 1;
		const text = state.doc.textBetween(parentStart, parentEnd, undefined, '￼');
		const slashIndex = text.lastIndexOf('/');
		if (slashIndex >= 0) {
			tr.delete(parentStart + slashIndex, parentEnd);
		}
		dispatch(tr);
		this.view.focus();
	}

	/** Backs the canvas's per-block hover handle nest/outdent buttons (task brief item 1's "block
	 * hover handle"), which act on whatever block the pointer is over rather than the current
	 * selection. Shares the same clamp-to-`MAX_INDENT` rule as the Tab/Shift-Tab keymap. */
	adjustIndentAtPos(pos: number, delta: number): void {
		if (!this.view) return;
		const { state } = this.view;
		const resolved = state.doc.resolve(Math.min(Math.max(pos, 0), state.doc.content.size));
		const depth = resolved.parent.attrs.indent as number;
		const next = Math.max(0, Math.min(MAX_INDENT, depth + delta));
		if (next === depth) return;
		const blockPos = resolved.before(resolved.depth);
		this.view.dispatch(state.tr.setNodeAttribute(blockPos, 'indent', next));
	}

	/** Maps a DOM point to a document position, for the hover handle to know which block the
	 * pointer is over. Returns `null` outside the editable area. */
	posAtClientPoint(x: number, y: number): number | null {
		if (!this.view) return null;
		return this.view.posAtCoords({ left: x, top: y })?.pos ?? null;
	}

	applyRemote(_change: EngineDiff): void {
		// `LoroSyncPlugin` subscribes to the bound Loro container itself and reconciles the
		// ProseMirror doc on every remote import automatically -- there is nothing this method
		// needs to do beyond existing for interface conformance
		// (`contracts/ui-surface-v1.md`'s `EditorAdapter.applyRemote`).
	}

	getSelection(): RelativeSelection | null {
		if (!this.view) return null;
		const { selection } = this.view.state;
		return { anchor: selection.anchor, head: selection.head };
	}

	restoreSelection(value: RelativeSelection): void {
		if (!this.view || !this.mods) return;
		const { pmState } = this.mods;
		const size = this.view.state.doc.content.size;
		const anchor = Math.min(value.anchor, size);
		const head = Math.min(value.head, size);
		const tr = this.view.state.tr.setSelection(pmState.TextSelection.create(this.view.state.doc, anchor, head));
		this.view.dispatch(tr);
	}

	undoLocal(): boolean {
		if (!this.view || !this.mods) return false;
		const { loroPm } = this.mods;
		if (!loroPm.canUndo(this.view.state)) return false;
		return loroPm.undo(this.view.state, this.view.dispatch);
	}

	redoLocal(): boolean {
		if (!this.view || !this.mods) return false;
		const { loroPm } = this.mods;
		if (!loroPm.canRedo(this.view.state)) return false;
		return loroPm.redo(this.view.state, this.view.dispatch);
	}

	async destroy(): Promise<void> {
		this.view?.destroy();
		this.view = null;
	}
}

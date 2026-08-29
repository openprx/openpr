// Renderer registry (`contracts/ui-surface-v1.md` "Renderer registry"): avoids a route-page type
// switch that grows every version. v0.4 registers `page:canvas` and `navigator:tree` plus the
// four block-level renderers this alpha ships. An unregistered `${objectType}:${viewType}` fails
// to `read_only`/`unsupported` -- it must never let an arbitrary component path reach the CRDT.

export type ObjectType = 'page' | 'navigator';
export type ViewType = 'canvas' | 'tree';
export type RendererKey = `${ObjectType}:${ViewType}`;
export type Capability = 'edit' | 'view';

export interface ViewRendererRegistration {
	readonly key: RendererKey;
	readonly capabilities: readonly Capability[];
	readonly fallback: 'unsupported' | 'read_only';
}

const REGISTRY: Partial<Record<RendererKey, ViewRendererRegistration>> = {
	'page:canvas': { key: 'page:canvas', capabilities: ['edit', 'view'], fallback: 'read_only' },
	'navigator:tree': { key: 'navigator:tree', capabilities: ['edit', 'view'], fallback: 'read_only' }
};

export function resolveRenderer(objectType: ObjectType, viewType: ViewType): ViewRendererRegistration | null {
	return REGISTRY[`${objectType}:${viewType}`] ?? null;
}

export type FlowBlockRendererType = 'paragraph' | 'heading' | 'bulletItem' | 'codeBlock';

const BLOCK_RENDERERS: ReadonlySet<FlowBlockRendererType> = new Set([
	'paragraph',
	'heading',
	'bulletItem',
	'codeBlock'
]);

export function isRegisteredBlockType(value: string): value is FlowBlockRendererType {
	return BLOCK_RENDERERS.has(value as FlowBlockRendererType);
}

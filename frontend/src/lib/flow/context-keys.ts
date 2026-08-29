// Svelte context keys shared between the Flow route layout and its descendants. Kept as a tiny
// standalone module (no engine imports) so importing it never risks pulling engine code into a
// bundle that only needs the key.

export const FLOW_REPOSITORY_CONTEXT = Symbol('flow-repository');

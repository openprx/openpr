// Client-side pre-check constants mirroring `sylvode.flow.limits.v1` (`contracts/limits-v1.md`).
//
// This is an interaction-quality improvement only: it lets the UI reject an obviously
// over-budget edit before spending a round trip. It is NOT the authority -- REST/WS
// re-validate every limit server-side regardless of what the client precomputed
// (`contracts/ui-surface-v1.md` "该预检只改善交互...REST/WS 服务端仍重新计算").
//
// The numbers below are copied from the frozen contract table, not invented:
// `contracts/limits-v1.md` lines 15 (`update_bytes_max`), 21-26 (tree/container/block/text/patch),
// and `contracts/collab-protocol-v1.md`'s "128 KiB frame, 64 KiB update, 8 KiB presence, 30s TTL".
import type { FlowLimitsV1 } from './types';

export const DEFAULT_FLOW_LIMITS: FlowLimitsV1 = {
	frameBytesMax: 131_072,
	updateBytesMax: 65_536,
	presenceBytesMax: 8_192,
	presenceTtlSecondsMax: 30,
	treeDepthMax: 32,
	containerCountMax: 10_000,
	documentBlockCountMax: 10_000,
	textBlockCharsMax: 100_000,
	documentTextCharsMax: 1_000_000,
	semanticPatchOperationsMax: 100
};

export interface LimitViolation {
	readonly limitKind: 'update_bytes' | 'text_block_chars' | 'tree_depth';
	readonly limit: number;
	readonly observed: number;
}

export function checkUpdateBytes(bytes: Uint8Array, limits: FlowLimitsV1 = DEFAULT_FLOW_LIMITS): LimitViolation | null {
	if (bytes.byteLength > limits.updateBytesMax) {
		return { limitKind: 'update_bytes', limit: limits.updateBytesMax, observed: bytes.byteLength };
	}
	return null;
}

export function checkTextBlockChars(
	text: string,
	limits: FlowLimitsV1 = DEFAULT_FLOW_LIMITS
): LimitViolation | null {
	if (text.length > limits.textBlockCharsMax) {
		return { limitKind: 'text_block_chars', limit: limits.textBlockCharsMax, observed: text.length };
	}
	return null;
}

export function checkTreeDepth(depth: number, limits: FlowLimitsV1 = DEFAULT_FLOW_LIMITS): LimitViolation | null {
	if (depth > limits.treeDepthMax) {
		return { limitKind: 'tree_depth', limit: limits.treeDepthMax, observed: depth };
	}
	return null;
}

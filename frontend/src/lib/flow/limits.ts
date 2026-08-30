// `sylvode.flow.limits.v1` on the client (`contracts/limits-v1.md`).
//
// Two jobs, deliberately separated:
//
//  1. Version negotiation + adoption of the SERVER's effective ceilings. `Bootstrap.limits` is
//     the authority; `negotiateFlowLimitsVersion` is the only door those numbers come through,
//     and it refuses anything it cannot prove it understands. `limits-v1.md`: "Client 不得自行
//     放宽或缓存跨 `version` limits" -- so an unrecognised `version`, or a payload missing any
//     wire field, degrades the session to read-only instead of silently falling back to this
//     build's compiled-in numbers and continuing to write.
//
//  2. Client-side pre-checks (`checkUpdateBytes`/`checkTextBlockChars`/`checkTreeDepth`). These
//     are an interaction-quality improvement only: they let the UI reject an obviously
//     over-budget edit before spending a round trip. They are NOT the authority -- REST/WS
//     re-validate every limit server-side regardless of what the client precomputed
//     (`contracts/ui-surface-v1.md` "该预检只改善交互...REST/WS 服务端仍重新计算").
//
// `DEFAULT_FLOW_LIMITS` is a pre-bootstrap fallback ONLY -- the values a session may use before
// any `Bootstrap` has been seen. Every number in it is copied from the frozen wire schema in
// `contracts/limits-v1.md` ("Bootstrap.limits wire schema", `FlowLimitsV1 = {...}`), which is the
// same table `apps/api/src/flow/collab/limits.rs::effective_limits()` is cross-checked against.
// Once a real `Bootstrap` arrives its values replace these wholesale; nothing here is ever
// merged field-by-field over a server payload.

import type { FlowLimitsNegotiation, FlowLimitsRejectionReason, FlowLimitsV1 } from './types';

/** The single `limits.version` string this client implements. */
export const FLOW_LIMITS_VERSION = 'sylvode.flow.limits.v1';

/** Pre-bootstrap fallback; see this file's header. Values are the frozen wire schema's. */
export const DEFAULT_FLOW_LIMITS: FlowLimitsV1 = {
	updateBytesMax: 65_536,
	websocketFrameBytesMax: 131_072,
	presencePayloadBytesMax: 8_192,
	presenceTtlSecondsMax: 30,
	bootstrapDecodedBytesMax: 8_388_608,
	bootstrapResponseBytesMax: 12_582_912,
	treeDepthMax: 32,
	containerCountMax: 10_000,
	documentBlockCountMax: 10_000,
	textBlockCharsMax: 100_000,
	documentTextCharsMax: 1_000_000,
	semanticPatchOperationsMax: 100,
	semanticPatchJsonBytesMax: 1_048_576,
	decodeApplyCpuMsMax: 50,
	decodeApplyWallMsMax: 100,
	isolatedApplyMemoryBytesMax: 134_217_728,
	openDocumentsPerConnectionMax: 8,
	connectionsPerUserMax: 16,
	connectionsPerDocumentMax: 100,
	connectionsPerWorkspaceMax: 500,
	presenceEntriesPerConnectionMax: 8,
	presenceEntriesPerDocumentMax: 100,
	framesPerConnectionPerSecond: 30,
	frameBurstMax: 60,
	updatesPerConnectionPerSecond: 10,
	updateBurstMax: 20,
	slowConsumerQueueFramesMax: 256,
	slowConsumerQueueBytesMax: 8_388_608,
	pageLimitDefault: 50,
	pageLimitMax: 100,
	authorizedScanRowsMax: 1_000,
	importArchiveBytesMax: 134_217_728,
	importExpandedBytesMax: 536_870_912,
	importEntryCountMax: 100_000,
	importCompressionRatioMax: 100
};

/** Every `FlowLimitsV1` key, taken from the fallback object so the list can never drift from the
 * interface: `DEFAULT_FLOW_LIMITS` is typed `FlowLimitsV1`, so adding a wire field without adding
 * it here is a compile error, and this array is derived from that object rather than re-typed. */
export const FLOW_LIMITS_FIELDS = Object.keys(DEFAULT_FLOW_LIMITS) as ReadonlyArray<keyof FlowLimitsV1>;

/** lowerCamelCase field name -> its snake_case name on the wire. Derived, never hand-mapped:
 * `presencePayloadBytesMax` -> `presence_payload_bytes_max`. */
export function wireFieldName(field: keyof FlowLimitsV1): string {
	return field.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();
}

function readWireNumber(payload: Record<string, unknown>, wireName: string): number | null {
	const raw = payload[wireName];
	if (typeof raw !== 'number' || !Number.isFinite(raw) || raw < 0) return null;
	return raw;
}

function rejected(
	version: string | null,
	reason: FlowLimitsRejectionReason,
	missingFields: readonly string[]
): FlowLimitsNegotiation {
	return {
		outcome: 'unknownVersion',
		version,
		reason,
		missingFields,
		// Read-side display only. `readOnly` is what callers must gate writes on -- these numbers
		// are this build's fallback, not the server's, and the contract forbids writing under
		// limits the client cannot confirm.
		limits: DEFAULT_FLOW_LIMITS,
		readOnly: true
	};
}

/** Version negotiation for a live `Bootstrap.limits` payload (`FlowBootstrap.limits`, typed
 * `unknown` because it is raw wire JSON).
 *
 * Returns `supported` with the SERVER's values only when the payload declares exactly
 * `FLOW_LIMITS_VERSION` and carries every wire field as a finite non-negative number. Anything
 * else -- absent payload, absent/non-string version, a future or unrecognised version, or a
 * recognised version with a truncated payload -- returns `unknownVersion` with `readOnly: true`,
 * which is the `unknown_version_read_only` behaviour `limits-v1.md`'s bootstrap_parity evidence
 * requires. */
export function negotiateFlowLimitsVersion(payload: unknown): FlowLimitsNegotiation {
	if (typeof payload !== 'object' || payload === null || Array.isArray(payload)) {
		return rejected(null, 'missing_payload', []);
	}
	const record = payload as Record<string, unknown>;
	const version = record.version;
	if (typeof version !== 'string' || version.length === 0) {
		return rejected(null, 'missing_version', []);
	}
	if (version !== FLOW_LIMITS_VERSION) {
		return rejected(version, 'unsupported_version', []);
	}

	const missingFields: string[] = [];
	const adopted: Partial<Record<keyof FlowLimitsV1, number>> = {};
	for (const field of FLOW_LIMITS_FIELDS) {
		const wireName = wireFieldName(field);
		const value = readWireNumber(record, wireName);
		if (value === null) {
			missingFields.push(wireName);
			continue;
		}
		adopted[field] = value;
	}
	if (missingFields.length > 0) {
		return rejected(version, 'incomplete_payload', missingFields);
	}

	return {
		outcome: 'supported',
		version,
		limits: adopted as FlowLimitsV1,
		readOnly: false
	};
}

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

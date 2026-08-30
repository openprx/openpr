/**
 * `sylvode.flow.limits.v1` client-side parity + version negotiation.
 *
 * Run with: `bun run test:flow-limits`
 *
 * Covers the two properties `contracts/limits-v1.md`'s `bootstrap_parity` evidence names:
 *
 *  - the client's `FlowLimitsV1` field set is exactly the server's `Bootstrap.limits` wire field
 *    set (`version` excluded -- it is negotiated, not a ceiling), field-for-field, with the wire
 *    spelling derived rather than hand-mapped; and
 *  - an unrecognised `limits.version` is refused rather than absorbed, yielding read-only
 *    ("Client 不得自行放宽或缓存跨 `version` limits").
 *
 * The `WIRE_SCHEMA` literal below is an INDEPENDENT transcription of the frozen wire schema in
 * `contracts/limits-v1.md` ("Bootstrap.limits wire schema"), kept separate from `limits.ts`'s
 * own fallback constant on purpose: if either copy drifts from the contract, the two stop
 * agreeing and this file fails. It is not imported from the implementation.
 */

import assert from 'node:assert/strict';
import {
	DEFAULT_FLOW_LIMITS,
	FLOW_LIMITS_FIELDS,
	FLOW_LIMITS_VERSION,
	checkUpdateBytes,
	negotiateFlowLimitsVersion,
	wireFieldName
} from '../src/lib/flow/limits';
import type { FlowLimitsV1 } from '../src/lib/flow/types';

/** Transcribed from `contracts/limits-v1.md`'s `FlowLimitsV1 = { ... }` block, in wire order. */
const WIRE_SCHEMA: Readonly<Record<string, number>> = {
	update_bytes_max: 65536,
	websocket_frame_bytes_max: 131072,
	presence_payload_bytes_max: 8192,
	presence_ttl_seconds_max: 30,
	bootstrap_decoded_bytes_max: 8388608,
	bootstrap_response_bytes_max: 12582912,
	tree_depth_max: 32,
	container_count_max: 10000,
	document_block_count_max: 10000,
	text_block_chars_max: 100000,
	document_text_chars_max: 1000000,
	semantic_patch_operations_max: 100,
	semantic_patch_json_bytes_max: 1048576,
	decode_apply_cpu_ms_max: 50,
	decode_apply_wall_ms_max: 100,
	isolated_apply_memory_bytes_max: 134217728,
	open_documents_per_connection_max: 8,
	connections_per_user_max: 16,
	connections_per_document_max: 100,
	connections_per_workspace_max: 500,
	presence_entries_per_connection_max: 8,
	presence_entries_per_document_max: 100,
	frames_per_connection_per_second: 30,
	frame_burst_max: 60,
	updates_per_connection_per_second: 10,
	update_burst_max: 20,
	slow_consumer_queue_frames_max: 256,
	slow_consumer_queue_bytes_max: 8388608,
	page_limit_default: 50,
	page_limit_max: 100,
	authorized_scan_rows_max: 1000,
	import_archive_bytes_max: 134217728,
	import_expanded_bytes_max: 536870912,
	import_entry_count_max: 100000,
	import_compression_ratio_max: 100
};

function wirePayload(overrides: Record<string, unknown> = {}): Record<string, unknown> {
	return { version: FLOW_LIMITS_VERSION, ...WIRE_SCHEMA, ...overrides };
}

let failures = 0;
function check(name: string, run: () => void): void {
	try {
		run();
		console.log(`ok   ${name}`);
	} catch (error) {
		failures += 1;
		console.error(`FAIL ${name}`);
		console.error(error instanceof Error ? error.message : String(error));
	}
}

// ---- field-set parity -------------------------------------------------------------------

check('client field set covers every wire field and adds none', () => {
	const clientWireNames = FLOW_LIMITS_FIELDS.map(wireFieldName).sort();
	const serverWireNames = Object.keys(WIRE_SCHEMA).sort();
	assert.deepEqual(clientWireNames, serverWireNames);
	assert.equal(clientWireNames.length, 35);
});

check('`version` is not a ceiling field', () => {
	assert.equal(
		FLOW_LIMITS_FIELDS.some((field) => wireFieldName(field) === 'version'),
		false
	);
});

check('wire names are derived, not hand-mapped', () => {
	assert.equal(
		wireFieldName('presencePayloadBytesMax' as keyof FlowLimitsV1),
		'presence_payload_bytes_max'
	);
	assert.equal(
		wireFieldName('framesPerConnectionPerSecond' as keyof FlowLimitsV1),
		'frames_per_connection_per_second'
	);
	assert.equal(
		wireFieldName('decodeApplyCpuMsMax' as keyof FlowLimitsV1),
		'decode_apply_cpu_ms_max'
	);
	assert.equal(wireFieldName('pageLimitDefault' as keyof FlowLimitsV1), 'page_limit_default');
});

check('pre-bootstrap fallback equals the frozen contract values', () => {
	for (const field of FLOW_LIMITS_FIELDS) {
		assert.equal(
			DEFAULT_FLOW_LIMITS[field],
			WIRE_SCHEMA[wireFieldName(field)],
			`${field} (${wireFieldName(field)}) drifted from contracts/limits-v1.md`
		);
	}
});

// ---- version negotiation ----------------------------------------------------------------

check('a matching version adopts the server values, not the fallback', () => {
	const result = negotiateFlowLimitsVersion(
		wirePayload({ update_bytes_max: 4096, page_limit_max: 7 })
	);
	assert.equal(result.outcome, 'supported');
	assert.equal(result.readOnly, false);
	assert.equal(result.limits.updateBytesMax, 4096);
	assert.equal(result.limits.pageLimitMax, 7);
	// Untouched fields still come from the payload, never merged over the fallback.
	assert.equal(result.limits.importExpandedBytesMax, WIRE_SCHEMA.import_expanded_bytes_max);
});

check('an unknown version is read-only and does not relax anything', () => {
	const result = negotiateFlowLimitsVersion(wirePayload({ version: 'sylvode.flow.limits.v2' }));
	assert.equal(result.outcome, 'unknownVersion');
	assert.equal(result.readOnly, true);
	assert.equal(result.outcome === 'unknownVersion' && result.reason, 'unsupported_version');
	assert.equal(result.version, 'sylvode.flow.limits.v2');
});

check('a missing version is read-only', () => {
	const noVersion = wirePayload();
	delete noVersion.version;
	const result = negotiateFlowLimitsVersion(noVersion);
	assert.equal(result.outcome, 'unknownVersion');
	assert.equal(result.outcome === 'unknownVersion' && result.reason, 'missing_version');
	assert.equal(result.version, null);
});

check('a truncated payload on a known version is read-only, naming the gaps', () => {
	const partial = wirePayload();
	delete partial.import_entry_count_max;
	delete partial.frame_burst_max;
	const result = negotiateFlowLimitsVersion(partial);
	assert.equal(result.outcome, 'unknownVersion');
	assert.equal(result.readOnly, true);
	assert.equal(result.outcome === 'unknownVersion' && result.reason, 'incomplete_payload');
	assert.deepEqual(result.outcome === 'unknownVersion' ? [...result.missingFields].sort() : [], [
		'frame_burst_max',
		'import_entry_count_max'
	]);
});

check('non-numeric and negative wire values count as missing', () => {
	const result = negotiateFlowLimitsVersion(
		wirePayload({ tree_depth_max: '32', page_limit_max: -1 })
	);
	assert.equal(result.outcome, 'unknownVersion');
	assert.deepEqual(result.outcome === 'unknownVersion' ? [...result.missingFields].sort() : [], [
		'page_limit_max',
		'tree_depth_max'
	]);
});

check('a non-object payload is read-only', () => {
	for (const payload of [null, undefined, 42, 'limits', []]) {
		const result = negotiateFlowLimitsVersion(payload);
		assert.equal(result.outcome, 'unknownVersion');
		assert.equal(result.outcome === 'unknownVersion' && result.reason, 'missing_payload');
	}
});

// ---- pre-checks use the negotiated ceilings ----------------------------------------------

check('checkUpdateBytes honours a negotiated server ceiling', () => {
	const negotiated = negotiateFlowLimitsVersion(wirePayload({ update_bytes_max: 8 }));
	assert.equal(negotiated.outcome, 'supported');
	assert.equal(checkUpdateBytes(new Uint8Array(8), negotiated.limits), null);
	const violation = checkUpdateBytes(new Uint8Array(9), negotiated.limits);
	assert.deepEqual(violation, { limitKind: 'update_bytes', limit: 8, observed: 9 });
	// The compiled-in fallback would have accepted it -- proving the server value is what ran.
	assert.equal(checkUpdateBytes(new Uint8Array(9)), null);
});

if (failures > 0) {
	console.error(`\n${failures} check(s) failed`);
	process.exit(1);
}
console.log('\nall flow limits negotiation checks passed');

import assert from 'node:assert/strict';
import { flowApi } from '../src/lib/api/flow';

const calls: Array<{ method: string; path: string; body: unknown; idempotencyKey: string | null }> =
	[];
globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
	const raw = String(input);
	const path = raw.startsWith('http') ? new URL(raw).pathname : raw;
	calls.push({
		method: init?.method ?? 'GET',
		path,
		body: init?.body ? JSON.parse(String(init.body)) : null,
		idempotencyKey: new Headers(init?.headers).get('Idempotency-Key')
	});
	return new Response(JSON.stringify({ code: 0, message: 'ok', data: {} }), {
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;

await flowApi.referenceObject('source', {
	target_type: 'form_record',
	target_id: 'target',
	display: { mode: 'embed' },
	idempotency_key: 'reference-key'
});
await flowApi.unreferenceObject('source', 'reference', 'unreference-key');
await flowApi.previewConversion({
	source_object_id: 'source',
	source_frontier: 'frontier',
	target_type: 'form_record',
	mapping: { target_form_id: 'form' },
	idempotency_key: 'preview-key'
});
await flowApi.commitConversion({
	preview_id: 'preview',
	source_frontier: 'frontier',
	target_schema_version: 7,
	idempotency_key: 'commit-key',
	confirm: true
});
await flowApi.getConversion('job');
await flowApi.retryConversion('job', 'retry-key');

assert.deepEqual(
	calls.map(({ method, path }) => [method, path]),
	[
		['POST', '/api/v1/flow/objects/source/references'],
		['DELETE', '/api/v1/flow/objects/source/references/reference'],
		['POST', '/api/v1/flow/conversions/preview'],
		['POST', '/api/v1/flow/conversions'],
		['GET', '/api/v1/flow/conversions/job'],
		['POST', '/api/v1/flow/conversions/job/retry']
	]
);
assert.equal(calls[1].idempotencyKey, 'unreference-key');
assert.deepEqual(calls[2].body, {
	source_object_id: 'source',
	source_frontier: 'frontier',
	target_type: 'form_record',
	mapping: { target_form_id: 'form' },
	idempotency_key: 'preview-key'
});
assert.deepEqual(calls[3].body, {
	preview_id: 'preview',
	source_frontier: 'frontier',
	target_schema_version: 7,
	idempotency_key: 'commit-key',
	confirm: true
});
assert.deepEqual(calls[5].body, { idempotency_key: 'retry-key', confirm: true });
console.log(`ok   Flow bridge Web adapter maps all ${calls.length} contract operations`);

import assert from 'node:assert/strict';
import { FlowPackageImportSession } from '../src/lib/flow/package-import';
import { flowApi } from '../src/lib/api/flow';

const calls: Array<{
	method: string;
	path: string;
	body: unknown;
	idempotencyKey: string | null;
	signal: AbortSignal | null;
}> = [];

const packageHash = 'a'.repeat(64);
const mappingHash = 'b'.repeat(64);
let previewFails = false;

globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
	const raw = String(input);
	const path = raw.startsWith('http') ? new URL(raw).pathname : raw;
	const body =
		init?.body instanceof FormData
			? {
					filename: (init.body.get('package') as File).name,
					size: (init.body.get('package') as File).size
				}
			: init?.body
				? JSON.parse(String(init.body))
				: null;
	calls.push({
		method: init?.method ?? 'GET',
		path,
		body,
		idempotencyKey: new Headers(init?.headers).get('Idempotency-Key'),
		signal: init?.signal ?? null
	});

	let response: unknown = {};
	let code = 0;
	let message = 'ok';
	if (path.endsWith('/import-artifacts')) {
		response = {
			artifact_id: 'artifact',
			package_sha256: packageHash,
			size: 7,
			expires_at: '2026-09-13T12:30:00Z'
		};
	} else if (path.endsWith('/imports/preview')) {
		if (previewFails) {
			code = 400;
			message = 'unsupported package engine';
		} else {
			response = {
				preview_id: 'preview',
				package_id: 'package',
				package_sha256: packageHash,
				mapping_hash: mappingHash,
				mapping: { project_map: {} },
				conflicts: [],
				warnings: [],
				estimated_changes: { objects: 2 },
				expires_at: '2026-09-13T12:30:00Z'
			};
		}
	} else if (path.endsWith('/commit')) {
		response = { job_id: 'job', import_id: 'job', status: 'completed' };
	} else if (path.endsWith('/imports/job')) {
		response = { import_id: 'job', status: 'completed' };
	}
	return new Response(JSON.stringify({ code, message, data: code === 0 ? response : null }), {
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;

const controller = new AbortController();
const session = new FlowPackageImportSession('workspace');
await session.upload(
	new Blob(['package']),
	'workspace.sylvode-flow.zip',
	'upload-key',
	controller.signal
);
const preview = await session.preview({
	projectMapping: { source: 'target' },
	externalReferencePolicy: 'detach',
	conflictPolicy: 'reject_existing',
	includeHistory: false,
	idempotencyKey: 'preview-key'
});

const callsBeforeWrongHash = calls.length;
await assert.rejects(
	session.commit({
		exactPackageSha256: '0'.repeat(64),
		idempotencyKey: 'must-not-send'
	}),
	/match the server preview/
);
assert.equal(calls.length, callsBeforeWrongHash, 'hash mismatch must not issue a commit request');

await session.commit({
	exactPackageSha256: preview.package_sha256,
	idempotencyKey: 'commit-key'
});
await flowApi.getPackageImport('workspace', 'job');

assert.deepEqual(
	calls.map(({ method, path }) => [method, path]),
	[
		['POST', '/api/v1/workspaces/workspace/flow/import-artifacts'],
		['POST', '/api/v1/workspaces/workspace/flow/imports/preview'],
		['POST', '/api/v1/workspaces/workspace/flow/imports/preview/commit'],
		['GET', '/api/v1/workspaces/workspace/flow/imports/job']
	]
);
assert.deepEqual(calls[0].body, { filename: 'workspace.sylvode-flow.zip', size: 7 });
assert.equal(calls[0].idempotencyKey, 'upload-key');
assert.equal(calls[0].signal, controller.signal, 'upload cancellation signal must reach fetch');
assert.deepEqual(calls[1].body, {
	artifact_id: 'artifact',
	project_mapping: { source: 'target' },
	external_reference_policy: 'detach',
	conflict_policy: 'reject_existing',
	include_history: false,
	idempotency_key: 'preview-key'
});
assert.deepEqual(calls[2].body, {
	package_sha256: packageHash,
	mapping_hash: mappingHash,
	conflict_policy: 'reject_existing',
	confirm: true,
	idempotency_key: 'commit-key'
});

previewFails = true;
const rejectedSession = new FlowPackageImportSession('workspace');
await rejectedSession.upload(new Blob(['package']), 'bad.sylvode-flow.zip', 'bad-upload');
await assert.rejects(
	rejectedSession.preview({
		externalReferencePolicy: 'reject',
		conflictPolicy: 'reject_existing',
		includeHistory: false,
		idempotencyKey: 'bad-preview'
	}),
	/unsupported package engine/
);
const callsBeforeRejectedCommit = calls.length;
await assert.rejects(
	rejectedSession.commit({
		exactPackageSha256: packageHash,
		idempotencyKey: 'must-not-send-after-preview-error'
	}),
	/successful server preview/
);
assert.equal(
	calls.length,
	callsBeforeRejectedCommit,
	'checksum, unsupported, or policy preview errors must keep confirm fail-closed'
);

assert.equal(
	Object.values(session as unknown as Record<string, unknown>).some(
		(value) => value instanceof Blob
	),
	false,
	'the session must not retain package bytes'
);
console.log(
	`ok   Flow package import adapter locks ${calls.length} upload/preview/commit/status calls`
);

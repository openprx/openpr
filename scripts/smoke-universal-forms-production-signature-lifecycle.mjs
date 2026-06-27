#!/usr/bin/env node

const frontendUrl = (process.env.OPENPR_FRONTEND_URL ?? 'http://10.72.0.3:3000').replace(/\/+$/, '');
const email = process.env.OPENPR_DEMO_EMAIL ?? 'demo@openpr.local';
const password = process.env.OPENPR_DEMO_PASSWORD ?? 'OpenPRDemo123!';
const workspaceOverride = process.env.OPENPR_WORKSPACE_ID;
const projectOverride = process.env.OPENPR_PROJECT_ID;
const cleanupCreated = process.env.OPENPR_SIGNATURE_CLEANUP !== '0';
const suffix = Date.now().toString(36);

let accessToken = '';
let workspaceId = '';
let projectId = '';
let createdFormId = '';

const signatureOne = 'data:image/png;base64,c2lnbmF0dXJlLW9uZQ==';
const signatureTwo = 'data:image/png;base64,c2lnbmF0dXJlLXR3bw==';

function usage() {
	console.log(`Usage:
  OPENPR_FRONTEND_URL=https://openpr.example \\
  OPENPR_DEMO_EMAIL=admin@example.com \\
  OPENPR_DEMO_PASSWORD=... \\
  scripts/smoke-universal-forms-production-signature-lifecycle.mjs

Optional:
  OPENPR_WORKSPACE_ID             Workspace to use. Defaults to first visible workspace.
  OPENPR_PROJECT_ID               Project to use. Defaults to first custom_form project, then first project.
  OPENPR_SIGNATURE_CLEANUP=0      Keep the temporary form for inspection.

This deployed smoke creates a temporary signature form, materializes a PNG
signature into object storage, verifies signed download, updates the signature,
	and checks /api/v1/form-records/{record_id}/signatures/audit-verification for
	digest verification, signature_lifecycle active/replacement metadata, and a
	verified signature_workflow_verification decision.`);
}

if (process.argv.includes('--help') || process.argv.includes('-h')) {
	usage();
	process.exit(0);
}

function assert(condition, message) {
	if (!condition) throw new Error(message);
}

function absoluteUrl(pathOrUrl) {
	return pathOrUrl.startsWith('http://') || pathOrUrl.startsWith('https://')
		? pathOrUrl
		: `${frontendUrl}${pathOrUrl}`;
}

async function rawJsonApi(path, options = {}) {
	const headers = new Headers(options.headers ?? {});
	headers.set('Content-Type', 'application/json');
	if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
	const response = await fetch(`${frontendUrl}${path}`, {
		...options,
		headers,
		body: options.body === undefined || typeof options.body === 'string' ? options.body : JSON.stringify(options.body)
	});
	const text = await response.text();
	const parsed = text ? JSON.parse(text) : { code: response.status, message: response.statusText, data: null };
	return { response, parsed, text };
}

async function api(path, options = {}) {
	const { response, parsed, text } = await rawJsonApi(path, options);
	if (!response.ok || parsed.code !== 0) {
		throw new Error(`${options.method ?? 'GET'} ${path} failed: HTTP ${response.status} ${text}`);
	}
	return parsed.data;
}

async function fetchBinary(pathOrUrl, label) {
	const headers = new Headers();
	if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
	const response = await fetch(absoluteUrl(pathOrUrl), { headers });
	const bytes = Buffer.from(await response.arrayBuffer());
	if (!response.ok || bytes.length === 0) {
		throw new Error(`${label} read failed: HTTP ${response.status}, bytes=${bytes.length}`);
	}
	return { response, bytes };
}

async function login() {
	const result = await api('/api/v1/auth/login', {
		method: 'POST',
		body: { email, password }
	});
	accessToken = result.tokens?.access_token;
	assert(accessToken, 'login did not return an access token');
}

async function chooseWorkspace() {
	if (workspaceOverride) return workspaceOverride;
	const workspaces = await api('/api/v1/workspaces');
	const workspace = workspaces.items?.[0] ?? workspaces[0];
	assert(workspace?.id, 'no workspace available');
	return workspace.id;
}

async function chooseProject(selectedWorkspaceId) {
	if (projectOverride) return projectOverride;
	const projects = await api(`/api/v1/workspaces/${selectedWorkspaceId}/projects?per_page=100`);
	const items = projects.items ?? projects;
	const project = items.find((item) => item.type_key === 'custom_form') ?? items[0];
	assert(project?.id, 'no project available');
	return project.id;
}

async function createForm() {
	const form = await api(`/api/v1/projects/${projectId}/forms`, {
		method: 'POST',
		body: {
			key: `signature_lifecycle_acceptance_${suffix}`,
			name: `Signature Lifecycle Acceptance ${suffix}`,
			description: 'Temporary form for deployed signature lifecycle acceptance.',
			title_template: '{name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'name', label: 'Name', type: 'text', required: true },
					{ key: 'signature_reason', label: 'Signature Reason', type: 'text', required: true },
					{
						key: 'guest_signature',
						label: 'Guest Signature',
						type: 'signature',
						required: true,
						signature: {
							retention_days: 30,
							reason_field: 'signature_reason',
							reason_required: true,
							consent_statement: 'I confirm this signature is intentional.'
						}
					}
				]
			}
		}
	});
	createdFormId = form.id;
	return form;
}

async function cleanup() {
	if (!cleanupCreated || !createdFormId) return;
	try {
		await api(`/api/v1/forms/${createdFormId}`, { method: 'DELETE' });
	} catch (error) {
		console.error(`form cleanup failed: ${error.message}`);
	}
}

function signatureFileName(url) {
	const prefix = '/api/v1/uploads/signatures/';
	assert(typeof url === 'string' && url.startsWith(prefix), `signature value is not a server URL: ${url}`);
	const fileName = url.slice(prefix.length).split('?')[0];
	assert(fileName.endsWith('.png') && !fileName.includes('/') && !fileName.includes('..'), `unsafe signature file name: ${fileName}`);
	return fileName;
}

function lifecycleField(audit, fieldKey = 'guest_signature') {
	const lifecycle = audit.signature_lifecycle;
	assert(lifecycle?.version === 'v1', `signature_lifecycle missing or wrong version: ${JSON.stringify(audit)}`);
	const field = lifecycle.fields?.find((item) => item.field_key === fieldKey);
	assert(field, `signature_lifecycle missing field ${fieldKey}: ${JSON.stringify(lifecycle)}`);
	return field;
}

	function assertVerifiedAudit(audit, expectedMinimumEvents = 1) {
		assert(audit.signature_audit?.total_count >= expectedMinimumEvents, `signature_audit total_count invalid: ${JSON.stringify(audit)}`);
		assert(audit.signature_audit?.failed_count === 0, `signature_audit has failures: ${JSON.stringify(audit.signature_audit)}`);
		assert(
			audit.signature_audit?.verified_count === audit.signature_audit?.total_count,
			`signature_audit verified count mismatch: ${JSON.stringify(audit.signature_audit)}`
		);
	}

	function assertVerifiedWorkflow(audit, label) {
		const workflow = audit.signature_workflow_verification;
		assert(workflow?.version === 'v1', `${label} signature_workflow_verification missing or wrong version: ${JSON.stringify(audit)}`);
		assert(workflow.status === 'verified', `${label} signature_workflow_verification status mismatch: ${JSON.stringify(workflow)}`);
		assert(workflow.verified === true, `${label} signature_workflow_verification verified flag mismatch: ${JSON.stringify(workflow)}`);
		assert(workflow.active_signature_field_count >= 1, `${label} signature_workflow_verification missing active fields: ${JSON.stringify(workflow)}`);
		assert(workflow.current_audit_entry_count >= 1, `${label} signature_workflow_verification missing current audit entries: ${JSON.stringify(workflow)}`);
		assert(workflow.failed_audit_count === 0, `${label} signature_workflow_verification has failed audits: ${JSON.stringify(workflow)}`);
		assert(
			workflow.verified_audit_count >= workflow.current_audit_entry_count,
			`${label} signature_workflow_verification audit count mismatch: ${JSON.stringify(workflow)}`
		);
		assert(workflow.reasoned_event_count >= 1, `${label} signature_workflow_verification missing reasoned events: ${JSON.stringify(workflow)}`);
		assert(workflow.consented_event_count >= 1, `${label} signature_workflow_verification missing consented events: ${JSON.stringify(workflow)}`);
	}

async function assertSignatureDownload(url, label) {
	const fileName = signatureFileName(url);
	const direct = await fetchBinary(url, `${label} direct signature`);
	assert(
		direct.response.headers.get('content-type')?.includes('image/png'),
		`${label} direct signature content-type is not image/png: ${direct.response.headers.get('content-type')}`
	);
	const signed = await api(`/api/v1/uploads/signatures/${fileName}/signed-url`, { method: 'POST' });
	assert(signed.url?.includes(`/api/v1/uploads/signatures/${fileName}/download?`), `signed URL mismatch: ${JSON.stringify(signed)}`);
	const downloaded = await fetchBinary(signed.url, `${label} signed signature`);
	assert(
		downloaded.response.headers.get('content-type')?.includes('image/png'),
		`${label} signed signature content-type is not image/png: ${downloaded.response.headers.get('content-type')}`
	);
	return { fileName, directBytes: direct.bytes.length, signedBytes: downloaded.bytes.length, signedUrl: signed.url };
}

async function main() {
	await login();
	workspaceId = await chooseWorkspace();
	projectId = await chooseProject(workspaceId);
	const form = await createForm();

	const createdRecord = await api(`/api/v1/forms/${form.id}/records`, {
		method: 'POST',
		body: {
			values: {
				name: `Signed Guest ${suffix}`,
				signature_reason: 'Initial acceptance',
				guest_signature: signatureOne
			},
			idempotency_key: `signature-lifecycle-create-${suffix}`
		}
	});
	assert(createdRecord.id, `record create did not return id: ${JSON.stringify(createdRecord)}`);
	const firstUrl = createdRecord.values?.guest_signature;
	const firstDownload = await assertSignatureDownload(firstUrl, 'created');
		const firstAudit = await api(`/api/v1/form-records/${createdRecord.id}/signatures/audit-verification`);
		assertVerifiedAudit(firstAudit, 1);
		assertVerifiedWorkflow(firstAudit, 'created');
		const firstField = lifecycleField(firstAudit);
	assert(firstField.status === 'active_with_audit', `created lifecycle status mismatch: ${JSON.stringify(firstField)}`);
	assert(firstField.current_object_key === `signatures/${firstDownload.fileName}`, `created lifecycle object mismatch: ${JSON.stringify(firstField)}`);
	assert(firstField.materialize_count === 1, `created lifecycle materialize_count mismatch: ${JSON.stringify(firstField)}`);
	assert(firstField.current_audit_entry_count === 1, `created lifecycle current audit count mismatch: ${JSON.stringify(firstField)}`);
	assert(firstAudit.signature_lifecycle.verifiable_event_count === 1, `created lifecycle verifiable count mismatch: ${JSON.stringify(firstAudit.signature_lifecycle)}`);

	const updatedRecord = await api(`/api/v1/form-records/${createdRecord.id}`, {
		method: 'PATCH',
		body: {
			values: {
				name: `Signed Guest ${suffix}`,
				signature_reason: 'Replacement acceptance',
				guest_signature: signatureTwo
			},
			idempotency_key: `signature-lifecycle-update-${suffix}`
		}
	});
	const secondUrl = updatedRecord.values?.guest_signature;
	assert(secondUrl !== firstUrl, 'signature update did not replace the materialized URL');
	const secondDownload = await assertSignatureDownload(secondUrl, 'updated');
		const replacementAudit = await api(`/api/v1/form-records/${createdRecord.id}/signatures/audit-verification`);
		assertVerifiedAudit(replacementAudit, 1);
		assertVerifiedWorkflow(replacementAudit, 'replacement');
		const replacementField = lifecycleField(replacementAudit);
	assert(replacementField.status === 'active_with_audit', `replacement lifecycle status mismatch: ${JSON.stringify(replacementField)}`);
	assert(replacementField.current_object_key === `signatures/${secondDownload.fileName}`, `replacement lifecycle object mismatch: ${JSON.stringify(replacementField)}`);
	assert(replacementField.replacement_count === 1, `replacement lifecycle replacement_count mismatch: ${JSON.stringify(replacementField)}`);
	assert(
		replacementField.previous_objects?.some((item) => item.object_key === `signatures/${firstDownload.fileName}`),
		`replacement lifecycle missing previous object: ${JSON.stringify(replacementField)}`
	);
	const materializeEvent = replacementAudit.signature_lifecycle.events?.find((event) => event.action === 'materialize');
	assert(materializeEvent?.reason_present === true, `replacement lifecycle missing reason presence: ${JSON.stringify(materializeEvent)}`);
	assert(materializeEvent?.consent_statement_present === true, `replacement lifecycle missing consent presence: ${JSON.stringify(materializeEvent)}`);
	assert(materializeEvent?.operation === 'record.update', `replacement lifecycle operation mismatch: ${JSON.stringify(materializeEvent)}`);

	console.log(JSON.stringify({
		ok: true,
		frontend_url: frontendUrl,
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: form.id,
		record_id: createdRecord.id,
		initial_signature_file: firstDownload.fileName,
		replacement_signature_file: secondDownload.fileName,
		signature_audit: {
			total_count: replacementAudit.signature_audit.total_count,
			verified_count: replacementAudit.signature_audit.verified_count,
			failed_count: replacementAudit.signature_audit.failed_count
		},
			signature_lifecycle: {
				status: replacementField.status,
				materialize_count: replacementField.materialize_count,
				replacement_count: replacementField.replacement_count,
				current_object_key: replacementField.current_object_key,
				previous_object_key: replacementField.previous_objects?.[0]?.object_key ?? null,
				verifiable_event_count: replacementAudit.signature_lifecycle.verifiable_event_count
			},
			signature_workflow_verification: {
				status: replacementAudit.signature_workflow_verification.status,
				verified: replacementAudit.signature_workflow_verification.verified,
				active_signature_field_count: replacementAudit.signature_workflow_verification.active_signature_field_count,
				current_audit_entry_count: replacementAudit.signature_workflow_verification.current_audit_entry_count,
				verified_audit_count: replacementAudit.signature_workflow_verification.verified_audit_count
			},
			bytes: {
			initial_direct: firstDownload.directBytes,
			initial_signed: firstDownload.signedBytes,
			replacement_direct: secondDownload.directBytes,
			replacement_signed: secondDownload.signedBytes
		},
		cleanup: cleanupCreated
	}, null, 2));
}

main()
	.catch(async (error) => {
		console.error(error.stack ?? error.message);
		process.exitCode = 1;
	})
	.finally(cleanup);

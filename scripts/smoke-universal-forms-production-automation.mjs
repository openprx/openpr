#!/usr/bin/env node

const frontendUrl = (process.env.OPENPR_FRONTEND_URL ?? 'http://10.72.0.3:3000').replace(/\/+$/, '');
const email = process.env.OPENPR_DEMO_EMAIL ?? 'demo@openpr.local';
const password = process.env.OPENPR_DEMO_PASSWORD ?? 'OpenPRDemo123!';
const externalEndpoint = process.env.OPENPR_AUTOMATION_ENDPOINT;
const workspaceOverride = process.env.OPENPR_WORKSPACE_ID;
const projectOverride = process.env.OPENPR_PROJECT_ID;
const formOverride = process.env.OPENPR_FORM_ID;
const connectorKind = process.env.OPENPR_AUTOMATION_CONNECTOR_KIND ?? 'webhook';
const cleanupCreated = process.env.OPENPR_AUTOMATION_CLEANUP !== '0';
const timeoutMs = Number.parseInt(process.env.OPENPR_AUTOMATION_TIMEOUT_MS ?? '90000', 10);
const pollMs = Number.parseInt(process.env.OPENPR_AUTOMATION_POLL_MS ?? '1500', 10);
const suffix = Date.now().toString(36);
const acceptanceId = `production-automation-${suffix}`;

let accessToken = '';
let createdConnectorId = '';
let createdFormId = '';
let workspaceId = '';
let projectId = '';
let form = null;

function usage() {
	console.log(`Usage:
  OPENPR_AUTOMATION_ENDPOINT=https://receiver.example/connector \\
  OPENPR_FRONTEND_URL=https://openpr.example \\
  OPENPR_DEMO_EMAIL=admin@example.com \\
  OPENPR_DEMO_PASSWORD=... \\
  scripts/smoke-universal-forms-production-automation.mjs

Required:
  OPENPR_AUTOMATION_ENDPOINT     Real external connector endpoint reachable by the deployed worker.

Optional:
  OPENPR_WORKSPACE_ID            Workspace to use. Defaults to first visible workspace.
  OPENPR_PROJECT_ID              Project to use. Defaults to first custom_form project, then first project.
  OPENPR_FORM_ID                 Existing form used as trigger_ref. If omitted, a temporary form is created.
  OPENPR_AUTOMATION_CONNECTOR_KIND  Connector kind. Default: webhook.
  OPENPR_AUTOMATION_CLEANUP=0    Keep the temporary connector/form for inspection.
  OPENPR_AUTOMATION_TIMEOUT_MS   Poll timeout. Default: 90000.

The external endpoint must receive the worker dispatch and call back to:
  POST /api/v1/invocations/{invocation_id}/receipt

Its receipt payload should include the form_id/form_key and acceptance_id from the dispatch payload so this
script can verify both invocation-scoped and form-scoped inbox diagnostics.`);
}

if (process.argv.includes('--help') || process.argv.includes('-h')) {
	usage();
	process.exit(0);
}

if (!externalEndpoint) {
	console.error('OPENPR_AUTOMATION_ENDPOINT is required for production automation acceptance.');
	usage();
	process.exit(2);
}

function sleep(ms) {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

function assert(condition, message) {
	if (!condition) throw new Error(message);
}

async function rawApi(path, options = {}) {
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
	const { response, parsed, text } = await rawApi(path, options);
	if (!response.ok || parsed.code !== 0) {
		throw new Error(`${options.method ?? 'GET'} ${path} failed: HTTP ${response.status} ${text}`);
	}
	return parsed.data;
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

async function loadOrCreateForm() {
	if (formOverride) {
		return api(`/api/v1/forms/${formOverride}`);
	}
	const created = await api(`/api/v1/projects/${projectId}/forms`, {
		method: 'POST',
		body: {
			key: `automation_acceptance_${suffix}`,
			name: `Automation Acceptance ${suffix}`,
			description: 'Temporary form for deployed automation acceptance.',
			title_template: '{name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [{ key: 'name', label: 'Name', type: 'text', required: true }]
			}
		}
	});
	createdFormId = created.id;
	return created;
}

async function createConnector() {
	const connector = await api(`/api/v1/workspaces/${workspaceId}/connectors`, {
		method: 'POST',
		body: {
			project_id: projectId,
			kind: connectorKind,
			name: `Automation acceptance ${suffix}`,
			description: 'Temporary connector for deployed production automation acceptance.',
			endpoint: externalEndpoint,
			auth_policy: { mode: 'none' },
			capability_manifest: {
				capabilities: ['universal_forms.production_automation_acceptance'],
				events: ['form.record.created', 'connector.invocation'],
				routing: {
					forms: {
						[form.key]: {
							subscribed_events: ['form.record.created'],
							routing_key: acceptanceId
						}
					}
				}
			},
			is_active: true
		}
	});
	createdConnectorId = connector.id;
	return connector;
}

async function cleanup() {
	const failures = [];
	if (!cleanupCreated) return;
	if (createdConnectorId) {
		try {
			await api(`/api/v1/workspaces/${workspaceId}/connectors/${createdConnectorId}`, { method: 'DELETE' });
		} catch (error) {
			failures.push(`connector cleanup failed: ${error.message}`);
		}
	}
	if (createdFormId) {
		try {
			await api(`/api/v1/forms/${createdFormId}`, { method: 'DELETE' });
		} catch (error) {
			failures.push(`form cleanup failed: ${error.message}`);
		}
	}
	if (failures.length) {
		console.error(failures.join('\n'));
	}
}

async function waitFor(label, check) {
	const deadline = Date.now() + timeoutMs;
	let last = '';
	while (Date.now() < deadline) {
		try {
			const value = await check();
			if (value) return value;
		} catch (error) {
			last = error.message;
		}
		await sleep(pollMs);
	}
	throw new Error(`Timed out waiting for ${label}${last ? `: ${last}` : ''}`);
}

function findAcceptanceInbox(page) {
	const items = page.items ?? [];
	return items.find((item) => {
		const serialized = JSON.stringify(item);
		return (
			serialized.includes(acceptanceId) ||
			serialized.includes(form.id) ||
			serialized.includes(form.key)
		);
	});
}

async function main() {
	await login();
	workspaceId = await chooseWorkspace();
	projectId = await chooseProject(workspaceId);
	form = await loadOrCreateForm();
	assert(form?.id && form?.key, 'form selection did not return id/key');
	const connector = await createConnector();

	const invocation = await api(`/api/v1/projects/${projectId}/invocations`, {
		method: 'POST',
		body: {
			connector_id: connector.id,
			trigger_kind: 'manual',
			trigger_ref_type: 'form',
			trigger_ref_id: form.id,
			payload: {
				event: 'form.record.created',
				acceptance_id: acceptanceId,
				form_id: form.id,
				form_key: form.key,
				expected_receipt_payload: {
					acceptance_id: acceptanceId,
					invocation_id: '<worker-dispatch-invocation_id>',
					connector_id: connector.id,
					project_id: projectId,
					form_id: form.id,
					form_key: form.key,
					trigger_ref_type: 'form',
					trigger_ref_id: form.id
				},
				receipt_callback: {
					method: 'POST',
					path_template: '/api/v1/invocations/{invocation_id}/receipt',
					status: 'completed',
					idempotency_key: `connector-receipt:${acceptanceId}:{invocation_id}:completed`
				}
			}
		}
	});

	const dispatched = await waitFor('worker connector delivery dispatch', async () => {
		const current = await api(`/api/v1/invocations/${invocation.id}`);
		const delivery = current.result?.connector_delivery;
		if (!delivery) return null;
		assert(delivery.endpoint === externalEndpoint, 'connector delivery endpoint mismatch');
		assert(delivery.status === 'delivered', `connector delivery status is ${delivery.status}`);
		assert(Number(delivery.status_code) >= 200 && Number(delivery.status_code) < 300, 'connector delivery was not 2xx');
		return current;
	});

	const completed = await waitFor('external connector receipt callback', async () => {
		const current = await api(`/api/v1/invocations/${invocation.id}`);
		return current.status === 'completed' ? current : null;
	});
	assert(completed.connector_id === connector.id, 'completed invocation connector_id mismatch');
	assert(completed.trigger_ref_id === form.id, 'completed invocation trigger_ref_id mismatch');

	const invocationInboxRow = await waitFor('invocation-scoped event_inbox receipt row', async () => {
		const page = await api(`/api/v1/invocations/${invocation.id}/inbox?per_page=50`);
		return findAcceptanceInbox(page);
	});
	assert(invocationInboxRow.status === 'processed', `invocation inbox row status is ${invocationInboxRow.status}`);
	assert(
		invocationInboxRow.event_type === 'connector.delivery.received',
		`invocation inbox event_type is ${invocationInboxRow.event_type}`
	);

	const formInboxRow = await waitFor('form-scoped event_inbox receipt row', async () => {
		const page = await api(`/api/v1/forms/${form.id}/inbox?per_page=50`);
		return findAcceptanceInbox(page);
	});
	assert(formInboxRow.id === invocationInboxRow.id, 'form inbox row did not correlate to invocation inbox row');

	const projectInvocations = await api(`/api/v1/projects/${projectId}/invocations?per_page=50`);
	assert(
		(projectInvocations.items ?? []).some((item) => item.id === invocation.id && item.status === 'completed'),
		'project invocation list does not show completed connector invocation'
	);

	console.log(
		JSON.stringify(
			{
				status: 'passed',
				acceptance_id: acceptanceId,
				workspace_id: workspaceId,
				project_id: projectId,
				form_id: form.id,
				form_key: form.key,
				connector_id: connector.id,
				invocation_id: invocation.id,
				delivery_status_code: dispatched.result.connector_delivery.status_code,
				inbox_id: invocationInboxRow.id,
				cleanup_created: cleanupCreated
			},
			null,
			2
		)
	);
}

try {
	await main();
} finally {
	await cleanup();
}

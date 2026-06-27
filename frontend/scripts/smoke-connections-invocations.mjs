#!/usr/bin/env node
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const workspaceId = 'ws-connections-smoke';
const projectId = 'project-connections-smoke';
const now = '2026-05-31T12:00:00.000Z';

let createConnectorPayload = null;
let togglePayloads = [];
let deletedConnectorId = null;
let cancelledInvocation = false;

let connectors = [
	{
		id: 'connector-existing-mcp',
		workspace_id: workspaceId,
		project_id: projectId,
		webhook_id: null,
		kind: 'mcp',
		name: 'OpenPR MCP',
		description: 'Project MCP connector',
		endpoint: 'http://127.0.0.1:8090',
		auth_policy: { mode: 'bot_token' },
		capability_manifest: { capabilities: ['context.get_project'] },
		is_active: true,
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	}
];

let invocations = [
	{
		id: 'invocation-pending-smoke',
		workspace_id: workspaceId,
		project_id: projectId,
		actor_id: 'user-smoke',
		target_agent_id: null,
		source_task_id: null,
		trigger_kind: 'manual',
		trigger_ref_type: 'connection_test',
		trigger_ref_id: null,
		connector_id: 'connector-existing-mcp',
		connector_kind: 'mcp',
		status: 'pending',
		payload: { source: 'connections-smoke' },
		result: null,
		error_message: null,
		audit_chain_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
		created_at: now,
		updated_at: now
	}
];

const project = {
	id: projectId,
	workspace_id: workspaceId,
	name: 'Connections Smoke Project',
	key: 'CONN',
	description: 'Connector and invocation browser smoke.',
	type_key: 'code_project',
	type_settings: {},
	created_at: now,
	updated_at: now,
	issue_counts: null
};

function apiResult(data) {
	return { code: 0, message: 'success', data };
}

function paginated(items) {
	return { items, total: items.length, page: 1, per_page: items.length, total_pages: 1 };
}

function json(res, status, data) {
	res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });
	res.end(JSON.stringify(data));
}

function html(res, body) {
	res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
	res.end(body);
}

async function readBody(req) {
	const chunks = [];
	for await (const chunk of req) chunks.push(chunk);
	const text = Buffer.concat(chunks).toString('utf8');
	return text ? JSON.parse(text) : {};
}

function contentType(filePath) {
	const ext = extname(filePath);
	if (ext === '.html') return 'text/html; charset=utf-8';
	if (ext === '.js') return 'application/javascript; charset=utf-8';
	if (ext === '.css') return 'text/css; charset=utf-8';
	if (ext === '.json') return 'application/json; charset=utf-8';
	if (ext === '.svg') return 'image/svg+xml';
	return 'application/octet-stream';
}

async function serveStatic(req, res, injectSmoke = false) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	let pathname = decodeURIComponent(url.pathname);
	if (pathname === '/') pathname = '/index.html';
	let filePath = normalize(join(buildDir, pathname));
	if (!filePath.startsWith(buildDir) || !existsSync(filePath)) {
		filePath = join(buildDir, 'index.html');
	}
	try {
		let data = await readFile(filePath, 'utf8');
		if (injectSmoke && basename(filePath) === 'index.html') {
			data = data.replace(
				'</body>',
				`<script>
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function mark(name, value = 'done') {
  document.body.setAttribute(name, value);
  sessionStorage.setItem('connections-smoke:' + name, value);
}
function restoreMarks() {
  for (let index = 0; index < sessionStorage.length; index += 1) {
    const key = sessionStorage.key(index);
    if (key && key.startsWith('connections-smoke:data-')) {
      document.body.setAttribute(key.replace('connections-smoke:', ''), sessionStorage.getItem(key) || 'done');
    }
  }
}
async function waitFor(check, label) {
  for (let i = 0; i < 100; i += 1) {
    restoreMarks();
    const value = check();
    if (value) return value;
    await sleep(120);
  }
  throw new Error('Timed out waiting for ' + label);
}
const textInElements = (text, selector = 'a,button,h1,h2,h3,p,span,td,div') => Array.from(document.querySelectorAll(selector)).some((el) => el.textContent.includes(text));
const findButtonExact = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.trim() === text);
const findInput = (placeholder) => Array.from(document.querySelectorAll('input')).find((input) => input.placeholder === placeholder || input.placeholder.includes(placeholder));
function rowWithText(text) {
  return Array.from(document.querySelectorAll('tr')).find((row) => row.textContent.includes(text));
}
function setInput(placeholder, value) {
  const input = findInput(placeholder);
  if (!input) throw new Error('input not found: ' + placeholder);
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}
(async () => {
  for (const key of Object.keys(sessionStorage)) {
    if (key.startsWith('connections-smoke:')) sessionStorage.removeItem(key);
  }
  await waitFor(() => textInElements('Automation Connections'), 'connections page');
  await waitFor(() => textInElements('OpenPR MCP'), 'existing connector');
  findButtonExact('New Connection').click();
  await waitFor(() => textInElements('New Connection'), 'connector modal');
  setInput('Document review bot', 'Document Review Webhook');
  setInput('https://example.com/agent', 'http://127.0.0.1:19092/webhook');
  setInput('Handles review and callback execution', 'Browser-created webhook connector');
  findButtonExact('Create').click();
  await waitFor(() => textInElements('Document Review Webhook'), 'created connector');
  mark('data-connector-created');

  const createdRow = await waitFor(() => rowWithText('Document Review Webhook'), 'created connector row');
  Array.from(createdRow.querySelectorAll('button')).find((button) => button.textContent.trim() === 'Disable').click();
  await waitFor(() => rowWithText('Document Review Webhook')?.textContent.includes('Inactive'), 'connector disabled');
  mark('data-connector-disabled');
  const disabledRow = await waitFor(() => rowWithText('Document Review Webhook'), 'disabled connector row');
  Array.from(disabledRow.querySelectorAll('button')).find((button) => button.textContent.trim() === 'Enable').click();
  await waitFor(() => rowWithText('Document Review Webhook')?.textContent.includes('Active'), 'connector enabled');
  mark('data-connector-enabled');

  const invocationButton = await waitFor(() => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.includes('manual') && button.textContent.includes('pending')), 'pending invocation');
  invocationButton.click();
  await waitFor(() => textInElements('Invocation Detail'), 'invocation detail');
  mark('data-invocation-detail-opened');
  findButtonExact('Cancel').click();
  await waitFor(() => textInElements('cancelled'), 'invocation cancelled');
  mark('data-invocation-cancelled');

  const deleteRow = await waitFor(() => rowWithText('Document Review Webhook'), 'delete connector row');
  window.confirm = () => true;
  Array.from(deleteRow.querySelectorAll('button')).find((button) => button.textContent.trim() === 'Delete').click();
  await waitFor(() => !textInElements('Document Review Webhook', 'td'), 'connector deleted');
  mark('data-connector-deleted');
  document.body.setAttribute('data-connections-smoke', 'done');
})().catch((error) => {
  document.body.setAttribute('data-connections-smoke', 'failed');
  document.body.setAttribute('data-connections-smoke-error', error.message);
  document.body.insertAdjacentHTML('beforeend', '<pre id="connections-smoke-error">' + error.message + '</pre>');
});
</script></body>`
			);
		}
		res.writeHead(200, { 'Content-Type': contentType(filePath) });
		res.end(data);
	} catch {
		res.writeHead(404);
		res.end('Not found');
	}
}

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const pathname = url.pathname;

	if (pathname === '/smoke-auth-seed') {
		html(
			res,
			`<!doctype html><meta charset="utf-8"><script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace('/workspace/${workspaceId}/connections?connections_smoke=1');
</script>`
		);
		return;
	}

	if (pathname === '/api/v1/auth/me') {
		json(res, 200, apiResult({ user: { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' } }));
		return;
	}
	if (pathname === '/api/v1/notifications/unread-count') {
		json(res, 200, apiResult({ count: 0 }));
		return;
	}
	if (pathname === '/api/v1/workspaces') {
		json(res, 200, apiResult(paginated([{ id: workspaceId, slug: 'connections-smoke', name: 'Connections Smoke', created_at: now, updated_at: now }])));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/projects`) {
		json(res, 200, apiResult(paginated([project])));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/connectors` && req.method === 'GET') {
		json(res, 200, apiResult(connectors));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/connectors` && req.method === 'POST') {
		createConnectorPayload = await readBody(req);
		const connector = {
			id: 'connector-created-smoke',
			workspace_id: workspaceId,
			project_id: createConnectorPayload.project_id ?? null,
			webhook_id: null,
			kind: createConnectorPayload.kind,
			name: createConnectorPayload.name,
			description: createConnectorPayload.description ?? null,
			endpoint: createConnectorPayload.endpoint ?? null,
			auth_policy: createConnectorPayload.auth_policy ?? {},
			capability_manifest: createConnectorPayload.capability_manifest ?? {},
			is_active: createConnectorPayload.is_active ?? true,
			created_by: 'user-smoke',
			created_at: now,
			updated_at: now
		};
		connectors = [connector, ...connectors];
		json(res, 200, apiResult(connector));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/connectors/connector-created-smoke` && req.method === 'PATCH') {
		const payload = await readBody(req);
		togglePayloads = [...togglePayloads, payload];
		connectors = connectors.map((connector) =>
			connector.id === 'connector-created-smoke' ? { ...connector, ...payload, updated_at: now } : connector
		);
		json(res, 200, apiResult(connectors.find((connector) => connector.id === 'connector-created-smoke')));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/connectors/connector-created-smoke` && req.method === 'DELETE') {
		deletedConnectorId = 'connector-created-smoke';
		connectors = connectors.filter((connector) => connector.id !== deletedConnectorId);
		json(res, 200, apiResult(null));
		return;
	}
	if (pathname === `/api/v1/projects/${projectId}/context`) {
		json(
			res,
			200,
			apiResult({
				project,
				project_type: { key: 'code_project', name: 'Code Project', description: '', domain: 'software' },
				resources: [{ id: 'resource-repo', project_id: projectId, kind: 'repo', name: 'Product Repo', locator: { url: 'https://git.example/product' }, permission_policy: {}, sync_status: 'synced', created_at: now, updated_at: now }],
				connectors,
				governance: null,
				workflow: null,
				recent_decisions: [],
				agent_policy: {
					project_id: projectId,
					project_type: 'code_project',
					capabilities: [],
					connector_kinds: ['webhook', 'mcp'],
					action_classes: {},
					mcp: {
						tool_registry: {
							enabled_tools: ['context.get_project', 'project_resources.list'],
							groups: { context: ['context.get_project'], resources: ['project_resources.list'] }
						}
					}
				}
			})
		);
		return;
	}
	if (pathname === `/api/v1/projects/${projectId}/invocations` && req.method === 'GET') {
		json(res, 200, apiResult(paginated(invocations)));
		return;
	}
	if (pathname === '/api/v1/invocations/invocation-pending-smoke/tool-calls') {
		json(res, 200, apiResult(paginated([{ id: 'tool-call-smoke', invocation_id: 'invocation-pending-smoke', workspace_id: workspaceId, project_id: projectId, actor_id: 'user-smoke', tool_name: 'context.get_project', transport: 'mcp', status: 'succeeded', arguments: {}, result_summary: 'context loaded', error_message: null, duration_ms: 12, started_at: now, completed_at: now, created_at: now }])));
		return;
	}
	if (pathname === '/api/v1/invocations/invocation-pending-smoke/cancel' && req.method === 'POST') {
		cancelledInvocation = true;
		invocations = invocations.map((invocation) =>
			invocation.id === 'invocation-pending-smoke' ? { ...invocation, status: 'cancelled', updated_at: now } : invocation
		);
		json(res, 200, apiResult(invocations[0]));
		return;
	}

	await serveStatic(req, res, url.searchParams.has('connections_smoke'));
}

async function runChromium(url) {
	return new Promise((resolve, reject) => {
		const args = [
			'--headless=new',
			'--disable-gpu',
			'--no-sandbox',
			'--disable-dev-shm-usage',
			'--virtual-time-budget=22000',
			'--dump-dom',
			url
		];
		const child = spawn(chromium, args, { stdio: ['ignore', 'pipe', 'pipe'] });
		let stdout = '';
		let stderr = '';
		child.stdout.on('data', (chunk) => {
			stdout += chunk.toString();
		});
		child.stderr.on('data', (chunk) => {
			stderr += chunk.toString();
		});
		child.on('error', reject);
		child.on('close', (code) => {
			if (code !== 0) reject(new Error(`Chromium exited with ${code}: ${stderr}`));
			else resolve(stdout);
		});
	});
}

if (!existsSync(buildDir)) {
	throw new Error(`Missing build directory: ${buildDir}. Run bun run build before this smoke.`);
}
if (!existsSync(chromium)) {
	throw new Error(`Chromium binary not found: ${chromium}`);
}

const server = createServer((req, res) => {
	handler(req, res).catch((error) => {
		json(res, 500, { code: 500, message: error.message, data: null });
	});
});

const port = 19080 + Math.floor(Math.random() * 1000);
await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));

try {
	const dom = await runChromium(`http://127.0.0.1:${port}/smoke-auth-seed`);
	if (!dom.includes('data-connections-smoke="done"')) {
		throw new Error(`Connections browser smoke failed. DOM excerpt:\n${dom.slice(-4000)}`);
	}
	if (!createConnectorPayload || createConnectorPayload.kind !== 'webhook') {
		throw new Error('Expected webhook connector create payload');
	}
	if (!togglePayloads.some((payload) => payload.is_active === false) || !togglePayloads.some((payload) => payload.is_active === true)) {
		throw new Error('Expected connector disable and enable PATCH payloads');
	}
	if (!cancelledInvocation) {
		throw new Error('Expected invocation cancel request');
	}
	if (deletedConnectorId !== 'connector-created-smoke') {
		throw new Error('Expected connector delete request');
	}
	console.log('Connections and invocations browser smoke passed');
} finally {
	server.close();
}

#!/usr/bin/env node
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const workspaceId = 'ws-phase1-smoke';
const projectId = 'project-phase1-smoke';
const now = '2026-05-31T12:00:00.000Z';

let createdTypePayload = null;
let updatedTypePayload = null;
let createdResourcePayload = null;
let updatedResourcePayload = null;
let deletedResourceId = null;

let projectTypes = [
	{
		key: 'code_project',
		workspace_id: null,
		name: 'Code Project',
		description: 'Software delivery project with repositories and directories.',
		domain: 'software',
		default_workflow_id: null,
		enabled_capabilities: ['issues', 'mcp'],
		field_schema: {},
		artifact_schema: {},
		created_at: now,
		updated_at: now
	},
	{
		key: 'contract_review',
		workspace_id: null,
		name: 'Contract Review',
		description: 'Document intake, legal risk review, and approval.',
		domain: 'legal',
		default_workflow_id: null,
		enabled_capabilities: ['issues', 'documents', 'mcp'],
		field_schema: {},
		artifact_schema: {},
		created_at: now,
		updated_at: now
	}
];

let resources = [];

const project = {
	id: projectId,
	workspace_id: workspaceId,
	name: 'Phase 1 Contract Project',
	key: 'P1',
	description: 'Project type and resources browser smoke.',
	type_key: 'contract_review',
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
  sessionStorage.setItem('phase1-smoke:' + name, value);
}
function restoreMarks() {
  for (let index = 0; index < sessionStorage.length; index += 1) {
    const key = sessionStorage.key(index);
    if (key && key.startsWith('phase1-smoke:data-')) {
      document.body.setAttribute(key.replace('phase1-smoke:', ''), sessionStorage.getItem(key) || 'done');
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
const findButton = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.includes(text));
const findButtonExact = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.trim() === text);
function setInputByPlaceholder(placeholder, value) {
  const inputs = Array.from(document.querySelectorAll('input'));
  const input = inputs.find((item) => item.placeholder === placeholder) || inputs.find((item) => item.placeholder.includes(placeholder));
  if (!input) throw new Error('input not found: ' + placeholder);
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  return input;
}
function rowWithText(text) {
  return Array.from(document.querySelectorAll('tr')).find((row) => row.textContent.includes(text));
}
async function navigateWithinApp(path) {
  const link = document.createElement('a');
  link.href = path;
  document.body.appendChild(link);
  link.click();
  link.remove();
  await waitFor(() => location.pathname === path, 'navigate ' + path);
}
(async () => {
  for (const key of Object.keys(sessionStorage)) {
    if (key.startsWith('phase1-smoke:')) sessionStorage.removeItem(key);
  }
  await waitFor(() => textInElements('Project types'), 'project types panel');
  await waitFor(() => textInElements('Code Project'), 'system type visible');
  setInputByPlaceholder('key', 'audit_review');
  setInputByPlaceholder('Name', 'Audit Review');
  setInputByPlaceholder('domain', 'audit');
  setInputByPlaceholder('Description', 'Audit package workflow');
  findButtonExact('Add type').click();
  await waitFor(() => textInElements('Audit Review'), 'created project type');
  mark('data-project-type-created');

  const typeRow = await waitFor(() => rowWithText('audit_review'), 'created type row');
  typeRow.querySelector('button').click();
  const nameInput = await waitFor(() => rowWithText('audit_review')?.querySelector('input[value="Audit Review"], input'), 'type edit input');
  nameInput.value = 'Audit Review Updated';
  nameInput.dispatchEvent(new Event('input', { bubbles: true }));
  Array.from(rowWithText('audit_review').querySelectorAll('button')).find((button) => button.textContent.trim() === 'Save').click();
  await waitFor(() => textInElements('Audit Review Updated'), 'updated project type');
  mark('data-project-type-updated');

  await navigateWithinApp('/workspace/${workspaceId}/projects/${projectId}');
  await waitFor(() => textInElements('Project resources'), 'resources panel');
  const kind = document.querySelector('select[aria-label="Resource kind"]');
  kind.value = 'document_library';
  kind.dispatchEvent(new Event('change', { bubbles: true }));
  setInputByPlaceholder('Name', 'Contract Library');
  setInputByPlaceholder('Locator', '{"url":"https://docs.example/contracts"}');
  findButtonExact('Add').click();
  await waitFor(() => textInElements('Contract Library'), 'created resource');
  mark('data-resource-created');

  const resourceRow = await waitFor(() => rowWithText('Contract Library'), 'resource row');
  Array.from(resourceRow.querySelectorAll('button')).find((button) => button.textContent.trim() === 'Edit').click();
  const editingResourceRow = await waitFor(() => Array.from(document.querySelectorAll('tr')).find((row) => row.querySelector('select') && row.querySelector('input')), 'resource editing row');
  const resourceNameInput = await waitFor(() => editingResourceRow.querySelector('input[value="Contract Library"], input'), 'resource edit input');
  resourceNameInput.value = 'Contract Library Updated';
  resourceNameInput.dispatchEvent(new Event('input', { bubbles: true }));
  Array.from(editingResourceRow.querySelectorAll('button')).find((button) => button.textContent.trim() === 'Save').click();
  await waitFor(() => textInElements('Contract Library Updated'), 'updated resource');
  mark('data-resource-updated');

  const updatedRow = await waitFor(() => rowWithText('Contract Library Updated'), 'updated resource row');
  Array.from(updatedRow.querySelectorAll('button')).find((button) => button.textContent.trim() === 'Delete').click();
  await waitFor(() => !textInElements('Contract Library Updated', 'td'), 'deleted resource');
  mark('data-resource-deleted');
  document.body.setAttribute('data-phase1-project-types-smoke', 'done');
})().catch((error) => {
  document.body.setAttribute('data-phase1-project-types-smoke', 'failed');
  document.body.setAttribute('data-phase1-project-types-smoke-error', error.message);
  document.body.insertAdjacentHTML('beforeend', '<pre id="phase1-smoke-error">' + error.message + '</pre>');
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
location.replace('/workspace/${workspaceId}/projects?phase1_project_types_smoke=1');
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
		json(res, 200, apiResult(paginated([{ id: workspaceId, slug: 'phase1-smoke', name: 'Phase 1 Smoke', created_at: now, updated_at: now }])));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}`) {
		json(res, 200, apiResult({ id: workspaceId, slug: 'phase1-smoke', name: 'Phase 1 Smoke', role: 'owner', created_at: now, updated_at: now }));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/project-types` && req.method === 'GET') {
		json(res, 200, apiResult(paginated(projectTypes)));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/project-types` && req.method === 'POST') {
		createdTypePayload = await readBody(req);
		const item = {
			key: createdTypePayload.key,
			workspace_id: workspaceId,
			name: createdTypePayload.name,
			description: createdTypePayload.description ?? '',
			domain: createdTypePayload.domain ?? 'custom',
			default_workflow_id: null,
			enabled_capabilities: [],
			field_schema: {},
			artifact_schema: {},
			created_at: now,
			updated_at: now
		};
		projectTypes = [...projectTypes, item];
		json(res, 200, apiResult(item));
		return;
	}

	if (pathname === '/api/v1/project-types/audit_review' && req.method === 'PATCH') {
		updatedTypePayload = await readBody(req);
		projectTypes = projectTypes.map((item) =>
			item.key === 'audit_review' ? { ...item, ...updatedTypePayload, updated_at: now } : item
		);
		json(res, 200, apiResult(projectTypes.find((item) => item.key === 'audit_review')));
		return;
	}

	if (pathname === '/api/v1/scenario-templates') {
		json(res, 200, apiResult(paginated([])));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/projects` && req.method === 'GET') {
		json(res, 200, apiResult(paginated([project])));
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}`) {
		json(res, 200, apiResult(project));
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}/issues`) {
		json(res, 200, apiResult(paginated([])));
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}/ai-participants`) {
		json(res, 200, apiResult(paginated([])));
		return;
	}


	if (pathname === `/api/v1/projects/${projectId}/resources` && req.method === 'GET') {
		json(res, 200, apiResult(paginated(resources)));
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}/resources` && req.method === 'POST') {
		createdResourcePayload = await readBody(req);
		const item = {
			id: 'resource-phase1-smoke',
			project_id: projectId,
			kind: createdResourcePayload.kind,
			name: createdResourcePayload.name,
			locator: createdResourcePayload.locator ?? {},
			permission_policy: {},
			sync_status: createdResourcePayload.sync_status ?? 'manual',
			created_by: 'user-smoke',
			created_at: now,
			updated_at: now
		};
		resources = [item];
		json(res, 200, apiResult(item));
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}/resources/resource-phase1-smoke` && req.method === 'PATCH') {
		updatedResourcePayload = await readBody(req);
		resources = resources.map((item) =>
			item.id === 'resource-phase1-smoke' ? { ...item, ...updatedResourcePayload, updated_at: now } : item
		);
		json(res, 200, apiResult(resources.find((item) => item.id === 'resource-phase1-smoke')));
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}/resources/resource-phase1-smoke` && req.method === 'DELETE') {
		deletedResourceId = 'resource-phase1-smoke';
		resources = [];
		json(res, 200, apiResult(null));
		return;
	}

	await serveStatic(req, res, url.searchParams.has('phase1_project_types_smoke'));
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
			if (code !== 0) {
				reject(new Error(`Chromium exited with ${code}: ${stderr}`));
			} else {
				resolve(stdout);
			}
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

const port = 18080 + Math.floor(Math.random() * 1000);
await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));

try {
	const dom = await runChromium(`http://127.0.0.1:${port}/smoke-auth-seed`);
	if (!dom.includes('data-phase1-project-types-smoke="done"')) {
		throw new Error(`Phase 1 browser smoke failed. DOM excerpt:\n${dom.slice(-4000)}`);
	}
	if (!createdTypePayload || createdTypePayload.key !== 'audit_review') {
		throw new Error('Expected project type create payload for audit_review');
	}
	if (!updatedTypePayload || updatedTypePayload.name !== 'Audit Review Updated') {
		throw new Error('Expected project type update payload');
	}
	if (!createdResourcePayload || createdResourcePayload.kind !== 'document_library') {
		throw new Error('Expected document_library resource create payload');
	}
	if (!updatedResourcePayload || updatedResourcePayload.name !== 'Contract Library Updated') {
		throw new Error('Expected resource update payload');
	}
	if (deletedResourceId !== 'resource-phase1-smoke') {
		throw new Error('Expected resource delete request');
	}
	console.log('Phase 1 project types/resources browser smoke passed');
} finally {
	server.close();
}

#!/usr/bin/env node
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const workspaceId = '11111111-1111-4111-8111-111111111111';
const botId = '22222222-2222-4222-8222-222222222222';
const now = '2026-08-16T12:00:00.000Z';
const observedQueries = [];

function apiResult(data) {
	return { code: 0, message: 'success', data };
}

function json(res, data) {
	res.writeHead(200, { 'Content-Type': 'application/json; charset=utf-8' });
	res.end(JSON.stringify(data));
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
	if (!filePath.startsWith(buildDir) || !existsSync(filePath)) filePath = join(buildDir, 'index.html');
	try {
		let data = await readFile(filePath, 'utf8');
		if (injectSmoke && basename(filePath) === 'index.html') {
			data = data.replace(
				'</body>',
				`<script>
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function waitFor(check, label) {
  for (let index = 0; index < 100; index += 1) {
    const value = check();
    if (value) return value;
    await sleep(100);
  }
  throw new Error('Timed out waiting for ' + label);
}
const byText = (text) => Array.from(document.querySelectorAll('button,h1,h2,p,td,span')).find((node) => node.textContent.includes(text));
const button = (text) => Array.from(document.querySelectorAll('button')).find((node) => node.textContent.trim() === text);
const input = (id) => document.getElementById(id);
(async () => {
  await waitFor(() => byText('Operation Records'), 'operation records page');
  await waitFor(() => byText('release_bot'), 'first operation row');
  if (!byText('forms.list') || !byText('mcp_http') || !byText('18 ms')) throw new Error('operation metadata is incomplete');

  input('operation-bot-filter').value = '${botId}';
  input('operation-bot-filter').dispatchEvent(new Event('input', { bubbles: true }));
  input('operation-tool-filter').value = 'forms.list';
  input('operation-tool-filter').dispatchEvent(new Event('input', { bubbles: true }));
  const outcome = document.getElementById('operation-outcome-filter');
  outcome.value = 'ok';
  outcome.dispatchEvent(new Event('change', { bubbles: true }));
  button('Apply').click();
  await waitFor(() => byText('filtered_bot'), 'filtered operation row');

  button('Next').click();
  await waitFor(() => byText('second_page_bot'), 'cursor page');
  button('Previous').click();
  await waitFor(() => byText('filtered_bot'), 'previous cursor page');
  button('Reset').click();
  await waitFor(() => byText('release_bot'), 'reset operation list');
  document.body.setAttribute('data-operation-log-smoke', 'done');
})().catch((error) => {
  document.body.setAttribute('data-operation-log-smoke', 'failed');
  document.body.setAttribute('data-operation-log-smoke-error', error.message);
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

function operation(id, name, toolName, surface, outcome = 'ok') {
	return {
		id,
		workspace_id: workspaceId,
		bot_id: botId,
		bot_name: name,
		tool_name: toolName,
		surface,
		method: 'GET',
		path: `/api/v1/workspaces/${workspaceId}/forms`,
		business_code: outcome === 'ok' ? 0 : 403,
		outcome,
		error_message: outcome === 'ok' ? null : 'forbidden',
		duration_ms: 18,
		request_id: '33333333-3333-4333-8333-333333333333',
		created_at: now
	};
}

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const pathname = url.pathname;
	if (pathname === '/smoke-auth-seed') {
		res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
		res.end(`<!doctype html><script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace('/workspace/${workspaceId}/connections?operation_log_smoke=1');
</script>`);
		return;
	}
	if (pathname === '/api/v1/auth/me') {
		json(res, apiResult({ user: { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' } }));
		return;
	}
	if (pathname === '/api/v1/notifications/unread-count') {
		json(res, apiResult({ count: 0 }));
		return;
	}
	if (pathname === '/api/v1/workspaces') {
		json(res, apiResult({ items: [{ id: workspaceId, slug: 'operation-smoke', name: 'Operation Smoke', created_at: now, updated_at: now }], total: 1, page: 1, per_page: 20, total_pages: 1 }));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/bot-operation-logs`) {
		observedQueries.push(url.searchParams.toString());
		const filtered = url.searchParams.get('tool_name') === 'forms.list';
		const cursor = url.searchParams.get('cursor');
		const item = cursor
			? operation('55555555-5555-4555-8555-555555555555', 'second_page_bot', 'forms.get', 'cli')
			: filtered
				? operation('44444444-4444-4444-8444-444444444444', 'filtered_bot', 'forms.list', 'mcp_http')
				: operation('66666666-6666-4666-8666-666666666666', 'release_bot', 'forms.list', 'mcp_http');
		json(res, apiResult({ items: [item], next_cursor: cursor ? null : 'next-page-cursor' }));
		return;
	}
	await serveStatic(req, res, url.searchParams.has('operation_log_smoke'));
}

async function runChromium(url) {
	return new Promise((resolve, reject) => {
		const child = spawn(chromium, ['--headless=new', '--disable-gpu', '--no-sandbox', '--disable-dev-shm-usage', '--virtual-time-budget=18000', '--dump-dom', url], { stdio: ['ignore', 'pipe', 'pipe'] });
		let stdout = '';
		let stderr = '';
		child.stdout.on('data', (chunk) => { stdout += chunk.toString(); });
		child.stderr.on('data', (chunk) => { stderr += chunk.toString(); });
		child.on('error', reject);
		child.on('close', (code) => code === 0 ? resolve(stdout) : reject(new Error(`Chromium exited with ${code}: ${stderr}`)));
	});
}

if (!existsSync(buildDir)) throw new Error(`Missing build directory: ${buildDir}. Run bun run build first.`);
if (!existsSync(chromium)) throw new Error(`Chromium binary not found: ${chromium}`);

const server = createServer((req, res) => {
	handler(req, res).catch((error) => json(res, { code: 500, message: error.message, data: null }));
});
const port = 19080 + Math.floor(Math.random() * 1000);
await new Promise((resolveListen) => server.listen(port, '127.0.0.1', resolveListen));

try {
	const dom = await runChromium(`http://127.0.0.1:${port}/smoke-auth-seed`);
	if (!dom.includes('data-operation-log-smoke="done"')) {
		throw new Error(`Operation records browser smoke failed. DOM excerpt:\n${dom.slice(-4000)}`);
	}
	if (!observedQueries.some((query) => query.includes(`bot_id=${botId}`) && query.includes('tool_name=forms.list') && query.includes('outcome=ok'))) {
		throw new Error('Expected bot, tool and outcome filters');
	}
	if (!observedQueries.some((query) => query.includes('cursor=next-page-cursor'))) {
		throw new Error('Expected cursor pagination request');
	}
	console.log('Operation records browser smoke passed');
} finally {
	server.close();
}

#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { rmSync } from 'node:fs';

const require = createRequire(import.meta.url);
const WebSocket = require('ws');

const frontendUrl = process.env.OPENPR_FRONTEND_URL ?? 'http://10.72.0.3:3000';
const mcpUrl = process.env.OPENPR_MCP_URL ?? 'http://10.72.0.3:8090/mcp/rpc';
const email = process.env.OPENPR_DEMO_EMAIL ?? 'demo@openpr.local';
const password = process.env.OPENPR_DEMO_PASSWORD ?? 'OpenPRDemo123!';
const chromium = process.env.CHROMIUM_BIN ?? '/usr/bin/chromium';
const suffix = Date.now().toString(36);
const sourceKey = `duplicate_${suffix}`;
const sourceName = `Duplicate Smoke ${suffix}`;
const duplicatedName = `${sourceName} Copy`;

let accessToken = '';
let refreshToken = '';
let user = null;
let sourceForm = null;
let duplicatedForm = null;
let apiDuplicatedForm = null;
let mcpDuplicatedForm = null;
let child = null;
let cdp = null;

function sleep(ms) {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

async function api(path, options = {}) {
	const headers = new Headers(options.headers ?? {});
	headers.set('Content-Type', 'application/json');
	if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
	const res = await fetch(`${frontendUrl}${path}`, {
		...options,
		headers,
		body: options.body && typeof options.body !== 'string' ? JSON.stringify(options.body) : options.body
	});
	const text = await res.text();
	const parsed = text ? JSON.parse(text) : { code: res.status, message: res.statusText, data: null };
	if (parsed.code !== 0) throw new Error(`${path}: ${parsed.message ?? text}`);
	return parsed.data;
}

async function mcpTool(name, args = {}) {
	const res = await fetch(mcpUrl, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({
			jsonrpc: '2.0',
			id: Date.now(),
			method: 'tools/call',
			params: { name, arguments: args }
		})
	});
	const rpc = await res.json();
	if (rpc.error) throw new Error(`MCP ${name}: ${rpc.error.message ?? JSON.stringify(rpc.error)}`);
	const text = rpc.result?.content?.[0]?.text ?? '';
	const parsed = text ? JSON.parse(text) : null;
	if (parsed?.code !== 0) throw new Error(`MCP ${name}: ${parsed?.message ?? text}`);
	return parsed.data;
}

async function setupFixture() {
	const login = await api('/api/v1/auth/login', {
		method: 'POST',
		body: { email, password }
	});
	accessToken = login.tokens.access_token;
	refreshToken = login.tokens.refresh_token;
	user = login.user;

	const workspace = (await api('/api/v1/workspaces')).items[0];
	const project = (await api(`/api/v1/workspaces/${workspace.id}/projects?per_page=100`)).items.find(
		(item) => item.type_key === 'custom_form'
	);
	if (!project) throw new Error('no custom_form project available');

	sourceForm = await api(`/api/v1/projects/${project.id}/forms`, {
		method: 'POST',
		body: {
			key: sourceKey,
			name: sourceName,
			description: 'Temporary deployed duplicate smoke source form.',
			title_template: '{customer_name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'customer_name', label: '客户名称', type: 'text', required: true },
					{
						key: 'contract_amount',
						label: '合同金额',
						type: 'amount',
						required: true,
						amount: { currency: 'CNY', scale: 2 }
					}
				]
			}
		}
	});
	await api(`/api/v1/forms/${sourceForm.id}/views`, {
		method: 'POST',
		body: {
			key: 'amount_view',
			name: '金额视图',
			view_type: 'grid',
			config: { columns: ['customer_name', 'contract_amount'] }
		}
	});
	await api(`/api/v1/forms/${sourceForm.id}/records`, {
		method: 'POST',
		body: {
			title: 'source record must not copy',
			values: { customer_name: '源客户', contract_amount: '123.45' },
			source: { type: 'duplicate-smoke' }
		}
	});
	return { workspaceId: workspace.id, projectId: project.id };
}

async function waitJson(port, path) {
	for (let index = 0; index < 80; index += 1) {
		try {
			const res = await fetch(`http://127.0.0.1:${port}${path}`);
			if (res.ok) return await res.json();
		} catch {
			// Chromium is still starting.
		}
		await sleep(100);
	}
	throw new Error('timed out waiting for Chrome DevTools');
}

function connect(wsUrl) {
	const ws = new WebSocket(wsUrl);
	let id = 0;
	const pending = new Map();
	ws.on('message', (data) => {
		const message = JSON.parse(data.toString());
		if (message.id && pending.has(message.id)) {
			const pair = pending.get(message.id);
			pending.delete(message.id);
			if (message.error) pair.reject(new Error(JSON.stringify(message.error)));
			else pair.resolve(message.result);
		}
	});
	return new Promise((resolve, reject) => {
		ws.on('open', () =>
			resolve({
				send(method, params = {}) {
					const callId = ++id;
					ws.send(JSON.stringify({ id: callId, method, params }));
					return new Promise((resolve, reject) => pending.set(callId, { resolve, reject }));
				},
				close() {
					ws.close();
				}
			})
		);
		ws.on('error', reject);
	});
}

async function openBrowser(targetUrl) {
	const port = 22800 + Math.floor(Math.random() * 300);
	const profile = `/tmp/openpr-duplicate-smoke-${process.pid}`;
	rmSync(profile, { recursive: true, force: true });
	child = spawn(
		chromium,
		[
			'--headless=new',
			'--disable-gpu',
			'--no-sandbox',
			'--disable-dev-shm-usage',
			`--remote-debugging-port=${port}`,
			`--user-data-dir=${profile}`,
			`${frontendUrl}/`
		],
		{ stdio: ['ignore', 'ignore', 'ignore'] }
	);
	const pages = await waitJson(port, '/json');
	const page = pages.find((item) => item.type === 'page') ?? pages[0];
	cdp = await connect(page.webSocketDebuggerUrl);
	await cdp.send('Page.enable');
	await cdp.send('Runtime.enable');
	await cdp.send('Page.bringToFront');
	await sleep(1000);
	await cdp.send('Runtime.evaluate', {
		expression: `
			localStorage.setItem('auth_token', ${JSON.stringify(accessToken)});
			localStorage.setItem('refresh_token', ${JSON.stringify(refreshToken)});
			localStorage.setItem('auth_user', ${JSON.stringify(JSON.stringify(user))});
			localStorage.setItem('locale', 'zh');
			location.href = ${JSON.stringify(targetUrl)};
		`
	});
}

async function evalPage(expression) {
	const result = await cdp.send('Runtime.evaluate', {
		expression,
		returnByValue: true,
		awaitPromise: true
	});
	if (result.exceptionDetails) {
		const description = result.exceptionDetails.exception?.description ?? result.exceptionDetails.text;
		throw new Error(description ?? 'page evaluation failed');
	}
	return result.result.value;
}

async function waitForPage(checkExpression, label) {
	for (let index = 0; index < 100; index += 1) {
		const result = await evalPage(checkExpression);
		if (result) return result;
		await sleep(150);
	}
	const text = await evalPage('document.body ? document.body.innerText.slice(0, 1600) : ""');
	throw new Error(`timed out waiting for ${label}. DOM text: ${text}`);
}

async function clickButtonByText(text) {
	const rect = await waitForPage(
		`
		(() => {
			const button = Array.from(document.querySelectorAll('button')).find(
				(item) => item.textContent.includes(${JSON.stringify(text)}) && !item.disabled
			);
			if (!button) return null;
			button.scrollIntoView({ block: 'center', inline: 'center' });
			const rect = button.getBoundingClientRect();
			return {
				x: rect.left + rect.width / 2,
				y: rect.top + rect.height / 2
			};
		})()
	`,
		`${text} button ready`
	);
	await sleep(2500);
	await cdp.send('Input.dispatchMouseEvent', {
		type: 'mousePressed',
		x: rect.x,
		y: rect.y,
		button: 'left',
		clickCount: 1
	});
	await cdp.send('Input.dispatchMouseEvent', {
		type: 'mouseReleased',
		x: rect.x,
		y: rect.y,
		button: 'left',
		clickCount: 1
	});
	await sleep(500);
	await evalPage(`
		(() => {
			const button = Array.from(document.querySelectorAll('button')).find(
				(item) => item.textContent.includes(${JSON.stringify(text)}) && !item.disabled
			);
			if (button && typeof button.__click === 'function') {
				button.__click(new MouseEvent('click', { bubbles: true, cancelable: true, view: window }));
			}
			return true;
		})()
	`);
}

async function runBrowserDuplicate({ workspaceId, projectId }) {
	await openBrowser(`${frontendUrl}/workspace/${workspaceId}/projects/${projectId}/forms`);
	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(sourceName)})`, 'source form listed');
	await evalPage(`
		(() => {
			const switcher = document.querySelector('#form-switcher');
			if (!switcher) throw new Error('form switcher not found');
			switcher.value = ${JSON.stringify(sourceForm.id)};
			switcher.dispatchEvent(new Event('change', { bubbles: true }));
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(sourceKey)})`, 'source form selected');
	await waitForPage(
		`
		(() => Array.from(document.querySelectorAll('button')).some(
			(item) => item.textContent.includes('复制表单') && !item.disabled
		))()
	`,
		'duplicate form button visible'
	);
}

async function assertDuplicate({ projectId }) {
	apiDuplicatedForm = await api(`/api/v1/forms/${sourceForm.id}/duplicate`, {
		method: 'POST',
		body: {
			key: `${sourceKey}_api_copy`,
			name: `${sourceName} API Copy`
		}
	});
	if (apiDuplicatedForm.title_template !== sourceForm.title_template) {
		throw new Error('API duplicate did not copy title template');
	}
	const apiDuplicateRecords = await api(`/api/v1/forms/${apiDuplicatedForm.id}/records?per_page=10`);
	if ((apiDuplicateRecords.items?.length ?? 0) !== 0) {
		throw new Error('API duplicate records should not be copied');
	}

	mcpDuplicatedForm = await mcpTool('forms.duplicate', {
		form_id: sourceForm.id,
		key: `${sourceKey}_mcp_copy`,
		name: `${sourceName} MCP Copy`
	});
	if (mcpDuplicatedForm.title_template !== sourceForm.title_template) {
		throw new Error('MCP duplicate did not copy title template');
	}
	if (JSON.stringify(mcpDuplicatedForm.schema) !== JSON.stringify(sourceForm.schema)) {
		throw new Error('MCP duplicate did not copy schema');
	}
	const mcpDuplicateViews = await api(`/api/v1/forms/${mcpDuplicatedForm.id}/views`);
	if (!mcpDuplicateViews.some((view) => view.key === 'amount_view')) throw new Error('MCP duplicate did not copy views');
	const mcpDuplicateRecords = await api(`/api/v1/forms/${mcpDuplicatedForm.id}/records?per_page=10`);
	if ((mcpDuplicateRecords.items?.length ?? 0) !== 0) {
		throw new Error('MCP duplicate records should not be copied');
	}

	const list = await api(`/api/v1/projects/${projectId}/forms?per_page=100`);
	if (!list.items.some((item) => item.id === apiDuplicatedForm.id)) {
		throw new Error('API duplicate not found in API list');
	}
	if (!list.items.some((item) => item.id === mcpDuplicatedForm.id)) {
		throw new Error('MCP duplicate not found in API list');
	}
	const sourceRecords = await api(`/api/v1/forms/${sourceForm.id}/records?per_page=10`);
	if ((sourceRecords.items?.length ?? 0) !== 1) throw new Error('source record disappeared');
}

async function cleanup() {
	if (cdp) cdp.close();
	if (child) child.kill('SIGKILL');
	for (const form of [mcpDuplicatedForm, apiDuplicatedForm, duplicatedForm, sourceForm]) {
		if (!form?.id) continue;
		try {
			await api(`/api/v1/forms/${form.id}`, { method: 'DELETE' });
		} catch {
			// Best-effort cleanup.
		}
	}
}

try {
	const fixture = await setupFixture();
	await runBrowserDuplicate(fixture);
	await assertDuplicate(fixture);
	await cleanup();
	console.log('Universal forms deployed duplicate smoke passed');
} catch (error) {
	await cleanup();
	console.error(error);
	process.exit(1);
}

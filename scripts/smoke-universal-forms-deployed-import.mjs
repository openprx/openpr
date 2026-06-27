#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { rmSync } from 'node:fs';

const require = createRequire(import.meta.url);
const WebSocket = require('ws');

const frontendUrl = process.env.OPENPR_FRONTEND_URL ?? 'http://10.72.0.3:3000';
const email = process.env.OPENPR_DEMO_EMAIL ?? 'demo@openpr.local';
const password = process.env.OPENPR_DEMO_PASSWORD ?? 'OpenPRDemo123!';
const chromium = process.env.CHROMIUM_BIN ?? '/usr/bin/chromium';
const suffix = Date.now().toString(36);
const formKey = `import_${suffix}`;
const formName = `Import Smoke ${suffix}`;

let accessToken = '';
let refreshToken = '';
let user = null;
let form = null;
let cdp = null;
let child = null;

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
	if (parsed.code !== 0) throw new Error(`${path}: ${parsed.message}`);
	return parsed.data;
}

async function login() {
	const result = await api('/api/v1/auth/login', {
		method: 'POST',
		body: { email, password }
	});
	accessToken = result.tokens.access_token;
	refreshToken = result.tokens.refresh_token;
	user = result.user;
}

async function createFixture() {
	const workspace = (await api('/api/v1/workspaces')).items[0];
	const project = (await api(`/api/v1/workspaces/${workspace.id}/projects?per_page=100`)).items.find(
		(item) => item.type_key === 'custom_form'
	);
	if (!project) throw new Error('no custom_form project available');
	form = await api(`/api/v1/projects/${project.id}/forms`, {
		method: 'POST',
		body: {
			key: formKey,
			name: formName,
			title_template: '{dish}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'dish', label: '菜品', type: 'text', required: true },
					{ key: 'qty', label: '数量', type: 'integer', required: true },
					{ key: 'price', label: '价格', type: 'amount', required: true, amount: { currency: 'CNY', scale: 2 } },
					{ key: 'status', label: '状态', type: 'single_select', options: ['draft', 'served'] }
				]
			}
		}
	});
	return { workspaceId: workspace.id, projectId: project.id };
}

async function assertApiImport() {
	const invalid = await api(`/api/v1/forms/${form.id}/records/import-preview`, {
		method: 'POST',
		body: {
			rows: [
				{
					row_number: 1,
					values: { dish: '无效菜品', qty: '1', price: 8.5, status: 'draft' },
					source: { type: 'phase9-import-smoke' }
				}
			]
		}
	});
	if (invalid.valid_rows !== 0 || invalid.invalid_rows !== 1) {
		throw new Error(`invalid amount JSON number should be rejected: ${JSON.stringify(invalid)}`);
	}
	const created = await api(`/api/v1/forms/${form.id}/records/import`, {
		method: 'POST',
		body: {
			rows: [
				{
					row_number: 1,
					values: { dish: 'API 米饭', qty: '2', price: '8.50', status: 'draft' },
					source: { type: 'phase9-import-smoke' }
				}
			]
		}
	});
	if (created.created_count !== 1 || created.invalid_rows !== 0) {
		throw new Error(`API import did not create one record: ${JSON.stringify(created)}`);
	}
	const record = created.records[0];
	if (record.values.price.decimal !== '8.50') throw new Error(`amount decimal not normalized: ${JSON.stringify(record)}`);
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
	const port = 22200 + Math.floor(Math.random() * 300);
	const profile = `/tmp/openpr-phase9-import-${process.pid}`;
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

async function runBrowserFlow({ workspaceId, projectId }) {
	await openBrowser(`${frontendUrl}/workspace/${workspaceId}/projects/${projectId}/forms`);
	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(formName)})`, 'fixture form listed');
	await evalPage(`
		(() => {
			const switcher = document.querySelector('#form-switcher');
			if (!switcher) throw new Error('form switcher not found');
			switcher.value = ${JSON.stringify(form.id)};
			switcher.dispatchEvent(new Event('change', { bubbles: true }));
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes('导入记录')`, 'import button');
	await evalPage(`
		(() => {
			const button = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('导入记录'));
			if (!button) throw new Error('import records button not found');
			button.click();
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes('导入内容')`, 'import modal');
	await evalPage(`
		(() => {
			const textarea = document.querySelector('#form-import-text');
			if (!textarea) throw new Error('import textarea not found');
			textarea.value = 'Title,菜品,数量,价格,状态\\n浏览器导入,浏览器米饭,3,12.30,draft';
			textarea.dispatchEvent(new Event('input', { bubbles: true }));
			return true;
		})()
	`);
	await evalPage(`
		(() => {
			const button = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('预览导入'));
			if (!button) throw new Error('preview import button not found');
			button.click();
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes('可导入 1 行')`, 'valid import preview');
	await evalPage(`
		(() => {
			const button = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('提交导入'));
			if (!button || button.disabled) throw new Error('commit import button not enabled');
			button.click();
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes('浏览器米饭')`, 'imported browser record');
}

async function cleanup() {
	if (cdp) cdp.close();
	if (child) child.kill('SIGKILL');
	if (form) {
		try {
			await api(`/api/v1/forms/${form.id}`, { method: 'DELETE' });
		} catch {
			// Best-effort cleanup.
		}
	}
}

try {
	await login();
	const fixture = await createFixture();
	await assertApiImport();
	await runBrowserFlow(fixture);
	console.log('Universal forms deployed import smoke passed');
} catch (error) {
	console.error(error);
	process.exitCode = 1;
} finally {
	await cleanup();
}

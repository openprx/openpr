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
const formKey = `saved_view_${suffix}`;
const formName = `Saved View ${suffix}`;
const viewKey = `compact_${suffix}`;
const viewName = `Phase9 compact ${suffix}`;

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
					{ key: 'status', label: '状态', type: 'single_select', options: ['draft', 'served'] }
				]
			}
		}
	});
	await api(`/api/v1/forms/${form.id}/records`, {
		method: 'POST',
		body: { values: { dish: '米饭', qty: '2', status: 'draft' }, source: { type: 'phase9-smoke' } }
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
	const port = 21600 + Math.floor(Math.random() * 300);
	const profile = `/tmp/openpr-phase9-${process.pid}`;
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
	const text = await evalPage('document.body ? document.body.innerText.slice(0, 1400) : ""');
	throw new Error(`timed out waiting for ${label}. DOM text: ${text}`);
}

async function selectFixtureForm() {
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
	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(formKey)})`, 'fixture form selected');
}

async function runBrowserFlow({ workspaceId, projectId }) {
	await openBrowser(`${frontendUrl}/workspace/${workspaceId}/projects/${projectId}/forms`);
	await selectFixtureForm();
	await waitForPage(`document.body?.innerText.includes('保存视图') && document.body?.innerText.includes('菜品') && document.body?.innerText.includes('状态')`, 'saved view panel');
	await evalPage(`(() => {
		const panel = Array.from(document.querySelectorAll('section')).find((item) => item.textContent.includes('保存视图') && item.textContent.includes('显示列'));
		if (!panel) throw new Error('saved view panel not found');
		const inputFor = (labelText) => {
			const label = Array.from(panel.querySelectorAll('label')).find((item) => item.textContent.includes(labelText));
			if (!label) throw new Error('label not found: ' + labelText);
			const input = label.querySelector('input');
			if (!input) throw new Error('input not found: ' + labelText);
			return input;
		};
		const keyInput = inputFor('视图标识');
		const nameInput = inputFor('视图名称');
		if (!keyInput || !nameInput) throw new Error('view key/name inputs not found');
		keyInput.value = ${JSON.stringify(viewKey)};
		keyInput.dispatchEvent(new Event('input', { bubbles: true }));
		nameInput.value = ${JSON.stringify(viewName)};
		nameInput.dispatchEvent(new Event('input', { bubbles: true }));
		const checkbox = Array.from(panel.querySelectorAll('input[type="checkbox"]')).find((item) => item.getAttribute('aria-label')?.includes('状态'));
		if (!checkbox) throw new Error('status column checkbox not found');
		if (checkbox.checked) {
			checkbox.click();
			checkbox.dispatchEvent(new Event('change', { bubbles: true }));
		}
		const save = Array.from(panel.querySelectorAll('button')).find((item) => item.textContent.includes('保存视图'));
		if (!save) throw new Error('save view button not found');
		save.click();
		return true;
	})()`);
	await waitForPage(
		`(() => {
			const headers = Array.from(document.querySelectorAll('th')).map((item) => item.textContent.trim());
			return document.body?.innerText.includes(${JSON.stringify(viewName)}) && headers.includes('菜品') && headers.includes('数量') && !headers.includes('状态');
		})()`,
		'saved view applied to grid columns'
	);
	await evalPage('location.reload()');
	await sleep(800);
	await selectFixtureForm();
	await waitForPage(
		`(() => {
			const headers = Array.from(document.querySelectorAll('th')).map((item) => item.textContent.trim());
			return document.body?.innerText.includes(${JSON.stringify(viewName)}) && headers.includes('菜品') && headers.includes('数量') && !headers.includes('状态');
		})()`,
		'saved view persisted after reload'
	);
	await evalPage(`(() => {
		const panel = Array.from(document.querySelectorAll('section')).find((item) => item.textContent.includes('保存视图') && item.textContent.includes(${JSON.stringify(viewName)}));
		if (!panel) throw new Error('saved view panel not found before delete');
		const del = Array.from(panel.querySelectorAll('button')).find((item) => item.textContent.includes('删除视图'));
		if (!del) throw new Error('delete view button not found');
		del.click();
		return true;
	})()`);
	await waitForPage(`!document.body?.innerText.includes(${JSON.stringify(viewName)})`, 'saved view deleted');
}

async function cleanup() {
	if (form) await api(`/api/v1/forms/${form.id}`, { method: 'DELETE' }).catch(() => {});
	if (cdp) cdp.close();
	if (child) child.kill('SIGTERM');
}

try {
	await login();
	const fixture = await createFixture();
	await runBrowserFlow(fixture);
	const views = await api(`/api/v1/forms/${form.id}/views`);
	if (views.some((view) => view.key === viewKey)) throw new Error('saved view still active after delete');
	console.log(
		JSON.stringify(
			{
				ok: true,
				frontend_url: frontendUrl,
				form_id: form.id,
				form_key: form.key,
				verified: [
					'saved_view_builder_visible',
					'saved_view_create',
					'grid_columns_follow_saved_view',
					'saved_view_reload_persistence',
					'saved_view_delete'
				]
			},
			null,
			2
		)
	);
} finally {
	await cleanup();
}

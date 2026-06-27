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

let accessToken = '';
let refreshToken = '';
let user = null;
let cdp = null;
let child = null;
let createdForm = null;

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
	const port = 21300 + Math.floor(Math.random() * 300);
	const profile = `/tmp/openpr-phase8-${process.pid}`;
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
	const text = await evalPage('document.body ? document.body.innerText.slice(0, 1200) : ""');
	throw new Error(`timed out waiting for ${label}. DOM text: ${text}`);
}

async function findNewForm(projectId, beforeIds) {
	for (let index = 0; index < 80; index += 1) {
		const forms = (await api(`/api/v1/projects/${projectId}/forms?per_page=200`)).items ?? [];
		const found = forms.find((item) => !beforeIds.has(item.id) && item.key.startsWith('restaurant_ordering'));
		if (found) return found;
		await sleep(150);
	}
	throw new Error('template-created form was not found through API');
}

async function runBrowserFlow({ workspaceId, projectId, beforeIds }) {
	await openBrowser(`${frontendUrl}/workspace/${workspaceId}/projects/${projectId}/forms`);
	await waitForPage(`document.body?.innerText.includes('万能表单')`, 'forms page');
	await evalPage(`(() => {
		const button = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('新建表单'));
		if (!button) throw new Error('new form button not found');
		button.click();
		return true;
	})()`);
	await waitForPage(
		`document.body?.innerText.includes('模板库') && document.body?.innerText.includes('餐厅点餐') && !document.body?.innerText.includes('餐厅示例')`,
		'template library without hard-coded restaurant sample'
	);
	await evalPage(`(() => {
		const install = Array.from(document.querySelectorAll('button')).find((button) => {
			if (!button.textContent.includes('直接安装')) return false;
			let node = button.parentElement;
			while (node) {
				const heading = node.querySelector('h4');
				if (heading) return heading.textContent.includes('餐厅点餐');
				node = node.parentElement;
			}
			return false;
		});
		if (!install) throw new Error('template install button not found');
		install.click();
		return true;
	})()`);
	createdForm = await findNewForm(projectId, beforeIds);
	await waitForPage(
		`document.body?.innerText.includes(${JSON.stringify(createdForm.key)}) && document.body?.innerText.includes('菜单 SKU')`,
		'installed template form selected'
	);
}

async function cleanup() {
	if (createdForm) await api(`/api/v1/forms/${createdForm.id}`, { method: 'DELETE' }).catch(() => {});
	if (cdp) cdp.close();
	if (child) child.kill('SIGTERM');
}

try {
	await login();
	const workspace = (await api('/api/v1/workspaces')).items[0];
	const project = (await api(`/api/v1/workspaces/${workspace.id}/projects?per_page=100`)).items.find(
		(item) => item.type_key === 'custom_form'
	);
	if (!project) throw new Error('no custom_form project available');
	const beforeForms = (await api(`/api/v1/projects/${project.id}/forms?per_page=200`)).items ?? [];
	const beforeIds = new Set(beforeForms.map((item) => item.id));
	await runBrowserFlow({ workspaceId: workspace.id, projectId: project.id, beforeIds });
	const views = await api(`/api/v1/forms/${createdForm.id}/views`);
	const fieldTypes = (createdForm.schema?.fields ?? []).map((field) => field.type);
	if (!views.some((view) => view.key === 'grid') || !views.some((view) => view.key === 'detail')) {
		throw new Error('installed template form is missing default views');
	}
	if (fieldTypes.includes('select')) throw new Error('template field type select was not normalized');
	if (!fieldTypes.includes('single_select')) throw new Error('template single_select field missing');
	console.log(
		JSON.stringify(
			{
				ok: true,
				frontend_url: frontendUrl,
				form_id: createdForm.id,
				form_key: createdForm.key,
				verified: [
					'template_library_visible',
					'hard_coded_restaurant_sample_removed',
					'from_template_api_install',
					'default_grid_detail_views',
					'template_field_type_normalization'
				]
			},
			null,
			2
		)
	);
} finally {
	await cleanup();
}

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
const now = Date.now().toString(36);
const formKey = `smoke_crud_${now}`;
const formName = `Smoke CRUD ${now}`;

let accessToken = '';
let refreshToken = '';
let user = null;
let formId = '';
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
	if (!res.ok && parsed.code === undefined) {
		throw new Error(`${path} HTTP ${res.status}: ${text}`);
	}
	return parsed;
}

async function login() {
	const result = await api('/api/v1/auth/login', {
		method: 'POST',
		body: { email, password }
	});
	if (result.code !== 0 || !result.data?.tokens?.access_token) {
		throw new Error(`login failed: ${result.message}`);
	}
	accessToken = result.data.tokens.access_token;
	refreshToken = result.data.tokens.refresh_token;
	user = result.data.user;
}

async function createFixtureForm() {
	const workspaces = await api('/api/v1/workspaces');
	const workspace = workspaces.data?.items?.[0];
	if (!workspace?.id) throw new Error('no workspace available');

	const projects = await api(`/api/v1/workspaces/${workspace.id}/projects?per_page=100`);
	const project = projects.data?.items?.find((item) => item.type_key === 'custom_form');
	if (!project?.id) throw new Error('no custom_form project available');

	const created = await api(`/api/v1/projects/${project.id}/forms`, {
		method: 'POST',
		body: {
			key: formKey,
			name: formName,
			description: 'Temporary deployed browser CRUD smoke form.',
			title_template: '{customer_name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'customer_name', label: '客户名称', type: 'text', required: true },
					{
						key: 'order_amount',
						label: '订单金额',
						type: 'amount',
						required: true,
						amount: { currency: 'CNY', scale: 2 }
					},
					{
						key: 'order_status',
						label: '订单状态',
						type: 'single_select',
						required: true,
						options: ['处理中', '已完成']
					},
					{ key: 'handoff_note', label: '备注', type: 'textarea' }
				]
			}
		}
	});
	if (created.code !== 0 || !created.data?.id) {
		throw new Error(`create fixture form failed: ${created.message}`);
	}
	formId = created.data.id;
	return {
		workspaceId: workspace.id,
		projectId: project.id
	};
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
	const port = 20600 + Math.floor(Math.random() * 300);
	const profile = `/tmp/openpr-deployed-crud-${process.pid}`;
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
		throw new Error(result.exceptionDetails.text ?? 'page evaluation failed');
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

async function runCrudFlow({ workspaceId, projectId }) {
	const targetUrl = `${frontendUrl}/workspace/${workspaceId}/projects/${projectId}/forms`;
	await openBrowser(targetUrl);

	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(formName)})`, 'fixture form');
	await evalPage(`
		(() => {
			const switcher = document.querySelector('#form-switcher');
			if (!switcher) throw new Error('form switcher not found');
			switcher.value = ${JSON.stringify(formId)};
			switcher.dispatchEvent(new Event('change', { bubbles: true }));
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(formKey)})`, 'selected fixture form');

	const helper = `
		const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
		const text = () => document.body?.innerText ?? '';
		const button = (label) => Array.from(document.querySelectorAll('button')).find((item) => item.textContent.trim().includes(label));
		const labelControl = (labelText) => {
			const label = Array.from(document.querySelectorAll('label')).find((item) => item.textContent.includes(labelText));
			if (!label) throw new Error('label not found: ' + labelText);
			const control = label.querySelector('input, textarea, select');
			if (!control) throw new Error('control not found: ' + labelText);
			return control;
		};
		const setLabel = (labelText, value) => {
			const control = labelControl(labelText);
			control.value = value;
			control.dispatchEvent(new Event(control.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
			control.dispatchEvent(new Event('input', { bubbles: true }));
		};
	`;

	await evalPage(`(async () => { ${helper}
		setLabel('标题覆盖', 'CRUD smoke initial');
		setLabel('客户名称', '张三');
		setLabel('订单金额', '123.45');
		setLabel('订单状态', '处理中');
		setLabel('备注', '初始备注');
		button('添加记录').click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('CRUD smoke initial') && document.body?.innerText.includes('123.45 CNY')`, 'created record in grid');

	await evalPage(`(() => {
		const recordButton = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('CRUD smoke initial'));
		if (!recordButton) throw new Error('created record button not found');
		recordButton.click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('记录关联') && document.body?.innerText.includes('初始备注')`, 'record detail');

	await evalPage(`(() => {
		const edit = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.trim().includes('编辑'));
		if (!edit) throw new Error('detail edit button not found');
		edit.click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('编辑记录')`, 'edit record form');

	await evalPage(`(async () => { ${helper}
		setLabel('标题覆盖', 'CRUD smoke updated');
		setLabel('客户名称', '李四');
		setLabel('订单金额', '234.56');
		setLabel('订单状态', '已完成');
		setLabel('备注', '更新备注');
		button('保存记录').click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('CRUD smoke updated') && document.body?.innerText.includes('234.56 CNY')`, 'updated record in grid');

	await evalPage(`(() => {
		const row = Array.from(document.querySelectorAll('tr')).find((item) => item.textContent.includes('CRUD smoke updated'));
		if (!row) throw new Error('updated record row not found');
		const del = row.querySelector('button[aria-label="删除记录"], button[aria-label="Delete record"]');
		if (!del) throw new Error('delete record button not found');
		del.click();
		return true;
	})()`);
	await waitForPage(`!document.body?.innerText.includes('CRUD smoke updated') && document.body?.innerText.includes('暂无记录')`, 'deleted record removed from grid');
}

async function cleanup() {
	if (formId) {
		const result = await api(`/api/v1/forms/${formId}`, { method: 'DELETE' });
		if (result.code !== 0) {
			console.error(`cleanup failed for ${formId}: ${result.message}`);
		}
	}
	if (cdp) cdp.close();
	if (child) child.kill('SIGTERM');
}

try {
	await login();
	const fixture = await createFixtureForm();
	await runCrudFlow(fixture);
	const records = await api(`/api/v1/forms/${formId}/records?per_page=10`);
	if (records.code !== 0 || (records.data?.items?.length ?? 0) !== 0) {
		throw new Error(`expected no active records after delete, got ${records.data?.items?.length ?? 'unknown'}`);
	}
	console.log(
		JSON.stringify(
			{
				ok: true,
				frontend_url: frontendUrl,
				form_id: formId,
				form_key: formKey,
				verified: ['create', 'detail', 'edit', 'delete', 'frontend_api_proxy']
			},
			null,
			2
		)
	);
} finally {
	await cleanup();
}

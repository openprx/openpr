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
const memberEmail = `phase9_permissions_${suffix}@openpr.local`;
const memberPassword = `OpenPRPerms${suffix}!`;
const formKey = `perms_${suffix}`;
const formName = `Permissions Smoke ${suffix}`;

let accessToken = '';
let refreshToken = '';
let user = null;
let adminToken = '';
let adminRefreshToken = '';
let adminUser = null;
let memberToken = '';
let form = null;
let cdp = null;
let child = null;

const allActions = {
	'form.view': true,
	'form.design': true,
	'record.create': true,
	'record.update': true,
	'record.delete': true,
	'record.export': true
};

function sleep(ms) {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

async function rawApi(path, options = {}) {
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
	return { status: res.status, parsed };
}

async function api(path, options = {}) {
	const { parsed } = await rawApi(path, options);
	if (parsed.code !== 0) throw new Error(`${path}: ${parsed.message}`);
	return parsed.data;
}

async function loginAs(loginEmail, loginPassword) {
	const result = await api('/api/v1/auth/login', {
		method: 'POST',
		body: { email: loginEmail, password: loginPassword }
	});
	return {
		accessToken: result.tokens.access_token,
		refreshToken: result.tokens.refresh_token,
		user: result.user
	};
}

async function setupFixture() {
	const admin = await loginAs(email, password);
	adminToken = admin.accessToken;
	adminRefreshToken = admin.refreshToken;
	adminUser = admin.user;
	accessToken = adminToken;
	refreshToken = adminRefreshToken;
	user = adminUser;

	const workspace = (await api('/api/v1/workspaces')).items[0];
	const project = (await api(`/api/v1/workspaces/${workspace.id}/projects?per_page=100`)).items.find(
		(item) => item.type_key === 'custom_form'
	);
	if (!project) throw new Error('no custom_form project available');

	const member = await api('/api/v1/auth/register', {
		method: 'POST',
		body: { email: memberEmail, password: memberPassword, name: `Phase9 Permissions ${suffix}` }
	});
	await api(`/api/v1/workspaces/${workspace.id}/members`, {
		method: 'POST',
		body: { user_id: member.user.id, role: 'member' }
	});

	form = await api(`/api/v1/projects/${project.id}/forms`, {
		method: 'POST',
		body: {
			key: formKey,
			name: formName,
			title_template: '{name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [{ key: 'name', label: '名称', type: 'text', required: true }]
			}
		}
	});

	const memberLogin = await loginAs(memberEmail, memberPassword);
	memberToken = memberLogin.accessToken;
	return { workspaceId: workspace.id, projectId: project.id };
}

async function configureMemberActions(actions) {
	accessToken = adminToken;
	return api(`/api/v1/forms/${form.id}/permissions`, {
		method: 'PATCH',
		body: {
			policies: [
				{
					subject_type: 'role',
					subject_id: 'member',
					policy: { actions }
				}
			]
		}
	});
}

async function expectDenied(label, path, options = {}) {
	const { parsed } = await rawApi(path, options);
	if (parsed.code === 0) throw new Error(`${label} should be denied`);
	if (!String(parsed.message ?? '').includes('permission denied')) {
		throw new Error(`${label} denial did not mention permission: ${JSON.stringify(parsed)}`);
	}
}

async function assertApiPermissions({ projectId }) {
	await configureMemberActions({
		...allActions,
		'form.view': false,
		'form.design': false,
		'record.create': false,
		'record.export': false
	});

	accessToken = memberToken;
	const hiddenList = await api(`/api/v1/projects/${projectId}/forms?per_page=100`);
	if (hiddenList.items.some((item) => item.id === form.id)) {
		throw new Error('member list should hide form when form.view is false');
	}
	await expectDenied('member view', `/api/v1/forms/${form.id}`);

	await configureMemberActions({
		...allActions,
		'form.design': false,
		'record.create': false,
		'record.export': false
	});

	accessToken = memberToken;
	const visibleList = await api(`/api/v1/projects/${projectId}/forms?per_page=100`);
	if (!visibleList.items.some((item) => item.id === form.id)) {
		throw new Error('member list should include form when form.view is true');
	}
	const permissions = await api(`/api/v1/forms/${form.id}/permissions`);
	if (permissions.effective.actions['form.view'] !== true) throw new Error('member should retain form.view');
	if (permissions.effective.actions['record.create'] !== false) throw new Error('member create should be false');
	if (permissions.effective.actions['record.export'] !== false) throw new Error('member export should be false');
	if (permissions.effective.actions['form.design'] !== false) throw new Error('member design should be false');

	await api(`/api/v1/forms/${form.id}`);
	await expectDenied('member create', `/api/v1/forms/${form.id}/records`, {
		method: 'POST',
		body: { values: { name: 'denied' }, source: { type: 'phase9-permissions-smoke' } }
	});
	await expectDenied('member export', `/api/v1/forms/${form.id}/records/export`);
	await expectDenied('member design', `/api/v1/forms/${form.id}`, {
		method: 'PATCH',
		body: { description: 'member should not edit design' }
	});

	await configureMemberActions(allActions);
	accessToken = memberToken;
	const created = await api(`/api/v1/forms/${form.id}/records`, {
		method: 'POST',
		body: { values: { name: 'allowed after restore' }, source: { type: 'phase9-permissions-smoke' } }
	});
	if (created.values.name !== 'allowed after restore') {
		throw new Error(`member create after restore failed: ${JSON.stringify(created)}`);
	}
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
	const port = 22500 + Math.floor(Math.random() * 300);
	const profile = `/tmp/openpr-phase9-permissions-${process.pid}`;
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
			localStorage.setItem('auth_token', ${JSON.stringify(adminToken)});
			localStorage.setItem('refresh_token', ${JSON.stringify(adminRefreshToken)});
			localStorage.setItem('auth_user', ${JSON.stringify(JSON.stringify(adminUser))});
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
	accessToken = adminToken;
	await configureMemberActions({
		...allActions,
		'record.create': false,
		'record.export': false
	});
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
	await waitForPage(`document.body?.innerText.includes('自动化')`, 'automation tab');
	await evalPage(`
		(() => {
			const button = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('自动化'));
			if (!button) throw new Error('automation tab button not found');
			button.click();
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes('权限')`, 'permissions panel');
	await waitForPage(`document.body?.innerText.includes('新增记录')`, 'permission action labels');
	await evalPage(`
		(() => {
			const button = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('保存权限'));
			if (!button) throw new Error('save permissions button not found');
			button.click();
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes('权限已保存')`, 'permissions saved toast');
}

async function cleanup() {
	if (cdp) cdp.close();
	if (child) child.kill('SIGKILL');
	accessToken = adminToken;
	if (form) {
		try {
			await configureMemberActions(allActions);
		} catch {
			// Best-effort cleanup.
		}
		try {
			await api(`/api/v1/forms/${form.id}`, { method: 'DELETE' });
		} catch {
			// Best-effort cleanup.
		}
	}
}

try {
	const fixture = await setupFixture();
	await assertApiPermissions(fixture);
	await runBrowserFlow(fixture);
	await cleanup();
	console.log('Universal forms deployed permissions smoke passed');
} catch (error) {
	await cleanup();
	console.error(error);
	process.exit(1);
}

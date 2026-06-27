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
const parentKey = `parent_${suffix}`;
const childKey = `child_${suffix}`;
const parentName = `Parent Relation ${suffix}`;
const childName = `Child Relation ${suffix}`;

let accessToken = '';
let refreshToken = '';
let user = null;
let parentForm = null;
let childForm = null;
let childRecord = null;
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

	childForm = await api(`/api/v1/projects/${project.id}/forms`, {
		method: 'POST',
		body: {
			key: childKey,
			name: childName,
			title_template: '{line_name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'line_name', label: '子项名称', type: 'text', required: true },
					{ key: 'qty', label: '数量', type: 'integer', required: true }
				]
			}
		}
	});
	childRecord = await api(`/api/v1/forms/${childForm.id}/records`, {
		method: 'POST',
		body: { values: { line_name: '牛肉面', qty: '2' }, source: { type: 'phase7-smoke' } }
	});
	parentForm = await api(`/api/v1/projects/${project.id}/forms`, {
		method: 'POST',
		body: {
			key: parentKey,
			name: parentName,
			title_template: '{customer}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'customer', label: '客户', type: 'text', required: true },
						{
							key: 'line_ref',
							label: '子项关联',
							type: 'relation',
						required: true,
						relation: {
							target_type: 'form_record',
							form_key: childKey,
							relation_key: 'lines',
							relation_type: 'parent_child',
								display_field: 'line_name'
							}
						},
						{
							key: 'total_qty',
							label: '合计数量',
							type: 'number',
							formula: { op: 'child_sum', relation_key: 'lines', field: 'qty' }
						}
					]
				}
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
	const port = 21000 + Math.floor(Math.random() * 300);
	const profile = `/tmp/openpr-phase7-${process.pid}`;
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
	if (result.exceptionDetails) throw new Error(result.exceptionDetails.text ?? 'page evaluation failed');
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

async function runBrowserFlow({ workspaceId, projectId }) {
	await openBrowser(`${frontendUrl}/workspace/${workspaceId}/projects/${projectId}/forms`);
	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(parentName)})`, 'parent form');
	await evalPage(`
		(() => {
			const switcher = document.querySelector('#form-switcher');
			if (!switcher) throw new Error('form switcher not found');
			switcher.value = ${JSON.stringify(parentForm.id)};
			switcher.dispatchEvent(new Event('change', { bubbles: true }));
			return true;
		})()
	`);
	await waitForPage(`document.body?.innerText.includes(${JSON.stringify(parentKey)})`, 'selected parent form');
	await waitForPage(
		`Array.from(document.querySelectorAll('option')).some((item) => item.value === ${JSON.stringify(childRecord.id)} && item.textContent.includes('牛肉面'))`,
		'relation target option'
	);

	const helper = `
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
		const byPlaceholder = (placeholder, value) => {
			const control = Array.from(document.querySelectorAll('input, textarea')).find((item) => item.placeholder && item.placeholder.includes(placeholder));
			if (!control) throw new Error('placeholder not found: ' + placeholder);
			control.value = value;
			control.dispatchEvent(new Event('input', { bubbles: true }));
			control.dispatchEvent(new Event('change', { bubbles: true }));
		};
	`;
	await evalPage(`(() => { ${helper}
		setLabel('标题覆盖', 'Phase7 parent');
		setLabel('客户', '一号桌');
		setLabel('子项关联', ${JSON.stringify(childRecord.id)});
		button('添加记录').click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('一号桌') && document.body?.innerText.includes('牛肉面')`, 'created parent with relation display');

	await evalPage(`(() => {
		const recordButton = Array.from(document.querySelectorAll('button')).find((item) => item.textContent.includes('一号桌'));
		if (!recordButton) throw new Error('parent record button not found');
		recordButton.click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('记录关联') && document.body?.innerText.includes('子记录')`, 'parent detail with child section');
	await waitForPage(
		`(() => {
			const label = Array.from(document.querySelectorAll('p')).find((item) => item.textContent.trim() === '合计数量');
			const value = label?.parentElement?.querySelector('p.mt-1')?.textContent.trim();
			return document.body?.innerText.includes('lines / parent_child') && document.body?.innerText.includes('牛肉面') && document.body?.innerText.includes('数量: 2') && value === '2';
		})()`,
		'child table after automatic parent-child link'
	);

	await evalPage(`(() => { ${helper}
		button('新增子记录').click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('新增子记录') && document.body?.innerText.includes('子项名称')`, 'inline child add editor');
	await evalPage(`(() => { ${helper}
		setLabel('标题覆盖', 'Phase7 inline child');
		setLabel('子项名称', '米饭');
		setLabel('数量', '3');
		button('保存记录').click();
		return true;
	})()`);
	await waitForPage(
		`(() => {
			const label = Array.from(document.querySelectorAll('p')).find((item) => item.textContent.trim() === '合计数量');
			const value = label?.parentElement?.querySelector('p.mt-1')?.textContent.trim();
			return document.body?.innerText.includes('米饭') && document.body?.innerText.includes('数量: 3') && value === '5';
		})()`,
		'inline child create and parent child_sum refresh'
	);

	await evalPage(`(() => {
		const row = Array.from(document.querySelectorAll('tr')).find((item) => item.textContent.includes('米饭'));
		if (!row) throw new Error('inline child row not found');
		const edit = row.querySelector('button[aria-label="编辑子记录"]');
		if (!edit) throw new Error('inline child edit action not found');
		edit.click();
		return true;
	})()`);
	await waitForPage(`document.body?.innerText.includes('编辑子记录')`, 'inline child edit editor');
	await evalPage(`(() => { ${helper}
		setLabel('数量', '4');
		button('保存记录').click();
		return true;
	})()`);
	await waitForPage(
		`(() => {
			const label = Array.from(document.querySelectorAll('p')).find((item) => item.textContent.trim() === '合计数量');
			const value = label?.parentElement?.querySelector('p.mt-1')?.textContent.trim();
			return document.body?.innerText.includes('米饭') && document.body?.innerText.includes('数量: 4') && value === '6';
		})()`,
		'inline child update and parent child_sum refresh'
	);

	await evalPage(`(() => {
		const row = Array.from(document.querySelectorAll('tr')).find((item) => item.textContent.includes('米饭'));
		if (!row) throw new Error('inline child row missing before delete');
		const del = row.querySelector('button[aria-label="删除子记录"]');
		if (!del) throw new Error('inline child delete action not found');
		del.click();
		return true;
	})()`);
	await waitForPage(
		`(() => {
			const label = Array.from(document.querySelectorAll('p')).find((item) => item.textContent.trim() === '合计数量');
			const value = label?.parentElement?.querySelector('p.mt-1')?.textContent.trim();
			return !document.body?.innerText.includes('米饭') && document.body?.innerText.includes('牛肉面') && value === '2';
		})()`,
		'inline child delete and parent child_sum refresh'
	);
}

async function cleanup() {
	if (parentForm) await api(`/api/v1/forms/${parentForm.id}`, { method: 'DELETE' }).catch(() => {});
	if (childForm) await api(`/api/v1/forms/${childForm.id}`, { method: 'DELETE' }).catch(() => {});
	if (cdp) cdp.close();
	if (child) child.kill('SIGTERM');
}

try {
	await login();
	const fixture = await createFixture();
	await runBrowserFlow(fixture);
	const targets = await api(`/api/v1/forms/${parentForm.id}/relation-targets?field_key=line_ref`);
	if ((targets.items?.length ?? 0) < 1) throw new Error('relation targets missing after browser flow');
	console.log(
		JSON.stringify(
			{
				ok: true,
				frontend_url: frontendUrl,
				parent_form_id: parentForm.id,
				child_form_id: childForm.id,
				child_record_id: childRecord.id,
				verified: [
					'relation_picker',
					'relation_targets_api',
					'automatic_parent_child_link',
					'children_api',
					'child_table',
					'inline_child_create',
					'inline_child_update',
					'inline_child_delete',
					'parent_child_sum_refresh'
				]
			},
			null,
			2
		)
	);
} finally {
	await cleanup();
}

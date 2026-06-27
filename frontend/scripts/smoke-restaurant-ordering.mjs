#!/usr/bin/env node
import { createServer } from 'node:http';
import { mkdir, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const screenshotDir = process.env.OPENPR_RESTAURANT_SCREENSHOT_DIR || '';
const workspaceId = 'ws-restaurant-smoke';
const projectId = 'project-restaurant-smoke';
const now = '2026-05-31T12:00:00.000Z';

let sequence = 1;
let lastOrderUpdatePayload = null;
let orderLinePayload = null;
let childLinkPayload = null;
const recordsByForm = new Map();
const linksByRecord = new Map();

const project = {
	id: projectId,
	workspace_id: workspaceId,
	name: 'Restaurant Ordering',
	key: 'REST',
	description: 'Restaurant universal form smoke',
	type_key: 'restaurant_ordering',
	type_settings: {},
	created_at: now,
	updated_at: now,
	issue_counts: null
};

const forms = [
	form('form-menu-category', 'menu_category', 'Menu Category', '{name}', [
		field('name', 'Name', 'text', true),
		field('sort_order', 'Sort Order', 'integer')
	]),
	form('form-sku', 'sku', 'SKU', '{name}', [
		field('name', 'Name', 'text', true),
		field('category_id', 'Category', 'relation', true),
		field('price', 'Price', 'amount', true),
		field('available', 'Available', 'boolean')
	]),
	form('form-table', 'table', 'Table', '{table_no}', [
		field('table_no', 'Table No', 'text', true),
		field('seat_count', 'Seat Count', 'integer'),
		field('status', 'Status', 'single_select', false, ['available', 'occupied', 'cleaning'])
	]),
	form('form-order', 'order', 'Order', '{order_no}', [
		field('order_no', 'Order No', 'text', true),
		field('table_id', 'Table', 'relation', true),
		field('status', 'Status', 'single_select', true, [
			'draft',
			'sent_to_kitchen',
			'served',
			'paid'
		]),
		field('total_amount', 'Total Amount', 'amount'),
		field('opened_at', 'Opened At', 'datetime')
	]),
	form('form-order-line', 'order_line', 'Order Line', '{sku_name}', [
		field('order_id', 'Order', 'relation', true),
		field('sku_id', 'SKU', 'relation', true),
		field('sku_name', 'SKU Name', 'text', true),
		field('quantity', 'Quantity', 'integer', true),
		field('unit_price', 'Unit Price', 'amount', true),
		field('line_total', 'Line Total', 'amount'),
		field('status', 'Status', 'single_select', false, ['draft', 'sent_to_kitchen', 'served'])
	]),
	form('form-print-job', 'print_job', 'Print Job', '{job_type}', [
		field('order_id', 'Order', 'relation', true),
		field('job_type', 'Job Type', 'single_select', true, ['kitchen', 'receipt']),
		field('status', 'Status', 'single_select', true, ['pending', 'printing', 'printed', 'failed']),
		field('printer', 'Printer', 'text'),
		field('payload', 'Payload', 'textarea'),
		field('retry_count', 'Retry Count', 'integer')
	]),
	form('form-business-report', 'business_report', 'Business Report', '{report_date}', [
		field('report_date', 'Report Date', 'date', true),
		field('gross_revenue', 'Gross Revenue', 'amount', true),
		field('orders_count', 'Orders Count', 'integer')
	])
];

function field(key, label, type, required = false, options = []) {
	const item = { key, label, type, required };
	if (options.length > 0) item.options = options;
	if (type === 'amount') item.amount = { currency: 'CNY', scale: 2 };
	return item;
}

function form(id, key, name, titleTemplate, fields) {
	recordsByForm.set(id, []);
	return {
		id,
		workspace_id: workspaceId,
		project_id: projectId,
		key,
		name,
		description: `${name} form`,
		icon: null,
		color: null,
		title_template: titleTemplate,
		schema: { version: 'openpr.form.schema.v1', fields },
		detail_layout: {},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	};
}

function apiResult(data) {
	return { code: 0, message: 'success', data };
}

function paginated(items) {
	return { items, total: items.length, page: 1, per_page: items.length || 100, total_pages: 1 };
}

function json(res, status, data) {
	res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });
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

async function readBody(req) {
	const chunks = [];
	for await (const chunk of req) chunks.push(chunk);
	const text = Buffer.concat(chunks).toString('utf8');
	return text ? JSON.parse(text) : {};
}

async function serveStatic(req, res, smokeMode = null) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	let pathname = decodeURIComponent(url.pathname);
	if (pathname === '/') pathname = '/index.html';
	let filePath = normalize(join(buildDir, pathname));
	if (!filePath.startsWith(buildDir) || !existsSync(filePath)) {
		filePath = join(buildDir, 'index.html');
	}

	let data = await readFile(filePath, 'utf8');
	if (smokeMode && basename(filePath) === 'index.html') {
		data = data.replace('</body>', `${injectedSmoke(smokeMode)}</body>`);
	}
	res.writeHead(200, { 'Content-Type': contentType(filePath) });
	res.end(data);
}

function injectedSmoke(mode) {
	return `<script>
const mode = ${JSON.stringify(mode)};
const formIds = ${JSON.stringify(Object.fromEntries(forms.map((item) => [item.key, item.id])))};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function mark(name, value = 'done') {
  document.body.setAttribute(name, value);
  sessionStorage.setItem('restaurant-smoke:' + name, value);
}
function restoreMarks() {
  for (let index = 0; index < sessionStorage.length; index += 1) {
    const key = sessionStorage.key(index);
    if (key && key.startsWith('restaurant-smoke:data-')) {
      document.body.setAttribute(key.replace('restaurant-smoke:', ''), sessionStorage.getItem(key) || 'done');
    }
  }
}
async function waitFor(check, label) {
  for (let i = 0; i < 160; i += 1) {
    restoreMarks();
    const value = check();
    if (value) return value;
    await sleep(120);
  }
  throw new Error('Timed out waiting for ' + label);
}
const textInElements = (text, selector = 'a,button,h1,h2,h3,p,span,td,div') => Array.from(document.querySelectorAll(selector)).some((el) => el.textContent.includes(text));
const buttonExact = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.trim() === text);
function labelControl(labelText) {
  const labels = Array.from(document.querySelectorAll('label'));
  const label = labels.find((item) => item.textContent.trim().startsWith(labelText));
  if (!label) throw new Error('label not found: ' + labelText);
  const control = label.querySelector('input, textarea, select');
  if (!control) throw new Error('control not found for label: ' + labelText);
  return control;
}
function setByLabel(labelText, value) {
  const control = labelControl(labelText);
  if (control.type === 'checkbox') {
    control.checked = Boolean(value);
    control.dispatchEvent(new Event('change', { bubbles: true }));
    return;
  }
  control.value = value;
  control.dispatchEvent(new Event(control.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function setByPlaceholder(placeholder, value) {
  const control = Array.from(document.querySelectorAll('input, textarea')).find((item) => item.placeholder && item.placeholder.includes(placeholder));
  if (!control) throw new Error('placeholder not found: ' + placeholder);
  control.value = value;
  control.dispatchEvent(new Event('input', { bubbles: true }));
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
async function selectForm(key) {
  const select = document.querySelector('#form-switcher');
  if (!select) throw new Error('form switcher not found');
  select.value = formIds[key];
  select.dispatchEvent(new Event('change', { bubbles: true }));
  await waitFor(() => select.value === formIds[key] && textInElements(formIds[key].replace('form-', '').replaceAll('-', '_')), 'selected ' + key);
  await sleep(100);
}
async function addRecord(title, values) {
  setByLabel('Title override', title);
  for (const [label, value] of Object.entries(values)) setByLabel(label, value);
  const button = buttonExact('Add record');
  if (!button) throw new Error('Add record button not found');
  button.click();
  await waitFor(() => textInElements(title), 'created ' + title);
}
(async () => {
  for (const key of Object.keys(sessionStorage)) {
    if (key.startsWith('restaurant-smoke:')) sessionStorage.removeItem(key);
  }
  await waitFor(() => textInElements('Forms') && textInElements('Menu Category'), 'forms page');

  if (mode === 'mobile') {
    await waitFor(() => document.body.scrollHeight > 0, 'mobile layout ready');
    const overflow = document.documentElement.scrollWidth - window.innerWidth;
    if (overflow > 2) throw new Error('mobile horizontal overflow: ' + overflow);
    mark('data-restaurant-mobile-smoke');
    return;
  }

  await selectForm('menu_category');
  await addRecord('Noodles', { 'Name': 'Noodles', 'Sort Order': '1' });
  mark('data-menu-category-created');

  await selectForm('sku');
  await addRecord('Beef Noodles', {
    'Name': 'Beef Noodles',
    'Category': '{"record_id":"record-menu-category-1"}',
    'Price': '9.99',
    'Available': true
  });
  mark('data-sku-created');

  await selectForm('table');
  await addRecord('A03', { 'Table No': 'A03', 'Seat Count': '4', 'Status': 'occupied' });
  await addRecord('B02', { 'Table No': 'B02', 'Seat Count': '4', 'Status': 'available' });
  mark('data-table-created');

  await selectForm('order');
  await addRecord('ORD-1001', {
    'Order No': 'ORD-1001',
    'Table': '{"record_id":"record-table-a03"}',
    'Status': 'draft',
    'Total Amount': '0.00',
    'Opened At': '2026-05-31T12:00'
  });
  const editButton = buttonExact('Edit');
  if (!editButton) throw new Error('Edit button not found');
  editButton.click();
  await waitFor(() => textInElements('Edit record') && buttonExact('Save record'), 'edit order mode');
  setByLabel('Table', '{"record_id":"record-table-b02"}');
  setByLabel('Status', 'sent_to_kitchen');
  buttonExact('Save record').click();
  await waitFor(() => textInElements('record-table-b02') && textInElements('sent_to_kitchen'), 'changed table');
  mark('data-order-table-changed');

  await selectForm('order_line');
  await addRecord('Beef Noodles x2', {
    'Order': '{"record_id":"record-order-1"}',
    'SKU': '{"record_id":"record-sku-1"}',
    'SKU Name': 'Beef Noodles',
    'Quantity': '2',
    'Unit Price': '9.99',
    'Status': 'sent_to_kitchen'
  });
  await waitFor(() => textInElements('19.98 CNY'), 'formula line total');
  setByPlaceholder('Target record id', 'record-order-1');
  setByPlaceholder('relation_key', 'order');
  setByPlaceholder('relation_type', 'parent_child');
  await waitFor(() => buttonExact('Add link') && !buttonExact('Add link').disabled, 'add link enabled');
  buttonExact('Add link').click();
  await waitFor(() => textInElements('order / parent_child'), 'order line linked');
  mark('data-order-line-linked');

  await selectForm('print_job');
  await addRecord('Kitchen ticket', {
    'Order': '{"record_id":"record-order-1"}',
    'Job Type': 'kitchen',
    'Status': 'printed',
    'Printer': 'kitchen-01',
    'Payload': 'Beef Noodles x2',
    'Retry Count': '1'
  });
  await addRecord('Receipt', {
    'Order': '{"record_id":"record-order-1"}',
    'Job Type': 'receipt',
    'Status': 'printed',
    'Printer': 'cashier-01',
    'Payload': 'Total 19.98',
    'Retry Count': '0'
  });
  await waitFor(() => textInElements('Print jobs') && textInElements('kitchen-01') && textInElements('cashier-01'), 'print jobs visible');
  mark('data-print-jobs-created');

  await selectForm('business_report');
  await addRecord('2026-05-31 report', {
    'Report Date': '2026-05-31',
    'Gross Revenue': '19.98',
    'Orders Count': '1'
  });
  await waitFor(() => textInElements('19.98 CNY') && textInElements('2026-05-31 report'), 'business report aggregate');
  mark('data-business-report-created');
  document.body.setAttribute('data-restaurant-ordering-smoke', 'done');
})().catch((error) => {
  document.body.setAttribute('data-restaurant-ordering-smoke', 'failed');
  document.body.setAttribute('data-restaurant-ordering-smoke-error', error.message);
  document.body.insertAdjacentHTML('beforeend', '<pre id="restaurant-smoke-error">' + error.message + '</pre>');
});
</script>`;
}

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const pathname = url.pathname;

	if (pathname === '/smoke-auth-seed') {
		const mode = url.searchParams.get('mode') ?? 'desktop';
		res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
		res.end(`<script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
location.replace('/workspace/${workspaceId}/projects/${projectId}/forms?restaurant_ordering_smoke=${mode}');
</script>`);
		return;
	}

	if (pathname === '/api/v1/auth/me') {
		json(
			res,
			200,
			apiResult({
				user: { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' }
			})
		);
		return;
	}
	if (pathname === '/api/v1/notifications/unread-count') {
		json(res, 200, apiResult({ count: 0 }));
		return;
	}
	if (pathname === '/api/v1/workspaces') {
		json(
			res,
			200,
			apiResult(
				paginated([
					{
						id: workspaceId,
						slug: 'restaurant',
						name: 'Restaurant',
						created_at: now,
						updated_at: now
					}
				])
			)
		);
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/members`) {
		json(
			res,
			200,
			apiResult(
				paginated([
					{ user_id: 'user-smoke', workspace_id: workspaceId, role: 'owner', joined_at: now }
				])
			)
		);
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/projects`) {
		json(res, 200, apiResult(paginated([project])));
		return;
	}
	if (pathname === `/api/v1/projects/${projectId}`) {
		json(res, 200, apiResult(project));
		return;
	}
	if (pathname === `/api/v1/projects/${projectId}/forms`) {
		json(res, 200, apiResult(paginated(forms)));
		return;
	}

	const recordsMatch = pathname.match(/^\/api\/v1\/forms\/([^/]+)\/records$/);
	if (recordsMatch && req.method === 'GET') {
		json(res, 200, apiResult(paginated(recordsByForm.get(recordsMatch[1]) ?? [])));
		return;
	}
	if (recordsMatch && req.method === 'POST') {
		const formId = recordsMatch[1];
		const formDef = forms.find((item) => item.id === formId);
		if (!formDef) {
			json(res, 404, { code: 404, message: 'form not found', data: null });
			return;
		}
		const body = await readBody(req);
		if (formDef.key === 'order_line') orderLinePayload = body;
		const record = makeRecord(formDef, body);
		const list = recordsByForm.get(formId) ?? [];
		recordsByForm.set(formId, [record, ...list]);
		json(res, 200, apiResult(record));
		return;
	}

	const aggregateMatch = pathname.match(/^\/api\/v1\/forms\/([^/]+)\/aggregate$/);
	if (aggregateMatch) {
		json(
			res,
			200,
			apiResult(aggregateFor(aggregateMatch[1], url.searchParams.get('field_key') ?? ''))
		);
		return;
	}

	const updateMatch = pathname.match(/^\/api\/v1\/form-records\/([^/]+)$/);
	if (updateMatch && req.method === 'PATCH') {
		const recordId = updateMatch[1];
		const body = await readBody(req);
		const updated = updateRecord(recordId, body);
		if (!updated) {
			json(res, 404, { code: 404, message: 'record not found', data: null });
			return;
		}
		if (recordId === 'record-order-1') lastOrderUpdatePayload = body;
		json(res, 200, apiResult(updated));
		return;
	}

	const linkMatch = pathname.match(/^\/api\/v1\/form-records\/([^/]+)\/links$/);
	if (linkMatch && req.method === 'GET') {
		json(res, 200, apiResult(linksByRecord.get(linkMatch[1]) ?? []));
		return;
	}
	if (linkMatch && req.method === 'POST') {
		const sourceRecordId = linkMatch[1];
		const body = await readBody(req);
		childLinkPayload = body;
		const link = {
			id: `link-${sequence++}`,
			workspace_id: workspaceId,
			project_id: projectId,
			source_record_id: sourceRecordId,
			target_type: body.target_type,
			target_id: body.target_id,
			relation_key: body.relation_key,
			relation_type: body.relation_type,
			metadata: body.metadata ?? {},
			created_by: 'user-smoke',
			created_at: now
		};
		linksByRecord.set(sourceRecordId, [link, ...(linksByRecord.get(sourceRecordId) ?? [])]);
		json(res, 200, apiResult(link));
		return;
	}

	await serveStatic(req, res, url.searchParams.get('restaurant_ordering_smoke'));
}

function makeRecord(formDef, body) {
	const suffix = recordSuffix(formDef, body);
	const values = normalizeValues(formDef, body.values ?? {});
	if (formDef.key === 'order_line') {
		values.line_total = { type: 'amount', decimal: '19.98', currency: 'CNY', scale: 2 };
	}
	return {
		id: `record-${formDef.key.replaceAll('_', '-')}-${suffix}`,
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: formDef.id,
		title: body.title || `${formDef.name} ${sequence}`,
		values,
		source: body.source ?? { type: 'web' },
		created_by: 'user-smoke',
		updated_by: 'user-smoke',
		created_at: now,
		updated_at: now
	};
}

function recordSuffix(formDef, body) {
	if (formDef.key === 'menu_category') return '1';
	if (formDef.key === 'sku') return '1';
	if (formDef.key === 'table') return body.title === 'A03' ? 'a03' : 'b02';
	if (formDef.key === 'order') return '1';
	if (formDef.key === 'order_line') return '1';
	if (formDef.key === 'print_job')
		return body.values?.job_type === 'kitchen' ? 'kitchen' : 'receipt';
	if (formDef.key === 'business_report') return '1';
	return String(sequence++);
}

function normalizeValues(formDef, rawValues) {
	const normalized = {};
	for (const item of formDef.schema.fields) {
		const value = rawValues[item.key];
		if (value === undefined || value === '') continue;
		if (item.type === 'amount') {
			normalized[item.key] = { type: 'amount', decimal: String(value), currency: 'CNY', scale: 2 };
		} else if (item.type === 'integer') {
			normalized[item.key] = { type: 'integer', value: Number(value) };
		} else {
			normalized[item.key] = value;
		}
	}
	return normalized;
}

function updateRecord(recordId, body) {
	for (const [formId, list] of recordsByForm.entries()) {
		const index = list.findIndex((item) => item.id === recordId);
		if (index === -1) continue;
		const formDef = forms.find((item) => item.id === formId);
		const updated = {
			...list[index],
			title: body.title ?? list[index].title,
			values: { ...list[index].values, ...normalizeValues(formDef, body.values ?? {}) },
			source: body.source ?? list[index].source,
			updated_at: now
		};
		const next = [...list];
		next[index] = updated;
		recordsByForm.set(formId, next);
		return updated;
	}
	return null;
}

function aggregateFor(formId, fieldKey) {
	const formDef = forms.find((item) => item.id === formId);
	const fieldDef = formDef?.schema.fields.find((item) => item.key === fieldKey);
	const values = (recordsByForm.get(formId) ?? [])
		.map((record) => record.values[fieldKey])
		.filter(Boolean);
	let total = 0;
	for (const value of values) {
		if (value && typeof value === 'object' && typeof value.decimal === 'string') {
			total += Number(value.decimal);
		} else if (value && typeof value === 'object' && typeof value.value === 'number') {
			total += value.value;
		}
	}
	return {
		form_id: formId,
		field_key: fieldKey,
		field_type: fieldDef?.type ?? 'amount',
		aggregate: 'sum',
		decimal: values.length > 0 ? total.toFixed(fieldDef?.type === 'integer' ? 0 : 2) : null,
		count: values.length,
		currency: fieldDef?.type === 'amount' ? 'CNY' : null,
		scale: fieldDef?.type === 'amount' ? 2 : null
	};
}

function runChromium(url, windowSize = '1366,900', screenshotPath = null) {
	return new Promise((resolve, reject) => {
		const args = [
			'--headless=new',
			'--disable-gpu',
			'--no-sandbox',
			'--disable-dev-shm-usage',
			`--window-size=${windowSize}`,
			'--virtual-time-budget=30000',
			'--dump-dom'
		];
		if (screenshotPath) args.push(`--screenshot=${screenshotPath}`);
		args.push(url);

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
	throw new Error(
		`Missing build directory: ${buildDir}. Run bun run build before smoke:restaurant-ordering.`
	);
}
if (!existsSync(chromium)) {
	throw new Error(`Chromium binary not found: ${chromium}`);
}

const server = createServer((req, res) => {
	handler(req, res).catch((error) => {
		json(res, 500, { code: 500, message: error.message, data: null });
	});
});
const port = 20580 + Math.floor(Math.random() * 1000);
await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));

try {
	if (screenshotDir) await mkdir(screenshotDir, { recursive: true });
	const desktopScreenshot = screenshotDir
		? join(screenshotDir, 'restaurant-ordering-desktop.png')
		: null;
	const mobileScreenshot = screenshotDir
		? join(screenshotDir, 'restaurant-ordering-mobile.png')
		: null;
	const desktopDom = await runChromium(
		`http://127.0.0.1:${port}/smoke-auth-seed?mode=desktop`,
		'1366,900',
		desktopScreenshot
	);
	if (!desktopDom.includes('data-restaurant-ordering-smoke="done"')) {
		throw new Error(
			`Restaurant ordering browser smoke failed. DOM excerpt:\n${desktopDom.slice(-5000)}`
		);
	}
	if (!orderLinePayload || orderLinePayload.values.unit_price !== '9.99') {
		throw new Error('Expected order line payload with decimal string unit_price');
	}
	if (
		!lastOrderUpdatePayload ||
		lastOrderUpdatePayload.values.table_id?.record_id !== 'record-table-b02'
	) {
		throw new Error('Expected order table change update payload');
	}
	if (!childLinkPayload || childLinkPayload.relation_type !== 'parent_child') {
		throw new Error('Expected parent_child order line link payload');
	}
	const mobileDom = await runChromium(
		`http://127.0.0.1:${port}/smoke-auth-seed?mode=mobile`,
		'390,844',
		mobileScreenshot
	);
	if (!mobileDom.includes('data-restaurant-mobile-smoke="done"')) {
		throw new Error(
			`Restaurant ordering mobile smoke failed. DOM excerpt:\n${mobileDom.slice(-5000)}`
		);
	}
	console.log('Restaurant ordering browser smoke passed');
	if (screenshotDir) {
		console.log(`Restaurant ordering screenshots: ${desktopScreenshot}, ${mobileScreenshot}`);
	}
} finally {
	server.close();
}

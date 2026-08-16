#!/usr/bin/env node
import { createServer } from 'node:http';
import { mkdir, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const screenshotDir = process.env.OPENPR_FORMS_UI_SCREENSHOT_DIR || '';
const requestedSmokeMode = process.env.OPENPR_FORMS_UI_SMOKE_MODE || 'full';
const workspaceId = 'ws-forms-smoke';
const projectId = 'project-forms-smoke';
const now = '2026-05-31T12:00:00.000Z';
const timelineEventWindowFrom = dateTimeLocal(now);
const timelineEventWindowTo = dateTimeLocal(new Date(Date.parse(now) + 70_000).toISOString());
const timelineEventDayWindowFrom = dateTimeLocalDayBoundary(now, 0, 0);
const timelineEventDayWindowTo = dateTimeLocalDayBoundary(now, 23, 59);

function dateTimeLocal(value) {
	const date = new Date(value);
	const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
	return local.toISOString().slice(0, 16);
}

function dateTimeLocalDayBoundary(value, hour, minute) {
	const date = new Date(value);
	date.setHours(hour, minute, 0, 0);
	return dateTimeLocal(date.toISOString());
}

let createdRecordPayload = null;
let createdLinkPayload = null;
let createdRecord = null;
let updatedRecordPayload = null;
let updatedRecordPayloads = [];
let updatedViewPayload = null;
let updatedViewPayloads = [];
let updatedFormPayloads = [];
let createdLinks = [];
let orderLineRecordListRequests = [];
let orderLineExportRequests = [];
let listedOrderLineViewIds = [];
let viewUpdateCounter = 0;
let createdAttachmentPayloads = [];
let deletedAttachmentIds = [];
let uploadedAttachmentPayloads = [];
let formAttachments = [];
let listedRecordCommentRecordIds = [];
let createdRecordCommentPayloads = [];
let formRecordComments = [
	{
		id: 'comment-record-existing',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: 'form-order-line',
		record_id: 'record-order-line-created',
		author_id: 'user-smoke',
		author_name: 'Smoke Admin',
		author_email: 'smoke@example.com',
		body: 'Kitchen lead reviewed the detail handoff.',
		metadata: { source: 'seed' },
		archived_at: null,
		created_at: now,
		updated_at: now
	}
];

function signedAttachmentUrl(id) {
	return `/api/v1/form-attachments/${id}/download?expires=1893456000&signature=signed-${id}`;
}

function serverAttachmentThumbnailUrl(filename = 'server-counter-ticket.png') {
	return `/api/v1/uploads/thumbnails/thumb-${filename}`;
}
let memberPermissionPolicy = {
	actions: {
		'form.view': true,
		'form.design': true,
		'record.create': true,
		'record.update': true,
		'record.delete': true,
		'record.export': true
	},
	record_scope: 'all',
	fields: {}
};

function applyMemberPermissionSmokePolicy() {
	memberPermissionPolicy = {
		...memberPermissionPolicy,
		record_scope: 'owned',
		fields: {
			...memberPermissionPolicy.fields,
			unit_price: { read: false, write: false },
			quantity: { write: false }
		}
	};
}

const project = {
	id: projectId,
	workspace_id: workspaceId,
	name: 'Restaurant Forms UI',
	key: 'RESTUI',
	description: 'Forms UI browser smoke',
	type_key: 'custom_form',
	type_settings: {},
	created_at: now,
	updated_at: now,
	issue_counts: null
};

const orderLineForm = {
	id: 'form-order-line',
	workspace_id: workspaceId,
	project_id: projectId,
	key: 'order_line',
	name: 'Order Line',
	description: 'Order line item as child table data.',
	icon: null,
	color: null,
	title_template: '{sku_name}',
	schema: {
		version: 'openpr.form.schema.v1',
		fields: [
			{ field_id: 'fld_order_id', key: 'order_id', label: 'Order', type: 'relation', required: true },
			{ field_id: 'fld_sku_id', key: 'sku_id', label: 'SKU', type: 'relation', required: true },
			{ field_id: 'fld_sku_name', key: 'sku_name', label: 'SKU Name', type: 'text', required: true },
			{ field_id: 'fld_quantity', key: 'quantity', label: 'Quantity', type: 'integer', required: true },
			{
				field_id: 'fld_unit_price',
				key: 'unit_price',
				label: 'Unit Price',
				type: 'amount',
				required: true,
				amount: { currency: 'CNY', scale: 2 }
			},
			{
				field_id: 'fld_line_total',
				key: 'line_total',
				label: 'Line Total',
				type: 'amount',
				amount: { currency: 'CNY', scale: 2 }
			},
			{ field_id: 'fld_seat_no', key: 'seat_no', label: 'Seat', type: 'text' },
			{
				field_id: 'fld_status',
				key: 'status',
				label: 'Status',
				type: 'single_select',
				options: ['draft', 'sent_to_kitchen', 'served', 'cancelled'],
				option_colors: {
					draft: '#64748b',
					sent_to_kitchen: '#f59e0b',
					served: '#16a34a',
					cancelled: '#dc2626'
				}
			},
			{ field_id: 'fld_service_date', key: 'service_date', label: 'Service Date', type: 'date' }
		]
	},
	detail_layout: {
		sections: [
			{ key: 'handoff', title: 'Kitchen Handoff', fields: ['sku_name', 'quantity', 'seat_no', 'status'] },
			{ key: 'pricing', title: 'Pricing', fields: ['unit_price', 'line_total'] }
		]
	},
	schema_version: 1,
	archived_at: null,
	created_by: 'user-smoke',
	created_at: now,
	updated_at: now
};

const orderLineViews = [
	{
		id: 'view-order-line-default',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'default_grid',
		name: 'Default Grid',
		view_type: 'grid',
		config: {
			columns: ['order_id', 'sku_name', 'quantity', 'unit_price', 'line_total', 'seat_no', 'status'],
			is_default: false
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-detail',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'detail_review',
		name: 'Detail Review',
		view_type: 'detail',
		config: {
			columns: ['sku_name', 'quantity', 'unit_price', 'line_total', 'seat_no', 'status']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-kanban',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'kitchen_board',
		name: 'Kitchen Board',
		view_type: 'kanban',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'seat_no', 'status'],
			group_by: 'status'
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-pivot',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'seat_status_pivot',
		name: 'Seat Status Pivot',
		view_type: 'pivot',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'seat_no', 'status']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-gantt',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'service_gantt',
		name: 'Service Gantt',
		view_type: 'gantt',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'status', 'service_date', 'seat_no']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-calendar',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'service_calendar',
		name: 'Service Calendar',
		view_type: 'calendar',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'status', 'service_date'],
			date_field: 'service_date'
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-card',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'line_cards',
		name: 'Line Cards',
		view_type: 'card',
		config: {
			columns: ['sku_name', 'quantity', 'unit_price', 'line_total', 'seat_no', 'status'],
			is_default: true
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-timeline',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'update_timeline',
		name: 'Update Timeline',
		view_type: 'timeline',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'status']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-private-other',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'other_private',
		name: 'Other Private',
		view_type: 'grid',
		config: {
			columns: ['sku_name', 'quantity', 'status'],
			visibility: 'private'
		},
		created_by: 'user-other',
		created_at: now,
		updated_at: now
	}
];

const printJobForm = {
	id: 'form-print-job',
	workspace_id: workspaceId,
	project_id: projectId,
	key: 'print_job',
	name: 'Print Job',
	description: 'Kitchen ticket and cashier receipt tasks.',
	icon: null,
	color: null,
	title_template: '{job_type}',
	schema: {
		version: 'openpr.form.schema.v1',
		fields: [
			{ key: 'order_id', label: 'Order', type: 'relation', required: true },
			{
				key: 'job_type',
				label: 'Job Type',
				type: 'single_select',
				required: true,
				options: ['kitchen', 'receipt']
			},
			{
				key: 'status',
				label: 'Status',
				type: 'single_select',
				required: true,
				options: ['pending', 'printing', 'printed', 'failed']
			},
			{ key: 'printer', label: 'Printer', type: 'text' },
			{ key: 'payload', label: 'Payload', type: 'textarea' },
			{ key: 'retry_count', label: 'Retry Count', type: 'integer' }
		]
	},
	detail_layout: {},
	schema_version: 1,
	archived_at: null,
	created_by: 'user-smoke',
	created_at: now,
	updated_at: now
};

const printJobs = [
	{
		id: 'print-job-kitchen',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: printJobForm.id,
		title: 'Kitchen ticket',
		values: {
			order_id: { record_id: 'order-1' },
			job_type: 'kitchen',
			status: 'printed',
			printer: 'kitchen-01',
			payload: 'Beef Noodles x2',
			retry_count: { type: 'integer', value: 1 }
		},
		source: { type: 'smoke' },
		created_by: 'user-smoke',
		updated_by: 'user-smoke',
		created_at: now,
		updated_at: now
	}
];

const comparisonRecord = {
	id: 'record-order-line-comparison',
	workspace_id: workspaceId,
	project_id: projectId,
	form_id: orderLineForm.id,
	title: 'Green Tea x1',
	values: {
		order_id: { record_id: 'order-1', title: 'Order 1', form_key: 'order' },
		sku_id: { record_id: 'sku-2', title: 'Green Tea', form_key: 'sku' },
		sku_name: 'Green Tea',
		quantity: { type: 'integer', value: 1 },
		unit_price: { type: 'amount', decimal: '5.00', currency: 'CNY', scale: 2 },
		line_total: { type: 'amount', decimal: '5.00', currency: 'CNY', scale: 2 },
		seat_no: '2',
		status: 'served',
		service_date: '2026-05-30',
		service_end_date: '2026-05-30'
	},
	source: { type: 'smoke' },
	created_by: 'user-other',
	updated_by: 'user-other',
	created_at: now,
	updated_at: now
};

function seedCreatedRecord() {
	if (!createdRecord) {
		createdRecord = {
			id: 'record-order-line-created',
			workspace_id: workspaceId,
			project_id: projectId,
			form_id: orderLineForm.id,
			title: 'Beef Noodles x2',
			values: {
				order_id: { record_id: 'order-1', title: 'Order 1', form_key: 'order' },
				sku_id: { record_id: 'sku-1', title: 'Beef Noodles', form_key: 'sku' },
				sku_name: 'Beef Noodles',
				quantity: { type: 'integer', value: 2 },
				unit_price: { type: 'amount', decimal: '9.99', currency: 'CNY', scale: 2 },
				line_total: { type: 'amount', decimal: '19.98', currency: 'CNY', scale: 2 },
				seat_no: '1',
				status: 'sent_to_kitchen',
				service_date: '2026-05-31',
				attachment_12: 'https://assets.example.test/detail-ticket.pdf',
				dish_photo: 'https://assets.example.test/beef-noodles.jpg',
				ticket_no: 'AUTO-000001'
			},
			source: { type: 'web' },
			created_by: 'user-smoke',
			updated_by: 'user-smoke',
			created_at: now,
			updated_at: now
		};
	}
	if (!formAttachments.some((attachment) => attachment.id === 'attachment-durable-detail')) {
		formAttachments = [
			{
				id: 'attachment-durable-detail',
				workspace_id: workspaceId,
				project_id: projectId,
				form_id: orderLineForm.id,
				record_id: 'record-order-line-created',
				field_id: 'fld_attachment_12',
				field_key: 'attachment_12',
				file_name: 'detail-ticket.pdf',
				content_type: 'application/pdf',
				byte_size: 2048,
				storage_key: 'smoke/detail-ticket.pdf',
				url: 'https://assets.example.test/detail-ticket.pdf',
				created_by: 'user-smoke',
				archived_at: null,
				created_at: now,
				updated_at: now
			},
			...formAttachments
		];
	}
	return createdRecord;
}

function orderLineEvents() {
	const items = [
		{
			id: 'event-order-line-comparison-updated',
			workspace_id: workspaceId,
			project_id: projectId,
			event_type: 'form.record.updated',
			aggregate_type: 'form_record',
			aggregate_id: comparisonRecord.id,
			actor_id: 'user-other',
			source: { type: 'smoke', origin: 'api' },
			payload: { title: comparisonRecord.title, status: comparisonRecord.values.status },
			metadata: { form_id: orderLineForm.id, form_key: orderLineForm.key },
			correlation_id: null,
			causation_id: null,
			idempotency_key: null,
			created_at: new Date(Date.parse(now) - 60_000).toISOString()
		}
	];
	if (createdRecord) {
		items.unshift({
			id: 'event-order-line-created',
			workspace_id: workspaceId,
			project_id: projectId,
			event_type: 'form.record.created',
			aggregate_type: 'form_record',
			aggregate_id: createdRecord.id,
			actor_id: 'user-smoke',
			source: { type: 'web', origin: 'forms-ui-smoke' },
			payload: { title: createdRecord.title, status: createdRecord.values.status },
			metadata: { form_id: orderLineForm.id, form_key: orderLineForm.key },
			correlation_id: null,
			causation_id: null,
			idempotency_key: 'forms-ui-smoke-created',
			created_at: new Date(Date.parse(now) + 5_000).toISOString()
		});
	}
	return items;
}

function apiResult(data) {
	return { code: 0, message: 'success', data };
}

function nextUpdatedAt() {
	viewUpdateCounter += 1;
	return new Date(Date.parse(now) + viewUpdateCounter * 1000).toISOString();
}

function paginated(items) {
	return { items, total: items.length, page: 1, per_page: items.length, total_pages: 1 };
}

function readableFieldKeys() {
	return Object.entries(memberPermissionPolicy.fields ?? {})
		.filter(([, policy]) => policy?.read === false)
		.map(([fieldKey]) => fieldKey);
}

function filterReadableRecord(record) {
	const hidden = readableFieldKeys();
	if (hidden.length === 0) return record;
	const values = { ...(record.values ?? {}) };
	for (const fieldKey of hidden) delete values[fieldKey];
	return { ...record, values };
}

function viewById(viewId) {
	return orderLineViews.find((view) => view.id === viewId) ?? null;
}

function filterNodeDisabled(node) {
	return Boolean(node && typeof node === 'object' && (node.disabled === true || node.enabled === false));
}

function recordMatchesFilter(record, filter) {
	const expected = typeof filter.value === 'string' ? filter.value.trim().toLowerCase() : '';
	const rawValue = record.values[filter.field];
	const actualValue =
		rawValue && typeof rawValue === 'object'
			? typeof rawValue.display === 'string'
				? rawValue.display
				: typeof rawValue.title === 'string'
					? rawValue.title
					: typeof rawValue.decimal === 'string'
						? rawValue.decimal
						: typeof rawValue.value === 'number'
							? String(rawValue.value)
							: JSON.stringify(rawValue)
			: rawValue;
	if (filter.operator === 'not_empty') {
		return String(actualValue ?? '').trim().length > 0;
	}
	if (!expected) return true;
	const actual = String(actualValue ?? '').toLowerCase();
	if (filter.operator === 'equals') return actual === expected;
	if (filter.operator === 'not_equals') return actual !== expected;
	return actual.includes(expected);
}

function recordMatchesFilterNode(record, filter) {
	if (filterNodeDisabled(filter)) return null;
	return recordMatchesFilter(record, filter);
}

function recordMatchesFilterExpression(record, expression) {
	if (!expression || typeof expression !== 'object') return true;
	if (filterNodeDisabled(expression)) return null;
	const filter = expression.filter && typeof expression.filter === 'object' ? expression.filter : expression;
	if (typeof filter.field === 'string') return recordMatchesFilterNode(record, filter);
	const rawChildren = Array.isArray(expression.children)
		? expression.children
		: Array.isArray(expression.expressions)
			? expression.expressions
			: [];
	const matches = rawChildren
		.map((child) => recordMatchesFilterExpression(record, child))
		.filter((match) => match !== null);
	if (matches.length === 0) return null;
	return expression.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
}

function orderLineRecordsForUrl(url) {
	let records = createdRecord ? [comparisonRecord, createdRecord] : [comparisonRecord];
	const view = viewById(url.searchParams.get('view_id'));
	if (memberPermissionPolicy.record_scope === 'owned') {
		records = records.filter((record) => record.created_by === 'user-smoke');
	}
	if (view?.config?.filter_expression) {
		records = records.filter((record) =>
			recordMatchesFilterExpression(record, view.config.filter_expression) !== false
		);
	} else {
		const filterGroups = Array.isArray(view?.config?.filter_groups)
		? view.config.filter_groups
		: Array.isArray(view?.config?.filters)
			? [{ logic: 'all', filters: view.config.filters }]
			: view?.config?.filter
				? [{ logic: 'all', filters: [view.config.filter] }]
				: [];
		for (const group of filterGroups) {
			if (filterNodeDisabled(group)) continue;
			const filters = Array.isArray(group.filters) ? group.filters : [];
			records = records.filter((record) => {
				const matches = filters
					.map((filter) => recordMatchesFilterNode(record, filter))
					.filter((match) => match !== null);
				if (matches.length === 0) return true;
				return group.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
			});
		}
	}
	if (view?.config?.sort?.field === 'quantity') {
		const direction = view.config.sort.direction === 'desc' ? -1 : 1;
		records = [...records].sort((left, right) => {
			const leftValue = Number(left.values.quantity?.value ?? left.values.quantity ?? 0);
			const rightValue = Number(right.values.quantity?.value ?? right.values.quantity ?? 0);
			return (leftValue - rightValue) * direction;
		});
	}
	return records.map(filterReadableRecord);
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

async function readRawBody(req) {
	const chunks = [];
	for await (const chunk of req) chunks.push(chunk);
	return Buffer.concat(chunks);
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
const smokeNow = ${JSON.stringify(now)};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function dateTimeLocal(value) {
  const date = new Date(value);
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}
function dateTimeLocalDayBoundary(value, hour, minute) {
  const date = new Date(value);
  date.setHours(hour, minute, 0, 0);
  return dateTimeLocal(date.toISOString());
}
const timelineEventDayWindowFrom = dateTimeLocalDayBoundary(smokeNow, 0, 0);
const timelineEventDayWindowTo = dateTimeLocalDayBoundary(smokeNow, 23, 59);
function mark(name, value = 'done') {
  document.body.setAttribute(name, value);
  sessionStorage.setItem('forms-smoke:' + name, value);
}
function restoreMarks() {
  for (let index = 0; index < sessionStorage.length; index += 1) {
    const key = sessionStorage.key(index);
    if (key && key.startsWith('forms-smoke:data-')) {
      document.body.setAttribute(key.replace('forms-smoke:', ''), sessionStorage.getItem(key) || 'done');
    }
  }
}
function smokeDebug() {
  const importMapping = document.querySelector('[data-import-mapping-wizard]');
  const importMappingDebug = importMapping
    ? '\\nImport mapping debug: ' + JSON.stringify({
        applied: importMapping.getAttribute('data-import-mapping-applied'),
        textarea: document.querySelector('#form-import-text')?.value,
        selections: Array.from(document.querySelectorAll('[data-import-mapping-column]')).map((select) => [
          select.getAttribute('data-import-mapping-column'),
          select.value
        ]),
        transforms: Array.from(document.querySelectorAll('[data-import-mapping-transform]')).map((select) => [
          select.getAttribute('data-import-mapping-transform'),
          select.value
        ])
      })
    : '';
  const designerFields = Array.from(document.querySelectorAll('[data-designer-field-key]')).map((field) => [
    field.getAttribute('data-designer-field-key'),
    field.getAttribute('data-designer-field-type'),
    field.textContent.trim()
  ]);
  const fieldButtons = Array.from(document.querySelectorAll('[data-field-library-type]')).map((button) => [
    button.getAttribute('data-field-library-type'),
    button.textContent.trim()
  ]);
  const designerLabels = Array.from(document.querySelectorAll('label')).map((label) => label.textContent.trim()).filter(Boolean).slice(0, 40);
  const optionColorInputs = Array.from(document.querySelectorAll('[data-option-colors-input]')).map((input) => [
    input.getAttribute('data-option-colors-input'),
    input.value
  ]);
  const optionArchivedInputs = Array.from(document.querySelectorAll('[data-option-archived-input]')).map((input) => [
    input.getAttribute('data-option-archived-input'),
    input.value
  ]);
  const designerDebug = designerFields.length || fieldButtons.length
    ? '\\nDesigner debug: ' + JSON.stringify({ fields: designerFields, fieldButtons, labels: designerLabels, optionColorInputs, optionArchivedInputs })
    : '';
  const childTableNodes = Array.from(document.querySelectorAll('[data-child-table-grid],[data-record-detail-child-table-widget],[data-record-detail-child-table-cell],[data-record-detail-section]')).map((node) => [
    node.getAttribute('data-child-table-grid') ||
      node.getAttribute('data-record-detail-child-table-widget') ||
      node.getAttribute('data-record-detail-child-table-cell') ||
      node.getAttribute('data-record-detail-section'),
    node.textContent.trim().slice(0, 160)
  ]);
  const childTableDebug = childTableNodes.length
    ? '\\nChild table debug: ' + JSON.stringify(childTableNodes)
    : '';
  const attachmentDetailNodes = Array.from(
    document.querySelectorAll(
      '[data-record-detail-attachments],[data-record-detail-attachment-field],[data-record-detail-attachment-item],[data-record-detail-attachment-url],[data-record-detail-attachment-value]'
    )
  ).map((node) => [
    node.getAttribute('data-record-detail-attachments') ||
      node.getAttribute('data-record-detail-attachment-field') ||
      node.getAttribute('data-record-detail-attachment-item') ||
      node.getAttribute('data-record-detail-attachment-url') ||
      node.getAttribute('data-record-detail-attachment-value'),
    node.getAttribute('data-record-detail-attachment-count') ||
      node.getAttribute('data-record-detail-attachment-field-count') ||
      '',
    node.textContent.trim().slice(0, 240)
  ]);
  const attachmentDetailDebug = attachmentDetailNodes.length
    ? '\\nRecord detail attachment debug: ' + JSON.stringify(attachmentDetailNodes)
    : '';
  const attachmentEditNodes = Array.from(
    document.querySelectorAll('[data-attachment-item],[data-attachment-thumbnail],[data-attachment-open]')
  ).map((node) => [
    node.getAttribute('data-attachment-item') ||
      node.getAttribute('data-attachment-thumbnail') ||
      node.getAttribute('data-attachment-open'),
    node.getAttribute('src') || node.getAttribute('href') || '',
    node.textContent.trim().slice(0, 160)
  ]);
  const attachmentEditDebug = attachmentEditNodes.length
    ? '\\nAttachment edit debug: ' + JSON.stringify(attachmentEditNodes)
    : '';
  const commentDetailNodes = Array.from(
    document.querySelectorAll(
      '[data-record-detail-comments],[data-record-detail-comment],[data-record-detail-comment-author],[data-record-detail-comment-body],[data-record-detail-comments-empty]'
    )
  ).map((node) => [
    node.getAttribute('data-record-detail-comments') ||
      node.getAttribute('data-record-detail-comment') ||
      node.getAttribute('data-record-detail-comment-author') ||
      node.getAttribute('data-record-detail-comment-body') ||
      node.getAttribute('data-record-detail-comments-empty'),
    node.getAttribute('data-record-detail-comment-count') || '',
    node.textContent.trim().slice(0, 260)
  ]);
  const commentDetailDebug = commentDetailNodes.length
    ? '\\nRecord detail comment debug: ' + JSON.stringify(commentDetailNodes)
    : '';
  const pivot = document.querySelector('[data-view-renderer="pivot"]');
  if (!pivot) return importMappingDebug + designerDebug + childTableDebug + attachmentDetailDebug + attachmentEditDebug + commentDetailDebug;
  const cells = Array.from(document.querySelectorAll('[data-pivot-cell]')).map((cell) => [
    cell.getAttribute('data-pivot-cell'),
    cell.textContent.trim()
  ]);
  const fields = Array.from(document.querySelectorAll('[data-pivot-row-field],[data-pivot-column-field],[data-pivot-value-field]')).map((field) => [
    field.getAttribute('data-pivot-row-field') || field.getAttribute('data-pivot-column-field') || field.getAttribute('data-pivot-value-field'),
    field.textContent.trim()
  ]);
  return importMappingDebug + designerDebug + childTableDebug + attachmentDetailDebug + attachmentEditDebug + commentDetailDebug + '\\nPivot debug: ' + JSON.stringify({ fields, cells, html: pivot.outerHTML.slice(0, 2000) });
}
async function waitFor(check, label) {
  for (let i = 0; i < 120; i += 1) {
    restoreMarks();
    const value = check();
    if (value) return value;
    await sleep(120);
  }
  throw new Error('Timed out waiting for ' + label);
}
const textInElements = (text, selector = 'a,button,h1,h2,h3,p,span,td,div,li') => Array.from(document.querySelectorAll(selector)).some((el) => el.textContent.includes(text));
const buttonExact = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.trim() === text);
function clickFieldLibraryType(type) {
  const button = document.querySelector('[data-field-library-type="' + type + '"]');
  if (!button) throw new Error('field library button not found: ' + type);
  button.scrollIntoView({ block: 'center', inline: 'nearest' });
  button.focus();
  button.click();
}
async function appendDesignerField(type, expectedKey, label) {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    clickFieldLibraryType(type);
    for (let index = 0; index < 20; index += 1) {
      restoreMarks();
      const field = document.querySelector('[data-designer-field-key="' + expectedKey + '"][data-designer-field-type="' + type + '"]');
      if (field || textInElements(expectedKey)) return;
      await sleep(120);
    }
  }
  throw new Error('Timed out waiting for ' + label);
}
function selectDesignerField(key) {
  const field = document.querySelector('[data-designer-field-key="' + key + '"]');
  if (!field) throw new Error('designer field not found: ' + key);
  field.scrollIntoView({ block: 'center', inline: 'nearest' });
  field.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}
	function labelControl(labelText) {
	  const labels = Array.from(document.querySelectorAll('label'));
	  const label =
	    labels.find((item) =>
	      Array.from(item.querySelectorAll('span')).some((span) => span.textContent.trim().startsWith(labelText))
	    ) ?? labels.find((item) => item.textContent.includes(labelText));
	  if (!label) throw new Error('label not found: ' + labelText);
	  const control = label.querySelector('input, textarea, select');
	  if (!control) throw new Error('control not found for label: ' + labelText);
	  return control;
	}
	function labelControlExact(labelText) {
	  const label = Array.from(document.querySelectorAll('label')).find((item) =>
	    Array.from(item.querySelectorAll('span')).some((span) => span.textContent.trim().startsWith(labelText))
	  );
	  if (!label) throw new Error('exact label not found: ' + labelText);
	  const control = label.querySelector('input, textarea, select');
	  if (!control) throw new Error('control not found for exact label: ' + labelText);
	  return control;
	}
function setByLabel(labelText, value) {
  const control = labelControl(labelText);
  control.value = value;
  control.dispatchEvent(new Event(control.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
  control.dispatchEvent(new Event('input', { bubbles: true }));
}
function setCheckedByLabel(labelText, checked) {
  const control = labelControl(labelText);
  if (!(control instanceof HTMLInputElement) || control.type !== 'checkbox') {
    throw new Error(labelText + ' is not a checkbox');
  }
  control.checked = checked;
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function setCheckedByAria(labelText, checked) {
  const control = Array.from(document.querySelectorAll('input[type="checkbox"]')).find((item) => item.getAttribute('aria-label') === labelText);
  if (!control) throw new Error('checkbox not found: ' + labelText);
  control.checked = checked;
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function permissionStateMatches(mode) {
  const summary = document.querySelector('[data-permission-state-summary]');
  return (
    summary?.getAttribute('data-permission-state-mode') === mode &&
    summary?.getAttribute('data-permission-state-scope') === 'owned' &&
    summary?.getAttribute('data-permission-state-hidden-fields') === 'Unit Price' &&
    summary?.getAttribute('data-permission-state-locked-fields') === 'Quantity|Unit Price'
  );
}
function setByPlaceholder(placeholder, value) {
  const control = Array.from(document.querySelectorAll('input, textarea')).find((item) => item.placeholder && item.placeholder.includes(placeholder));
  if (!control) throw new Error('placeholder not found: ' + placeholder);
  control.value = value;
  control.dispatchEvent(new Event('input', { bubbles: true }));
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function expectExactValue(labelText, value) {
  const actual = labelControlExact(labelText).value;
  if (actual !== value) throw new Error(labelText + ' expected ' + value + ' but got ' + actual);
}
async function drawSignatureCanvas(fieldKey) {
  const canvas = await waitFor(
    () => document.querySelector('[data-signature-canvas="' + fieldKey + '"]'),
    fieldKey + ' signature canvas'
  );
  const rect = canvas.getBoundingClientRect();
  const startX = rect.left + Math.max(12, rect.width * 0.2);
  const startY = rect.top + Math.max(12, rect.height * 0.45);
  const endX = rect.left + Math.max(48, rect.width * 0.78);
  const endY = rect.top + Math.max(24, rect.height * 0.58);
  canvas.dispatchEvent(
    new PointerEvent('pointerdown', {
      bubbles: true,
      cancelable: true,
      clientX: startX,
      clientY: startY,
      pointerId: 19,
      pointerType: 'pen'
    })
  );
  canvas.dispatchEvent(
    new PointerEvent('pointermove', {
      bubbles: true,
      cancelable: true,
      clientX: endX,
      clientY: endY,
      pointerId: 19,
      pointerType: 'pen'
    })
  );
  canvas.dispatchEvent(
    new PointerEvent('pointerup', {
      bubbles: true,
      cancelable: true,
      clientX: endX,
      clientY: endY,
      pointerId: 19,
      pointerType: 'pen'
    })
  );
  return waitFor(() => {
    const value = document.querySelector('[data-signature-value="' + fieldKey + '"]')?.value;
    return typeof value === 'string' && value.startsWith('data:image/png') ? value : null;
  }, fieldKey + ' signature data url');
}
function setSelectByCurrentValue(currentValue, nextValue) {
  const control = Array.from(document.querySelectorAll('select')).find((item) => item.value === currentValue);
  if (!control) throw new Error('select with current value not found: ' + currentValue);
  control.value = nextValue;
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function setDateInputByCurrentValue(currentValue, nextValue) {
  const control = Array.from(document.querySelectorAll('input[type="date"]')).find((item) => item.value === currentValue);
  if (!control) throw new Error('date input with current value not found: ' + currentValue);
  control.value = nextValue;
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function setCardSelect(recordId, fieldKey, value) {
  const control = document.querySelector('[data-card-select="' + recordId + ':' + fieldKey + '"]');
  if (!control) throw new Error('card select not found: ' + recordId + ':' + fieldKey);
  control.value = value;
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function setTimelineSelect(recordId, fieldKey, value) {
  const control = document.querySelector('[data-timeline-select="' + recordId + ':' + fieldKey + '"]');
  if (!control) throw new Error('timeline select not found: ' + recordId + ':' + fieldKey);
  control.value = value;
  control.dispatchEvent(new Event('change', { bubbles: true }));
}
function dragKanbanCardToColumn(recordId, columnLabel) {
  const card = document.querySelector('[data-kanban-card="' + recordId + '"]');
  const column = document.querySelector('[data-kanban-column="' + columnLabel + '"]');
  if (!card) throw new Error('kanban card not found: ' + recordId);
  if (!column) throw new Error('kanban column not found: ' + columnLabel);
  const dataTransfer = new DataTransfer();
  card.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer }));
  column.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer }));
  column.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer }));
  card.dispatchEvent(new DragEvent('dragend', { bubbles: true, cancelable: true, dataTransfer }));
}
function dragCalendarCardToDate(recordId, isoDate) {
  const card = document.querySelector('[data-calendar-card="' + recordId + '"]');
  const column = document.querySelector('[data-calendar-date="' + isoDate + '"]');
  if (!card) throw new Error('calendar card not found: ' + recordId);
  if (!column) throw new Error('calendar date not found: ' + isoDate);
  const dataTransfer = new DataTransfer();
  card.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer }));
  column.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer }));
  column.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer }));
  card.dispatchEvent(new DragEvent('dragend', { bubbles: true, cancelable: true, dataTransfer }));
}
function dragGanttBarToDate(recordId, isoDate) {
  const bar = document.querySelector('[data-gantt-bar="' + recordId + '"]');
  const date = document.querySelector('[data-gantt-date="' + isoDate + '"]');
  if (!bar) throw new Error('gantt bar not found: ' + recordId);
  if (!date) throw new Error('gantt date not found: ' + isoDate);
  const dataTransfer = new DataTransfer();
  bar.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer }));
  date.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer }));
  date.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer }));
  bar.dispatchEvent(new DragEvent('dragend', { bubbles: true, cancelable: true, dataTransfer }));
}
(async () => {
  for (const key of Object.keys(sessionStorage)) {
    if (key.startsWith('forms-smoke:')) sessionStorage.removeItem(key);
  }
  await waitFor(() => textInElements('Forms') && textInElements('Order Line') && textInElements('Data') && textInElements('Design') && textInElements('Automation'), 'forms workflow page');
  mark('data-workflow-tabs-seen');

  if (mode === 'mobile') {
    await waitFor(() => document.body.scrollHeight > 0, 'mobile layout ready');
    const overflow = document.documentElement.scrollWidth - window.innerWidth;
    if (overflow > 2) throw new Error('mobile horizontal overflow: ' + overflow);
    mark('data-forms-mobile-smoke');
    return;
  }

  if (mode === 'detail-url') {
    await waitFor(
      () => {
        const params = new URL(location.href).searchParams;
        return (
          params.get('form') === 'form-order-line' &&
          params.get('record') === 'record-order-line-created' &&
          document.querySelector('[data-record-detail-drawer="record-order-line-created"]') &&
          document.querySelector('[data-record-detail-url-record="record-order-line-created"]') &&
          document.querySelector('[data-record-detail-layout="record-order-line-created"]') &&
          document.querySelector('[data-record-detail-section="record-order-line-created:handoff"]')?.textContent?.includes('Kitchen Handoff') &&
          textInElements('Record links') &&
          textInElements('Beef Noodles x2')
        );
      },
      'record detail restored from URL'
    );
    mark('data-forms-detail-url-smoke');
    return;
  }

  buttonExact('Design').click();
  await waitFor(() => textInElements('Field library') && textInElements('Properties') && textInElements('Save design'), 'designer mode');
  mark('data-designer-mode-seen');
  await waitFor(() => textInElements('Field safety') && document.querySelector('[data-field-safety-values="1"][data-field-safety-dependencies="1"][data-field-safety-blocking="1"]'), 'designer field safety');
  const removeFieldButton = buttonExact('Remove field');
  if (!removeFieldButton || !removeFieldButton.disabled) throw new Error('blocking dependency did not disable field removal');
  mark('data-designer-safety');
  await waitFor(() => labelControl('Field key').value === 'order_id', 'relation field reselected after option colors');
  setByLabel('Field key', 'order_ref');
  setByLabel('Target form', 'order');
  setByLabel('Relation type', 'parent_child');
  setByLabel('Relation key', 'order_lines');
  setByLabel('Display field', 'order_no');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('Schema changes require confirmation') && textInElements('Rename') && textInElements('order_id -> order_ref'), 'schema change confirmation');
  buttonExact('Confirm save').click();
  await waitFor(() => !textInElements('Schema changes require confirmation') && textInElements('v2'), 'confirmed schema save');
  mark('data-designer-confirmed-save');
  await appendDesignerField('text', 'text_10', 'new designer field');
  setByLabel('Placeholder', 'Short note');
  setByLabel('Help text', 'Use a short note for the kitchen handoff');
  setByLabel('Default value', 'auto note');
  setByLabel('Max length', '40');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v3') && textInElements('text_10'), 'designer save');
  mark('data-designer-save');
  await appendDesignerField('formula', 'formula_11', 'new formula field');
  setByLabel('Operation', 'multiply');
  setByLabel('Arguments', 'quantity\\nunit_price');
  setByLabel('Scale', '2');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v4') && textInElements('formula_11'), 'formula config save');
  mark('data-designer-formula-save');
  await appendDesignerField('attachment', 'attachment_12', 'new attachment field');
  setByLabel('Accepted types', 'application/pdf\\nimage/png');
  setByLabel('Max size MB', '12');
  setByLabel('Storage policy', 'private');
  setByLabel('Retention days', '30');
  setByLabel('Signed URL TTL', '15');
  setByLabel('Thumbnail format', 'jpeg');
  setByLabel('Preview format', 'webp');
  setByLabel('Variant policies', 'gallery:1200:webp\\ncard:640:jpeg');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v5') && textInElements('attachment_12'), 'attachment config save');
  mark('data-designer-attachment-save');
  mark('data-designer-attachment-storage-policy');
  if (mode === 'attachment-storage-policy') {
    document.body.setAttribute('data-forms-ui-smoke', 'done');
    return;
  }
  await appendDesignerField('date', 'date_13', 'new gantt end date field');
  setByLabel('Field key', 'service_end_date');
  setByLabel('Field name', 'Service End Date');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v6') && textInElements('service_end_date'), 'gantt end date field save');
  mark('data-designer-gantt-end-date-save');
  await appendDesignerField('image', 'image_14', 'new card cover image field');
  setByLabel('Field key', 'dish_photo');
  setByLabel('Field name', 'Dish Photo');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v7') && textInElements('dish_photo'), 'card cover image field save');
  mark('data-designer-card-cover-save');
  await appendDesignerField('single_select', 'single_select_15', 'new option color field');
  setByLabel('Field key', 'service_status');
  setByLabel('Field name', 'Service Status');
  setByLabel('Options', 'normal\\npriority\\nlegacy');
  setByLabel('Default value', 'normal');
  await waitFor(() => document.querySelector('[data-option-colors-input="service_status"]'), 'service_status option color input');
  const serviceStatusColors = document.querySelector('[data-option-colors-input="service_status"]');
  if (!serviceStatusColors) throw new Error('service_status option color input not found');
  serviceStatusColors.value = 'normal: #2563eb\\npriority: #dc2626\\nlegacy: #64748b';
  serviceStatusColors.dispatchEvent(new Event('input', { bubbles: true }));
  await waitFor(() => document.querySelector('[data-option-disabled-input="service_status"]'), 'service_status option disabled input');
  const serviceStatusDisabled = document.querySelector('[data-option-disabled-input="service_status"]');
  if (!serviceStatusDisabled) throw new Error('service_status option disabled input not found');
  serviceStatusDisabled.value = 'legacy';
  serviceStatusDisabled.dispatchEvent(new Event('input', { bubbles: true }));
  await waitFor(() => document.querySelector('[data-option-archived-input="service_status"]'), 'service_status option archived input');
  setByLabel('Archived options', 'priority');
  await waitFor(
    () => document.querySelector('[data-option-archived-input="service_status"]')?.getAttribute('data-option-archived-state') === 'priority',
    'service_status option archived state'
  );
  await waitFor(() => document.querySelector('[data-option-descriptions-input="service_status"]'), 'service_status option descriptions input');
  const serviceStatusDescriptions = document.querySelector('[data-option-descriptions-input="service_status"]');
  if (!serviceStatusDescriptions) throw new Error('service_status option descriptions input not found');
  serviceStatusDescriptions.value = 'normal: Standard guest service\\npriority: Requires manager approval';
  serviceStatusDescriptions.dispatchEvent(new Event('input', { bubbles: true }));
  await waitFor(() => document.querySelector('[data-option-groups-input="service_status"]'), 'service_status option groups input');
  const serviceStatusGroups = document.querySelector('[data-option-groups-input="service_status"]');
  if (!serviceStatusGroups) throw new Error('service_status option groups input not found');
  serviceStatusGroups.value = 'normal: Service\\npriority: Escalated';
  serviceStatusGroups.dispatchEvent(new Event('input', { bubbles: true }));
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v8') && textInElements('service_status'), 'option config field save');
  const serviceStatusSchemaAfterSave = await fetch('/api/v1/forms/form-order-line').then((response) => response.json());
  const serviceStatusFieldAfterSave = serviceStatusSchemaAfterSave?.data?.schema?.fields?.find(
    (field) => field.key === 'service_status'
  );
  if (!serviceStatusFieldAfterSave?.option_archived?.includes('priority')) {
    throw new Error('Expected archived option metadata immediately after save: ' + JSON.stringify(serviceStatusFieldAfterSave));
  }
  mark('data-designer-option-config-save');
  await appendDesignerField('email', 'email_16', 'new email field');
  setByLabel('Field key', 'guest_email');
  setByLabel('Field name', 'Guest Email');
  await appendDesignerField('phone', 'phone_17', 'new phone field');
  setByLabel('Field key', 'guest_phone');
  setByLabel('Field name', 'Guest Phone');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v9') && textInElements('guest_email') && textInElements('guest_phone'), 'contact field save');
  mark('data-designer-contact-fields-save');
  await appendDesignerField('address', 'address_18', 'new address field');
  setByLabel('Field key', 'delivery_address');
  setByLabel('Field name', 'Delivery Address');
  await appendDesignerField('location', 'location_19', 'new location field');
  setByLabel('Field key', 'delivery_location');
  setByLabel('Field name', 'Delivery Location');
  buttonExact('Save design').click();
  await waitFor(
    () => textInElements('v10') && textInElements('delivery_address') && textInElements('delivery_location'),
    'address/location field save'
  );
  mark('data-designer-address-location-fields-save');
  await appendDesignerField('rating', 'rating_20', 'new rating field');
  setByLabel('Field key', 'service_rating');
  setByLabel('Field name', 'Service Rating');
  await appendDesignerField('progress', 'progress_21', 'new progress field');
  setByLabel('Field key', 'prep_progress');
  setByLabel('Field name', 'Prep Progress');
  buttonExact('Save design').click();
  await waitFor(
    () => textInElements('v11') && textInElements('service_rating') && textInElements('prep_progress'),
    'rating/progress field save'
  );
  mark('data-designer-rating-progress-fields-save');
  await appendDesignerField('scan', 'scan_22', 'new scan field');
  setByLabel('Field key', 'scan_code');
  setByLabel('Field name', 'Scan Code');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v12') && textInElements('scan_code'), 'scan field save');
  mark('data-designer-scan-field-save');
  await appendDesignerField('signature', 'signature_23', 'new signature field');
  setByLabel('Field key', 'guest_signature');
  setByLabel('Field name', 'Guest Signature');
  const signatureReadOnlyCondition = await waitFor(
    () => document.querySelector('[data-conditional-readonly-input="guest_signature"]'),
    'guest_signature conditional readonly input'
  );
  signatureReadOnlyCondition.value = 'all:\\nservice_status not_equals priority\\nscan_code contains SKU-2026';
  signatureReadOnlyCondition.dispatchEvent(new Event('input', { bubbles: true }));
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v13') && textInElements('guest_signature'), 'signature field save');
  mark('data-designer-signature-field-save');
  await appendDesignerField('autonumber', 'autonumber_24', 'new autonumber field');
  setByLabel('Field key', 'ticket_no');
  setByLabel('Field name', 'Ticket No');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v14') && textInElements('ticket_no'), 'autonumber field save');
  mark('data-designer-autonumber-field-save');
  await appendDesignerField('member', 'member_25', 'new member field');
  setByLabel('Field key', 'assignee');
  setByLabel('Field name', 'Assigned Member');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v15') && textInElements('assignee'), 'member field save');
  mark('data-designer-member-field-save');
  await appendDesignerField('text', 'text_26', 'new conditional field');
  setByLabel('Field key', 'vip_note');
  setByLabel('Field name', 'VIP Note');
  const vipNoteHiddenCondition = await waitFor(
    () => document.querySelector('[data-conditional-hidden-input="vip_note"]'),
    'vip_note conditional hidden input'
  );
  vipNoteHiddenCondition.value = 'any:\\nservice_status=normal\\nservice_rating gte 4';
  vipNoteHiddenCondition.dispatchEvent(new Event('input', { bubbles: true }));
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v16') && textInElements('vip_note'), 'conditional field save');
  mark('data-designer-conditional-field-save');
  await appendDesignerField('child_table', 'child_table_27', 'new child table field');
  setByLabel('Field key', 'print_jobs');
  setByLabel('Field name', 'Print Jobs');
  setByLabel('Target form', 'print_job');
  setByLabel('Relation key', 'line_print_jobs');
  setByLabel('Display field', 'job_type');
  buttonExact('Save design').click();
  await waitFor(() => textInElements('v17') && textInElements('print_jobs'), 'child table field save');
  mark('data-designer-child-table-field-save');

  buttonExact('Data').click();
  await waitFor(() => textInElements('New record') && textInElements('Records') && textInElements('Use a short note for the kitchen handoff'), 'data mode');
  await waitFor(() => document.querySelector('[data-child-table-field="print_jobs"]'), 'child table placeholder');
  await waitFor(() => labelControlExact('Text').value === 'auto note', 'designer default value applied');
  await waitFor(() => labelControlExact('Service Status').value === 'normal', 'select default value applied');
  setByLabel('Title override', 'Beef Noodles x2');
  setByLabel('Order', 'order-1');
  setByLabel('SKU', '{"record_id":"sku-1"}');
  setByLabel('SKU Name', 'Beef Noodles');
  setByLabel('Quantity', '2');
  setByLabel('Unit Price', '9.99');
  setByLabel('Seat', '1');
  setByLabel('Status', 'sent_to_kitchen');
  setByLabel('Service Date', '2026-05-31');
  setByLabel('Service End Date', '2026-05-31');
  setByLabel('Dish Photo', 'https://assets.example.test/beef-noodles.jpg');
  setByLabel('Guest Email', 'guest@example.test');
  setByLabel('Guest Phone', '+1 555 0100');
  setByLabel('Delivery Address', '123 Test Ave');
  setByLabel('Delivery Location', '40.7128,-74.0060');
  setByLabel('Service Rating', '4');
  setByLabel('Prep Progress', '75');
  const drawnSignatureValue = await drawSignatureCanvas('guest_signature');
  setByLabel('Scan Code', 'SKU-2026-0001');
  setByLabel('Service Status', 'normal');
  await waitFor(
    () => document.querySelector('[data-record-edit-field="vip_note"][data-conditional-hidden="vip_note"]'),
    'conditional hidden field state'
  );
  await waitFor(() => labelControlExact('Guest Signature').readOnly, 'conditional readonly signature field');
  setByLabel('Assigned Member', 'user-smoke');
  const disabledLegacyOption = document.querySelector('select option[data-option-disabled="legacy"]');
  if (!disabledLegacyOption?.disabled) {
    throw new Error('Expected disabled service_status option in record editor');
  }
  const serviceStatusSelect = document.querySelector('[data-record-edit-field="service_status"] select');
  if (!serviceStatusSelect) {
    throw new Error('Expected service_status select in record editor');
  }
  if (serviceStatusSelect.querySelector('option[value="priority"]')) {
    throw new Error(
      'Expected archived service_status option to be hidden in record editor: ' +
        JSON.stringify({
          options: Array.from(serviceStatusSelect.querySelectorAll('option')).map((option) => ({
            value: option.value,
            archived: option.getAttribute('data-option-archived')
          }))
        })
    );
  }
  const describedStatusOption = document.querySelector(
    'select option[data-option-description="Standard guest service"][data-option-group="Service"]'
  );
  if (!describedStatusOption || !describedStatusOption.textContent?.includes('Service / normal')) {
    throw new Error('Expected service_status option description/group metadata in record editor');
  }
	  expectExactValue('Title override', 'Beef Noodles x2');
	  expectExactValue('SKU Name', 'Beef Noodles');
	  expectExactValue('Quantity', '2');
	  expectExactValue('Unit Price', '9.99');
	  expectExactValue('Seat', '1');
	  expectExactValue('Status', 'sent_to_kitchen');
	  expectExactValue('Service Date', '2026-05-31');
	  expectExactValue('Service End Date', '2026-05-31');
	  expectExactValue('Dish Photo', 'https://assets.example.test/beef-noodles.jpg');
	  expectExactValue('Guest Email', 'guest@example.test');
	  expectExactValue('Guest Phone', '+1 555 0100');
	  expectExactValue('Delivery Address', '123 Test Ave');
	  expectExactValue('Delivery Location', '40.7128,-74.0060');
	  expectExactValue('Service Rating', '4');
	  expectExactValue('Prep Progress', '75');
	  expectExactValue('Scan Code', 'SKU-2026-0001');
	  expectExactValue('Guest Signature', drawnSignatureValue);
	  expectExactValue('Service Status', 'normal');
	  expectExactValue('Assigned Member', 'user-smoke');
	  if (!document.querySelector('[data-autonumber-field="ticket_no"]')) {
	    throw new Error('Expected readonly autonumber field in record editor');
	  }
	  buttonExact('Add record').click();
	  await waitFor(() => textInElements('Beef Noodles x2') && textInElements('19.98 CNY'), 'created record grid with amount');
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-option-color="status:sent_to_kitchen"]')
	        ?.getAttribute('data-option-color-hex') === '#f59e0b',
	    'select option color rendered in grid'
	  );
	  mark('data-form-option-color-grid');
	  mark('data-form-record-created');
	  buttonExact('Detail').click();
	  await waitFor(() => textInElements('Record links') && textInElements('Beef Noodles x2'), 'created record selected for attachment');
	  buttonExact('Edit').click();
	  await waitFor(() => document.querySelector('[data-attachment-field="attachment_12"]'), 'attachment field editor');
	  const attachmentUrlInput = document.querySelector('[data-attachment-url-input="attachment_12"]');
	  if (!attachmentUrlInput) throw new Error('attachment URL input not found');
	  attachmentUrlInput.value = 'https://assets.example.test/order-ticket.pdf';
	  attachmentUrlInput.dispatchEvent(new Event('input', { bubbles: true }));
	  await waitFor(
	    () => document.querySelector('[data-attachment-field="attachment_12"] button')?.disabled === false,
	    'attachment link action enabled'
	  );
	  document.querySelector('[data-attachment-field="attachment_12"] button')?.click();
	  await waitFor(
	    () =>
	      textInElements('Attachment linked') &&
	      document.querySelector('[data-attachment-item="attachment-1"]')?.textContent?.includes('order-ticket.pdf') &&
	      document.querySelector('[data-attachment-open="attachment-1"]')?.getAttribute('href') === '/api/v1/form-attachments/attachment-1/download?expires=1893456000&signature=signed-attachment-1' &&
	      document.querySelector('[data-attachment-download="attachment-1"]')?.getAttribute('download') === 'order-ticket.pdf',
	    'attachment metadata linked with preview actions'
	  );
	  document.querySelector('[data-attachment-item="attachment-1"] button')?.click();
	  await waitFor(
	    () => textInElements('Attachment removed') && !document.querySelector('[data-attachment-item="attachment-1"]'),
	    'attachment metadata removed'
	  );
	  const localFileInput = document.querySelector('[data-attachment-file-input="attachment_12"]');
	  if (!localFileInput) throw new Error('attachment local file input not found');
	  const localAttachmentFile = new File(['local-ticket-preview'], 'counter-ticket.png', { type: 'image/png' });
	  const localAttachmentFiles = new DataTransfer();
	  localAttachmentFiles.items.add(localAttachmentFile);
	  localFileInput.files = localAttachmentFiles.files;
	  localFileInput.dispatchEvent(new Event('change', { bubbles: true }));
	  await waitFor(
	    () => {
	      const openHref = document.querySelector('[data-attachment-open="attachment-2"]')?.getAttribute('href') ?? '';
	      const thumbnailSrc =
	        document.querySelector('[data-attachment-thumbnail="attachment-2"]')?.getAttribute('src') ?? '';
	      return (
	        textInElements('Attachment linked') &&
	        document.querySelector('[data-attachment-item="attachment-2"]')?.textContent?.includes('counter-ticket.png') &&
	        thumbnailSrc === '/api/v1/uploads/thumbnails/thumb-server-counter-ticket.png' &&
	        openHref === '/api/v1/form-attachments/attachment-2/download?expires=1893456000&signature=signed-attachment-2'
	      );
	    },
	    'local attachment file uploaded with server URL preview'
	  );
	  mark('data-form-attachment-server-upload');
	  if (mode === 'attachment-server-upload') {
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  document.querySelector('[data-attachment-item="attachment-2"] button')?.click();
	  await waitFor(
	    () => textInElements('Attachment removed') && !document.querySelector('[data-attachment-item="attachment-2"]'),
	    'local attachment metadata removed'
	  );
	  attachmentUrlInput.value = 'https://assets.example.test/detail-ticket.pdf';
	  attachmentUrlInput.dispatchEvent(new Event('input', { bubbles: true }));
	  await waitFor(
	    () => document.querySelector('[data-attachment-field="attachment_12"] button')?.disabled === false,
	    'retained detail attachment action enabled'
	  );
	  document.querySelector('[data-attachment-field="attachment_12"] button')?.click();
	  await waitFor(
	    () =>
	      textInElements('Attachment linked') &&
	      document.querySelector('[data-attachment-item="attachment-3"]')?.textContent?.includes('detail-ticket.pdf') &&
	      document.querySelector('[data-attachment-open="attachment-3"]')?.getAttribute('href') === '/api/v1/form-attachments/attachment-3/download?expires=1893456000&signature=signed-attachment-3' &&
	      document.querySelector('[data-attachment-download="attachment-3"]')?.getAttribute('download') === 'detail-ticket.pdf',
	    'retained detail attachment metadata linked'
	  );
	  mark('data-form-attachment-record-lifecycle');
	  buttonExact('Cancel').click();
	  if (mode === 'detail-widget-hide-empty-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-hide-empty="links"]'),
	      'detail widget hide empty builder visible'
	    );
	    const linksHideEmpty = document.querySelector('[data-detail-page-widget-hide-empty="links"]');
	    if (!linksHideEmpty) throw new Error('links widget hide empty input not found');
	    linksHideEmpty.click();
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget hide empty saved');
	    mark('data-form-detail-widget-hide-empty-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-widget-count-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-show-count="comments"]'),
	      'detail widget count builder visible'
	    );
	    const commentsShowCount = document.querySelector('[data-detail-page-widget-show-count="comments"]');
	    if (!commentsShowCount) throw new Error('comments widget show count input not found');
	    commentsShowCount.click();
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget count saved');
	    mark('data-form-detail-widget-count-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-widget-density-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-density="comments"]'),
	      'detail widget density builder visible'
	    );
	    const commentsDensity = document.querySelector('[data-detail-page-widget-density="comments"]');
	    if (!commentsDensity) throw new Error('comments widget density select not found');
	    commentsDensity.value = 'compact';
	    commentsDensity.dispatchEvent(new Event('change', { bubbles: true }));
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget density saved');
	    mark('data-form-detail-widget-density-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-widget-collapsible-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-collapsible="comments"]') &&
	        document.querySelector('[data-detail-page-widget-default-collapsed="comments"]') &&
	        document.querySelector('[data-detail-page-widget-collapsible="children"]') &&
	        document.querySelector('[data-detail-page-widget-default-collapsed="children"]'),
	      'detail widget collapsible builder visible'
	    );
	    const commentsCollapsible = document.querySelector('[data-detail-page-widget-collapsible="comments"]');
	    const commentsDefaultCollapsed = document.querySelector('[data-detail-page-widget-default-collapsed="comments"]');
	    const childrenCollapsible = document.querySelector('[data-detail-page-widget-collapsible="children"]');
	    const childrenDefaultCollapsed = document.querySelector('[data-detail-page-widget-default-collapsed="children"]');
	    if (!commentsCollapsible) throw new Error('comments widget collapsible input not found');
	    if (!commentsDefaultCollapsed) throw new Error('comments widget default collapsed input not found');
	    if (!childrenCollapsible) throw new Error('children widget collapsible input not found');
	    if (!childrenDefaultCollapsed) throw new Error('children widget default collapsed input not found');
	    commentsCollapsible.click();
	    await waitFor(() => commentsDefaultCollapsed.disabled === false, 'comments default collapsed enabled');
	    commentsDefaultCollapsed.click();
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget collapsible saved');
	    await waitFor(() => childrenCollapsible.disabled === false, 'children collapsible enabled');
	    childrenCollapsible.click();
	    await waitFor(() => childrenDefaultCollapsed.disabled === false, 'children default collapsed enabled');
	    childrenDefaultCollapsed.click();
	    await waitFor(() => textInElements('Detail page widgets saved'), 'children widget collapsible saved');
	    mark('data-form-detail-widget-collapsible-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-widget-empty-text-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-empty-text="links"]'),
	      'detail widget empty text builder visible'
	    );
	    const linksEmptyText = document.querySelector('[data-detail-page-widget-empty-text="links"]');
	    if (!linksEmptyText) throw new Error('links widget empty text input not found');
	    linksEmptyText.value = 'No linked service records yet.';
	    linksEmptyText.dispatchEvent(new Event('change', { bubbles: true }));
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget empty text saved');
	    mark('data-form-detail-widget-empty-text-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-widget-description-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-description="comments"]'),
	      'detail widget description builder visible'
	    );
	    const commentsDescription = document.querySelector('[data-detail-page-widget-description="comments"]');
	    if (!commentsDescription) throw new Error('comments widget description input not found');
	    commentsDescription.value = 'Track kitchen follow-ups before service.';
	    commentsDescription.dispatchEvent(new Event('change', { bubbles: true }));
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget description saved');
	    mark('data-form-detail-widget-description-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-widget-limit-builder') {
	    await fetch('/api/v1/form-records/record-order-line-created/comments', {
	      method: 'POST',
	      headers: { 'Content-Type': 'application/json' },
	      body: JSON.stringify({
	        body: 'Limit smoke follow-up.',
	        visibility: 'internal',
	        metadata: { source: 'web', form_id: 'form-order-line', form_key: 'order_line' }
	      })
	    });
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-limit="comments"]'),
	      'detail widget limit builder visible'
	    );
	    const commentsLimit = document.querySelector('[data-detail-page-widget-limit="comments"]');
	    if (!commentsLimit) throw new Error('comments widget limit input not found');
	    commentsLimit.value = '1';
	    commentsLimit.dispatchEvent(new Event('change', { bubbles: true }));
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget limit saved');
	    mark('data-form-detail-widget-limit-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-widget-title-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-title="comments"]'),
	      'detail widget title builder visible'
	    );
	    const commentsTitle = document.querySelector('[data-detail-page-widget-title="comments"]');
	    if (!commentsTitle) throw new Error('comments widget title input not found');
	    commentsTitle.value = 'Ops Notes';
	    commentsTitle.dispatchEvent(new Event('change', { bubbles: true }));
	    await waitFor(() => textInElements('Detail page widgets saved'), 'detail widget title saved');
	    mark('data-form-detail-widget-title-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-page-composition-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-composition-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-region]'),
	      'detail page composition builder visible'
	    );
	    const regionSelect = document.querySelector('[data-detail-page-widget-region]');
	    if (!regionSelect) throw new Error('detail page widget region select not found');
	    regionSelect.value = 'below';
	    regionSelect.dispatchEvent(new Event('change', { bubbles: true }));
	    document.querySelector('[data-detail-page-composition-save] button')?.click();
	    await waitFor(() => textInElements('Detail page composition saved'), 'detail page composition saved');
	    mark('data-form-detail-page-composition-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-field-columns-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-composition-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-field-columns]'),
	      'detail field columns builder visible'
	    );
	    const columnsSelect = document.querySelector('[data-detail-page-field-columns]');
	    if (!columnsSelect) throw new Error('detail page field columns select not found');
	    columnsSelect.value = '1';
	    columnsSelect.dispatchEvent(new Event('change', { bubbles: true }));
	    document.querySelector('[data-detail-page-composition-save] button')?.click();
	    await waitFor(() => textInElements('Detail page composition saved'), 'detail field columns saved');
	    mark('data-form-detail-field-columns-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-highlights-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-highlights-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-highlight-field="quantity"]') &&
	        document.querySelector('[data-detail-highlight-field="line_total"]'),
	      'detail highlights builder visible'
	    );
	    document.querySelector('[data-detail-highlight-toggle="quantity"]')?.click();
	    document.querySelector('[data-detail-highlight-toggle="line_total"]')?.click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-highlight-selected="quantity:true"]') &&
	        document.querySelector('[data-detail-highlight-selected="line_total:true"]'),
	      'detail highlights selected'
	    );
	    document.querySelector('[data-detail-highlight-move-up="line_total"]')?.click();
	    document.querySelector('[data-detail-highlights-save] button')?.click();
	    await waitFor(() => textInElements('Detail highlights saved'), 'detail highlights saved');
	    mark('data-form-detail-highlights-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-header-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-header-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-header-subtitle-field]') &&
	        document.querySelector('[data-detail-header-badge-field]'),
	      'detail header builder visible'
	    );
	    const subtitleSelect = document.querySelector('[data-detail-header-subtitle-field]');
	    const badgeSelect = document.querySelector('[data-detail-header-badge-field]');
	    if (!subtitleSelect || !badgeSelect) throw new Error('detail header selects not found');
	    subtitleSelect.value = 'seat_no';
	    subtitleSelect.dispatchEvent(new Event('change', { bubbles: true }));
	    badgeSelect.value = 'status';
	    badgeSelect.dispatchEvent(new Event('change', { bubbles: true }));
	    document.querySelector('[data-detail-header-save] button')?.click();
	    await waitFor(() => textInElements('Detail header saved'), 'detail header saved');
	    mark('data-form-detail-header-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-section-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-layout-section-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-layout-section-row="handoff"]') &&
	        document.querySelector('[data-detail-layout-section-row="pricing"]'),
	      'detail layout section builder visible'
	    );
	    const handoffTitle = document.querySelector('[data-detail-layout-section-title="handoff"]');
	    if (!handoffTitle) throw new Error('handoff section title input not found');
	    handoffTitle.value = 'Service Snapshot';
	    handoffTitle.dispatchEvent(new Event('input', { bubbles: true }));
	    const handoffUnitPrice = document.querySelector('[data-detail-layout-section-field="handoff:unit_price"] input[type="checkbox"]');
	    if (!handoffUnitPrice) throw new Error('handoff unit_price checkbox not found');
	    handoffUnitPrice.click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-layout-section-field="handoff:unit_price"]')?.getAttribute('data-detail-layout-section-field-assigned') === 'handoff',
	      'unit price assigned to handoff section'
	    );
	    document.querySelector('[data-detail-layout-section-save] button')?.click();
	    await waitFor(() => textInElements('Detail sections saved'), 'detail layout sections saved');
	    mark('data-form-detail-layout-section-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-section-description-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-layout-section-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-layout-section-description="handoff"]'),
	      'detail layout section description builder visible'
	    );
	    const handoffDescription = document.querySelector('[data-detail-layout-section-description="handoff"]');
	    if (!handoffDescription) throw new Error('handoff section description input not found');
	    handoffDescription.value = 'Review service notes before closing the line.';
	    handoffDescription.dispatchEvent(new Event('input', { bubbles: true }));
	    document.querySelector('[data-detail-layout-section-save] button')?.click();
	    await waitFor(() => textInElements('Detail sections saved'), 'detail layout section description saved');
	    mark('data-form-detail-section-description-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-section-hide-empty-fields-builder') {
	    const recordResponse = await fetch('/api/v1/form-records/record-order-line-created').then((response) =>
	      response.json()
	    );
	    const record = recordResponse.data;
	    await fetch('/api/v1/form-records/record-order-line-created', {
	      method: 'PATCH',
	      headers: { 'Content-Type': 'application/json' },
	      body: JSON.stringify({
	        title: record.title,
	        values: { ...record.values, seat_no: '' },
	        source: { type: 'web', origin: 'detail-section-hide-empty-fields-smoke' }
	      })
	    });
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-layout-section-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-layout-section-hide-empty-fields="handoff"]'),
	      'detail layout section hide empty fields builder visible'
	    );
	    const handoffHideEmptyFields = document.querySelector('[data-detail-layout-section-hide-empty-fields="handoff"]');
	    if (!handoffHideEmptyFields) throw new Error('handoff section hide empty fields input not found');
	    handoffHideEmptyFields.click();
	    document.querySelector('[data-detail-layout-section-save] button')?.click();
	    await waitFor(() => textInElements('Detail sections saved'), 'detail layout section hide empty fields saved');
	    mark('data-form-detail-section-hide-empty-fields-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-section-columns-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-layout-section-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-layout-section-columns="handoff"]'),
	      'detail layout section columns builder visible'
	    );
	    const handoffColumns = document.querySelector('[data-detail-layout-section-columns="handoff"]');
	    if (!handoffColumns) throw new Error('handoff section columns select not found');
	    handoffColumns.value = '3';
	    handoffColumns.dispatchEvent(new Event('change', { bubbles: true }));
	    document.querySelector('[data-detail-layout-section-save] button')?.click();
	    await waitFor(() => textInElements('Detail sections saved'), 'detail layout section columns saved');
	    mark('data-form-detail-section-columns-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-section-density-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-layout-section-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-layout-section-density="handoff"]'),
	      'detail layout section density builder visible'
	    );
	    const handoffDensity = document.querySelector('[data-detail-layout-section-density="handoff"]');
	    if (!handoffDensity) throw new Error('handoff section density select not found');
	    handoffDensity.value = 'compact';
	    handoffDensity.dispatchEvent(new Event('change', { bubbles: true }));
	    document.querySelector('[data-detail-layout-section-save] button')?.click();
	    await waitFor(() => textInElements('Detail sections saved'), 'detail layout section density saved');
	    mark('data-form-detail-section-density-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'detail-section-collapsible-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-layout-section-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-layout-section-collapsible="handoff"]') &&
	        document.querySelector('[data-detail-layout-section-default-collapsed="handoff"]'),
	      'detail layout section collapsible builder visible'
	    );
	    const handoffCollapsible = document.querySelector('[data-detail-layout-section-collapsible="handoff"]');
	    const handoffDefaultCollapsed = document.querySelector('[data-detail-layout-section-default-collapsed="handoff"]');
	    if (!handoffCollapsible) throw new Error('handoff section collapsible input not found');
	    if (!handoffDefaultCollapsed) throw new Error('handoff section default collapsed input not found');
	    handoffCollapsible.click();
	    await waitFor(() => handoffDefaultCollapsed.disabled === false, 'handoff default collapsed enabled');
	    handoffDefaultCollapsed.click();
	    document.querySelector('[data-detail-layout-section-save] button')?.click();
	    await waitFor(() => textInElements('Detail sections saved'), 'detail layout section collapsible saved');
	    mark('data-form-detail-section-collapsible-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'page-widget-builder') {
	    buttonExact('Design').click();
	    await waitFor(
	      () =>
	        document.querySelector('[data-detail-page-widget-builder="form-order-line"]') &&
	        document.querySelector('[data-detail-page-widget-row="comments"]') &&
	        document.querySelector('[data-detail-page-widget-row="events"]'),
	      'detail page widget builder visible'
	    );
	    document.querySelector('[data-detail-page-widget-toggle="events"]')?.click();
	    await waitFor(
	      () =>
	        textInElements('Detail page widgets saved') &&
	        document.querySelector('[data-detail-page-widget-enabled="events:false"]'),
	      'detail page events widget disabled'
	    );
	    await waitFor(
	      () => document.querySelector('[data-detail-page-widget-move-up="comments"]')?.disabled === false,
	      'comments widget move enabled'
	    );
	    document.querySelector('[data-detail-page-widget-move-up="comments"]')?.click();
	    await waitFor(
	      () =>
	        Array.from(document.querySelectorAll('[data-detail-page-widget-row]'))
	          .map((element) => element.getAttribute('data-detail-page-widget-row'))
	          .join('|')
	          .startsWith('children|comments|links') &&
	        document.querySelector('[data-detail-page-widget-enabled="events:false"]'),
	      'comments widget moved above links'
	    );
	    mark('data-form-detail-page-widget-builder');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  if (mode === 'attachment-detail' || mode === 'comments-detail') {
	    buttonExact('Detail').click();
	    await waitFor(
	      () =>
	        textInElements('Record links') &&
	        textInElements('Beef Noodles x2') &&
	        document.querySelector('[data-record-detail-attachments="record-order-line-created"]')?.getAttribute('data-record-detail-attachment-count') === '2' &&
	        document.querySelector('[data-record-detail-attachment-field="attachment_12"]') &&
	        document.querySelector('[data-record-detail-attachment-field-count="attachment_12:2"]') &&
	        document.querySelector('[data-record-detail-attachment-item="attachment-3"]')?.textContent?.includes('detail-ticket.pdf') &&
	        document.querySelector('[data-record-detail-attachment-item="attachment-durable-detail"]')?.textContent?.includes('detail-ticket.pdf') &&
	        document.querySelector('[data-record-detail-attachment-url="attachment-3"]')?.textContent?.includes('https://assets.example.test/detail-ticket.pdf') &&
	        document.querySelector('[data-record-detail-attachment-open="attachment-3"]')?.getAttribute('href') === '/api/v1/form-attachments/attachment-3/download?expires=1893456000&signature=signed-attachment-3' &&
	        document.querySelector('[data-record-detail-attachment-download="attachment-3"]')?.getAttribute('download') === 'detail-ticket.pdf',
	      'record detail attachment widget'
	    );

	    await waitFor(
	      () =>
	        document.querySelector('[data-record-detail-comments="record-order-line-created"]')?.getAttribute('data-record-detail-comment-count') === '1' &&
	        document.querySelector('[data-record-detail-comment="comment-record-existing"]') &&
	        document.querySelector('[data-record-detail-comment-author="comment-record-existing"]')?.textContent?.includes('Smoke Admin') &&
	        document.querySelector('[data-record-detail-comment-body="comment-record-existing"]')?.textContent?.includes('Kitchen lead reviewed'),
	      'record detail comments widget'
	    );
	    if (mode === 'comments-detail') {
	      const commentInput = document.querySelector('[data-record-detail-comment-input="record-order-line-created"]');
	      if (!commentInput) throw new Error('record detail comment input not found');
	      commentInput.value = 'Chef added allergy follow-up.';
	      commentInput.dispatchEvent(new Event('input', { bubbles: true }));
	      await waitFor(
	        () => document.querySelector('[data-record-detail-comment-submit="record-order-line-created"] button')?.disabled === false,
	        'record detail comment submit enabled'
	      );
	      document.querySelector('[data-record-detail-comment-submit="record-order-line-created"] button')?.click();
	      await waitFor(
	        () =>
	          textInElements('Comment added') &&
	          document.querySelector('[data-record-detail-comments="record-order-line-created"]')?.getAttribute('data-record-detail-comment-count') === '2' &&
	          document.querySelector('[data-record-detail-comment="comment-record-created-1"]') &&
	          document.querySelector('[data-record-detail-comment-body="comment-record-created-1"]')?.textContent?.includes('Chef added allergy follow-up.'),
	        'record detail comment created'
	      );
	    }
	    mark('data-form-record-detail-comments-widget');
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  await waitFor(() => textInElements('Green Tea x1'), 'comparison record visible');
	  setByLabel('Current view', 'view-order-line-default');
	  await waitFor(() => document.querySelector('[data-view-renderer="grid"]'), 'default grid selected for view rules');
	  setCheckedByAria('Visible columns: Guest Signature', true);
	  setCheckedByAria('Visible columns: VIP Note', true);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'conditional indicator columns saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-column-conditional-readonly="guest_signature"]') &&
	      document.querySelector('[data-view-column-conditional-hidden="vip_note"]') &&
	      document.querySelector('[data-grid-header-conditional-readonly="guest_signature"]') &&
	      document.querySelector('[data-grid-header-conditional-hidden="vip_note"]') &&
	      document.querySelector('[data-grid-cell-conditional-readonly="record-order-line-created:guest_signature"]') &&
	      document.querySelector('[data-grid-cell-conditional-hidden="record-order-line-created:vip_note"]'),
	    'grid conditional field indicators'
	  );
	  mark('data-form-conditional-cross-view-grid');
	  buttonExact('Add filter').click();
	  await waitFor(() => {
	    try {
	      return Boolean(labelControl('Filter field'));
	    } catch {
	      return false;
	    }
	  }, 'first filter condition');
	  setByLabel('Filter field', 'sku_name');
	  setByLabel('Filter value', 'Beef');
	  const duplicateFirstFilter = document.querySelector('[data-filter-duplicate="1.1"]');
	  if (!duplicateFirstFilter) throw new Error('filter duplicate control not found');
	  duplicateFirstFilter.click();
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field 2').value === 'sku_name';
	    } catch {
	      return false;
	    }
	  }, 'duplicated filter condition');
	  setByLabel('Filter field 2', 'quantity');
	  setByLabel('Filter operator 2', 'not_empty');
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field 2').value === 'quantity' && labelControl('Filter operator 2').value === 'not_empty';
	    } catch {
	      return false;
	    }
	  }, 'duplicated filter condition edited');
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-match-count="1.1"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-filter-match-count="1.2"]')?.getAttribute('data-record-count') === '2' &&
	      document.querySelector('[data-filter-group-match-count="1"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-filter-expression-match-count]')?.getAttribute('data-record-count') === '1',
	    'filter expression match counts rendered'
	  );
	  mark('data-form-view-filter-match-counts');
	  document.querySelector('[data-filter-expression-copy-ids]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-expression-copy-ids]')?.getAttribute('data-filter-expression-copied') === 'true' &&
	      document.querySelector('[data-filter-expression-match-count]')?.getAttribute('data-record-count') === '1',
	    'filter expression matching record IDs copied'
	  );
	  mark('data-form-view-filter-expression-copy-ids');
	  document.querySelector('[data-filter-expression-copy-json]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-expression-copy-json]')?.getAttribute('data-filter-expression-json-copied') === 'true' &&
	      document.querySelector('[data-filter-expression-match-count]')?.getAttribute('data-record-count') === '1',
	    'filter expression JSON copied'
	  );
	  mark('data-form-view-filter-expression-copy-json');
	  document.querySelector('[data-filter-expression-export-config-json]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-filter-expression-export-config-json]')
	        ?.getAttribute('data-filter-expression-config-json-exported') === 'true' &&
	      document.querySelector('[data-filter-expression-match-count]')?.getAttribute('data-record-count') === '1',
	    'filter expression JSON exported'
	  );
	  mark('data-form-view-filter-expression-export-config-json');
	  document.querySelector('[data-filter-expression-copy-summary]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-expression-copy-summary]')?.getAttribute('data-filter-expression-summary-copied') === 'true' &&
	      document.querySelector('[data-filter-expression-match-count]')?.getAttribute('data-record-count') === '1',
	    'filter expression summary copied'
	  );
	  mark('data-form-view-filter-expression-copy-summary');
	  document.querySelector('[data-filter-expression-export-summary]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-expression-export-summary]')?.getAttribute('data-filter-expression-summary-exported') ===
	        'true' &&
	      document.querySelector('[data-filter-expression-match-count]')?.getAttribute('data-record-count') === '1',
	    'filter expression summary exported'
	  );
	  mark('data-form-view-filter-expression-export-summary');
	  document.querySelector('[data-filter-expression-focus-records]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-record-focus="record-order-line-created"]') &&
	      document.querySelector('[data-filter-expression-focus-records]')?.getAttribute('data-record-count') === '1',
	    'filter expression matching records focused'
	  );
	  mark('data-form-view-filter-expression-focus-records');
	  document.querySelector('[data-record-focus-clear]')?.click();
	  await waitFor(() => !document.querySelector('[data-record-focus]'), 'filter expression focused records cleared');
	  document.querySelector('[data-filter-expression-export-csv]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-expression-export-csv]')?.getAttribute('data-filter-expression-exported') === 'true' &&
	      document.querySelector('[data-filter-expression-export-csv]')?.getAttribute('data-record-count') === '1',
	    'filter expression matching records exported'
	  );
	  mark('data-form-view-filter-expression-export-csv');
	  document.querySelector('[data-filter-expression-copy-matches-csv]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-filter-expression-copy-matches-csv]')
	        ?.getAttribute('data-filter-expression-matches-csv-copied') === 'true' &&
	      document.querySelector('[data-filter-expression-copy-matches-csv]')?.getAttribute('data-record-count') === '1',
	    'filter expression matching records CSV copied'
	  );
	  mark('data-form-view-filter-expression-copy-matches-csv');
	  document.querySelector('[data-filter-expression-copy-matches-json]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-filter-expression-copy-matches-json]')
	        ?.getAttribute('data-filter-expression-matches-json-copied') === 'true' &&
	      document.querySelector('[data-filter-expression-copy-matches-json]')?.getAttribute('data-record-count') === '1',
	    'filter expression matching records JSON copied'
	  );
	  mark('data-form-view-filter-expression-copy-matches-json');
	  document.querySelector('[data-filter-expression-export-json]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-expression-export-json]')?.getAttribute('data-filter-expression-json-exported') === 'true' &&
	      document.querySelector('[data-filter-expression-export-json]')?.getAttribute('data-record-count') === '1',
	    'filter expression matching records exported json'
	  );
	  mark('data-form-view-filter-expression-export-json');
	  mark('data-form-view-filter-condition-duplicated');
	  const moveSecondFilterUp = document.querySelector('[data-filter-move-up="1.2"]');
	  if (!moveSecondFilterUp) throw new Error('filter move up control not found');
	  moveSecondFilterUp.click();
	  await waitFor(
	    () => labelControl('Filter field').value === 'quantity' && labelControl('Filter field 2').value === 'sku_name',
	    'filter condition reordered up'
	  );
	  const moveFirstFilterDown = document.querySelector('[data-filter-move-down="1.1"]');
	  if (!moveFirstFilterDown) throw new Error('filter move down control not found');
	  moveFirstFilterDown.click();
	  await waitFor(
	    () => labelControl('Filter field').value === 'sku_name' && labelControl('Filter field 2').value === 'quantity',
	    'filter condition reordered down'
	  );
	  const splitSecondFilter = document.querySelector('[data-filter-split-group="1.2"]');
	  if (!splitSecondFilter) throw new Error('filter split-to-group control not found');
	  splitSecondFilter.click();
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field').value === 'sku_name' && labelControl('Filter field 2.1').value === 'quantity';
	    } catch {
	      return false;
	    }
	  }, 'filter condition split to group');
	  mark('data-form-view-filter-condition-split-group');
	  const mergeSecondGroup = document.querySelector('[data-filter-group-merge-previous="2"]');
	  if (!mergeSecondGroup) throw new Error('filter group merge control not found');
	  mergeSecondGroup.click();
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field').value === 'sku_name' && labelControl('Filter field 2').value === 'quantity';
	    } catch {
	      return false;
	    }
	  }, 'filter group merged to previous');
	  mark('data-form-view-filter-group-merged');
	  setByLabel('Sort field', 'quantity');
	  setByLabel('Sort direction', 'desc');
	  setByLabel('Group field', 'status');
	  setByLabel('Visibility', 'private');
	  setCheckedByLabel('Default view', true);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('1 rows') && textInElements('Beef Noodles x2') && !textInElements('Green Tea x1'), 'saved view filter and sort');
	  mark('data-form-view-rules-filtered');
	  const toggleFirstFilterDisabled = document.querySelector('[data-filter-toggle-disabled="1.1"]');
	  if (!toggleFirstFilterDisabled) throw new Error('filter disable control not found');
	  toggleFirstFilterDisabled.click();
	  await waitFor(
	    () => document.querySelector('[data-filter-toggle-disabled="1.1"]')?.getAttribute('data-filter-disabled') === 'true',
	    'filter condition disabled in builder'
	  );
	  buttonExact('Save view').click();
	  await waitFor(
	    () => textInElements('2 rows') && textInElements('Beef Noodles x2') && textInElements('Green Tea x1'),
	    'disabled filter condition skipped'
	  );
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-match-count="1.1"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-filter-group-match-count="1"]')?.getAttribute('data-record-count') === '2',
	    'disabled filter match counts updated'
	  );
	  mark('data-form-view-filter-condition-disabled');
	  document.querySelector('[data-filter-toggle-disabled="1.1"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-filter-toggle-disabled="1.1"]')?.getAttribute('data-filter-disabled') === 'false',
	    'filter condition re-enabled in builder'
	  );
	  buttonExact('Save view').click();
	  await waitFor(
	    () => textInElements('1 rows') && textInElements('Beef Noodles x2') && !textInElements('Green Tea x1'),
	    're-enabled filter condition applied'
	  );
	  const duplicateFirstGroup = document.querySelector('[data-filter-group-duplicate="1"]');
	  if (!duplicateFirstGroup) throw new Error('filter group duplicate control not found');
	  duplicateFirstGroup.click();
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field 2.1').value === 'sku_name' && labelControl('Filter field 2.2').value === 'quantity';
	    } catch {
	      return false;
	    }
	  }, 'duplicated filter group condition');
	  const duplicateSecondGroupFilter = document.querySelector('[data-filter-duplicate="2.2"]');
	  if (!duplicateSecondGroupFilter) throw new Error('second group filter duplicate control not found');
	  duplicateSecondGroupFilter.click();
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field 2.3').value === 'quantity';
	    } catch {
	      return false;
	    }
	  }, 'extra duplicated filter condition');
	  const removeExtraFilter = document.querySelector('[data-filter-remove="2.3"]');
	  if (!removeExtraFilter) throw new Error('filter remove control not found');
	  removeExtraFilter.click();
	  await waitFor(() => {
	    try {
	      labelControl('Filter field 2.3');
	      return false;
	    } catch {
	      return labelControl('Filter field 2.2').value === 'quantity';
	    }
	  }, 'filter condition removed');
	  mark('data-form-view-filter-condition-removed');
	  buttonExact('Add group').click();
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field 3.1').value === '';
	    } catch {
	      return false;
	    }
	  }, 'temporary filter group added');
	  const removeThirdGroup = document.querySelector('[data-filter-group-remove="3"]');
	  if (!removeThirdGroup) throw new Error('filter group remove control not found');
	  removeThirdGroup.click();
	  await waitFor(() => {
	    try {
	      labelControl('Filter field 3.1');
	      return false;
	    } catch {
	      return labelControl('Filter field 2.1').value === 'sku_name';
	    }
	  }, 'filter group removed');
	  mark('data-form-view-filter-group-removed');
	  setByLabel('Filter groups', 'any');
	  setByLabel('Filter group 2', 'any');
	  setByLabel('Filter field 2.1', 'status');
	  setByLabel('Filter operator 2.1', 'equals');
	  setByLabel('Filter value 2.1', 'served');
	  setByLabel('Filter field 2.2', 'status');
	  setByLabel('Filter operator 2.2', 'not_equals');
	  setByLabel('Filter value 2.2', 'sent_to_kitchen');
	  await waitFor(() => {
	    try {
	      return labelControl('Filter field 2.2').value === 'status' &&
	        labelControl('Filter operator 2.2').value === 'not_equals' &&
	        labelControl('Filter value 2.2').value === 'sent_to_kitchen';
	    } catch {
	      return false;
	    }
	  }, 'negative filter condition edited');
	  await waitFor(
	    () =>
	      document.querySelector('[data-filter-expression-match-count]')?.getAttribute('data-record-count') === '2',
	    'root filter expression match count updated'
	  );
	  mark('data-form-view-filter-expression-match-count');
	  mark('data-form-view-filter-negative-condition');
	  mark('data-form-view-filter-group-duplicated');
	  const moveSecondGroupUp = document.querySelector('[data-filter-group-move-up="2"]');
	  if (!moveSecondGroupUp) throw new Error('filter group move up control not found');
	  moveSecondGroupUp.click();
	  await waitFor(() => labelControl('Filter field').value === 'status', 'filter group reordered up');
	  const moveFirstGroupDown = document.querySelector('[data-filter-group-move-down="1"]');
	  if (!moveFirstGroupDown) throw new Error('filter group move down control not found');
	  moveFirstGroupDown.click();
	  await waitFor(
	    () => labelControl('Filter field').value === 'sku_name' && labelControl('Filter field 2.1').value === 'status',
	    'filter group reordered down'
	  );
	  mark('data-form-view-filter-expression-reordered');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('2 rows') && textInElements('Beef Noodles x2') && textInElements('Green Tea x1'), 'saved view nested filter expression');
	  mark('data-form-view-rules-nested-filtered');
	  buttonExact('Export current view').click();
	  await waitFor(() => textInElements('Records exported'), 'current view export');
	  mark('data-form-view-exported');
	  document.querySelector('[data-current-view-export-json]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-current-view-export-json]')?.getAttribute('data-current-view-json-record-count') === '2',
	    'current view JSON export'
	  );
	  mark('data-form-view-json-exported');
	  document.querySelector('[data-attachment-manifest-export]')?.click();
	  await waitFor(
	    () => Number(document.querySelector('[data-attachment-manifest-export]')?.getAttribute('data-attachment-manifest-count') ?? '0') > 0,
	    'attachment manifest exported'
	  );
	  mark('data-form-attachment-manifest-export');
	  document.querySelector('[data-attachment-bundle-export]')?.click();
	  await waitFor(
	    () => Number(document.querySelector('[data-attachment-bundle-export]')?.getAttribute('data-attachment-bundle-count') ?? '0') > 0,
	    'attachment bundle exported'
	  );
	  mark('data-form-attachment-bundle-export');
	  document.querySelector('[data-attachment-package-export]')?.click();
	  await waitFor(
	    () =>
	      Number(document.querySelector('[data-attachment-package-export]')?.getAttribute('data-attachment-package-count') ?? '0') > 0 &&
	      Number(document.querySelector('[data-attachment-package-export]')?.getAttribute('data-attachment-package-file-count') ?? '0') > 0,
	    'attachment package exported'
	  );
	  mark('data-form-attachment-package-export');
	  if (mode === 'attachment-package-export') {
	    document.body.setAttribute('data-forms-ui-smoke', 'done');
	    return;
	  }
	  buttonExact('Import records').click();
	  await waitFor(() => document.querySelector('[data-import-template-export]'), 'import records modal');
	  document.querySelector('[data-import-template-export]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-import-template-export]')?.getAttribute('data-import-template-exported') ===
	      'true',
	    'import template exported'
	  );
	  mark('data-form-import-template-export');
	  const importFileInput = document.querySelector('[data-import-file-upload]');
	  if (!importFileInput) throw new Error('import file input not found');
	  const importFile = new File(['Record Name,SKU Label,Qty\\nImported from file, tea ,1\\n'], 'order-lines-import.csv', {
	    type: 'text/csv'
	  });
	  const importFileTransfer = new DataTransfer();
	  importFileTransfer.items.add(importFile);
	  importFileInput.files = importFileTransfer.files;
	  importFileInput.dispatchEvent(new Event('change', { bubbles: true }));
	  await waitFor(
	    () =>
	      document.querySelector('[data-import-file-upload]')?.getAttribute('data-import-file-loaded') === 'true' &&
	      document.querySelector('[data-import-file-upload]')?.getAttribute('data-import-file-name') ===
	        'order-lines-import.csv' &&
	      document.querySelector('#form-import-text')?.value.includes('Imported from file'),
	    'import file loaded'
	  );
	  mark('data-form-import-file-load');
	  const mappingTitle = document.querySelector('[data-import-mapping-column="0"]');
	  const mappingSkuName = document.querySelector('[data-import-mapping-column="1"]');
	  const mappingQuantity = document.querySelector('[data-import-mapping-column="2"]');
	  const skuTransform = document.querySelector('[data-import-mapping-transform="1"]');
	  if (!mappingTitle || !mappingSkuName || !mappingQuantity || !skuTransform) {
	    throw new Error('import mapping controls not found');
	  }
	  mappingTitle.value = '__title';
	  mappingTitle.dispatchEvent(new Event('input', { bubbles: true }));
	  mappingTitle.dispatchEvent(new Event('change', { bubbles: true }));
	  mappingSkuName.value = 'sku_name';
	  mappingSkuName.dispatchEvent(new Event('input', { bubbles: true }));
	  mappingSkuName.dispatchEvent(new Event('change', { bubbles: true }));
	  mappingQuantity.value = 'quantity';
	  mappingQuantity.dispatchEvent(new Event('input', { bubbles: true }));
	  mappingQuantity.dispatchEvent(new Event('change', { bubbles: true }));
	  skuTransform.value = 'uppercase';
	  skuTransform.dispatchEvent(new Event('input', { bubbles: true }));
	  skuTransform.dispatchEvent(new Event('change', { bubbles: true }));
	  document.querySelector('[data-import-mapping-template-save]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-import-mapping-wizard]')?.getAttribute('data-import-mapping-template-saved') ===
	        'true',
	    'import mapping template saved'
	  );
	  mappingSkuName.value = '__skip';
	  mappingSkuName.dispatchEvent(new Event('input', { bubbles: true }));
	  mappingSkuName.dispatchEvent(new Event('change', { bubbles: true }));
	  skuTransform.value = 'lowercase';
	  skuTransform.dispatchEvent(new Event('input', { bubbles: true }));
	  skuTransform.dispatchEvent(new Event('change', { bubbles: true }));
	  document.querySelector('[data-import-mapping-template-restore]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-import-mapping-wizard]')?.getAttribute('data-import-mapping-template-restored') ===
	        'true' &&
	      document.querySelector('[data-import-mapping-column="1"]')?.value === 'sku_name' &&
	      document.querySelector('[data-import-mapping-transform="1"]')?.value === 'uppercase',
	    'import mapping template restored'
	  );
	  mark('data-form-import-mapping-template');
	  document.querySelector('[data-import-mapping-apply]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-import-mapping-wizard]')?.getAttribute('data-import-mapping-applied') ===
	        'true' &&
	      document.querySelector('#form-import-text')?.value.includes('"Title","sku_name","quantity"') &&
	      document.querySelector('#form-import-text')?.value.includes('"Imported from file","TEA","1"'),
	    'import mapping applied'
	  );
	  mark('data-form-import-mapping-applied');
	  buttonExact('Cancel').click();
	  setByLabel('Current view', 'view-order-line-detail');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="detail"]') &&
	      document.querySelector('[data-detail-view-section="record-order-line-created:handoff"]')?.textContent?.includes('Kitchen Handoff') &&
	      document.querySelector('[data-detail-view-field="record-order-line-created:handoff:sku_name"]')?.textContent?.includes('Beef Noodles') &&
	      document.querySelector('[data-detail-view-field="record-order-line-created:pricing:line_total"]')?.textContent?.includes('19.98 CNY'),
	    'detail view layout renderer'
	  );
	  [...document.querySelectorAll('[data-view-renderer="detail"] button')]
	    .find((button) => button.textContent?.includes('Beef Noodles x2'))
	    ?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-record-detail-layout="record-order-line-created"]') &&
	      new URL(location.href).searchParams.get('form') === 'form-order-line' &&
	      new URL(location.href).searchParams.get('record') === 'record-order-line-created' &&
	      document.querySelector('[data-record-detail-url-record="record-order-line-created"]') &&
	      document.querySelector('[data-record-detail-section="record-order-line-created:handoff"]')?.textContent?.includes('Kitchen Handoff') &&
	      document.querySelector('[data-record-detail-field="record-order-line-created:handoff:status"]')?.textContent?.includes('sent_to_kitchen') &&
	      document.querySelector('[data-record-detail-field="record-order-line-created:pricing:line_total"]')?.textContent?.includes('19.98 CNY'),
	    'record detail layout renderer'
	  );
	  mark('data-form-detail-view-rendered');
	  mark('data-form-record-detail-layout-rendered');
	  mark('data-form-record-detail-url-state');
	  buttonExact('Data').click();
	  await waitFor(() => document.querySelector('[data-view-renderer="detail"]'), 'detail view restored after record detail layout');
	  setByLabel('Current view', 'view-order-line-pivot');
	  await waitFor(() => document.querySelector('[data-view-renderer="pivot"]'), 'pivot view renderer');
	  setByLabel('Pivot row field', 'seat_no');
	  setByLabel('Pivot column field', 'status');
	  setByLabel('Pivot value field', 'line_total');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot view saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-cell="1:sent_to_kitchen"]')?.textContent?.trim() === '19.98 CNY' &&
	      document.querySelector('[data-pivot-cell="2:served"]')?.textContent?.trim() === '5.00 CNY' &&
	      document.querySelector('[data-pivot-grand-total]')?.textContent?.trim() === '24.98 CNY',
	    'pivot aggregation rendered'
	  );
	  mark('data-form-pivot-view-rendered');
	  document.querySelector('[data-pivot-copy-table]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-copy-table]')?.getAttribute('data-pivot-table-copied') === 'true' &&
	      document.querySelector('[data-pivot-cell="1:sent_to_kitchen"]')?.textContent?.trim() === '19.98 CNY' &&
	      document.querySelector('[data-pivot-grand-total]')?.textContent?.trim() === '24.98 CNY',
	    'pivot table matrix copied'
	  );
	  mark('data-form-pivot-copy-table');
	  document.querySelector('[data-pivot-export-table]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-export-table]')?.getAttribute('data-pivot-table-exported') === 'true' &&
	      document.querySelector('[data-pivot-cell="1:sent_to_kitchen"]')?.textContent?.trim() === '19.98 CNY' &&
	      document.querySelector('[data-pivot-grand-total]')?.textContent?.trim() === '24.98 CNY',
	    'pivot table matrix exported'
	  );
	  mark('data-form-pivot-export-table');
	  setByLabel('Pivot sort', 'row_total_asc');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot sort view saved');
	  await waitFor(
	    () => {
	      const rows = Array.from(document.querySelectorAll('[data-pivot-row]')).map((row) =>
	        row.getAttribute('data-pivot-row')
	      );
	      return (
	        document.querySelector('[data-pivot-sort="row_total_asc"]') &&
	        rows[0] === '2' &&
	        rows[1] === '1'
	      );
	    },
	    'pivot row total sort rendered'
	  );
	  mark('data-form-pivot-row-total-sort');
	  setByLabel('Pivot aggregate', 'count');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot aggregate view saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-aggregate="count"]') &&
	      document.querySelector('[data-pivot-cell="1:sent_to_kitchen"]')?.textContent?.trim() === '1' &&
	      document.querySelector('[data-pivot-cell="2:served"]')?.textContent?.trim() === '1' &&
	      document.querySelector('[data-pivot-grand-total]')?.textContent?.trim() === '2',
	    'pivot count aggregation rendered'
		  );
		  mark('data-form-pivot-aggregate-count');
	  document.querySelector('[data-pivot-row-total-button="1"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-drilldown="1:Total"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-drilldown-record="record-order-line-created"]'),
	    'pivot row total drilldown rendered'
	  );
	  document.querySelector('[data-pivot-column-total-button="served"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-drilldown="Total:served"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-drilldown-record="record-order-line-comparison"]'),
	    'pivot column total drilldown rendered'
	  );
	  mark('data-form-pivot-total-drilldown');
		  document.querySelector('[data-pivot-cell-button="1:sent_to_kitchen"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-drilldown="1:sent_to_kitchen"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-drilldown-record="record-order-line-created"]') &&
	      document.querySelector('[data-pivot-drilldown-copy-ids]') &&
	      document.querySelector('[data-pivot-drilldown-export-csv]') &&
	      document.querySelector('[data-pivot-drilldown-apply-filters]') &&
	      document.querySelector('[data-pivot-drilldown-open="record-order-line-created"]'),
	    'pivot drilldown rendered'
	  );
	  mark('data-form-pivot-drilldown-rendered');
	  document.querySelector('[data-pivot-drilldown-copy-ids]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-pivot-drilldown="1:sent_to_kitchen"]')
	        ?.getAttribute('data-pivot-drilldown-copied') === '1:sent_to_kitchen',
	    'pivot drilldown copy record ids'
	  );
	  mark('data-form-pivot-drilldown-copy-record-ids');
	  document.querySelector('[data-pivot-drilldown-export-csv]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-pivot-drilldown-export-csv]')
	        ?.getAttribute('data-pivot-drilldown-exported') === 'true' &&
	      document.querySelector('[data-pivot-drilldown="1:sent_to_kitchen"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-drilldown-record="record-order-line-created"]'),
	    'pivot drilldown records exported'
	  );
	  mark('data-form-pivot-drilldown-export-csv');
	  document.querySelector('[data-pivot-drilldown-open="record-order-line-created"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-record-detail-layout="record-order-line-created"]') &&
	      textInElements('Record links') &&
	      textInElements('Beef Noodles x2'),
	    'pivot drilldown open source record'
	  );
	  mark('data-form-pivot-drilldown-open-record');
	  buttonExact('Data').click();
	  await waitFor(() => document.querySelector('[data-view-renderer="pivot"]'), 'pivot view restored after drilldown open record');
	  document.querySelector('[data-pivot-cell-button="1:sent_to_kitchen"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-drilldown="1:sent_to_kitchen"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-drilldown-focus]'),
	    'pivot drilldown restored after open record'
	  );
	  document.querySelector('[data-pivot-drilldown-focus]')?.click();
	  await waitFor(
	    () => {
	      const grid = document.querySelector('[data-view-renderer="grid"]');
	      return grid &&
	      document.querySelector('[data-record-focus="record-order-line-created"]') &&
	      document.querySelectorAll('[data-view-renderer="grid"] tbody tr').length === 1 &&
	      grid.textContent.includes('Beef Noodles x2') &&
	      !grid.textContent.includes('Green Tea x1');
	    },
	    'pivot drilldown focused records'
	  );
	  mark('data-form-pivot-drilldown-focus');
	  document.querySelector('[data-record-focus-clear]')?.click();
	  await waitFor(() => !document.querySelector('[data-record-focus]'), 'pivot drilldown focused records cleared');
	  setByLabel('Current view', 'view-order-line-pivot');
	  await waitFor(() => document.querySelector('[data-view-renderer="pivot"]'), 'pivot view restored after drilldown focus');
	  document.querySelector('[data-pivot-swap-axes]')?.click();
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot swapped axes saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-row-field="status"]') &&
	      document.querySelector('[data-pivot-column-field="seat_no"]') &&
	      document.querySelector('[data-pivot-cell="sent_to_kitchen:1"]')?.textContent?.trim() === '1' &&
	      document.querySelector('[data-pivot-cell="served:2"]')?.textContent?.trim() === '1',
	    'pivot swapped axes rendered'
	  );
	  mark('data-form-pivot-swap-axes');
	  setByLabel('Pivot totals', 'none');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot totals view saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-totals="none"]') &&
	      document.querySelector('[data-pivot-cell="sent_to_kitchen:1"]')?.textContent?.trim() === '1' &&
	      !document.querySelector('[data-pivot-row-total]') &&
	      !document.querySelector('[data-pivot-column-total]') &&
	      !document.querySelector('[data-pivot-grand-total]'),
	    'pivot totals hidden'
	  );
	  mark('data-form-pivot-totals-none');
	  setByLabel('Pivot value display', 'percent_total');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot value display view saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-value-display="percent_total"]') &&
	      document.querySelector('[data-pivot-cell="sent_to_kitchen:1"]')?.textContent?.trim() === '50.00%' &&
	      document.querySelector('[data-pivot-cell="served:2"]')?.textContent?.trim() === '50.00%',
	    'pivot percent total rendered'
	  );
	  mark('data-form-pivot-value-display-percent-total');
	  setCheckedByLabel('Show empty buckets', true);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot empty buckets saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-empty-buckets="true"]') &&
	      document.querySelector('[data-pivot-row="cancelled"]') &&
	      document.querySelector('[data-pivot-row="draft"]') &&
	      document.querySelector('[data-pivot-cell="cancelled:1"]')?.textContent?.trim() === '0.00%' &&
	      document.querySelector('[data-pivot-cell="draft:2"]')?.textContent?.trim() === '0.00%',
	    'pivot empty buckets rendered'
	  );
	  mark('data-form-pivot-empty-buckets');
	  setCheckedByLabel('Show cell counts', true);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot show counts saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-show-counts="true"]') &&
	      document.querySelector('[data-pivot-cell-count="sent_to_kitchen:1"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-cell-count="served:2"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-cell-count="cancelled:1"]')?.getAttribute('data-record-count') === '0',
	    'pivot cell counts rendered'
	  );
	  mark('data-form-pivot-cell-counts');
	  setCheckedByLabel('Cell heatmap', true);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot heatmap saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-heatmap="true"]') &&
	      document.querySelector('[data-pivot-cell-button="sent_to_kitchen:1"]')?.getAttribute('data-pivot-cell-intensity') === '100' &&
	      document.querySelector('[data-pivot-cell-button="served:2"]')?.getAttribute('data-pivot-cell-intensity') === '100' &&
	      document.querySelector('[data-pivot-cell-button="cancelled:1"]')?.getAttribute('data-pivot-cell-intensity') === '0',
	    'pivot heatmap rendered'
	  );
	  mark('data-form-pivot-heatmap');
	  setCheckedByLabel('Hide zero buckets', true);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot hide zero buckets saved');
	  await waitFor(
	    () => {
	      const rows = Array.from(document.querySelectorAll('[data-pivot-row]')).map((row) =>
	        row.getAttribute('data-pivot-row')
	      );
	      return (
	        document.querySelector('[data-pivot-hide-zero-buckets="true"]') &&
	        rows.includes('sent_to_kitchen') &&
	        rows.includes('served') &&
	        !rows.includes('cancelled') &&
	        !rows.includes('draft') &&
	        document.querySelector('[data-pivot-cell="sent_to_kitchen:1"]') &&
	        document.querySelector('[data-pivot-cell="served:2"]')
	      );
	    },
	    'pivot hide zero buckets rendered'
		  );
		  mark('data-form-pivot-hide-zero-buckets');
	  document.querySelector('[data-pivot-cell-copy="sent_to_kitchen:1"]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-pivot-cell-copy="sent_to_kitchen:1"]')
	        ?.getAttribute('data-pivot-cell-copied') === 'true' &&
	      document.querySelector('[data-pivot-cell="sent_to_kitchen:1"]')?.textContent?.includes('50.00%'),
	    'pivot cell value copied'
	  );
	  mark('data-form-pivot-copy-cell-value');
		  setByLabel('Pivot sort', 'row_total_desc');
	  setByLabel('Pivot row limit', '2');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot row limit saved');
	  await waitFor(
	    () => {
	      const rows = Array.from(document.querySelectorAll('[data-pivot-row]')).map((row) =>
	        row.getAttribute('data-pivot-row')
	      );
	      return (
	        document.querySelector('[data-pivot-row-limit="2"]') &&
	        rows.length === 2 &&
	        rows.includes('sent_to_kitchen') &&
	        rows.includes('served') &&
	        !rows.includes('cancelled') &&
	        !rows.includes('draft')
	      );
	    },
	    'pivot row limit rendered'
	  );
	  mark('data-form-pivot-row-limit');
	  setByLabel('Pivot row field', 'seat_no');
	  await waitFor(() => labelControl('Pivot row field').value === 'seat_no', 'pivot column limit row field selected');
	  await waitFor(() => labelControl('Pivot column field').value === '', 'pivot column field cleared after row change');
	  setByLabel('Pivot column field', 'status');
	  await waitFor(() => labelControl('Pivot column field').value === 'status', 'pivot column limit column field selected');
	  setByLabel('Pivot sort', 'column_total_desc');
	  setByLabel('Pivot row limit', '0');
	  setByLabel('Pivot column limit', '2');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot column limit saved');
	  await waitFor(
	    () => {
	      const columns = Array.from(document.querySelectorAll('[data-pivot-column]')).map((column) =>
	        column.getAttribute('data-pivot-column')
	      );
	      return (
	        document.querySelector('[data-pivot-column-limit="2"]') &&
	        columns.length === 2 &&
	        columns.includes('sent_to_kitchen') &&
	        columns.includes('served') &&
	        !columns.includes('cancelled') &&
	        !columns.includes('draft') &&
	        document.querySelector('[data-pivot-cell="1:sent_to_kitchen"]') &&
	        document.querySelector('[data-pivot-cell="2:served"]')
	      );
	    },
	    'pivot column limit rendered'
	  );
	  mark('data-form-pivot-column-limit');
	  document.querySelector('[data-pivot-cell-button="1:sent_to_kitchen"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-drilldown="1:sent_to_kitchen"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-pivot-drilldown-apply-filters]'),
	    'pivot drilldown restored before filter apply'
	  );
	  document.querySelector('[data-pivot-drilldown-apply-filters]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-pivot-drilldown-apply-filters]')
	        ?.getAttribute('data-pivot-drilldown-filters-applied') === 'true' &&
	      labelControl('Filter field 1').value === 'seat_no' &&
	      labelControl('Filter operator 1').value === 'equals' &&
	      labelControl('Filter value 1').value === '1' &&
	      labelControl('Filter field 2').value === 'status' &&
	      labelControl('Filter operator 2').value === 'equals' &&
	      labelControl('Filter value 2').value === 'sent_to_kitchen',
	    'pivot drilldown bucket filters applied to draft'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot drilldown bucket filters saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-cell="1:sent_to_kitchen"]')?.textContent?.includes('100.00%') &&
	      document.querySelector('[data-pivot-cell-count="1:sent_to_kitchen"]')?.getAttribute('data-record-count') === '1' &&
	      !document.querySelector('[data-pivot-row="2"]') &&
	      !document.querySelector('[data-pivot-column="served"]'),
	    'pivot drilldown bucket filters rendered'
	  );
	  mark('data-form-pivot-drilldown-apply-filters');
	  setByLabel('Pivot row field', 'vip_note');
	  setByLabel('Pivot column field', 'status');
	  setByLabel('Pivot value field', 'line_total');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'pivot conditional indicator config saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-pivot-row-field-conditional-hidden="vip_note"]') &&
	      document.querySelector('[data-pivot-table-row-field-conditional-hidden="vip_note"]') &&
	      document.querySelector('[data-pivot-row-conditional-hidden$=":vip_note"]') &&
	      document.querySelector('[data-pivot-cell-row-conditional-hidden$=":vip_note"]'),
	    'pivot conditional field indicators'
	  );
	  mark('data-form-pivot-conditional-indicators');
	  setByLabel('Current view', 'view-order-line-gantt');
	  await waitFor(() => document.querySelector('[data-view-renderer="gantt"]'), 'gantt view renderer');
	  setByLabel('Gantt start field', 'service_date');
	  setByLabel('Gantt end field', 'service_end_date');
	  setByLabel('Gantt group field', 'status');
	  setByLabel('Gantt dependency field', 'order_ref');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'gantt view saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-gantt-start-field="service_date"]') &&
	      document.querySelector('[data-gantt-end-field="service_end_date"]') &&
	      document.querySelector('[data-gantt-group-field="status"]') &&
	      document.querySelector('[data-gantt-dependency-field="order_ref"]') &&
	      document.querySelector('[data-gantt-date="2026-05-30"]') &&
	      document.querySelector('[data-gantt-date="2026-05-31"]') &&
	      document.querySelector('[data-gantt-lane="sent_to_kitchen"]') &&
	      document.querySelector('[data-gantt-lane="served"]') &&
	      document.querySelector('[data-gantt-bar="record-order-line-created"]')?.getAttribute('data-gantt-bar-start') === '2026-05-31' &&
	      document.querySelector('[data-gantt-bar="record-order-line-created"]')?.getAttribute('data-gantt-bar-end') === '2026-05-31' &&
	      document.querySelector('[data-gantt-bar="record-order-line-created"]')?.getAttribute('data-gantt-bar-dependency')?.includes('Order 1') &&
	      document.querySelector('[data-gantt-dependency="record-order-line-created"]')?.textContent?.includes('Order 1') &&
	      document.querySelector('[data-gantt-bar="record-order-line-comparison"]')?.getAttribute('data-gantt-bar-start') === '2026-05-30',
	    'gantt timeline rendered'
	  );
	  dragGanttBarToDate('record-order-line-created', '2026-05-30');
	  await waitFor(() => textInElements('Gantt date updated'), 'gantt drag date update');
	  await waitFor(
	    () =>
	      document.querySelector('[data-gantt-bar="record-order-line-created"]')?.getAttribute('data-gantt-bar-start') === '2026-05-30' &&
	      document.querySelector('[data-gantt-bar="record-order-line-created"]')?.getAttribute('data-gantt-bar-end') === '2026-05-30',
	    'gantt bar date updated'
	  );
	  mark('data-form-gantt-date-updated');
	  document.querySelector('[data-gantt-resize-end="record-order-line-created"]')?.click();
	  await waitFor(() => textInElements('Gantt end date updated'), 'gantt resize end update');
	  await waitFor(
	    () =>
	      document.querySelector('[data-gantt-bar="record-order-line-created"]')?.getAttribute('data-gantt-bar-end') === '2026-05-31',
	    'gantt bar end resized'
	  );
	  mark('data-form-gantt-resize-end');
	  setByLabel('Gantt scale', 'week');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'gantt scale saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-gantt-scale="week"]') &&
	      document.querySelector('[data-gantt-date="2026-05-25"]') &&
	      document.querySelector('[data-gantt-bar="record-order-line-created"]')?.getAttribute('data-gantt-bar-start') === '2026-05-30',
	    'gantt scale rendered'
	  );
	  mark('data-form-gantt-scale');
	  setByLabel('Gantt dependency field', 'guest_signature');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'gantt conditional indicator config saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-gantt-dependency-field-conditional-readonly="guest_signature"]') &&
	      document.querySelector('[data-gantt-task-dependency-conditional-readonly="record-order-line-created:guest_signature"]'),
	    'gantt conditional field indicators'
	  );
	  mark('data-form-gantt-conditional-indicators');
	  mark('data-form-gantt-dependency-field');
	  mark('data-form-gantt-view-rendered');
	  setByLabel('Current view', 'view-order-line-kanban');
	  await waitFor(() => document.querySelector('[data-view-renderer="kanban"]') && textInElements('sent_to_kitchen') && textInElements('Beef Noodles x2'), 'kanban view renderer');
	  await waitFor(() => document.querySelector('[data-kanban-column="cancelled"]'), 'kanban empty option column');
	  await waitFor(() => {
	    try {
	      return Boolean(labelControl('Swimlane field')) && Boolean(labelControl('WIP limit served'));
	    } catch {
	      return false;
	    }
	  }, 'kanban WIP policy controls');
	  setByLabel('Swimlane field', 'seat_no');
	  setByLabel('WIP limit served', '1');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'kanban WIP policy saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-kanban-swimlane="1"]') &&
	      document.querySelector('[data-kanban-swimlane="2"]'),
	    'kanban swimlanes rendered'
	  );
	  await waitFor(() => textInElements('1/1 WIP'), 'kanban WIP policy applied');
	  dragKanbanCardToColumn('record-order-line-created', 'served');
	  await waitFor(() => textInElements('WIP limit reached for this column'), 'kanban WIP limit');
	  dragKanbanCardToColumn('record-order-line-created', 'cancelled');
	  await waitFor(() => textInElements('Kanban status updated'), 'kanban drag status update');
	  setCheckedByAria('Select kanban card: Beef Noodles x2', true);
	  setCheckedByAria('Select kanban card: Green Tea x1', true);
	  await waitFor(() => textInElements('2 selected'), 'kanban bulk selection');
	  setByLabel('Move selected to Status', 'served');
	  await waitFor(() => buttonExact('Move selected')?.disabled === true, 'kanban bulk WIP disabled');
	  setByLabel('Move selected to Status', 'draft');
	  await waitFor(() => buttonExact('Move selected')?.disabled === false, 'kanban bulk draft enabled');
	  buttonExact('Move selected').click();
	  await waitFor(() => textInElements('Moved 2 records'), 'kanban bulk move');
	  mark('data-form-kanban-bulk-move');
	  mark('data-form-kanban-view-rendered');
	  setByLabel('Current view', 'view-order-line-calendar');
	  await waitFor(() => document.querySelector('[data-view-renderer="calendar"]') && textInElements('Beef Noodles x2'), 'calendar view renderer');
	  setByLabel('Calendar range', 'month');
	  setByLabel('Calendar end field', 'service_end_date');
	  setByLabel('Calendar resource field', 'quantity');
	  setByLabel('Calendar recurrence', 'daily');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar range saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-range="month"]') &&
	      document.querySelector('[data-calendar-resource="1"]') &&
	      document.querySelector('[data-calendar-resource="2"]') &&
	      document.querySelector('[data-calendar-date="2026-05-01"]') &&
	      document.querySelector('[data-calendar-date="2026-05-31"]') &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-30"]')?.getAttribute('data-calendar-end') === '2026-05-31',
	    'calendar month resource grid'
	  );
	  dragCalendarCardToDate('record-order-line-created', '2026-05-29');
	  await waitFor(() => textInElements('Calendar date updated'), 'calendar drag date update');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-29"]')?.getAttribute('data-calendar-end') === '2026-05-30' &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-29:2026-05-30"]')?.getAttribute('data-calendar-start') === '2026-05-29',
	    'calendar multi-day span shifted'
	  );
	  const calendarEndDateInput = document.querySelector('[data-calendar-end-date="record-order-line-created:2026-05-29"]');
	  if (!calendarEndDateInput) throw new Error('calendar end date input not found');
	  calendarEndDateInput.value = '2026-05-31';
	  calendarEndDateInput.dispatchEvent(new Event('change', { bubbles: true }));
	  await waitFor(() => textInElements('Calendar end date updated'), 'calendar end date update');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-29"]')?.getAttribute('data-calendar-end') === '2026-05-31' &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-29:2026-05-31"]')?.getAttribute('data-calendar-start') === '2026-05-29',
	    'calendar end date resized span'
	  );
	  mark('data-form-calendar-end-date-resized');
		  await waitFor(
		    () =>
		      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-30"]') &&
		      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-31"]'),
		    'calendar recurring occurrences'
		  );
		  setByLabel('Calendar exception record', 'record-order-line-created');
		  setByLabel('Calendar exception date', '2026-05-30');
		  await waitFor(() => buttonExact('Add exception')?.disabled === false, 'calendar exception add enabled');
		  buttonExact('Add exception').click();
		  buttonExact('Save view').click();
		  await waitFor(() => textInElements('View saved'), 'calendar exception saved');
		  await waitFor(
		    () =>
		      !document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-30"]') &&
		      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-31"]'),
		    'calendar recurring exception applied'
		  );
		  mark('data-form-calendar-exception');
		  const seriesDateInput = document.querySelector('[data-calendar-series-date="record-order-line-created:2026-05-31"]');
		  if (!seriesDateInput) throw new Error('calendar recurring series input not found');
	  seriesDateInput.value = '2026-05-17';
	  seriesDateInput.dispatchEvent(new Event('change', { bubbles: true }));
	  await waitFor(() => textInElements('Calendar series updated'), 'calendar recurring series update');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]') &&
	      !document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-30"]') &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-31"]'),
	    'calendar recurring series shifted'
	  );
	  setByLabel('Calendar recurrence', 'weekly');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar weekly recurrence saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]') &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-24"]') &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-31"]') &&
	      !document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-18"]'),
	    'calendar weekly recurring occurrences'
	  );
	  setByLabel('Calendar recurrence', 'monthly');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar monthly recurrence saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]') &&
	      !document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-24"]'),
	    'calendar monthly recurrence current range'
	  );
	  document.querySelector('[data-calendar-focus-shift="1"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-calendar-focus-date-control]')?.value === '2026-06-01',
	    'calendar next range focus draft'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar focus date saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-focus-date="2026-06-01"]') &&
	      document.querySelector('[data-calendar-date="2026-06-01"]') &&
	      document.querySelector('[data-calendar-date="2026-06-30"]') &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-06-17"]') &&
	      !document.querySelector('[data-calendar-date="2026-05-17"]'),
	    'calendar focus date next range rendered'
	  );
	  document.querySelector('[data-calendar-focus-shift="-1"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-calendar-focus-date-control]')?.value === '2026-05-01',
	    'calendar previous range focus draft'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar previous focus date saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-focus-date="2026-05-01"]') &&
	      document.querySelector('[data-calendar-date="2026-05-17"]') &&
	      !document.querySelector('[data-calendar-date="2026-06-17"]'),
	    'calendar focus date previous range rendered'
	  );
	  mark('data-form-calendar-focus-navigation');
	  setByLabel('Calendar days', 'workweek');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar workweek saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-day-scope="workweek"]') &&
	      document.querySelector('[data-calendar-date="2026-05-18"]') &&
	      !document.querySelector('[data-calendar-date="2026-05-17"]') &&
	      !document.querySelector('[data-calendar-date="2026-05-24"]'),
	    'calendar workweek rendered'
	  );
	  mark('data-form-calendar-workweek');
	  setByLabel('Calendar days', 'all');
	  setByLabel('Calendar layout', 'agenda');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar agenda saved');
		  await waitFor(
		    () =>
		      document.querySelector('[data-calendar-layout="agenda"]') &&
		      document.querySelector('[data-calendar-agenda-resource="1"]') &&
	      document.querySelector('[data-calendar-agenda-resource="2"]') &&
	      document.querySelector('[data-calendar-agenda-date="2026-05-17"]') &&
	      document.querySelector('[data-calendar-agenda-occurrence="record-order-line-created:2026-05-17"]')?.getAttribute('data-calendar-end') === '2026-05-19' &&
	      !document.querySelector('[data-calendar-date="2026-05-17"]'),
		    'calendar agenda layout rendered'
		  );
		  mark('data-form-calendar-agenda-layout');
		  setByLabel('Calendar layout', 'grid');
		  setCheckedByLabel('Hide empty days', true);
		  buttonExact('Save view').click();
		  await waitFor(() => textInElements('View saved'), 'calendar hide empty days saved');
		  await waitFor(
		    () =>
		      document.querySelector('[data-calendar-layout="grid"]') &&
		      document.querySelector('[data-calendar-hide-empty-days="true"]') &&
		      document.querySelector('[data-calendar-date="2026-05-17"]') &&
		      !document.querySelector('[data-calendar-date="2026-05-20"]'),
		    'calendar hide empty days rendered'
		  );
		  mark('data-form-calendar-hide-empty-days');
	  document.querySelector('[data-calendar-copy-schedule="record-order-line-created:2026-05-17"]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]')
	        ?.getAttribute('data-calendar-schedule-copied') === 'true',
	    'calendar schedule copied'
	  );
	  mark('data-form-calendar-copy-schedule');
	  document.querySelector('[data-calendar-shift-next="record-order-line-created:2026-05-17"]')?.click();
	  await waitFor(() => textInElements('Calendar schedule moved'), 'calendar shift next toast');
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-18"]')?.getAttribute('data-calendar-end') === '2026-05-20' &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-18"]')?.getAttribute('data-calendar-shifted') === 'true' &&
	      !document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]'),
	    'calendar occurrence shifted next day'
	  );
	  document.querySelector('[data-calendar-shift-previous="record-order-line-created:2026-05-18"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]')?.getAttribute('data-calendar-end') === '2026-05-19' &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]')?.getAttribute('data-calendar-shifted') === 'true' &&
	      !document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-18"]'),
	    'calendar occurrence shifted previous day'
	  );
	  mark('data-form-calendar-shift-occurrence');
	  document.querySelector('[data-calendar-apply-filters="2026-05-17:2"]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-calendar-apply-filters="2026-05-17:2"]')
	        ?.getAttribute('data-calendar-filters-applied') === 'true' &&
	      labelControl('Filter field 1').value === 'service_date' &&
	      labelControl('Filter operator 1').value === 'equals' &&
	      labelControl('Filter value 1').value === '2026-05-17' &&
	      labelControl('Filter field 2').value === 'quantity' &&
	      labelControl('Filter operator 2').value === 'equals' &&
	      labelControl('Filter value 2').value === '2',
	    'calendar bucket filters applied to draft'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'calendar bucket filters saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="calendar"]') &&
	      document.querySelector('[data-calendar-resource="2"]') &&
	      !document.querySelector('[data-calendar-resource="1"]') &&
	      document.querySelector('[data-calendar-date="2026-05-17"]') &&
	      document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]'),
	    'calendar bucket filters rendered'
	  );
	  const calendarBucketFiltered =
	    document.querySelector('[data-calendar-resource="2"]') &&
	    !document.querySelector('[data-calendar-resource="1"]') &&
	    document.querySelector('[data-calendar-date="2026-05-17"]') &&
	    document.querySelector('[data-calendar-occurrence="record-order-line-created:2026-05-17"]');
	  if (!calendarBucketFiltered) {
	    const calendarDebug = {
	      resources: Array.from(document.querySelectorAll('[data-calendar-resource]')).map((item) => item.getAttribute('data-calendar-resource')),
	      dates: Array.from(document.querySelectorAll('[data-calendar-date]')).map((item) => item.getAttribute('data-calendar-date')),
	      occurrences: Array.from(document.querySelectorAll('[data-calendar-occurrence]')).map((item) => item.getAttribute('data-calendar-occurrence'))
	    };
	    throw new Error('calendar bucket filters rendered: ' + JSON.stringify(calendarDebug));
	  }
	  mark('data-form-calendar-apply-bucket-filters');
		  mark('data-form-calendar-series-updated');
	  mark('data-form-calendar-end-field');
	  mark('data-form-calendar-weekly-monthly-recurrence');
	  mark('data-form-calendar-view-rendered');
	  setByLabel('Current view', 'view-order-line-card');
	  await waitFor(() => document.querySelector('[data-view-renderer="card"]') && textInElements('Beef Noodles x2') && textInElements('19.98 CNY'), 'card view renderer');
	  setByLabel('Group field', 'status');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card group field saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-group-field="status"]') &&
	      document.querySelector('[data-card-group="draft"]')?.getAttribute('data-record-count') === '2' &&
	      document.querySelector('[data-card-group="cancelled"]')?.getAttribute('data-record-count') === '0',
	    'card grouped sections rendered'
	  );
	  mark('data-form-card-group-sections');
	  document.querySelector('[data-card-group-copy-record-ids="status:draft"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-card-group="draft"]')?.getAttribute('data-card-group-record-ids-copied') === 'true',
	    'card group record ids copied'
	  );
	  mark('data-form-card-group-copy-record-ids');
	  document.querySelector('[data-card-group-export-records="status:draft"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-card-group="draft"]')?.getAttribute('data-card-group-records-exported') === 'true',
	    'card group records exported'
	  );
	  mark('data-form-card-group-export-records');
	  document.querySelector('[data-card-group-focus-records="status:draft"]')?.click();
	  await waitFor(
	    () => {
	      const focusedIds = new Set((document.querySelector('[data-record-focus]')?.getAttribute('data-record-focus') ?? '').split('|'));
	      return (
	        document.querySelector('[data-view-renderer="grid"]') &&
	        focusedIds.has('record-order-line-created') &&
	        focusedIds.has('record-order-line-comparison') &&
	        document.querySelectorAll('[data-view-renderer="grid"] tbody tr').length === 2 &&
	        textInElements('Beef Noodles x2') &&
	        textInElements('Green Tea x1')
	      );
	    },
	    'card group focused records'
	  );
	  mark('data-form-card-group-focus-records');
	  document.querySelector('[data-record-focus-clear]')?.click();
	  await waitFor(() => !document.querySelector('[data-record-focus]'), 'card group focused records cleared');
	  setByLabel('Current view', 'view-order-line-card');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="card"]') &&
	      document.querySelector('[data-card-group-field="status"]') &&
	      document.querySelector('[data-card-group="draft"]')?.getAttribute('data-record-count') === '2',
	    'card grouped sections restored after focus'
	  );
	  const draftCardGroupToggle = document.querySelector('[data-card-group-toggle="draft"]');
	  if (!draftCardGroupToggle) throw new Error('card group collapse control not found');
	  draftCardGroupToggle.click();
	  await waitFor(() => {
	    const group = document.querySelector('[data-card-group="draft"]');
	    return (
	      group?.getAttribute('data-card-group-collapsed') === 'true' &&
	      !group.querySelector('[data-card-title="record-order-line-created"]')
	    );
	  }, 'card group collapsed');
	  document.querySelector('[data-card-group-toggle="draft"]')?.click();
	  await waitFor(() => {
	    const group = document.querySelector('[data-card-group="draft"]');
	    return (
	      group?.getAttribute('data-card-group-collapsed') === 'false' &&
	      group.querySelector('[data-card-title="record-order-line-created"]')
	    );
	  }, 'card group expanded');
	  mark('data-form-card-group-collapse');
	  setByLabel('Group field', 'seat_no');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card seat group field saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-group-field="seat_no"]') &&
	      document.querySelector('[data-card-group="1"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-card-group="2"]')?.getAttribute('data-record-count') === '1',
	    'card seat grouped sections rendered'
	  );
	  document.querySelector('[data-card-group-apply-filter="seat_no:1"]')?.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-card-group-apply-filter="seat_no:1"]')
	        ?.getAttribute('data-card-group-filter-applied') === 'true' &&
	      labelControl('Filter field 1').value === 'seat_no' &&
	      labelControl('Filter operator 1').value === 'equals' &&
	      labelControl('Filter value 1').value === '1',
	    'card group filter applied to draft'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card group filter saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-group-field="seat_no"]') &&
	      document.querySelector('[data-card-group="1"]')?.getAttribute('data-record-count') === '1' &&
	      document.querySelector('[data-card-title="record-order-line-created"]') &&
	      !document.querySelector('[data-card-title="record-order-line-comparison"]'),
	    'card group filter rendered'
	  );
	  mark('data-form-card-group-apply-filter');
	  setByLabel('Card title field', 'sku_name');
	  setByLabel('Card subtitle field', 'seat_no');
	  setByLabel('Card badge field', 'status');
	  setByLabel('Card cover field', 'dish_photo');
	  setByLabel('Card layout', 'gallery');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card layout saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-layout="gallery"]') &&
	      document.querySelector('[data-card-title="record-order-line-created"]')?.textContent?.trim() === 'Beef Noodles' &&
	      document.querySelector('[data-card-subtitle="record-order-line-created"]')?.textContent?.trim() === '1' &&
	      document.querySelector('[data-card-badge="record-order-line-created"]')?.textContent?.trim() === 'draft' &&
	      document.querySelector('[data-card-badge="record-order-line-created"] [data-option-color="status:draft"]')?.getAttribute('data-option-color-hex') === '#64748b' &&
	      document.querySelector('[data-card-cover="record-order-line-created"]')?.getAttribute('src') ===
	        'https://assets.example.test/beef-noodles.jpg',
	    'card configured title and subtitle'
	  );
	  setByLabel('Card cover aspect', 'portrait');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card cover aspect saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-layout="gallery"]') &&
	      document.querySelector('[data-card-cover-aspect="portrait"]') &&
	      document
	        .querySelector('[data-card-cover="record-order-line-created"]')
	        ?.className.includes('aspect-[3/4]'),
	    'card cover aspect rendered'
	  );
	  mark('data-form-card-cover-aspect');
	  setByLabel('Card cover fit', 'contain');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card cover fit saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-layout="gallery"]') &&
	      document.querySelector('[data-card-cover-fit="contain"]') &&
	      document
	        .querySelector('[data-card-cover="record-order-line-created"]')
	        ?.className.includes('object-contain'),
	    'card cover fit rendered'
	  );
	  mark('data-form-card-cover-fit');
	  setByLabel('Card cover position', 'left');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card cover position left saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-cover-position="left"]') &&
	      document
	        .querySelector('[data-card-cover="record-order-line-created"]')
	        ?.getAttribute('data-card-cover-position') === 'left' &&
	      document
	        .querySelector('[data-card-cover="record-order-line-created"]')
	        ?.className.includes('h-24'),
	    'card cover position left rendered'
	  );
	  mark('data-form-card-cover-position-left');
	  setByLabel('Card cover position', 'hidden');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card cover position hidden saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-cover-position="hidden"]') &&
	      !document.querySelector('[data-card-cover="record-order-line-created"]'),
	    'card cover position hidden rendered'
	  );
	  mark('data-form-card-cover-position-hidden');
	  setByLabel('Card cover position', 'top');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card cover position restored');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-cover-position="top"]') &&
	      document
	        .querySelector('[data-card-cover="record-order-line-created"]')
	        ?.getAttribute('data-card-cover-position') === 'top',
	    'card cover position top restored'
	  );
	  setByLabel('Card layout', 'compact');
	  await waitFor(() => labelControl('Card layout').value === 'compact', 'card compact layout selected');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card compact layout saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-layout="compact"]') &&
	      document.querySelector('[data-card-compact-row="record-order-line-created"]') &&
	      document.querySelector('[data-card-title="record-order-line-created"]')?.textContent?.trim() === 'Beef Noodles' &&
	      document.querySelector('[data-card-cover="record-order-line-created"]')?.getAttribute('src') ===
	        'https://assets.example.test/beef-noodles.jpg',
	    'card compact layout rendered'
	  );
	  mark('data-form-card-compact-layout');
	  setByLabel('Card layout', 'list');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card list layout saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-layout="list"]') &&
	      document.querySelector('[data-card-list-row="record-order-line-created"]') &&
	      document.querySelector('[data-card-title="record-order-line-created"]')?.textContent?.trim() === 'Beef Noodles' &&
	      !document.querySelector('[data-card-cover="record-order-line-created"]'),
	    'card list layout rendered'
	  );
	  mark('data-form-card-list-layout');
	  setCheckedByAria('Visible columns: Dish Photo', true);
	  setByLabel('Card fields', '99');
	  setCheckedByLabel('Hide empty fields', true);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card hide empty fields saved');
		  await waitFor(
		    () =>
		      document.querySelector('[data-card-hide-empty-fields="true"]') &&
		      document.querySelector('[data-card-field="record-order-line-created:dish_photo"]')?.textContent?.includes('https://assets.example.test/beef-noodles.jpg') &&
		      !document.querySelector('[data-card-field="record-order-line-comparison:dish_photo"]'),
		    'card hide empty fields rendered'
		  );
		  mark('data-form-card-hide-empty-fields');
		  setCheckedByLabel('Show timestamps', true);
		  buttonExact('Save view').click();
		  await waitFor(() => textInElements('View saved'), 'card timestamps saved');
		  await waitFor(
		    () =>
		      document.querySelector('[data-card-timestamps="record-order-line-created"]') &&
		      document.querySelector('[data-card-created-at="record-order-line-created"]')?.textContent?.includes('Created') &&
		      document.querySelector('[data-card-updated-at="record-order-line-created"]')?.textContent?.includes('Updated'),
		    'card timestamps rendered'
		  );
		  mark('data-form-card-show-timestamps');
		  setCheckedByLabel('Show record ID', true);
		  buttonExact('Save view').click();
		  await waitFor(() => textInElements('View saved'), 'card record id saved');
		  await waitFor(
		    () =>
		      document.querySelector('[data-card-show-record-id="true"]') &&
		      document.querySelector('[data-card-record-id="record-order-line-created"]')?.textContent?.includes('record-order-line-created'),
		    'card record id rendered'
		  );
		  mark('data-form-card-show-record-id');
		  setByLabel('Card layout', 'compact');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card compact layout restored');
	  document.querySelector('[data-card-menu="record-order-line-created"] summary')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-action="record-order-line-created:open"]') &&
	      document.querySelector('[data-card-action="record-order-line-created:copy-id"]') &&
	      document.querySelector('[data-card-action="record-order-line-created:export-record"]') &&
	      document.querySelector('[data-card-action="record-order-line-created:delete"]'),
	    'card action menu'
	  );
	  document.querySelector('[data-card-action="record-order-line-created:copy-id"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-card-copied="record-order-line-created"]'),
	    'card copy record id'
	  );
	  document.querySelector('[data-card-action="record-order-line-created:export-record"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-card-exported="record-order-line-created"]'),
	    'card export record'
	  );
	  mark('data-form-card-export-record');
	  mark('data-form-card-action-menu');
	  setCardSelect('record-order-line-created', 'status', 'served');
	  await waitFor(() => textInElements('Card field updated'), 'card status update');
	  await waitFor(
	    () => document.querySelector('[data-card-badge="record-order-line-created"]')?.textContent?.trim() === 'served',
	    'card badge updated after status change'
	  );
	  mark('data-form-card-status-updated');
	  const moveQuantityCardFieldUp = document.querySelector('[data-card-field-move-up="quantity"]');
	  if (!moveQuantityCardFieldUp) throw new Error('card field order control not found');
	  moveQuantityCardFieldUp.click();
	  await waitFor(
	    () =>
	      document
	        .querySelector('[data-card-field-order^="1:"]')
	        ?.getAttribute('data-card-field-order') === '1:quantity',
	    'card field order updated in rules'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card field order saved');
	  await waitFor(
	    () =>
	      Array.from(document.querySelectorAll('[data-card-field^="record-order-line-created:"]'))[0]?.getAttribute('data-card-field') ===
	      'record-order-line-created:quantity',
	    'card field order rendered'
	  );
	  mark('data-form-card-field-order');
	  setByLabel('Card fields', '3');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'card field limit saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-card-field-limit="3"]') &&
	      document.querySelectorAll('[data-card-field^="record-order-line-created:"]').length === 3,
	    'card field limit rendered'
	  );
	  mark('data-form-card-field-limit');
	  mark('data-form-card-view-rendered');
	  setByLabel('Current view', 'view-order-line-timeline');
	  await waitFor(() => document.querySelector('[data-view-renderer="timeline"]') && textInElements('Beef Noodles x2'), 'timeline view renderer');
	  setByLabel('Timeline title field', 'sku_name');
	  setByLabel('Timeline subtitle field', 'seat_no');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline layout saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-title="record-order-line-created"]')?.textContent?.trim() === 'Beef Noodles' &&
	      document.querySelector('[data-timeline-subtitle="record-order-line-created"]')?.textContent?.trim() === '1',
	    'timeline configured title and subtitle'
	  );
	  setTimelineSelect('record-order-line-created', 'status', 'sent_to_kitchen');
	  await waitFor(() => textInElements('Timeline field updated'), 'timeline status update');
	  mark('data-form-timeline-status-updated');
	  setByLabel('Timeline source', 'events');
	  setByLabel('Timeline event layout', 'detailed');
	  setByLabel('Timeline event type', 'form.record.created');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event source saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-source="events"]') &&
	      document.querySelector('[data-timeline-event-layout="detailed"]') &&
	      document.querySelector('[data-timeline-event-type-filter="form.record.created"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]') &&
	      document.querySelector('[data-timeline-event-type="form.record.created"]') &&
	      !document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]') &&
	      document.querySelector('[data-timeline-event-title="event-order-line-created"]')?.textContent?.trim() === 'Beef Noodles x2',
	    'timeline event source renderer and filter'
	  );
	  document.querySelector('[data-timeline-copy-event-ids]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-view-renderer="timeline"]')?.getAttribute('data-timeline-event-ids-copied') === '1',
	    'timeline filtered event ids copied'
	  );
	  mark('data-form-timeline-event-copy-filtered-ids');
	  document.querySelector('[data-timeline-copy-aggregate-ids]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-view-renderer="timeline"]')?.getAttribute('data-timeline-aggregate-ids-copied') === '1',
	    'timeline filtered aggregate ids copied'
	  );
	  mark('data-form-timeline-event-copy-filtered-aggregate-ids');
	  document.querySelector('[data-timeline-export-events-json]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-view-renderer="timeline"]')?.getAttribute('data-timeline-events-json-exported') === '1',
	    'timeline filtered events json exported'
	  );
	  mark('data-form-timeline-event-export-json');
	  document.querySelector('[data-timeline-export-events-csv]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-view-renderer="timeline"]')?.getAttribute('data-timeline-events-exported') === '1',
	    'timeline filtered events exported'
	  );
	  mark('data-form-timeline-event-export-csv');
	  document.querySelector('[data-timeline-focus-event-records]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="grid"]') &&
	      document.querySelector('[data-record-focus="record-order-line-created"]') &&
	      document.querySelectorAll('[data-view-renderer="grid"] tbody tr').length === 1 &&
	      textInElements('Beef Noodles x2') &&
	      !textInElements('Green Tea x1'),
	    'timeline filtered events focused linked records'
	  );
	  mark('data-form-timeline-event-focus-filtered-records');
	  document.querySelector('[data-record-focus-clear]')?.click();
	  await waitFor(() => !document.querySelector('[data-record-focus]'), 'timeline filtered event records focus cleared');
	  setByLabel('Current view', 'view-order-line-timeline');
	  await waitFor(() => document.querySelector('[data-view-renderer="timeline"]'), 'timeline view restored after filtered event focus');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-detail="event-order-line-created"]')?.open === true &&
	      document.querySelector('[data-timeline-event-payload="event-order-line-created"]')?.textContent?.includes('"status"') &&
	      document.querySelector('[data-timeline-event-metadata="event-order-line-created"]')?.textContent?.includes('"form_key"') &&
	      document.querySelector('[data-timeline-event-source-detail="event-order-line-created"]')?.textContent?.includes('forms-ui-smoke'),
	    'timeline event drill-down'
	  );
	  document.querySelector('[data-timeline-event-menu="event-order-line-created"] summary')?.click();
		  await waitFor(
		    () =>
		      document.querySelector('[data-timeline-event-action="event-order-line-created:open-record"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:focus-record"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:pin-event"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:filter-type"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:filter-actor"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:filter-aggregate"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:filter-day"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:copy-event-id"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:copy-aggregate"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:copy-details"]') &&
		      document.querySelector('[data-timeline-event-action="event-order-line-created:export-details"]'),
		    'timeline event action menu'
		  );
		  document.querySelector('[data-timeline-event-action="event-order-line-created:pin-event"]')?.click();
		  await waitFor(
		    () =>
		      document
		        .querySelector('[data-timeline-event="event-order-line-created"]')
		        ?.getAttribute('data-timeline-event-pinned') === 'true' &&
		      document
		        .querySelector('[data-view-renderer="timeline"]')
		        ?.getAttribute('data-timeline-event-pinned-ids') === 'event-order-line-created',
		    'timeline event pinned'
		  );
		  if (!document.querySelector('[data-timeline-event-action="event-order-line-created:unpin-event"]')) {
		    document.querySelector('[data-timeline-event-menu="event-order-line-created"] summary')?.click();
		    await waitFor(
		      () => document.querySelector('[data-timeline-event-action="event-order-line-created:unpin-event"]'),
		      'timeline event unpin action menu'
		    );
		  }
		  document.querySelector('[data-timeline-event-action="event-order-line-created:unpin-event"]')?.click();
		  await waitFor(
		    () =>
		      document
		        .querySelector('[data-timeline-event="event-order-line-created"]')
		        ?.getAttribute('data-timeline-event-pinned') === 'false' &&
		      document
		        .querySelector('[data-view-renderer="timeline"]')
		        ?.getAttribute('data-timeline-event-pinned-ids') === '',
		    'timeline event unpinned'
		  );
		  mark('data-form-timeline-event-pin');
		  if (!document.querySelector('[data-timeline-event-action="event-order-line-created:copy-event-id"]')) {
		    document.querySelector('[data-timeline-event-menu="event-order-line-created"] summary')?.click();
		    await waitFor(
		      () => document.querySelector('[data-timeline-event-action="event-order-line-created:copy-event-id"]'),
		      'timeline event copy action menu'
		    );
		  }
		  document.querySelector('[data-timeline-event-action="event-order-line-created:copy-event-id"]')?.click();
		  await waitFor(
		    () =>
		      document
		        .querySelector('[data-timeline-event-menu="event-order-line-created"]')
		        ?.getAttribute('data-timeline-event-copied') === 'event-order-line-created:event-id',
		    'timeline event copy id'
		  );
		  document.querySelector('[data-timeline-event-action="event-order-line-created:copy-details"]')?.click();
		  await waitFor(
		    () =>
		      document
		        .querySelector('[data-timeline-event-menu="event-order-line-created"]')
		        ?.getAttribute('data-timeline-event-copied') === 'event-order-line-created:details',
		    'timeline event copy details'
		  );
		  mark('data-form-timeline-event-copy-details');
		  document.querySelector('[data-timeline-event-action="event-order-line-created:export-details"]')?.click();
		  await waitFor(
		    () =>
		      document
		        .querySelector('[data-timeline-event-menu="event-order-line-created"]')
		        ?.getAttribute('data-timeline-event-exported') === 'event-order-line-created:details',
		    'timeline event export details'
		  );
		  mark('data-form-timeline-event-export-details');
		  document.querySelector('[data-timeline-event-action="event-order-line-created:focus-record"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="grid"]') &&
	      document.querySelector('[data-record-focus="record-order-line-created"]') &&
	      document.querySelectorAll('[data-view-renderer="grid"] tbody tr').length === 1 &&
	      textInElements('Beef Noodles x2') &&
	      !textInElements('Green Tea x1'),
	    'timeline event focused linked record'
	  );
	  mark('data-form-timeline-event-focus-record');
	  document.querySelector('[data-record-focus-clear]')?.click();
	  await waitFor(() => !document.querySelector('[data-record-focus]'), 'timeline event focused record cleared');
	  setByLabel('Current view', 'view-order-line-timeline');
	  await waitFor(() => document.querySelector('[data-view-renderer="timeline"]'), 'timeline view restored after event focus');
	  document.querySelector('[data-timeline-event-menu="event-order-line-created"] summary')?.click();
	  await waitFor(
	    () => document.querySelector('[data-timeline-event-action="event-order-line-created:filter-aggregate"]'),
	    'timeline event aggregate quick filter action menu'
	  );
	  document.querySelector('[data-timeline-event-action="event-order-line-created:filter-aggregate"]')?.click();
	  await waitFor(
	    () => labelControl('Timeline event aggregate').value === 'form_record:record-order-line-created',
	    'timeline event aggregate quick filter applied'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event aggregate filter saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-aggregate-filter="form_record:record-order-line-created"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]') &&
	      !document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]'),
	    'timeline event aggregate filter'
	  );
	  mark('data-form-timeline-event-aggregate-filter');
	  setByLabel('Timeline event aggregate', '');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event aggregate filter cleared');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-aggregate-filter=""]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]'),
	    'timeline event aggregate filter cleared renderer'
	  );
	  document.querySelector('[data-timeline-event-menu="event-order-line-created"] summary')?.click();
	  await waitFor(
	    () => document.querySelector('[data-timeline-event-action="event-order-line-created:open-record"]'),
	    'timeline event action menu restored'
	  );
	  document.querySelector('[data-timeline-event-action="event-order-line-created:open-record"]')?.click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-record-detail-layout="record-order-line-created"]') &&
	      textInElements('Record links'),
	    'timeline event open linked record'
	  );
	  buttonExact('Data').click();
	  await waitFor(() => document.querySelector('[data-view-renderer="timeline"]'), 'timeline data view restored after event action');
	  setByLabel('Timeline event type', '');
	  setByLabel('Timeline event search', 'Green Tea');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event search saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-query="Green Tea"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]') &&
	      !document.querySelector('[data-timeline-event="event-order-line-created"]'),
	    'timeline event search filter'
	  );
	  mark('data-form-timeline-event-search-filter');
	  setByLabel('Timeline event search', '');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event search cleared');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-query=""]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]'),
	    'timeline event search cleared renderer'
	  );
	  const timelineEventFrom = dateTimeLocal(smokeNow);
	  const timelineEventTo = dateTimeLocal(new Date(Date.parse(smokeNow) + 70_000).toISOString());
	  setByLabel('Timeline event from', timelineEventFrom);
	  setByLabel('Timeline event to', timelineEventTo);
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event date window saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-from="' + timelineEventFrom + '"]') &&
	      document.querySelector('[data-timeline-event-to="' + timelineEventTo + '"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]') &&
	      !document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]'),
	    'timeline event date window filter'
	  );
	  mark('data-form-timeline-event-date-window-filter');
	  setByLabel('Timeline event from', '');
	  setByLabel('Timeline event to', '');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event date window cleared');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-from=""]') &&
	      document.querySelector('[data-timeline-event-to=""]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]'),
	    'timeline event date window cleared renderer'
	  );
	  document.querySelector('[data-timeline-event-menu="event-order-line-created"] summary')?.click();
	  await waitFor(
	    () => document.querySelector('[data-timeline-event-action="event-order-line-created:filter-day"]'),
	    'timeline event day quick filter action menu'
	  );
	  document.querySelector('[data-timeline-event-action="event-order-line-created:filter-day"]')?.click();
	  await waitFor(
	    () =>
	      labelControl('Timeline event from').value === timelineEventDayWindowFrom &&
	      labelControl('Timeline event to').value === timelineEventDayWindowTo,
	    'timeline event day quick filter applied'
	  );
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event day quick filter saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-from="' + timelineEventDayWindowFrom + '"]') &&
	      document.querySelector('[data-timeline-event-to="' + timelineEventDayWindowTo + '"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]'),
	    'timeline event day quick filter renderer'
	  );
	  mark('data-form-timeline-event-day-filter');
	  setByLabel('Timeline event from', '');
	  setByLabel('Timeline event to', '');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event day quick filter cleared');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-from=""]') &&
	      document.querySelector('[data-timeline-event-to=""]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]'),
	    'timeline event day quick filter cleared renderer'
	  );
	  document.querySelector('[data-timeline-event-menu="event-order-line-created"] summary')?.click();
	  await waitFor(
	    () => document.querySelector('[data-timeline-event-action="event-order-line-created:filter-actor"]'),
	    'timeline event quick filter action menu'
	  );
	  document.querySelector('[data-timeline-event-action="event-order-line-created:filter-actor"]')?.click();
	  await waitFor(() => labelControl('Timeline event actor').value === 'user-smoke', 'timeline event actor quick filter applied');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline event actor filter saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-timeline-event-actor-filter="user-smoke"]') &&
	      document.querySelector('[data-timeline-event="event-order-line-created"]') &&
	      !document.querySelector('[data-timeline-event="event-order-line-comparison-updated"]'),
	    'timeline event actor filter'
	  );
	  mark('data-form-timeline-event-quick-filter-actor');
	  mark('data-form-timeline-event-actor-filter');
	  mark('data-form-timeline-event-source');
	  mark('data-form-timeline-event-drill-down');
	  mark('data-form-timeline-event-actions');
	  mark('data-form-timeline-view-rendered');
	  setByLabel('Current view', 'view-order-line-default');
	  await waitFor(() => document.querySelector('[data-view-renderer="grid"]') && textInElements('Beef Noodles x2'), 'grid view renderer restored');
	  buttonExact('Permissions').click();
	  await waitFor(() => {
	    try {
	      return Boolean(labelControl('Record scope')) &&
	        textInElements('Current role: member') &&
	        document.querySelector('[data-field-read-permission="unit_price"]')?.checked === true &&
	        document.querySelector('[data-field-write-permission="quantity"]')?.checked === true &&
	        document.querySelector('[data-field-read-denied-count="0"]') &&
	        document.querySelector('[data-field-write-denied-count="0"]') &&
	        document.querySelector('[data-permission-effect-hidden-fields]')?.getAttribute('data-permission-effect-hidden-fields') === '' &&
	        document.querySelector('[data-permission-effect-locked-fields]')?.getAttribute('data-permission-effect-locked-fields') === '';
	    } catch {
	      return false;
	    }
	  }, 'permissions record scope controls');
	  const unitPriceReadPermission = document.querySelector('[data-field-read-permission="unit_price"]');
	  if (!unitPriceReadPermission) throw new Error('unit_price field read permission control missing');
	  if (unitPriceReadPermission.checked) unitPriceReadPermission.click();
	  const quantityWritePermission = document.querySelector('[data-field-write-permission="quantity"]');
	  if (!quantityWritePermission) throw new Error('quantity field write permission control missing');
	  if (quantityWritePermission.checked) quantityWritePermission.click();
	  setByLabel('Record scope', 'owned');
	  buttonExact('Save permissions').click();
	  await waitFor(() =>
	    textInElements('Permissions saved') &&
	    textInElements('Owned records only') &&
	    document.querySelector('[data-field-read-denied-count="1"]') &&
	    document.querySelector('[data-field-write-denied-count="2"]') &&
	    document.querySelector('[data-permission-effect-hidden-fields]')?.getAttribute('data-permission-effect-hidden-fields') === 'Unit Price' &&
	    document.querySelector('[data-permission-effect-locked-fields]')?.getAttribute('data-permission-effect-locked-fields') === 'Quantity|Unit Price' &&
	    permissionStateMatches('permissions'),
	    'record ownership scope saved'
	  );
	  const scopedList = await fetch('/api/v1/forms/form-order-line/records?view_id=view-order-line-default').then((response) => response.json());
	  const scopedListIds = (scopedList?.data?.items ?? []).map((record) => record.id);
	  if (scopedListIds.length !== 1 || scopedListIds[0] !== 'record-order-line-created') {
	    throw new Error('record ownership scope did not filter list records: ' + JSON.stringify(scopedListIds));
	  }
	  if ('unit_price' in (scopedList?.data?.items?.[0]?.values ?? {})) {
	    throw new Error('field read policy did not hide unit_price from list records');
	  }
	  const scopedExport = await fetch('/api/v1/forms/form-order-line/records/export?view_id=view-order-line-default').then((response) => response.json());
	  const scopedExportIds = (scopedExport?.data?.rows ?? []).map((row) => row.record_id);
	  if (scopedExportIds.length !== 1 || scopedExportIds[0] !== 'record-order-line-created') {
	    throw new Error('record ownership scope did not filter export records: ' + JSON.stringify(scopedExportIds));
	  }
	  if ((scopedExport?.data?.columns ?? []).some((column) => column.key === 'unit_price')) {
	    throw new Error('field read policy did not remove unit_price from export columns');
	  }
	  if ('unit_price' in (scopedExport?.data?.rows?.[0]?.values ?? {})) {
	    throw new Error('field read policy did not remove unit_price from export row values');
	  }
	  const deniedUpdate = await fetch('/api/v1/form-records/record-order-line-comparison', {
	    method: 'PATCH',
	    headers: { 'content-type': 'application/json' },
	    body: JSON.stringify({ values: { sku_name: 'Green Tea', quantity: '1', status: 'served', seat_no: '9' } })
	  }).then((response) => response.json());
	  if (deniedUpdate.code !== 403) {
	    throw new Error('record ownership scope did not deny other-user update: ' + JSON.stringify(deniedUpdate));
	  }
	  const deniedFieldUpdate = await fetch('/api/v1/form-records/record-order-line-created', {
	    method: 'PATCH',
	    headers: { 'content-type': 'application/json' },
	    body: JSON.stringify({ values: { sku_name: 'Beef Noodles', quantity: '3', status: 'served', seat_no: '12' } })
	  }).then((response) => response.json());
	  if (deniedFieldUpdate.code !== 403) {
	    throw new Error('field write policy did not deny locked field update: ' + JSON.stringify(deniedFieldUpdate));
	  }
	  mark('data-form-record-ownership-scope');
	  buttonExact('Data').click();
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="grid"]') &&
	      permissionStateMatches('data') &&
	      textInElements('Beef Noodles x2') &&
	      document.querySelector('[data-grid-header-permission-hidden="unit_price"]') &&
	      document.querySelector('[data-grid-header-permission-locked="unit_price"]') &&
	      document.querySelector('[data-grid-header-permission-locked="quantity"]') &&
	      document.querySelector('[data-view-column-permission-hidden="unit_price"]') &&
	      document.querySelector('[data-view-column-permission-locked="unit_price"]') &&
	      document.querySelector('[data-view-column-permission-locked="quantity"]'),
	    'data mode restored with grid and view field permission indicators'
		  );
		  mark('data-form-permission-state-data');
	  setByLabel('Current view', 'view-order-line-kanban');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="kanban"]') &&
	      document.querySelector('[data-kanban-card-field-permission-locked="record-order-line-created:quantity"]'),
	    'kanban view permission indicators'
	  );
	  setByLabel('Current view', 'view-order-line-calendar');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="calendar"]') &&
	      document.querySelector('[data-calendar-resource-field-permission-locked="quantity"]'),
	    'calendar view permission indicators'
	  );
	  setByLabel('Current view', 'view-order-line-card');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="card"]') &&
	      document.querySelector('[data-card-field-permission-locked="record-order-line-created:quantity"]'),
	    'card view permission indicators'
	  );
	  setByLabel('Current view', 'view-order-line-timeline');
	  setByLabel('Timeline source', 'records');
	  setByLabel('Timeline title field', 'quantity');
	  buttonExact('Save view').click();
	  await waitFor(() => textInElements('View saved'), 'timeline permission indicator config saved');
	  await waitFor(
	    () =>
	      document.querySelector('[data-view-renderer="timeline"]') &&
	      document.querySelector('[data-timeline-title-permission-locked="record-order-line-created:quantity"]'),
	    'timeline view permission indicators'
	  );
	  buttonExact('Detail').click();
  await waitFor(
	    () =>
	      textInElements('Record links') &&
	      permissionStateMatches('detail') &&
	      textInElements('Beef Noodles x2') &&
      textInElements('19.98 CNY') &&
      document.querySelector('[data-record-detail-permission-hidden="unit_price"]') &&
      document.querySelector('[data-record-detail-permission-locked="unit_price"]') &&
      document.querySelector('[data-record-detail-permission-locked="quantity"]') &&
	      document.querySelector('[data-record-detail-attachments="record-order-line-created"]')?.getAttribute('data-record-detail-attachment-count') === '2' &&
	      document.querySelector('[data-record-detail-attachment-field="attachment_12"]') &&
	      document.querySelector('[data-record-detail-attachment-field-count="attachment_12:2"]') &&
	      document.querySelector('[data-record-detail-attachment-item="attachment-3"]')?.textContent?.includes('detail-ticket.pdf') &&
	      document.querySelector('[data-record-detail-attachment-item="attachment-durable-detail"]')?.textContent?.includes('detail-ticket.pdf') &&
	      document.querySelector('[data-record-detail-attachment-url="attachment-3"]')?.textContent?.includes('https://assets.example.test/detail-ticket.pdf') &&
      document.querySelector('[data-record-detail-attachment-open="attachment-3"]')?.getAttribute('href') === '/api/v1/form-attachments/attachment-3/download?expires=1893456000&signature=signed-attachment-3' &&
      document.querySelector('[data-record-detail-attachment-download="attachment-3"]')?.getAttribute('download') === 'detail-ticket.pdf' &&
      document.querySelector('[data-record-detail-event-history="record-order-line-created"]')?.getAttribute('data-record-detail-event-count') === '1' &&
      document.querySelector('[data-record-detail-event="event-order-line-created"]') &&
      document.querySelector('[data-record-detail-event-kind="event-order-line-created"]')?.textContent?.includes('form.record.created') &&
      document.querySelector('[data-record-detail-event-title="event-order-line-created"]')?.textContent?.includes('Beef Noodles x2') &&
      document.querySelector('[data-record-detail-event-actor="event-order-line-created"]')?.textContent?.includes('user-smoke') &&
      document.querySelector('[data-record-detail-event-payload="event-order-line-created"]')?.textContent?.includes('"status"') &&
      document.querySelector('[data-record-detail-event-source="event-order-line-created"]')?.textContent?.includes('forms-ui-smoke'),
    'created record detail with amount and permission indicators'
  );
  mark('data-form-record-detail-attachment-widget');
  mark('data-form-record-detail-event-history');
  await waitFor(
    () =>
      document.querySelector('[data-child-table-grid="print_jobs"][data-child-table-relation-key="line_print_jobs"]') &&
      document.querySelector('[data-record-detail-child-table-widget="record-order-line-created:child_tables:print_jobs"]') &&
      document.querySelector('[data-record-detail-child-table-column="print_jobs:job_type"]')?.textContent?.includes('Job Type') &&
      document.querySelector('[data-record-detail-child-table-cell="print-job-kitchen:printer"]')?.textContent?.includes('kitchen-01') &&
      document.querySelector('[data-child-table-grid-column="print_jobs:job_type"]')?.textContent?.includes('Job Type') &&
      document.querySelector('[data-child-table-grid-column="print_jobs:status"]')?.textContent?.includes('Status') &&
      document.querySelector('[data-child-table-grid-cell="print-job-kitchen:job_type"]')?.textContent?.includes('kitchen') &&
      document.querySelector('[data-child-table-grid-cell="print-job-kitchen:printer"]')?.textContent?.includes('kitchen-01') &&
      document.querySelector('[data-child-table-edit-row="print_jobs:print-job-kitchen"]') &&
      document.querySelector('[data-child-table-delete-row="print_jobs:print-job-kitchen"]'),
    'child table field grid'
  );
  mark('data-form-child-table-grid');
  mark('data-form-detail-layout-child-table-widget');
  buttonExact('Edit').click();
  await waitFor(
    () =>
      document.querySelector('[data-record-edit-permission-hidden="unit_price"]') &&
      document.querySelector('[data-record-edit-permission-locked="unit_price"]') &&
      document.querySelector('[data-record-edit-permission-locked="quantity"]'),
    'record edit permission indicators'
  );
	  buttonExact('Design').click();
	  await waitFor(
	    () =>
	      permissionStateMatches('design') &&
	      document.querySelector('[data-designer-field-permission-hidden="unit_price"]') &&
      document.querySelector('[data-designer-field-permission-locked="unit_price"]') &&
      document.querySelector('[data-designer-field-permission-locked="quantity"]'),
    'designer canvas permission indicators'
  );
  document.querySelector('[data-designer-field-permission-locked="quantity"]')?.closest('[role="button"]')?.click();
	  await waitFor(
	    () => document.querySelector('[data-designer-property-permission-locked="quantity"]'),
	    'designer property permission indicators'
	  );
	  mark('data-form-permission-state-cross-mode');
  buttonExact('Detail').click();
  await waitFor(() => textInElements('Record links') && textInElements('Beef Noodles x2'), 'detail mode restored after edit indicator check');
  mark('data-form-record-detail');
  setByPlaceholder('Target record id', 'order-1');
  setByPlaceholder('relation_key', 'order_lines');
  setByPlaceholder('relation_type', 'parent_child');
  await waitFor(() => !buttonExact('Add link').disabled, 'add link enabled');
  buttonExact('Add link').click();
  await waitFor(() => textInElements('order_lines / parent_child'), 'linked child record');
  mark('data-form-child-link');
  document.body.setAttribute('data-forms-ui-smoke', 'done');
})().catch((error) => {
  document.body.setAttribute('data-forms-ui-smoke', 'failed');
  const message = error.message + smokeDebug();
  document.body.setAttribute('data-forms-ui-smoke-error', message);
  document.body.insertAdjacentHTML('beforeend', '<pre id="forms-smoke-error">' + message + '</pre>');
});
</script>`;
}

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const pathname = url.pathname;

	if (pathname === '/smoke-auth-seed') {
		const mode = url.searchParams.get('mode') ?? 'desktop';
		if (mode === 'durable-detail') applyMemberPermissionSmokePolicy();
		const extra = mode === 'detail-url' ? '&form=form-order-line&record=record-order-line-created' : '';
		const target =
			mode === 'durable-detail'
				? `/workspace/${workspaceId}/projects/${projectId}/forms/records/record-order-line-created?record_detail_smoke=${mode}`
				: `/workspace/${workspaceId}/projects/${projectId}/forms?forms_smoke=${mode}${extra}`;
		res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
		res.end(`<script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace(${JSON.stringify(target)});
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
						slug: 'forms-smoke',
						name: 'Forms Smoke',
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
					{
						user_id: 'user-smoke',
						workspace_id: workspaceId,
						email: 'smoke@example.com',
						name: 'Smoke Admin',
						role: 'owner',
						joined_at: now
					}
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
	if (pathname === `/api/v1/projects//forms`) {
			json(res, 200, apiResult(paginated([orderLineForm, printJobForm])));
			return;
		}
		if (pathname === `/api/v1/forms/${orderLineForm.id}/views`) {
			const visibleViews = orderLineViews
				.filter((view) => view.config?.visibility !== 'private' || view.created_by === 'user-smoke')
				.sort((left, right) => {
					if (Boolean(left.config?.is_default) === Boolean(right.config?.is_default)) return 0;
					return left.config?.is_default ? -1 : 1;
				});
			listedOrderLineViewIds = visibleViews.map((view) => view.id);
			json(
				res,
				200,
				apiResult(visibleViews)
			);
			return;
		}
		if (pathname === `/api/v1/forms/${orderLineForm.id}/field-usage`) {
		const fields = orderLineForm.schema.fields.map((field) => ({
			field_id: field.field_id ?? `fld_${field.key}`,
			field_key: field.key,
			field_type: field.type,
			value_count: field.field_id === 'fld_order_id' || field.field_id === 'fld_sku_name' ? 1 : 0,
			indexed_count: field.field_id === 'fld_order_id' || field.field_id === 'fld_sku_name' ? 1 : 0,
			dependency_count: field.field_id === 'fld_order_id' ? 1 : 0
		}));
		json(res, 200, apiResult({ form_id: orderLineForm.id, schema_version: orderLineForm.schema_version, fields }));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/field-dependencies`) {
		const orderField = orderLineForm.schema.fields.find((field) => field.field_id === 'fld_order_id');
		json(
			res,
			200,
			apiResult({
				form_id: orderLineForm.id,
				schema_version: orderLineForm.schema_version,
				dependencies: [
					{
						field_id: 'fld_order_id',
						field_key: orderField?.key ?? 'order_id',
						source_type: 'view',
						source_id: 'view-order-line-default',
						source_key: 'default_grid',
						reason: 'visible column',
						blocking: true
					}
				]
			})
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/events`) {
		json(res, 200, apiResult(paginated(orderLineEvents())));
		return;
	}
	const recordCommentsMatch = pathname.match(/^\/api\/v1\/form-records\/([^/]+)\/comments$/);
	if (recordCommentsMatch) {
		const recordId = decodeURIComponent(recordCommentsMatch[1]);
		if (req.method === 'POST') {
			const body = await readBody(req);
			createdRecordCommentPayloads.push(body);
			const comment = {
				id: `comment-record-created-${createdRecordCommentPayloads.length}`,
				workspace_id: workspaceId,
				project_id: projectId,
				form_id: orderLineForm.id,
				record_id: recordId,
				author_id: 'user-smoke',
				author_name: 'Smoke Admin',
				author_email: 'smoke@example.com',
				body: body.body,
				metadata: body.metadata ?? {},
				archived_at: null,
				created_at: now,
				updated_at: now
			};
			formRecordComments = [comment, ...formRecordComments];
			json(res, 200, apiResult(comment));
			return;
		}
		listedRecordCommentRecordIds.push(recordId);
		json(
			res,
			200,
			apiResult(paginated(formRecordComments.filter((comment) => comment.record_id === recordId && !comment.archived_at)))
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/attachments`) {
		if (req.method === 'POST') {
			const body = await readBody(req);
			createdAttachmentPayloads.push(body);
			const attachment = {
				id: `attachment-${createdAttachmentPayloads.length}`,
				workspace_id: workspaceId,
				project_id: projectId,
				form_id: orderLineForm.id,
				record_id: body.record_id ?? null,
				field_id: `fld_${body.field_key}`,
				field_key: body.field_key,
				file_name: body.file_name,
				content_type: body.content_type ?? '',
				byte_size: body.byte_size ?? 0,
				storage_key: body.storage_key,
				url: body.url ?? '',
				thumbnail_url: body.thumbnail_url ?? null,
				media_metadata: body.media_metadata ?? {},
				created_by: 'user-smoke',
				archived_at: null,
				created_at: now,
				updated_at: now
			};
			formAttachments = [attachment, ...formAttachments];
			json(res, 200, apiResult(attachment));
			return;
		}
		const recordId = url.searchParams.get('record_id');
		const fieldKey = url.searchParams.get('field_key');
		const includeArchived = url.searchParams.get('include_archived') === 'true';
		const items = formAttachments.filter(
			(attachment) =>
				(!recordId || attachment.record_id === recordId) &&
				(!fieldKey || attachment.field_key === fieldKey) &&
				(includeArchived || !attachment.archived_at)
		);
		json(res, 200, apiResult(paginated(items)));
		return;
	}
	if (pathname === '/api/v1/upload' && req.method === 'POST') {
		const body = await readRawBody(req);
		uploadedAttachmentPayloads.push({
			content_type: req.headers['content-type'] ?? '',
			byte_size: body.length
		});
		json(
			res,
			200,
			apiResult({
				url: '/api/v1/uploads/server-counter-ticket.png',
				filename: 'server-counter-ticket.png',
				thumbnail_url: serverAttachmentThumbnailUrl()
			})
		);
		return;
	}
	if (pathname === serverAttachmentThumbnailUrl() && req.method === 'GET') {
		res.writeHead(200, { 'Content-Type': 'image/png' });
		res.end(Buffer.from('server-counter-ticket-thumbnail'));
		return;
	}
	if (pathname === '/api/v1/uploads/server-counter-ticket.png' && req.method === 'GET') {
		res.writeHead(200, { 'Content-Type': 'image/png' });
		res.end(Buffer.from('server-counter-ticket'));
		return;
	}
	const attachmentSignedUrlMatch = pathname.match(/^\/api\/v1\/form-attachments\/([^/]+)\/signed-url$/);
	if (attachmentSignedUrlMatch && req.method === 'GET') {
		const attachmentId = attachmentSignedUrlMatch[1];
		json(
			res,
			200,
			apiResult({
				url: signedAttachmentUrl(attachmentId),
				expires_at: '2030-01-01T00:00:00.000Z'
			})
		);
		return;
	}
	const attachmentDownloadMatch = pathname.match(/^\/api\/v1\/form-attachments\/([^/]+)\/download$/);
	if (attachmentDownloadMatch && req.method === 'GET') {
		const attachmentId = attachmentDownloadMatch[1];
		const attachment = formAttachments.find((item) => item.id === attachmentId);
		if (!attachment || url.searchParams.get('signature') !== `signed-${attachmentId}`) {
			json(res, 401, { code: 401, message: 'invalid signature', data: null });
			return;
		}
		res.writeHead(302, { Location: attachment.url });
		res.end();
		return;
	}
	const attachmentDeleteMatch = pathname.match(/^\/api\/v1\/form-attachments\/([^/]+)$/);
	if (attachmentDeleteMatch && req.method === 'DELETE') {
		const attachmentId = attachmentDeleteMatch[1];
		deletedAttachmentIds.push(attachmentId);
		formAttachments = formAttachments.map((attachment) =>
			attachment.id === attachmentId ? { ...attachment, archived_at: now, updated_at: now } : attachment
		);
		json(res, 200, apiResult(null));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/relation-targets`) {
		json(
			res,
			200,
			apiResult(
				paginated([
					{
						record_id: 'order-1',
						form_id: 'form-order',
						form_key: 'order',
						form_name: 'Order',
						title: 'Order 1',
						display: 'Order 1',
						values: { order_no: 'Order 1' },
						updated_at: now
					}
				])
			)
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/permissions`) {
		if (req.method === 'PATCH') {
			const body = await readBody(req);
			const memberPolicy = body?.policies?.find(
				(policy) => policy.subject_type === 'role' && policy.subject_id === 'member'
			);
			if (memberPolicy?.policy) memberPermissionPolicy = memberPolicy.policy;
		}
		json(
			res,
			200,
			apiResult({
				form_id: orderLineForm.id,
				policies: [
					{
						id: 'permission-member',
						workspace_id: workspaceId,
						project_id: projectId,
						form_id: orderLineForm.id,
						subject_type: 'role',
						subject_id: 'member',
						policy: memberPermissionPolicy,
						created_at: now,
						updated_at: now
					}
				],
				effective: {
					role: 'member',
					is_bot: false,
					actions: memberPermissionPolicy.actions,
					record_scope: memberPermissionPolicy.record_scope,
					fields: memberPermissionPolicy.fields ?? {}
				}
			})
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}` && req.method === 'PATCH') {
		const body = await readBody(req);
		updatedFormPayloads.push(body);
		if (body.schema) {
			orderLineForm.schema = body.schema;
			orderLineForm.schema_version += 1;
			orderLineForm.updated_at = now;
		}
		if (body.detail_layout && typeof body.detail_layout === 'object') {
			orderLineForm.detail_layout = body.detail_layout;
			orderLineForm.updated_at = now;
		}
		json(res, 200, apiResult(orderLineForm));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}` && req.method === 'GET') {
		json(res, 200, apiResult(orderLineForm));
		return;
	}
	if (pathname === `/api/v1/forms/${printJobForm.id}` && req.method === 'GET') {
		json(res, 200, apiResult(printJobForm));
		return;
	}
		if (pathname === `/api/v1/forms/${orderLineForm.id}/records/export` && req.method === 'GET') {
			const records = orderLineRecordsForUrl(url);
			const view = viewById(url.searchParams.get('view_id'));
			const exportFormat = url.searchParams.get('format') === 'json' ? 'json' : 'csv';
			const hidden = new Set(readableFieldKeys());
			const columns = (view?.config?.columns ?? ['sku_name', 'quantity', 'status'])
				.filter((key) => !hidden.has(key))
				.map((key) => ({
					key,
					label: orderLineForm.schema.fields.find((field) => field.key === key)?.label ?? key
				}));
			orderLineExportRequests.push({
				view_id: url.searchParams.get('view_id'),
				format: exportFormat,
				rows: records.map((record) => record.id),
				columns: columns.map((column) => column.key)
			});
			const rows = records.map((record) => ({
				record_id: record.id,
				title: record.title,
				values: Object.fromEntries(columns.map((column) => [column.key, String(record.values[column.key] ?? '')]))
			}));
			json(
				res,
				200,
				apiResult({
					form_id: orderLineForm.id,
					format: exportFormat,
					file_name: `order_line_records.${exportFormat}`,
					columns,
					rows,
					...(exportFormat === 'csv'
						? { csv: ['title', ...records.map((record) => record.title)].join('\\n') }
						: {})
				})
			);
			return;
		}
		if (pathname === `/api/v1/forms/${orderLineForm.id}/records` && req.method === 'GET') {
			orderLineRecordListRequests.push({
				view_id: url.searchParams.get('view_id'),
				rows: orderLineRecordsForUrl(url).map((record) => record.id)
			});
			json(res, 200, apiResult(paginated(orderLineRecordsForUrl(url))));
			return;
		}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/records` && req.method === 'POST') {
		createdRecordPayload = await readBody(req);
		createdRecord = {
			id: 'record-order-line-created',
			workspace_id: workspaceId,
			project_id: projectId,
			form_id: orderLineForm.id,
			title: createdRecordPayload.title || 'Beef Noodles x2',
			values: {
				...createdRecordPayload.values,
				quantity: { type: 'integer', value: 2 },
				unit_price: { type: 'amount', decimal: '9.99', currency: 'CNY', scale: 2 },
				line_total: { type: 'amount', decimal: '19.98', currency: 'CNY', scale: 2 },
				ticket_no: 'AUTO-000001'
			},
			source: { type: 'web' },
			created_by: 'user-smoke',
			updated_by: 'user-smoke',
			created_at: now,
			updated_at: now
		};
			json(res, 200, apiResult(createdRecord));
			return;
		}
		if (pathname.startsWith('/api/v1/form-views/') && req.method === 'PATCH') {
			const viewId = pathname.slice('/api/v1/form-views/'.length);
			const view = orderLineViews.find((item) => item.id === viewId);
			if (!view) {
				json(res, 404, { code: 404, message: 'view not found', data: null });
				return;
			}
			updatedViewPayload = await readBody(req);
			updatedViewPayloads.push(updatedViewPayload);
			if (updatedViewPayload.expected_updated_at !== view.updated_at) {
				json(res, 200, { code: 409, message: 'view update conflict', data: null });
				return;
			}
			if (typeof updatedViewPayload.name === 'string') view.name = updatedViewPayload.name;
			if (typeof updatedViewPayload.view_type === 'string') view.view_type = updatedViewPayload.view_type;
			if (updatedViewPayload.config && typeof updatedViewPayload.config === 'object') {
				view.config = updatedViewPayload.config;
				if (view.config.is_default === true) {
					for (const item of orderLineViews) {
						if (item.id !== view.id) item.config = { ...item.config, is_default: false };
					}
				}
			}
			view.updated_at = nextUpdatedAt();
		json(res, 200, apiResult(view));
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-comparison' && req.method === 'PATCH') {
		updatedRecordPayload = await readBody(req);
		updatedRecordPayloads.push(updatedRecordPayload);
		if (memberPermissionPolicy.record_scope === 'owned') {
			json(res, 403, { code: 403, message: 'form record ownership policy denied access', data: null });
			return;
		}
		comparisonRecord.values = updatedRecordPayload.values || comparisonRecord.values;
		comparisonRecord.source = updatedRecordPayload.source || comparisonRecord.source;
		comparisonRecord.updated_by = 'user-smoke';
		comparisonRecord.updated_at = now;
		json(res, 200, apiResult(comparisonRecord));
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-created' && req.method === 'GET') {
		json(res, 200, apiResult(seedCreatedRecord()));
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-created' && req.method === 'PATCH') {
		updatedRecordPayload = await readBody(req);
		updatedRecordPayloads.push(updatedRecordPayload);
		const lockedField = Object.keys(updatedRecordPayload.values ?? {}).find(
			(fieldKey) => memberPermissionPolicy.fields?.[fieldKey]?.write === false
		);
		if (lockedField) {
			json(res, 403, { code: 403, message: 'form field write permission denied: ' + lockedField, data: null });
			return;
		}
		createdRecord = {
			...createdRecord,
			title: updatedRecordPayload.title || createdRecord.title,
			values: updatedRecordPayload.values || createdRecord.values,
			source: updatedRecordPayload.source || createdRecord.source,
			updated_by: 'user-smoke',
			updated_at: now
		};
		json(res, 200, apiResult(createdRecord));
		return;
	}
	if (pathname === `/api/v1/forms/${printJobForm.id}/records`) {
		json(res, 200, apiResult(paginated(printJobs)));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/aggregate`) {
		const fieldKey = url.searchParams.get('field_key');
		const aggregateByField = {
			quantity: {
				field_type: 'integer',
				decimal: createdRecord ? '2' : null,
				currency: null,
				scale: null
			},
			unit_price: {
				field_type: 'amount',
				decimal: createdRecord ? '9.99' : null,
				currency: 'CNY',
				scale: 2
			},
			line_total: {
				field_type: 'amount',
				decimal: createdRecord ? '19.98' : null,
				currency: 'CNY',
				scale: 2
			}
		};
		const aggregate = aggregateByField[fieldKey] ?? {
			field_type: 'number',
			decimal: createdRecord ? '0' : null,
			currency: null,
			scale: null
		};
		json(
			res,
			200,
			apiResult({
				form_id: orderLineForm.id,
				field_key: fieldKey,
				field_type: aggregate.field_type,
				aggregate: 'sum',
				decimal: aggregate.decimal,
				count: createdRecord ? 1 : 0,
				currency: aggregate.currency,
				scale: aggregate.scale
			})
		);
		return;
	}
	if (pathname === `/api/v1/forms/${printJobForm.id}/aggregate`) {
		json(
			res,
			200,
			apiResult({
				form_id: printJobForm.id,
				field_key: url.searchParams.get('field_key'),
				field_type: 'integer',
				aggregate: 'sum',
				decimal: '1',
				count: 1,
				currency: null,
				scale: null
			})
		);
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-created/links' && req.method === 'GET') {
		json(
			res,
			200,
			apiResult(
				['detail-widget-empty-text-builder', 'detail-widget-hide-empty-builder'].includes(
					requestedSmokeMode
				)
					? []
					: createdLinks
			)
		);
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-created/children' && req.method === 'GET') {
		json(
			res,
			200,
			apiResult(
				paginated(
					printJobs.map((record) => ({
						link_id: `link-${record.id}`,
						relation_key: 'line_print_jobs',
						relation_type: 'parent_child',
						record
					}))
				)
			)
		);
		return;
	}
	if (
		pathname === '/api/v1/form-records/record-order-line-created/links' &&
		req.method === 'POST'
	) {
		createdLinkPayload = await readBody(req);
		const link = {
			id: 'link-order-line',
			workspace_id: workspaceId,
			project_id: projectId,
			source_record_id: 'record-order-line-created',
			target_type: createdLinkPayload.target_type,
			target_id: createdLinkPayload.target_id,
			relation_key: createdLinkPayload.relation_key,
			relation_type: createdLinkPayload.relation_type,
			metadata: createdLinkPayload.metadata ?? {},
			created_by: 'user-smoke',
			created_at: now
		};
		createdLinks = [link];
		json(res, 200, apiResult(link));
		return;
	}

	await serveStatic(req, res, url.searchParams.get('forms_smoke'));
}

function runChromium(url, windowSize = '1366,900', screenshotPath = null, virtualTimeBudget = 60000) {
	return new Promise((resolve, reject) => {
		const args = [
			'--headless=new',
			'--disable-gpu',
			'--no-sandbox',
			'--disable-dev-shm-usage',
			`--window-size=${windowSize}`,
			`--virtual-time-budget=${virtualTimeBudget}`,
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

function extractSmokeError(dom) {
	const match = dom.match(/data-forms-ui-smoke-error="([^"]*)"/);
	if (!match) return '';
	return match[1]
		.replace(/&quot;/g, '"')
		.replace(/&#10;/g, '\n')
		.replace(/&amp;/g, '&')
		.replace(/&lt;/g, '<')
		.replace(/&gt;/g, '>');
}

function assertAttachmentMetadataPayloads() {
	if (
		createdAttachmentPayloads[0]?.field_key !== 'attachment_12' ||
		createdAttachmentPayloads[0]?.record_id !== 'record-order-line-created' ||
		createdAttachmentPayloads[0]?.url !== 'https://assets.example.test/order-ticket.pdf' ||
		deletedAttachmentIds[0] !== 'attachment-1' ||
		createdAttachmentPayloads[1]?.field_key !== 'attachment_12' ||
		createdAttachmentPayloads[1]?.record_id !== 'record-order-line-created' ||
		createdAttachmentPayloads[1]?.file_name !== 'counter-ticket.png' ||
		createdAttachmentPayloads[1]?.content_type !== 'image/png' ||
		createdAttachmentPayloads[1]?.byte_size <= 0 ||
		!createdAttachmentPayloads[1]?.storage_key?.includes('/server-upload-') ||
		createdAttachmentPayloads[1]?.url !== '/api/v1/uploads/server-counter-ticket.png' ||
		createdAttachmentPayloads[1]?.thumbnail_url !== serverAttachmentThumbnailUrl() ||
		uploadedAttachmentPayloads.length < 1 ||
		!String(uploadedAttachmentPayloads[0]?.content_type ?? '').includes('multipart/form-data') ||
		deletedAttachmentIds[1] !== 'attachment-2' ||
		createdAttachmentPayloads[2]?.field_key !== 'attachment_12' ||
		createdAttachmentPayloads[2]?.record_id !== 'record-order-line-created' ||
		createdAttachmentPayloads[2]?.url !== 'https://assets.example.test/detail-ticket.pdf'
	) {
		throw new Error('Expected attachment metadata create/delete payloads');
	}
}

function assertRecordCommentPayloads() {
	if (
		createdRecordCommentPayloads[0]?.body !== 'Chef added allergy follow-up.' ||
		createdRecordCommentPayloads[0]?.metadata?.source !== 'web' ||
		createdRecordCommentPayloads[0]?.metadata?.form_id !== 'form-order-line' ||
		createdRecordCommentPayloads[0]?.metadata?.form_key !== 'order_line'
	) {
		throw new Error('Expected record comment create payload');
	}
}

function assertDetailPageWidgetPayloads() {
	const widgetPayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return (
			Array.isArray(widgets) &&
			widgets.map((widget) => widget.key).join('|').startsWith('children|comments|links') &&
			widgets.some((widget) => widget?.key === 'comments') &&
			widgets.some((widget) => widget?.key === 'events' && widget?.enabled === false)
		);
	});
	const widgets = widgetPayload?.detail_layout?.widgets ?? [];
	const order = widgets.map((widget) => widget.key).join('|');
	if (!widgetPayload || !order.startsWith('children|comments|links') || !widgets.some((widget) => widget.key === 'events' && widget.enabled === false)) {
		throw new Error(`Expected detail page widget layout payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailLayoutSectionPayloads() {
	const sectionPayload = updatedFormPayloads.find((payload) => {
		const sections = payload?.detail_layout?.sections;
		const handoff = Array.isArray(sections) ? sections.find((section) => section?.key === 'handoff') : null;
		const pricing = Array.isArray(sections) ? sections.find((section) => section?.key === 'pricing') : null;
		return (
			handoff?.title === 'Service Snapshot' &&
			Array.isArray(handoff.fields) &&
			handoff.fields.includes('unit_price') &&
			Array.isArray(pricing?.fields) &&
			!pricing.fields.includes('unit_price')
		);
	});
	if (!sectionPayload) {
		throw new Error(`Expected detail layout section payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailLayoutSectionDescriptionPayloads() {
	const sectionDescriptionPayload = updatedFormPayloads.find((payload) => {
		const sections = payload?.detail_layout?.sections;
		const handoff = Array.isArray(sections) ? sections.find((section) => section?.key === 'handoff') : null;
		return handoff?.description === 'Review service notes before closing the line.';
	});
	if (!sectionDescriptionPayload) {
		throw new Error(`Expected detail layout section description payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailLayoutSectionHideEmptyFieldsPayloads() {
	const sectionHideEmptyPayload = updatedFormPayloads.find((payload) => {
		const sections = payload?.detail_layout?.sections;
		const handoff = Array.isArray(sections) ? sections.find((section) => section?.key === 'handoff') : null;
		return handoff?.hide_empty_fields === true;
	});
	if (!sectionHideEmptyPayload) {
		throw new Error(`Expected detail layout section hide empty fields payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailLayoutSectionColumnsPayloads() {
	const sectionColumnsPayload = updatedFormPayloads.find((payload) => {
		const sections = payload?.detail_layout?.sections;
		const handoff = Array.isArray(sections) ? sections.find((section) => section?.key === 'handoff') : null;
		return handoff?.columns === 3;
	});
	if (!sectionColumnsPayload) {
		throw new Error(`Expected detail layout section columns payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailLayoutSectionDensityPayloads() {
	const sectionDensityPayload = updatedFormPayloads.find((payload) => {
		const sections = payload?.detail_layout?.sections;
		const handoff = Array.isArray(sections) ? sections.find((section) => section?.key === 'handoff') : null;
		return handoff?.density === 'compact';
	});
	if (!sectionDensityPayload) {
		throw new Error(`Expected detail layout section density payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailLayoutSectionCollapsiblePayloads() {
	const sectionCollapsiblePayload = updatedFormPayloads.find((payload) => {
		const sections = payload?.detail_layout?.sections;
		const handoff = Array.isArray(sections) ? sections.find((section) => section?.key === 'handoff') : null;
		return handoff?.collapsible === true && handoff?.default_collapsed === true;
	});
	if (!sectionCollapsiblePayload) {
		throw new Error(`Expected detail layout section collapsible payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertAttachmentServerUploadPayloads() {
	if (
		uploadedAttachmentPayloads.length < 1 ||
		!String(uploadedAttachmentPayloads[0]?.content_type ?? '').includes('multipart/form-data') ||
		uploadedAttachmentPayloads[0]?.byte_size <= 0 ||
		createdAttachmentPayloads[1]?.field_key !== 'attachment_12' ||
		createdAttachmentPayloads[1]?.record_id !== 'record-order-line-created' ||
		createdAttachmentPayloads[1]?.file_name !== 'counter-ticket.png' ||
		createdAttachmentPayloads[1]?.content_type !== 'image/png' ||
		createdAttachmentPayloads[1]?.byte_size <= 0 ||
		!createdAttachmentPayloads[1]?.storage_key?.includes('/server-upload-') ||
		createdAttachmentPayloads[1]?.url !== '/api/v1/uploads/server-counter-ticket.png' ||
		createdAttachmentPayloads[1]?.thumbnail_url !== serverAttachmentThumbnailUrl()
	) {
		throw new Error('Expected server-backed attachment upload payloads');
	}
}

function assertAttachmentStoragePolicyPayloads() {
	const attachmentField = orderLineForm.schema.fields.find((field) => field.key === 'attachment_12');
	if (
		attachmentField?.attachment?.storage_policy !== 'private' ||
		attachmentField?.attachment?.retention_days !== 30 ||
		attachmentField?.attachment?.signed_url_ttl_minutes !== 15 ||
		attachmentField?.attachment?.max_size_mb !== 12 ||
		attachmentField?.attachment?.thumbnail_format !== 'jpeg' ||
		attachmentField?.attachment?.preview_format !== 'webp' ||
		!Array.isArray(attachmentField?.attachment?.variants) ||
		attachmentField.attachment.variants[0]?.kind !== 'gallery' ||
		attachmentField.attachment.variants[0]?.max_dimension !== 1200 ||
		attachmentField.attachment.variants[0]?.format !== 'webp' ||
		attachmentField.attachment.variants[1]?.kind !== 'card' ||
		attachmentField.attachment.variants[1]?.max_dimension !== 640 ||
		attachmentField.attachment.variants[1]?.format !== 'jpeg' ||
		!Array.isArray(attachmentField?.attachment?.accept) ||
		attachmentField.attachment.accept.join('|') !== 'application/pdf|image/png'
	) {
		throw new Error(`Expected attachment storage policy metadata, got ${JSON.stringify(attachmentField)}`);
	}
}

function assertDetailHeaderPayloads() {
	const headerPayload = updatedFormPayloads.find((payload) => {
		const header = payload?.detail_layout?.header;
		return header?.subtitle_field === 'seat_no' && header?.badge_field === 'status';
	});
	if (!headerPayload) {
		throw new Error(`Expected detail header payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailHighlightPayloads() {
	const highlightPayload = updatedFormPayloads.find((payload) => {
		const highlights = payload?.detail_layout?.highlights;
		return Array.isArray(highlights) && highlights.join('|') === 'line_total|quantity';
	});
	if (!highlightPayload) {
		throw new Error(`Expected detail highlight payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailPageCompositionPayloads() {
	const compositionPayload = updatedFormPayloads.find((payload) => {
		const page = payload?.detail_layout?.page;
		return page?.widget_region === 'below';
	});
	if (!compositionPayload) {
		throw new Error(`Expected detail page composition payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailFieldColumnsPayloads() {
	const fieldColumnsPayload = updatedFormPayloads.find((payload) => {
		const page = payload?.detail_layout?.page;
		return page?.field_columns === 1;
	});
	if (!fieldColumnsPayload) {
		throw new Error(`Expected detail field columns payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetTitlePayloads() {
	const widgetTitlePayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return Array.isArray(widgets) && widgets.some((widget) => widget?.key === 'comments' && widget?.title === 'Ops Notes');
	});
	if (!widgetTitlePayload) {
		throw new Error(`Expected detail widget title payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetDescriptionPayloads() {
	const widgetDescriptionPayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return Array.isArray(widgets) && widgets.some((widget) => widget?.key === 'comments' && widget?.description === 'Track kitchen follow-ups before service.');
	});
	if (!widgetDescriptionPayload) {
		throw new Error(`Expected detail widget description payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetEmptyTextPayloads() {
	const widgetEmptyTextPayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return Array.isArray(widgets) && widgets.some((widget) => widget?.key === 'links' && widget?.empty_text === 'No linked service records yet.');
	});
	if (!widgetEmptyTextPayload) {
		throw new Error(`Expected detail widget empty text payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetCountPayloads() {
	const widgetCountPayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return Array.isArray(widgets) && widgets.some((widget) => widget?.key === 'comments' && widget?.show_count === true);
	});
	if (!widgetCountPayload) {
		throw new Error(`Expected detail widget count payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetDensityPayloads() {
	const widgetDensityPayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return Array.isArray(widgets) && widgets.some((widget) => widget?.key === 'comments' && widget?.density === 'compact');
	});
	if (!widgetDensityPayload) {
		throw new Error(`Expected detail widget density payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetHideEmptyPayloads() {
	const widgetHideEmptyPayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return Array.isArray(widgets) && widgets.some((widget) => widget?.key === 'links' && widget?.hide_empty === true);
	});
	if (!widgetHideEmptyPayload) {
		throw new Error(`Expected detail widget hide empty payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetCollapsiblePayloads() {
	const widgetCollapsiblePayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		const comments = Array.isArray(widgets) ? widgets.find((widget) => widget?.key === 'comments') : null;
		const children = Array.isArray(widgets) ? widgets.find((widget) => widget?.key === 'children') : null;
		return (
			Array.isArray(widgets) &&
			comments?.collapsible === true &&
			comments?.default_collapsed === true &&
			children?.collapsible === true &&
			children?.default_collapsed === true
		);
	});
	if (!widgetCollapsiblePayload) {
		throw new Error(`Expected detail widget collapsible payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

function assertDetailWidgetLimitPayloads() {
	const widgetLimitPayload = updatedFormPayloads.find((payload) => {
		const widgets = payload?.detail_layout?.widgets;
		return Array.isArray(widgets) && widgets.some((widget) => widget?.key === 'comments' && widget?.limit === 1);
	});
	if (!widgetLimitPayload) {
		throw new Error(`Expected detail widget limit payload, got ${JSON.stringify(updatedFormPayloads)}`);
	}
}

if (!existsSync(buildDir)) {
	throw new Error(`Missing build directory: ${buildDir}. Run bun run build before smoke:forms-ui.`);
}
if (!existsSync(chromium)) {
	throw new Error(`Chromium binary not found: ${chromium}`);
}

const server = createServer((req, res) => {
	handler(req, res).catch((error) => {
		json(res, 500, { code: 500, message: error.message, data: null });
	});
});
const port = 19580 + Math.floor(Math.random() * 1000);
await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));

	try {
		if (screenshotDir) await mkdir(screenshotDir, { recursive: true });
		const desktopScreenshot = screenshotDir ? join(screenshotDir, 'forms-ui-desktop.png') : null;
		const mobileScreenshot = screenshotDir ? join(screenshotDir, 'forms-ui-mobile.png') : null;
			if (requestedSmokeMode === 'detail-widget-hide-empty-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-hide-empty-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-hide-empty-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-hide-empty-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetHideEmptyPayloads();
			const durableWidgetHideEmptyDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-route="record-order-line-created"',
				'data-durable-record-detail-comments="record-order-line-created"',
				'data-durable-record-detail-widget-order="children|links|comments|attachments|events"'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetHideEmptyDom.includes(fragment));
			if (
				missing.length > 0 ||
				durableWidgetHideEmptyDom.includes('data-durable-record-detail-links="record-order-line-created"')
			) {
				throw new Error(
					`Forms UI detail-widget-hide-empty-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetHideEmptyDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-hide-empty-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-widget-count-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-count-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-count-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-count-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetCountPayloads();
			const durableWidgetCountDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const badgePattern = /<span[^>]*data-durable-record-detail-widget-count-badge="comments:record-order-line-created"[^>]*>\s*1\s*<\/span>/;
			const requiredFragments = [
				'data-durable-record-detail-comments="record-order-line-created"',
				'data-durable-record-detail-comment-count="1"'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetCountDom.includes(fragment));
			if (missing.length > 0 || !badgePattern.test(durableWidgetCountDom)) {
				throw new Error(
					`Forms UI detail-widget-count-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetCountDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-count-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-widget-density-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-density-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-density-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-density-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetDensityPayloads();
			const durableWidgetDensityDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-comments="record-order-line-created"',
				'data-durable-record-detail-widget-density="comments:record-order-line-created:compact"',
				'data-durable-record-detail-widget-density="children:record-order-line-created:comfortable"',
				'data-durable-record-detail-widget-density="attachments:record-order-line-created:comfortable"'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetDensityDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-widget-density-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetDensityDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-density-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-widget-collapsible-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-collapsible-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-collapsible-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-collapsible-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetCollapsiblePayloads();
			const durableWidgetCollapsibleDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-children="record-order-line-created"',
				'data-durable-record-detail-widget-collapsible="children:record-order-line-created:true:true"',
				'data-durable-record-detail-widget-open="children:record-order-line-created:false"',
				'data-durable-record-detail-comments="record-order-line-created"',
				'data-durable-record-detail-widget-collapsible="comments:record-order-line-created:true:true"',
				'data-durable-record-detail-widget-open="comments:record-order-line-created:false"',
				'data-durable-record-detail-widget-collapsible="links:record-order-line-created:false:false"',
				'data-durable-record-detail-widget-collapsible="attachments:record-order-line-created:false:false"',
				'data-durable-record-detail-widget-collapsible="events:record-order-line-created:false:false"'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetCollapsibleDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-widget-collapsible-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetCollapsibleDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-collapsible-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-widget-empty-text-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-empty-text-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-empty-text-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-empty-text-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetEmptyTextPayloads();
			const durableWidgetEmptyTextDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-widget-empty="links:record-order-line-created"',
				'No linked service records yet.',
				'data-durable-record-detail-links="record-order-line-created"'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetEmptyTextDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-widget-empty-text-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetEmptyTextDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-empty-text-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-widget-description-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-description-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-description-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-description-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetDescriptionPayloads();
			const durableWidgetDescriptionDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-widget-description="comments:record-order-line-created"',
				'Track kitchen follow-ups before service.',
				'data-durable-record-detail-comments="record-order-line-created"'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetDescriptionDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-widget-description-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetDescriptionDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-description-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-widget-limit-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-limit-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-limit-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-limit-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetLimitPayloads();
			const durableWidgetLimitDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-comments="record-order-line-created"',
				'data-durable-record-detail-comment-count="2"',
				'data-durable-record-detail-widget-limit="comments:1"',
				'data-durable-record-detail-comment="comment-record-created-1"',
				'Limit smoke follow-up.'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetLimitDom.includes(fragment));
			if (missing.length > 0 || durableWidgetLimitDom.includes('data-durable-record-detail-comment="comment-record-existing"')) {
				throw new Error(
					`Forms UI detail-widget-limit-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetLimitDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-limit-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-widget-title-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-widget-title-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-widget-title-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-widget-title-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailWidgetTitlePayloads();
			const durableWidgetTitleDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-widget-title="comments:record-order-line-created"',
				'Ops Notes',
				'data-durable-record-detail-comments="record-order-line-created"'
			];
			const missing = requiredFragments.filter((fragment) => !durableWidgetTitleDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-widget-title-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetTitleDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-widget-title-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-page-composition-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-page-composition-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-page-composition-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-page-composition-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailPageCompositionPayloads();
			const durableCompositionDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-page-composition="record-order-line-created:below"',
				'data-durable-record-detail-widget-region="below"',
				'data-durable-record-detail-widget-order="children|links|comments|attachments|events"',
				'data-durable-record-detail-comments="record-order-line-created"',
				'data-durable-record-detail-events="record-order-line-created"'
			];
			const missing = requiredFragments.filter((fragment) => !durableCompositionDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-page-composition-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableCompositionDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-page-composition-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-field-columns-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-field-columns-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-field-columns-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-field-columns-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailFieldColumnsPayloads();
			const durableFieldColumnsDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-field-columns="record-order-line-created:1"',
				'data-durable-record-detail-section="record-order-line-created:handoff"',
				'data-durable-record-detail-field="record-order-line-created:sku_name"'
			];
			const missing = requiredFragments.filter((fragment) => !durableFieldColumnsDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-field-columns-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableFieldColumnsDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-field-columns-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-highlights-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-highlights-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-highlights-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-highlights-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailHighlightPayloads();
			const durableHighlightDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-highlights="record-order-line-created"',
				'data-durable-record-detail-highlight-order="line_total|quantity"',
				'data-durable-record-detail-highlight="record-order-line-created:line_total"',
				'data-durable-record-detail-highlight="record-order-line-created:quantity"',
				'19.98 CNY',
				'2'
			];
			const missing = requiredFragments.filter((fragment) => !durableHighlightDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-highlights-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableHighlightDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-highlights-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-header-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-header-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-header-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-header-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailHeaderPayloads();
			const durableHeaderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-header-subtitle="seat_no:record-order-line-created"',
				'data-durable-record-detail-header-badge="status:record-order-line-created"',
				'Seat',
				'sent_to_kitchen'
			];
			const missing = requiredFragments.filter((fragment) => !durableHeaderDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-header-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableHeaderDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-header-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-section-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-section-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-layout-section-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-section-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailLayoutSectionPayloads();
			const durableSectionDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-section="record-order-line-created:handoff"',
				'Service Snapshot',
				'data-durable-record-detail-field="record-order-line-created:unit_price"'
			];
			const missing = requiredFragments.filter((fragment) => !durableSectionDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-section-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableSectionDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-section-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-section-description-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-section-description-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-section-description-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-section-description-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailLayoutSectionDescriptionPayloads();
			const durableSectionDescriptionDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-section-description="record-order-line-created:handoff"',
				'Review service notes before closing the line.',
				'data-durable-record-detail-section="record-order-line-created:handoff"'
			];
			const missing = requiredFragments.filter((fragment) => !durableSectionDescriptionDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-section-description-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableSectionDescriptionDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-section-description-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-section-hide-empty-fields-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-section-hide-empty-fields-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-section-hide-empty-fields-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-section-hide-empty-fields-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailLayoutSectionHideEmptyFieldsPayloads();
			const durableSectionHideEmptyDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-section="record-order-line-created:handoff"',
				'data-durable-record-detail-field="record-order-line-created:sku_name"',
				'data-durable-record-detail-field="record-order-line-created:status"'
			];
			const missing = requiredFragments.filter((fragment) => !durableSectionHideEmptyDom.includes(fragment));
			if (
				missing.length > 0 ||
				durableSectionHideEmptyDom.includes('data-durable-record-detail-field="record-order-line-created:seat_no"')
			) {
				throw new Error(
					`Forms UI detail-section-hide-empty-fields-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableSectionHideEmptyDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-section-hide-empty-fields-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-section-columns-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-section-columns-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-section-columns-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-section-columns-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailLayoutSectionColumnsPayloads();
			const durableSectionColumnsDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-section-columns="record-order-line-created:handoff:3"',
				'data-durable-record-detail-field-columns="record-order-line-created:3"',
				'data-durable-record-detail-field="record-order-line-created:sku_name"'
			];
			const missing = requiredFragments.filter((fragment) => !durableSectionColumnsDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-section-columns-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableSectionColumnsDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-section-columns-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-section-density-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-section-density-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-section-density-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-section-density-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailLayoutSectionDensityPayloads();
			const durableSectionDensityDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-section-density="record-order-line-created:handoff:compact"',
				'data-durable-record-detail-section-density="record-order-line-created:pricing:comfortable"',
				'data-durable-record-detail-section="record-order-line-created:handoff"'
			];
			const missing = requiredFragments.filter((fragment) => !durableSectionDensityDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-section-density-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableSectionDensityDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-section-density-builder smoke passed');
		} else if (requestedSmokeMode === 'detail-section-collapsible-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-section-collapsible-builder`,
				'1366,900',
				desktopScreenshot,
				60000
			);
			if (
				!builderDom.includes('data-forms-ui-smoke="done"') ||
				!builderDom.includes('data-form-detail-section-collapsible-builder="done"')
			) {
				const smokeError = extractSmokeError(builderDom);
				throw new Error(
					`Forms UI detail-section-collapsible-builder smoke failed.${
						smokeError ? `\nSmoke error:\n${smokeError}` : ''
					}\nDOM excerpt:\n${builderDom.slice(-12000)}`
				);
			}
			assertDetailLayoutSectionCollapsiblePayloads();
			const durableSectionCollapsibleDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
				'1366,900',
				null,
				30000
			);
			const requiredFragments = [
				'data-durable-record-detail-section="record-order-line-created:handoff"',
				'data-durable-record-detail-section-collapsible="record-order-line-created:handoff:true:true"',
				'data-durable-record-detail-section-open="record-order-line-created:handoff:false"'
			];
			const missing = requiredFragments.filter((fragment) => !durableSectionCollapsibleDom.includes(fragment));
			if (missing.length > 0) {
				throw new Error(
					`Forms UI detail-section-collapsible-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableSectionCollapsibleDom.slice(-12000)}`
				);
			}
			console.log('Forms UI detail-section-collapsible-builder smoke passed');
		} else if (requestedSmokeMode === 'page-widget-builder') {
			const builderDom = await runChromium(
				`http://127.0.0.1:${port}/smoke-auth-seed?mode=page-widget-builder`,
			'1366,900',
			desktopScreenshot,
			60000
		);
		if (
			!builderDom.includes('data-forms-ui-smoke="done"') ||
			!builderDom.includes('data-form-detail-page-widget-builder="done"')
		) {
			const smokeError = extractSmokeError(builderDom);
			throw new Error(
				`Forms UI page-widget-builder smoke failed.${
					smokeError ? `\nSmoke error:\n${smokeError}` : ''
				}\nDOM excerpt:\n${builderDom.slice(-12000)}`
			);
		}
		assertDetailPageWidgetPayloads();
		const durableWidgetDom = await runChromium(
			`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
			'1366,900',
			null,
			30000
		);
		const requiredFragments = [
			'data-durable-record-detail-widget-order="children|comments|links|attachments"',
			'data-durable-record-detail-children="record-order-line-created"',
			'data-durable-record-detail-comments="record-order-line-created"',
			'data-durable-record-detail-links="record-order-line-created"',
			'data-durable-record-detail-attachments="record-order-line-created"'
		];
		const missing = requiredFragments.filter((fragment) => !durableWidgetDom.includes(fragment));
		if (missing.length > 0 || durableWidgetDom.includes('data-durable-record-detail-events="record-order-line-created"')) {
			throw new Error(
				`Forms UI page-widget-builder durable consumption failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableWidgetDom.slice(-12000)}`
			);
		}
		console.log('Forms UI page-widget-builder smoke passed');
	} else if (requestedSmokeMode === 'attachment-storage-policy') {
		const attachmentPolicyDom = await runChromium(
			`http://127.0.0.1:${port}/smoke-auth-seed?mode=attachment-storage-policy`,
			'1366,900',
			desktopScreenshot,
			60000
		);
		if (
			!attachmentPolicyDom.includes('data-forms-ui-smoke="done"') ||
			!attachmentPolicyDom.includes('data-designer-attachment-storage-policy="done"')
		) {
			const smokeError = extractSmokeError(attachmentPolicyDom);
			throw new Error(
				`Forms UI attachment-storage-policy smoke failed.${
					smokeError ? `\nSmoke error:\n${smokeError}` : ''
				}\nDOM excerpt:\n${attachmentPolicyDom.slice(-12000)}`
			);
		}
		assertAttachmentStoragePolicyPayloads();
		console.log('Forms UI attachment-storage-policy smoke passed');
	} else if (requestedSmokeMode === 'attachment-package-export') {
		const attachmentPackageDom = await runChromium(
			`http://127.0.0.1:${port}/smoke-auth-seed?mode=attachment-package-export`,
			'1366,900',
			desktopScreenshot,
			60000
		);
		if (
			!attachmentPackageDom.includes('data-forms-ui-smoke="done"') ||
			!attachmentPackageDom.includes('data-form-attachment-package-export="done"')
		) {
			const smokeError = extractSmokeError(attachmentPackageDom);
			throw new Error(
				`Forms UI attachment-package-export smoke failed.${
					smokeError ? `\nSmoke error:\n${smokeError}` : ''
				}\nDOM excerpt:\n${attachmentPackageDom.slice(-12000)}`
			);
		}
		console.log('Forms UI attachment-package-export smoke passed');
	} else if (requestedSmokeMode === 'attachment-server-upload') {
		const attachmentUploadDom = await runChromium(
			`http://127.0.0.1:${port}/smoke-auth-seed?mode=attachment-server-upload`,
			'1366,900',
			desktopScreenshot,
			60000
		);
		if (
			!attachmentUploadDom.includes('data-forms-ui-smoke="done"') ||
			!attachmentUploadDom.includes('data-form-attachment-server-upload="done"')
		) {
			const smokeError = extractSmokeError(attachmentUploadDom);
			throw new Error(
				`Forms UI attachment-server-upload smoke failed.${
					smokeError ? `\nSmoke error:\n${smokeError}` : ''
				}\nDOM excerpt:\n${attachmentUploadDom.slice(-12000)}`
			);
		}
		assertAttachmentServerUploadPayloads();
		const localAttachmentLinkedPayload = updatedRecordPayloads.find(
			(payload) =>
				payload?.source?.origin === 'attachment' &&
				payload?.values?.attachment_12 === '/api/v1/uploads/server-counter-ticket.png'
		);
		if (!localAttachmentLinkedPayload) {
			throw new Error('Expected attachment server upload record value sync payload');
		}
		console.log('Forms UI attachment-server-upload smoke passed');
	} else if (requestedSmokeMode === 'durable-detail') {
		const durableDetailDom = await runChromium(
			`http://127.0.0.1:${port}/smoke-auth-seed?mode=durable-detail`,
			'1366,900',
			desktopScreenshot,
			30000
		);
		const requiredFragments = [
			'data-durable-record-detail-route="record-order-line-created"',
			'data-durable-record-detail-form="form-order-line"',
			'data-durable-record-detail-permission-state="record-order-line-created"',
			'data-permission-state-mode="durable-detail"',
			'data-permission-state-scope="owned"',
			'data-permission-state-hidden-fields="Unit Price"',
			'data-permission-state-locked-fields="Quantity|Unit Price"',
			'data-durable-record-detail-section="record-order-line-created:handoff"',
			'data-durable-record-detail-field="record-order-line-created:sku_name"',
			'data-durable-record-detail-permission-hidden="unit_price"',
			'data-durable-record-detail-permission-locked="unit_price"',
			'data-durable-record-detail-permission-locked="quantity"',
			'data-durable-record-detail-children="record-order-line-created"',
			'data-durable-record-detail-child-count="1"',
			'data-durable-record-detail-child="print-job-kitchen"',
			'data-durable-record-detail-comments="record-order-line-created"',
			'data-durable-record-detail-comment-count="1"',
			'data-durable-record-detail-comment="comment-record-existing"',
			'data-durable-record-detail-attachments="record-order-line-created"',
			'data-durable-record-detail-attachment-count="1"',
			'data-durable-record-detail-attachment="attachment-durable-detail"',
			'data-durable-record-detail-events="record-order-line-created"',
			'data-durable-record-detail-event-count="1"',
			'data-durable-record-detail-event="event-order-line-created"',
			'Beef Noodles',
			'Kitchen lead reviewed the detail handoff.',
			'detail-ticket.pdf'
		];
		const missing = requiredFragments.filter((fragment) => !durableDetailDom.includes(fragment));
		if (missing.length > 0) {
			throw new Error(
				`Forms UI durable-detail smoke failed. Missing ${JSON.stringify(missing)}\nDOM excerpt:\n${durableDetailDom.slice(-12000)}`
			);
		}
		console.log('Forms UI durable-detail smoke passed');
	} else if (
		requestedSmokeMode === 'attachment-detail' ||
		requestedSmokeMode === 'comments-detail'
	) {
		const detailWidgetDom = await runChromium(
			`http://127.0.0.1:${port}/smoke-auth-seed?mode=${requestedSmokeMode}`,
			'1366,900',
			desktopScreenshot,
			60000
		);
		const requiredMarker = requestedSmokeMode === 'comments-detail'
			? 'data-form-record-detail-comments-widget="done"'
			: 'data-form-record-detail-attachment-widget="done"';
		if (
			!detailWidgetDom.includes('data-forms-ui-smoke="done"') ||
			!detailWidgetDom.includes(requiredMarker)
		) {
			const smokeError = extractSmokeError(detailWidgetDom);
			throw new Error(
				`Forms UI ${requestedSmokeMode} smoke failed.${
					smokeError ? `\nSmoke error:\n${smokeError}` : ''
				}\nRecord comment mock debug: ${JSON.stringify({
					listedRecordCommentRecordIds,
					formRecordComments: formRecordComments.map((comment) => ({
						id: comment.id,
						record_id: comment.record_id,
						archived_at: comment.archived_at
					})),
					createdRecordCommentPayloads
				})}\nDOM excerpt:\n${detailWidgetDom.slice(-12000)}`
			);
		}
		assertAttachmentMetadataPayloads();
		if (requestedSmokeMode === 'comments-detail') assertRecordCommentPayloads();
		console.log(`Forms UI ${requestedSmokeMode} smoke passed`);
	} else if (requestedSmokeMode !== 'full') {
		throw new Error(`Unsupported OPENPR_FORMS_UI_SMOKE_MODE: ${requestedSmokeMode}`);
	} else {
	const desktopDom = await runChromium(
		`http://127.0.0.1:${port}/smoke-auth-seed?mode=desktop`,
		'1366,900',
		desktopScreenshot,
		50000
	);
		if (!desktopDom.includes('data-forms-ui-smoke="done"')) {
			const smokeError = extractSmokeError(desktopDom);
			throw new Error(
				`Forms UI browser smoke failed. Server state: ${JSON.stringify({
					createdRecordPayload,
					createdRecord: Boolean(createdRecord),
					updatedViewPayload,
					serviceStatusField: orderLineForm.schema.fields.find((field) => field.key === 'service_status')
				})}${smokeError ? `\nSmoke error:\n${smokeError}` : ''}\nDOM excerpt:\n${desktopDom.slice(-12000)}`
			);
		}
	const detailUrlDom = await runChromium(
		`http://127.0.0.1:${port}/smoke-auth-seed?mode=detail-url`,
		'1366,900',
		null,
		10000
	);
	if (!detailUrlDom.includes('data-forms-detail-url-smoke="done"')) {
		const smokeError = extractSmokeError(detailUrlDom);
		throw new Error(
			`Forms UI detail URL smoke failed.${smokeError ? `\nSmoke error:\n${smokeError}` : ''}\nDOM excerpt:\n${detailUrlDom.slice(-12000)}`
		);
	}
		if (!createdRecordPayload || createdRecordPayload.values.unit_price !== '9.99') {
			throw new Error('Expected created record payload with decimal string amount');
		}
		if (Object.prototype.hasOwnProperty.call(createdRecordPayload.values, 'ticket_no')) {
			throw new Error('Expected autonumber field to be omitted from user-created record payload');
		}
		if (Object.prototype.hasOwnProperty.call(createdRecordPayload.values, 'vip_note')) {
			throw new Error('Expected conditionally hidden field to be omitted from user-created record payload');
		}
		if (Object.prototype.hasOwnProperty.call(createdRecordPayload.values, 'print_jobs')) {
			throw new Error('Expected child_table field to be omitted from user-created record payload');
		}
		if (createdRecord?.values?.ticket_no !== 'AUTO-000001') {
			throw new Error('Expected mock API to return generated autonumber value');
		}
		assertAttachmentMetadataPayloads();
		const attachmentLinkedPayload = updatedRecordPayloads.find(
			(payload) =>
				payload?.source?.origin === 'attachment' &&
				payload?.values?.attachment_12 === 'https://assets.example.test/order-ticket.pdf'
		);
		const attachmentRemovedPayload = updatedRecordPayloads.find(
			(payload) => payload?.source?.origin === 'attachment' && payload?.values?.attachment_12 === ''
		);
		const localAttachmentLinkedPayload = updatedRecordPayloads.find(
			(payload) =>
				payload?.source?.origin === 'attachment' &&
				typeof payload?.values?.attachment_12 === 'string' &&
				payload.values.attachment_12 === '/api/v1/uploads/server-counter-ticket.png'
		);
		if (!attachmentLinkedPayload || !attachmentRemovedPayload || !localAttachmentLinkedPayload) {
			throw new Error('Expected attachment record value update payloads');
		}
		const multiFilterViewPayload = updatedViewPayloads.find(
			(payload) =>
				payload?.config?.filters?.[0]?.field === 'sku_name' &&
				payload.config.filters[0].value === 'Beef' &&
				payload.config.filters?.[1]?.field === 'quantity' &&
				payload.config.filters[1].operator === 'not_empty'
		);
		const disabledFilterViewPayload = updatedViewPayloads.find(
			(payload) =>
				payload?.config?.filter_expression?.children?.[0]?.children?.[0]?.disabled === true &&
				payload.config.filter_expression.children[0].children[0].filter?.field === 'sku_name' &&
				payload.config.filter_expression.children[0].children[0].filter?.value === 'Beef' &&
				payload.config.filter_expression.children[0].children?.[1]?.filter?.field === 'quantity'
		);
		const nestedFilterViewPayload = updatedViewPayloads.find(
			(payload) =>
				payload?.config?.filter_expression?.logic === 'any' &&
				payload.config.filter_expression.children?.[0]?.logic === 'all' &&
				payload.config.filter_expression.children[0].children?.[0]?.filter?.field === 'sku_name' &&
				payload.config.filter_expression.children[0].children?.[1]?.filter?.field === 'quantity' &&
				payload.config.filter_expression.children[0].children[1].filter.operator === 'not_empty' &&
				payload.config.filter_expression.children?.[1]?.logic === 'any' &&
				payload.config.filter_expression.children[1].children?.[0]?.filter?.field === 'status' &&
				payload.config.filter_expression.children[1].children[0].filter.value === 'served' &&
				payload.config.filter_expression.children[1].children?.[1]?.filter?.field === 'status' &&
				payload.config.filter_expression.children[1].children[1].filter.operator === 'not_equals' &&
				payload.config.filter_expression.children[1].children[1].filter.value === 'sent_to_kitchen'
		);
			const kanbanWipViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'kanban' &&
					payload?.config?.wip_limits?.served === 1 &&
					payload?.config?.swimlane_by === 'seat_no'
			);
			const pivotViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'seat_no' &&
					payload?.config?.pivot_column_field === 'status' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count'
			);
			const pivotSortViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'seat_no' &&
					payload?.config?.pivot_column_field === 'status' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_sort === 'row_total_asc'
			);
			const pivotSwapAxesViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count'
			);
			const pivotTotalsViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_totals === 'none'
			);
			const pivotValueDisplayViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_value_display === 'percent_total'
			);
			const pivotEmptyBucketsViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_value_display === 'percent_total' &&
					payload?.config?.pivot_empty_buckets === true
			);
			const pivotShowCountsViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_value_display === 'percent_total' &&
					payload?.config?.pivot_empty_buckets === true &&
					payload?.config?.pivot_show_counts === true
			);
			const pivotHeatmapViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_value_display === 'percent_total' &&
					payload?.config?.pivot_empty_buckets === true &&
					payload?.config?.pivot_show_counts === true &&
					payload?.config?.pivot_heatmap === true
			);
			const pivotHideZeroBucketsViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_value_display === 'percent_total' &&
					payload?.config?.pivot_empty_buckets === true &&
					payload?.config?.pivot_show_counts === true &&
					payload?.config?.pivot_heatmap === true &&
					payload?.config?.pivot_hide_zero_buckets === true
			);
			const pivotRowLimitViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'status' &&
					payload?.config?.pivot_column_field === 'seat_no' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_sort === 'row_total_desc' &&
					payload?.config?.pivot_empty_buckets === true &&
					payload?.config?.pivot_row_limit === 2
			);
			const pivotColumnLimitViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'pivot' &&
					payload?.config?.pivot_row_field === 'seat_no' &&
					payload?.config?.pivot_column_field === 'status' &&
					payload?.config?.pivot_value_field === 'line_total' &&
					payload?.config?.pivot_aggregate === 'count' &&
					payload?.config?.pivot_sort === 'column_total_desc' &&
					payload?.config?.pivot_empty_buckets === true &&
					payload?.config?.pivot_column_limit === 2
			);
			const ganttViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'gantt' &&
					payload?.config?.gantt_start_field === 'service_date' &&
					payload?.config?.gantt_end_field === 'service_end_date' &&
					payload?.config?.gantt_group_by === 'status' &&
					payload?.config?.gantt_dependency_field === 'order_ref' &&
					payload?.config?.gantt_scale === 'week'
			);
			const ganttRecordPayload = updatedRecordPayloads.find(
				(payload) =>
					payload?.source?.origin === 'gantt' &&
					payload?.values?.service_date === '2026-05-30'
			);
			const ganttResizeRecordPayload = updatedRecordPayloads.find(
				(payload) =>
					payload?.source?.origin === 'gantt_resize' &&
					payload?.values?.service_end_date === '2026-05-31'
			);
			const calendarEndResizeRecordPayload = updatedRecordPayloads.find(
				(payload) =>
					payload?.source?.origin === 'calendar_end_resize' &&
					payload?.values?.service_end_date === '2026-05-31'
			);
				const calendarRangeViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'calendar' &&
					payload?.config?.calendar_range === 'month' &&
					payload?.config?.calendar_recurrence === 'daily' &&
					payload?.config?.calendar_end_field === 'service_end_date' &&
					payload?.config?.calendar_resource_by === 'quantity'
			);
				const calendarExceptionViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'calendar' &&
						payload?.config?.calendar_exception_dates?.['record-order-line-created']?.includes(
							'2026-05-30'
						)
				);
				const calendarWeeklyViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'calendar' &&
						payload?.config?.calendar_recurrence === 'weekly'
				);
				const calendarMonthlyViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'calendar' &&
						payload?.config?.calendar_recurrence === 'monthly'
				);
				const calendarFocusNextViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'calendar' &&
						payload?.config?.calendar_recurrence === 'monthly' &&
						payload?.config?.calendar_focus_date === '2026-06-01'
				);
				const calendarFocusPreviousViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'calendar' &&
						payload?.config?.calendar_recurrence === 'monthly' &&
						payload?.config?.calendar_focus_date === '2026-05-01'
				);
				const calendarWorkweekViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'calendar' &&
						payload?.config?.calendar_recurrence === 'monthly' &&
						payload?.config?.calendar_focus_date === '2026-05-01' &&
						payload?.config?.calendar_day_scope === 'workweek'
				);
					const calendarAgendaViewPayload = updatedViewPayloads.find(
						(payload) =>
							payload?.view_type === 'calendar' &&
							payload?.config?.calendar_recurrence === 'monthly' &&
							payload?.config?.calendar_day_scope === 'all' &&
							payload?.config?.calendar_layout === 'agenda'
					);
					const calendarHideEmptyDaysViewPayload = updatedViewPayloads.find(
						(payload) =>
							payload?.view_type === 'calendar' &&
							payload?.config?.calendar_recurrence === 'monthly' &&
							payload?.config?.calendar_day_scope === 'all' &&
							payload?.config?.calendar_layout === 'grid' &&
							payload?.config?.calendar_hide_empty_days === true
					);
					const calendarSeriesRecordPayload = updatedRecordPayloads.find(
					(payload) =>
						payload?.source?.origin === 'calendar_series' &&
						payload?.values?.service_date === '2026-05-17' &&
						payload?.values?.service_end_date === '2026-05-19'
				);
				const cardLayoutViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'card' &&
							payload?.config?.card_title_field === 'sku_name' &&
							payload?.config?.card_subtitle_field === 'seat_no' &&
							payload?.config?.card_badge_field === 'status' &&
							payload?.config?.card_cover_field === 'dish_photo' &&
							payload?.config?.card_layout === 'gallery'
					);
				const cardCoverAspectViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'card' &&
						payload?.config?.card_title_field === 'sku_name' &&
						payload?.config?.card_subtitle_field === 'seat_no' &&
						payload?.config?.card_badge_field === 'status' &&
						payload?.config?.card_cover_field === 'dish_photo' &&
						payload?.config?.card_layout === 'gallery' &&
						payload?.config?.card_cover_aspect === 'portrait'
				);
				const cardCoverFitViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'card' &&
						payload?.config?.card_title_field === 'sku_name' &&
						payload?.config?.card_subtitle_field === 'seat_no' &&
						payload?.config?.card_badge_field === 'status' &&
						payload?.config?.card_cover_field === 'dish_photo' &&
						payload?.config?.card_layout === 'gallery' &&
						payload?.config?.card_cover_aspect === 'portrait' &&
						payload?.config?.card_cover_fit === 'contain'
				);
				const cardCoverPositionViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'card' &&
						payload?.config?.card_title_field === 'sku_name' &&
						payload?.config?.card_subtitle_field === 'seat_no' &&
						payload?.config?.card_badge_field === 'status' &&
						payload?.config?.card_cover_field === 'dish_photo' &&
						payload?.config?.card_layout === 'gallery' &&
						payload?.config?.card_cover_aspect === 'portrait' &&
						payload?.config?.card_cover_fit === 'contain' &&
						payload?.config?.card_cover_position === 'left'
				);
				const cardCompactLayoutViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'card' &&
						payload?.config?.card_title_field === 'sku_name' &&
						payload?.config?.card_subtitle_field === 'seat_no' &&
						payload?.config?.card_badge_field === 'status' &&
						payload?.config?.card_cover_field === 'dish_photo' &&
						payload?.config?.card_layout === 'compact'
				);
				const cardListLayoutViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'card' &&
						payload?.config?.card_title_field === 'sku_name' &&
						payload?.config?.card_subtitle_field === 'seat_no' &&
						payload?.config?.card_badge_field === 'status' &&
						payload?.config?.card_cover_field === 'dish_photo' &&
						payload?.config?.card_layout === 'list'
				);
						const cardHideEmptyFieldsViewPayload = updatedViewPayloads.find(
							(payload) =>
								payload?.view_type === 'card' &&
								payload?.config?.card_layout === 'list' &&
								payload?.config?.card_field_limit === 99 &&
								payload?.config?.card_hide_empty_fields === true &&
								Array.isArray(payload?.config?.columns) &&
								payload.config.columns.includes('dish_photo')
						);
							const cardShowTimestampsViewPayload = updatedViewPayloads.find(
								(payload) =>
									payload?.view_type === 'card' &&
									payload?.config?.card_layout === 'list' &&
									payload?.config?.card_field_limit === 99 &&
									payload?.config?.card_hide_empty_fields === true &&
									payload?.config?.card_show_timestamps === true
							);
							const cardShowRecordIdViewPayload = updatedViewPayloads.find(
								(payload) =>
									payload?.view_type === 'card' &&
									payload?.config?.card_layout === 'list' &&
									payload?.config?.card_field_limit === 99 &&
									payload?.config?.card_hide_empty_fields === true &&
									payload?.config?.card_show_timestamps === true &&
									payload?.config?.card_show_record_id === true
							);
							const cardGroupViewPayload = updatedViewPayloads.find(
							(payload) => payload?.view_type === 'card' && payload?.config?.group_by === 'status'
						);
						const cardGroupFilterViewPayload = updatedViewPayloads.find(
							(payload) =>
								payload?.view_type === 'card' &&
								payload?.config?.group_by === 'seat_no' &&
								payload?.config?.filter_expression?.children?.[0]?.children?.[0]?.filter?.field === 'seat_no' &&
								payload.config.filter_expression.children[0].children[0].filter.value === '1'
						);
					const cardFieldOrderViewPayload = updatedViewPayloads.find(
						(payload) =>
							payload?.view_type === 'card' &&
						payload?.config?.card_layout === 'compact' &&
						Array.isArray(payload?.config?.card_fields) &&
						payload.config.card_fields[0] === 'quantity' &&
						payload.config.card_fields[1] === 'sku_name'
				);
				const cardFieldLimitViewPayload = updatedViewPayloads.find(
					(payload) =>
						payload?.view_type === 'card' &&
						payload?.config?.card_title_field === 'sku_name' &&
						payload?.config?.card_subtitle_field === 'seat_no' &&
						payload?.config?.card_badge_field === 'status' &&
						payload?.config?.card_cover_field === 'dish_photo' &&
						payload?.config?.card_layout === 'compact' &&
						Array.isArray(payload?.config?.card_fields) &&
						payload.config.card_fields[0] === 'quantity' &&
						payload?.config?.card_field_limit === 3
				);
			const timelineLayoutViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'timeline' &&
					payload?.config?.timeline_title_field === 'sku_name' &&
					payload?.config?.timeline_subtitle_field === 'seat_no'
			);
			const timelineEventSourceViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'timeline' &&
					payload?.config?.timeline_source === 'events' &&
					payload?.config?.timeline_event_layout === 'detailed' &&
					payload?.config?.timeline_event_type === 'form.record.created' &&
					!payload?.config?.timeline_title_field &&
					!payload?.config?.timeline_subtitle_field
			);
			const timelineEventActorViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'timeline' &&
					payload?.config?.timeline_source === 'events' &&
					payload?.config?.timeline_event_layout === 'detailed' &&
					payload?.config?.timeline_event_actor === 'user-smoke' &&
					!payload?.config?.timeline_event_type &&
					!payload?.config?.timeline_event_aggregate
			);
			const timelineEventAggregateViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'timeline' &&
					payload?.config?.timeline_source === 'events' &&
					payload?.config?.timeline_event_layout === 'detailed' &&
					payload?.config?.timeline_event_type === 'form.record.created' &&
					payload?.config?.timeline_event_aggregate === 'form_record:record-order-line-created'
			);
			const timelineEventSearchViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'timeline' &&
					payload?.config?.timeline_source === 'events' &&
					payload?.config?.timeline_event_layout === 'detailed' &&
					payload?.config?.timeline_event_query === 'Green Tea' &&
					!payload?.config?.timeline_event_type &&
					!payload?.config?.timeline_event_actor &&
					!payload?.config?.timeline_event_aggregate
			);
			const timelineEventDateWindowViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'timeline' &&
					payload?.config?.timeline_source === 'events' &&
					payload?.config?.timeline_event_layout === 'detailed' &&
					payload?.config?.timeline_event_from === timelineEventWindowFrom &&
					payload?.config?.timeline_event_to === timelineEventWindowTo &&
					!payload?.config?.timeline_event_type &&
					!payload?.config?.timeline_event_actor &&
					!payload?.config?.timeline_event_aggregate &&
					!payload?.config?.timeline_event_query
			);
			const timelineEventDayQuickFilterViewPayload = updatedViewPayloads.find(
				(payload) =>
					payload?.view_type === 'timeline' &&
					payload?.config?.timeline_source === 'events' &&
					payload?.config?.timeline_event_layout === 'detailed' &&
					payload?.config?.timeline_event_from === timelineEventDayWindowFrom &&
					payload?.config?.timeline_event_to === timelineEventDayWindowTo &&
					!payload?.config?.timeline_event_type &&
					!payload?.config?.timeline_event_actor &&
					!payload?.config?.timeline_event_aggregate &&
					!payload?.config?.timeline_event_query
			);
		if (
			listedOrderLineViewIds.includes('view-order-line-private-other') ||
			!listedOrderLineViewIds.includes('view-order-line-default')
		) {
			throw new Error('Expected private views to be owner-filtered');
		}
		if (
			!multiFilterViewPayload?.config ||
			typeof multiFilterViewPayload.expected_updated_at !== 'string' ||
			multiFilterViewPayload.config.sort?.field !== 'quantity' ||
			multiFilterViewPayload.config.sort?.direction !== 'desc' ||
			multiFilterViewPayload.config.group_by !== 'status' ||
			multiFilterViewPayload.config.filter_groups?.[0]?.logic !== 'all' ||
			multiFilterViewPayload.config.visibility !== 'private' ||
			multiFilterViewPayload.config.is_default !== true ||
			orderLineViews.filter((view) => view.config?.is_default === true).length !== 1
		) {
			throw new Error('Expected saved view multi-filter config to be persisted');
		}
		if (!disabledFilterViewPayload?.config) {
			throw new Error('Expected saved view disabled filter config to be persisted');
		}
		if (
			!nestedFilterViewPayload?.config ||
			nestedFilterViewPayload.config.filter_groups?.length !== 2 ||
			nestedFilterViewPayload.config.filter_groups[0].logic !== 'all' ||
			nestedFilterViewPayload.config.filter_groups[1].logic !== 'any'
		) {
			throw new Error('Expected saved view nested filter expression config to be persisted');
		}
			if (!kanbanWipViewPayload?.config) {
				throw new Error('Expected kanban WIP policy and swimlane config to be persisted');
			}
			if (!pivotViewPayload?.config) {
				throw new Error('Expected pivot config to be persisted');
			}
			if (!pivotSortViewPayload?.config) {
				throw new Error('Expected pivot sort config to be persisted');
			}
			if (!pivotSwapAxesViewPayload?.config) {
				throw new Error('Expected pivot swap axes config to be persisted');
			}
			if (!pivotTotalsViewPayload?.config) {
				throw new Error('Expected pivot totals config to be persisted');
			}
			if (!pivotValueDisplayViewPayload?.config) {
				throw new Error('Expected pivot value display config to be persisted');
			}
			if (!pivotEmptyBucketsViewPayload?.config) {
				throw new Error('Expected pivot empty buckets config to be persisted');
			}
			if (!pivotShowCountsViewPayload?.config) {
				throw new Error('Expected pivot show counts config to be persisted');
			}
			if (!pivotHeatmapViewPayload?.config) {
				throw new Error('Expected pivot heatmap config to be persisted');
			}
			if (!pivotHideZeroBucketsViewPayload?.config) {
				throw new Error('Expected pivot hide zero buckets config to be persisted');
			}
			if (!pivotRowLimitViewPayload?.config) {
				throw new Error('Expected pivot row limit config to be persisted');
			}
			if (!pivotColumnLimitViewPayload?.config) {
				throw new Error('Expected pivot column limit config to be persisted');
			}
			if (!ganttViewPayload?.config) {
				throw new Error('Expected gantt config to be persisted');
			}
			if (!ganttRecordPayload) {
				throw new Error('Expected gantt record date update payload');
			}
			if (!ganttResizeRecordPayload) {
				throw new Error('Expected gantt resize record update payload');
			}
			if (!calendarEndResizeRecordPayload) {
				throw new Error('Expected calendar end date resize update payload');
			}
			if (!calendarRangeViewPayload?.config) {
				throw new Error('Expected calendar range and resource config to be persisted');
			}
				if (!calendarExceptionViewPayload?.config) {
					throw new Error('Expected calendar exception config to be persisted');
				}
				if (!calendarWeeklyViewPayload?.config || !calendarMonthlyViewPayload?.config) {
					throw new Error('Expected calendar weekly and monthly recurrence configs to be persisted');
				}
				if (!calendarFocusNextViewPayload?.config || !calendarFocusPreviousViewPayload?.config) {
					throw new Error('Expected calendar focus date navigation configs to be persisted');
				}
				if (!calendarWorkweekViewPayload?.config) {
					throw new Error('Expected calendar workweek config to be persisted');
				}
					if (!calendarAgendaViewPayload?.config) {
						throw new Error('Expected calendar agenda layout config to be persisted');
					}
					if (!calendarHideEmptyDaysViewPayload?.config) {
						throw new Error('Expected calendar hide empty days config to be persisted');
					}
					if (!calendarSeriesRecordPayload) {
					throw new Error('Expected calendar recurring series update payload');
				}
			if (!cardLayoutViewPayload?.config) {
				throw new Error('Expected card layout config to be persisted');
			}
			if (!cardCoverAspectViewPayload?.config) {
				throw new Error('Expected card cover aspect config to be persisted');
			}
			if (!cardCoverFitViewPayload?.config) {
				throw new Error('Expected card cover fit config to be persisted');
			}
			if (!cardCoverPositionViewPayload?.config) {
				throw new Error('Expected card cover position config to be persisted');
			}
			if (!cardCompactLayoutViewPayload?.config) {
				throw new Error('Expected card compact layout config to be persisted');
			}
			if (!cardListLayoutViewPayload?.config) {
				throw new Error('Expected card list layout config to be persisted');
			}
				if (!cardHideEmptyFieldsViewPayload?.config) {
					throw new Error('Expected card hide empty fields config to be persisted');
				}
					if (!cardShowTimestampsViewPayload?.config) {
						throw new Error('Expected card show timestamps config to be persisted');
					}
					if (!cardShowRecordIdViewPayload?.config) {
						throw new Error('Expected card show record ID config to be persisted');
					}
					if (!cardFieldOrderViewPayload?.config) {
				throw new Error('Expected card field order config to be persisted');
			}
			if (!cardFieldLimitViewPayload?.config) {
				throw new Error('Expected card field limit config to be persisted');
			}
			if (!timelineLayoutViewPayload?.config) {
				throw new Error('Expected timeline layout config to be persisted');
			}
			if (!timelineEventSourceViewPayload?.config) {
				throw new Error('Expected timeline event source config to be persisted');
			}
			if (!timelineEventActorViewPayload?.config) {
				throw new Error('Expected timeline event actor filter config to be persisted');
			}
			if (!timelineEventAggregateViewPayload?.config) {
				throw new Error('Expected timeline event aggregate filter config to be persisted');
			}
			if (!timelineEventSearchViewPayload?.config) {
				throw new Error('Expected timeline event search config to be persisted');
			}
			if (!timelineEventDateWindowViewPayload?.config) {
				throw new Error('Expected timeline event date window config to be persisted');
			}
			if (!timelineEventDayQuickFilterViewPayload?.config) {
				throw new Error('Expected timeline event day quick filter config to be persisted');
			}
			if (!cardGroupViewPayload?.config) {
				throw new Error('Expected card group_by config to be persisted');
			}
			if (!cardGroupFilterViewPayload?.config) {
				throw new Error('Expected card group filter config to be persisted');
			}
		if (
			updatedViewPayloads.length < 2 ||
			updatedViewPayloads[1].expected_updated_at !== new Date(Date.parse(now) + 1000).toISOString()
		) {
			throw new Error('Expected saved view updates to carry optimistic locking timestamps');
		}
		if (
			!orderLineRecordListRequests.some(
				(request) =>
					request.view_id === 'view-order-line-default' &&
					request.rows.length === 2 &&
					request.rows.includes('record-order-line-created') &&
					request.rows.includes('record-order-line-comparison')
			)
		) {
			throw new Error('Expected record list to use saved view nested filter expression on the server mock');
		}
		if (
			!orderLineExportRequests.some(
				(request) =>
					request.view_id === 'view-order-line-default' &&
					request.format === 'json' &&
					request.rows.length === 2 &&
					request.rows.includes('record-order-line-created') &&
					request.rows.includes('record-order-line-comparison')
			)
		) {
			throw new Error('Expected current view JSON export to use API format=json with saved view rules');
		}
		if (
			!orderLineExportRequests.some(
				(request) =>
					request.view_id === 'view-order-line-default' &&
					request.format === 'csv' &&
					request.rows.length === 1 &&
					request.rows.includes('record-order-line-created') &&
					!request.rows.includes('record-order-line-comparison')
			)
		) {
			throw new Error('Expected current view export to use saved view nested filter expression on the server mock');
		}
		if (memberPermissionPolicy.record_scope !== 'owned') {
			throw new Error('Expected member record ownership scope to be persisted');
		}
		if (
			memberPermissionPolicy.fields?.unit_price?.read !== false ||
			memberPermissionPolicy.fields?.unit_price?.write !== false ||
			memberPermissionPolicy.fields?.quantity?.write !== false
		) {
			throw new Error('Expected member field read/write policy to persist locked fields');
		}
		if (!updatedRecordPayloads.some((payload) => payload?.values?.status === 'cancelled')) {
			throw new Error('Expected kanban drag payload to move the record to cancelled');
		}
		const kanbanBulkPayloads = updatedRecordPayloads.filter(
			(payload) => payload?.source?.origin === 'kanban_bulk' && payload?.values?.status === 'draft'
		);
		if (kanbanBulkPayloads.length < 2) {
			throw new Error('Expected kanban bulk payloads to move selected records to draft');
		}
		if (
			!updatedRecordPayloads.some(
				(payload) =>
					payload?.source?.origin === 'calendar' &&
					payload?.values?.service_date === '2026-05-29' &&
					payload?.values?.service_end_date === '2026-05-30'
			)
		) {
			throw new Error('Expected calendar date update payload to move the record date range');
		}
		if (
			!updatedRecordPayloads.some(
				(payload) =>
					payload?.source?.origin === 'calendar_shift' &&
					payload?.values?.service_date === '2026-05-18' &&
					payload?.values?.service_end_date === '2026-05-20'
			) ||
			!updatedRecordPayloads.some(
				(payload) =>
					payload?.source?.origin === 'calendar_shift' &&
					payload?.values?.service_date === '2026-05-17' &&
					payload?.values?.service_end_date === '2026-05-19'
			)
		) {
			throw new Error('Expected calendar shift payloads to move and restore the record date range');
		}
		if (!updatedRecordPayloads.some((payload) => payload?.source?.origin === 'card' && payload?.values?.status === 'served')) {
			throw new Error('Expected card payload to update the record status');
		}
		if (!updatedRecordPayloads.some((payload) => payload?.source?.origin === 'timeline' && payload?.values?.status === 'sent_to_kitchen')) {
			throw new Error('Expected timeline payload to update the record status');
		}
		if (!createdLinkPayload || createdLinkPayload.relation_type !== 'parent_child') {
			throw new Error('Expected child link payload');
		}
			if (
				createdRecordPayload?.values?.guest_email !== 'guest@example.test' ||
					createdRecordPayload?.values?.guest_phone !== '+1 555 0100' ||
					createdRecordPayload?.values?.delivery_address !== '123 Test Ave' ||
						createdRecordPayload?.values?.delivery_location !== '40.7128,-74.0060' ||
						createdRecordPayload?.values?.service_rating !== '4' ||
						createdRecordPayload?.values?.prep_progress !== '75' ||
						createdRecordPayload?.values?.scan_code !== 'SKU-2026-0001' ||
						typeof createdRecordPayload?.values?.guest_signature !== 'string' ||
						!createdRecordPayload.values.guest_signature.startsWith('data:image/png') ||
						createdRecordPayload?.values?.service_status !== 'normal' ||
						createdRecordPayload?.values?.assignee?.user_id !== 'user-smoke' ||
						createdRecordPayload?.values?.assignee?.name !== 'Smoke Admin' ||
						createdRecordPayload?.values?.assignee?.email !== 'smoke@example.com'
					) {
						throw new Error('Expected contact, location, rating, progress, scan, signature, option config, and member field values in created record payload');
					}
	const relationField = orderLineForm.schema.fields.find((field) => field.field_id === 'fld_order_id');
	if (
		!relationField?.relation ||
		relationField.relation.form_key !== 'order' ||
		relationField.relation.relation_type !== 'parent_child' ||
		relationField.relation.relation_key !== 'order_lines'
	) {
		throw new Error('Expected relation metadata to be saved');
	}
	const formulaField = orderLineForm.schema.fields.find((field) => field.key === 'formula_11');
	if (
		!formulaField?.formula ||
		formulaField.formula.op !== 'multiply' ||
		!Array.isArray(formulaField.formula.args) ||
		formulaField.formula.args.join(',') !== 'quantity,unit_price'
	) {
		throw new Error('Expected formula metadata to be saved');
	}
	const unitPriceField = orderLineForm.schema.fields.find((field) => field.field_id === 'fld_unit_price');
	if (unitPriceField?.formula) {
		throw new Error('Expected ordinary amount field to stay free of default formula metadata');
	}
	const statusField = orderLineForm.schema.fields.find((field) => field.key === 'status');
	if (
		statusField?.option_colors?.draft !== '#64748b' ||
		statusField?.option_colors?.sent_to_kitchen !== '#f59e0b' ||
		statusField?.option_colors?.served !== '#16a34a' ||
		statusField?.option_colors?.cancelled !== '#dc2626'
	) {
		throw new Error('Expected option color metadata to be saved');
	}
	const optionColorField = orderLineForm.schema.fields.find((field) => field.key === 'service_status');
	if (
		optionColorField?.option_colors?.normal !== '#2563eb' ||
		optionColorField?.option_colors?.priority !== '#dc2626' ||
		optionColorField?.option_colors?.legacy !== '#64748b'
	) {
		throw new Error('Expected designer option color metadata to be saved');
	}
	if (optionColorField.default_value !== 'normal') {
		throw new Error('Expected designer default option metadata to be saved');
	}
	if (!Array.isArray(optionColorField?.option_disabled) || !optionColorField.option_disabled.includes('legacy')) {
		throw new Error('Expected designer disabled option metadata to be saved');
	}
	if (!Array.isArray(optionColorField?.option_archived) || !optionColorField.option_archived.includes('priority')) {
		throw new Error('Expected designer archived option metadata to be saved');
	}
	if (
		optionColorField?.option_descriptions?.normal !== 'Standard guest service' ||
		optionColorField?.option_descriptions?.priority !== 'Requires manager approval' ||
		optionColorField?.option_groups?.normal !== 'Service' ||
		optionColorField?.option_groups?.priority !== 'Escalated'
	) {
		throw new Error('Expected designer option description/group metadata to be saved');
	}
	const conditionalSignatureField = orderLineForm.schema.fields.find((field) => field.key === 'guest_signature');
	if (
		conditionalSignatureField?.conditional?.read_only_when?.logic !== 'all' ||
		!Array.isArray(conditionalSignatureField.conditional.read_only_when.conditions) ||
		conditionalSignatureField.conditional.read_only_when.conditions.length !== 2 ||
		conditionalSignatureField.conditional.read_only_when.conditions[0]?.field !== 'service_status' ||
		conditionalSignatureField.conditional.read_only_when.conditions[0]?.operator !== 'not_equals' ||
		conditionalSignatureField.conditional.read_only_when.conditions[0]?.value !== 'priority' ||
		conditionalSignatureField.conditional.read_only_when.conditions[1]?.field !== 'scan_code' ||
		conditionalSignatureField.conditional.read_only_when.conditions[1]?.operator !== 'contains' ||
		conditionalSignatureField.conditional.read_only_when.conditions[1]?.value !== 'SKU-2026'
	) {
		throw new Error('Expected conditional read-only multi-condition operator metadata to be saved');
	}
	const conditionalHiddenField = orderLineForm.schema.fields.find((field) => field.key === 'vip_note');
	if (
		conditionalHiddenField?.conditional?.hidden_when?.logic !== 'any' ||
		!Array.isArray(conditionalHiddenField.conditional.hidden_when.conditions) ||
		conditionalHiddenField.conditional.hidden_when.conditions.length !== 2 ||
		conditionalHiddenField.conditional.hidden_when.conditions[0]?.field !== 'service_status' ||
		conditionalHiddenField.conditional.hidden_when.conditions[0]?.equals !== 'normal' ||
		conditionalHiddenField.conditional.hidden_when.conditions[1]?.field !== 'service_rating' ||
		conditionalHiddenField.conditional.hidden_when.conditions[1]?.operator !== 'gte' ||
		conditionalHiddenField.conditional.hidden_when.conditions[1]?.value !== '4'
	) {
		throw new Error('Expected conditional hidden multi-condition operator metadata to be saved');
	}
	const childTableField = orderLineForm.schema.fields.find((field) => field.key === 'print_jobs');
	if (
		childTableField?.type !== 'child_table' ||
		childTableField?.relation?.target_type !== 'form_record' ||
		childTableField.relation.form_key !== 'print_job' ||
		childTableField.relation.relation_key !== 'line_print_jobs' ||
		childTableField.relation.relation_type !== 'parent_child' ||
		childTableField.relation.display_field !== 'job_type'
	) {
		throw new Error('Expected child_table relation metadata to be saved');
	}
		const emailField = orderLineForm.schema.fields.find((field) => field.key === 'guest_email');
		const phoneField = orderLineForm.schema.fields.find((field) => field.key === 'guest_phone');
		const addressField = orderLineForm.schema.fields.find((field) => field.key === 'delivery_address');
			const locationField = orderLineForm.schema.fields.find((field) => field.key === 'delivery_location');
			const ratingField = orderLineForm.schema.fields.find((field) => field.key === 'service_rating');
				const progressField = orderLineForm.schema.fields.find((field) => field.key === 'prep_progress');
				const scanField = orderLineForm.schema.fields.find((field) => field.key === 'scan_code');
				const signatureField = orderLineForm.schema.fields.find((field) => field.key === 'guest_signature');
				const autonumberField = orderLineForm.schema.fields.find((field) => field.key === 'ticket_no');
				const memberField = orderLineForm.schema.fields.find((field) => field.key === 'assignee');
		if (emailField?.type !== 'email' || phoneField?.type !== 'phone') {
			throw new Error('Expected phone/email field metadata to be saved');
		}
		if (addressField?.type !== 'address' || locationField?.type !== 'location') {
			throw new Error('Expected address/location field metadata to be saved');
		}
		if (ratingField?.type !== 'rating' || progressField?.type !== 'progress') {
			throw new Error('Expected rating/progress field metadata to be saved');
		}
		if (scanField?.type !== 'scan') {
			throw new Error('Expected scan field metadata to be saved');
		}
		if (signatureField?.type !== 'signature') {
			throw new Error('Expected signature field metadata to be saved');
		}
		if (autonumberField?.type !== 'autonumber') {
			throw new Error('Expected autonumber field metadata to be saved');
		}
		if (memberField?.type !== 'member') {
			throw new Error('Expected member field metadata to be saved');
		}
	const attachmentField = orderLineForm.schema.fields.find((field) => field.key === 'attachment_12');
	if (
		!attachmentField?.attachment ||
		attachmentField.attachment.max_size_mb !== 12 ||
		!Array.isArray(attachmentField.attachment.accept) ||
		attachmentField.attachment.accept.join(',') !== 'application/pdf,image/png'
	) {
		throw new Error('Expected attachment metadata to be saved');
	}
	const mobileDom = await runChromium(
		`http://127.0.0.1:${port}/smoke-auth-seed?mode=mobile`,
		'390,844',
		mobileScreenshot,
		10000
	);
		if (!mobileDom.includes('data-forms-mobile-smoke="done"')) {
			throw new Error(`Forms UI mobile smoke failed. DOM excerpt:\n${mobileDom.slice(-12000)}`);
		}
	console.log('Forms UI browser smoke passed');
	if (screenshotDir) {
		console.log(`Forms UI screenshots: ${desktopScreenshot}, ${mobileScreenshot}`);
	}
	}
} finally {
	server.close();
}

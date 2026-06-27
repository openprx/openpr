#!/usr/bin/env node
import { createServer } from 'node:http';
import { mkdir, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const screenshotDir = process.env.OPENPR_TEMPLATE_WORK_ITEMS_SCREENSHOT_DIR || '';
const workspaceId = 'ws-template-work-items';
const now = '2026-05-31T12:00:00.000Z';

const templatePlans = [
	{
		key: 'code_delivery_default',
		projectId: 'project-code-delivery',
		projectName: 'Code Template Work Items',
		projectKey: 'CODESMK',
		projectType: 'code_project',
		firstState: 'backlog',
		workflowStates: ['backlog', 'ready', 'done'],
		title: 'Review checkout repository',
		fields: { repo: 'https://example.com/acme/checkout', branch: 'main', ci_status: 'passing' },
		fieldSchema: [
			{ key: 'repo', label: 'Repository', type: 'text', required: true },
			{ key: 'branch', label: 'Branch', type: 'text' },
			{ key: 'ci_status', label: 'CI status', type: 'select', options: ['unknown', 'passing', 'failing'] }
		]
	},
	{
		key: 'contract_review_default',
		projectId: 'project-contract-review',
		projectName: 'Contract Template Work Items',
		projectKey: 'LEGALSMK',
		projectType: 'contract_review',
		firstState: 'intake',
		workflowStates: ['intake', 'risk_review', 'approval'],
		title: 'Review ACME contract',
		attachment: { filename: 'contract-evidence.png', mime: 'image/png' },
		fields: { counterparty: 'ACME Manufacturing', amount: '125000', risk_level: 'high' },
		fieldSchema: [
			{ key: 'counterparty', label: 'Counterparty', type: 'text', required: true },
			{ key: 'amount', label: 'Amount', type: 'number' },
			{ key: 'risk_level', label: 'Risk level', type: 'select', options: ['low', 'medium', 'high'], required: true }
		]
	},
	{
		key: 'equipment_maintenance_default',
		projectId: 'project-equipment-maintenance',
		projectName: 'Maintenance Template Work Items',
		projectKey: 'MAINTSMK',
		projectType: 'equipment_maintenance',
		firstState: 'reported',
		workflowStates: ['reported', 'triage', 'closed'],
		title: 'Inspect pump A17 alarm',
		attachment: { filename: 'pump-a17-photo.png', mime: 'image/png' },
		fields: { equipment_id: 'PUMP-A17', fault_type: 'Bearing temperature alarm', downtime_impact: 'major' },
		fieldSchema: [
			{ key: 'equipment_id', label: 'Equipment ID', type: 'text', required: true },
			{ key: 'fault_type', label: 'Fault type', type: 'text', required: true },
			{ key: 'downtime_impact', label: 'Downtime impact', type: 'select', options: ['none', 'minor', 'major'] }
		]
	},
	{
		key: 'quality_corrective_action_default',
		projectId: 'project-quality-corrective-action',
		projectName: 'Quality Template Work Items',
		projectKey: 'QASMK',
		projectType: 'quality_corrective_action',
		firstState: 'detected',
		workflowStates: ['detected', 'root_cause', 'closed'],
		title: 'Investigate batch Q2026 paint defect',
		attachment: { filename: 'quality-defect-photo.png', mime: 'image/png' },
		fields: { defect_type: 'Paint adhesion defect', batch: 'Q2026-05', root_cause: 'Surface preparation variance' },
		fieldSchema: [
			{ key: 'defect_type', label: 'Defect type', type: 'text', required: true },
			{ key: 'batch', label: 'Batch or order', type: 'text' },
			{ key: 'root_cause', label: 'Root cause', type: 'textarea' }
		]
	},
	{
		key: 'customer_delivery_default',
		projectId: 'project-customer-delivery',
		projectName: 'Delivery Template Work Items',
		projectKey: 'DELSMK',
		projectType: 'custom_form',
		firstState: 'planned',
		workflowStates: ['planned', 'acceptance', 'delivered'],
		title: 'Prepare ACME milestone acceptance',
		fields: { customer: 'ACME Manufacturing', milestone: 'Milestone 2 acceptance', delivery_risk: 'medium' },
		fieldSchema: [
			{ key: 'customer', label: 'Customer', type: 'text', required: true },
			{ key: 'milestone', label: 'Milestone', type: 'text', required: true },
			{ key: 'delivery_risk', label: 'Delivery risk', type: 'select', options: ['low', 'medium', 'high'] }
		]
	}
];

let createdIssuePayloads = [];
let createdIssuesByProject = {};
let uploadCount = 0;

function apiResult(data) {
	return { code: 0, message: 'success', data };
}

function paginated(items) {
	return { items, total: items.length, page: 1, per_page: items.length, total_pages: 1 };
}

function json(res, status, data) {
	res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });
	res.end(JSON.stringify(data));
}

function html(res, body) {
	res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
	res.end(body);
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

function planByTemplateKey(templateKey) {
	return templatePlans.find((plan) => plan.key === templateKey);
}

function planByProjectId(projectId) {
	return templatePlans.find((plan) => plan.projectId === projectId);
}

function projectForPlan(plan) {
	return {
		id: plan.projectId,
		workspace_id: workspaceId,
		name: plan.projectName,
		key: plan.projectKey,
		description: '',
		type_key: plan.projectType,
		type_settings: {
			scenario_template_key: plan.key,
			scenario_template: {
				key: plan.key,
				name: plan.projectName,
				project_type_key: plan.projectType,
				field_schema: { fields: plan.fieldSchema },
				workflow_template: { states: plan.workflowStates.map((key, index) => ({ key, initial: index === 0 })) },
				resource_schema: { resources: [] },
				ai_roles: [],
				connector_suggestions: [],
				governance_policy: {},
				sample_data: {}
			}
		},
		created_at: now,
		updated_at: now,
		issue_counts: null
	};
}

function workflowForPlan(plan) {
	return {
		id: `workflow-${plan.key}`,
		name: `${plan.projectName} workflow`,
		states: plan.workflowStates.map((key, index) => ({
			key,
			display_name: key.replaceAll('_', ' '),
			category: index === plan.workflowStates.length - 1 ? 'done' : 'active',
			position: index + 1,
			is_initial: index === 0,
			is_terminal: index === plan.workflowStates.length - 1
		}))
	};
}

function injectedSmoke(plan) {
	return `<script>
const plan = ${JSON.stringify(plan)};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const findButton = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.includes(text));
const findButtonExact = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.trim() === text);
const findInput = (placeholder) => Array.from(document.querySelectorAll('input')).find((input) => input.placeholder.includes(placeholder));
const textInElements = (text, selector = 'a,button,h1,h2,h3,p,span,td,div') => Array.from(document.querySelectorAll(selector)).some((el) => el.textContent.includes(text));
async function waitFor(check, label) {
  for (let i = 0; i < 80; i += 1) {
    const value = check();
    if (value) return value;
    await sleep(150);
  }
  throw new Error('Timed out waiting for ' + label);
}
async function setScenarioField(key, value) {
  const control = await waitFor(() => document.querySelector('#scenario-field-' + key), 'scenario field ' + key);
  control.value = value;
  control.dispatchEvent(new Event(control.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
  control.dispatchEvent(new Event('input', { bubbles: true }));
}
(async () => {
  await waitFor(() => Array.from(document.querySelectorAll('#statusFilter option')).some((option) => option.value === plan.firstState), 'workflow loaded');
  const createIssue = await waitFor(() => findButton('Create Issue'), 'create issue button');
  createIssue.click();
  for (const [key, value] of Object.entries(plan.fields)) {
    await setScenarioField(key, value);
  }
  if (plan.attachment) {
    const fileInput = await waitFor(() => document.querySelector('input[type="file"]'), 'attachment input');
    const dataTransfer = new DataTransfer();
    dataTransfer.items.add(new File(['smoke attachment'], plan.attachment.filename, { type: plan.attachment.mime }));
    fileInput.files = dataTransfer.files;
    fileInput.dispatchEvent(new Event('change', { bubbles: true }));
    await waitFor(() => document.querySelector('#issueDescription')?.value.includes('/api/v1/uploads/'), 'uploaded attachment markdown');
  }
  const issueTitle = await waitFor(() => findInput('Fix login'), 'issue title');
  issueTitle.value = plan.title;
  issueTitle.dispatchEvent(new Event('input', { bubbles: true }));
  findButtonExact('Create Issue').click();
  await waitFor(() => textInElements(plan.title, 'p,td,button,span'), 'created scenario issue');
  document.body.setAttribute('data-template-work-item-smoke', plan.key);
})().catch((error) => {
  document.body.setAttribute('data-template-work-item-smoke', 'failed');
  document.body.setAttribute('data-template-work-item-smoke-error', error.message);
});
</script>`;
}

async function serveStatic(req, res, templateKey = null) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	let pathname = decodeURIComponent(url.pathname);
	if (pathname === '/') pathname = '/index.html';
	let filePath = normalize(join(buildDir, pathname));
	if (!filePath.startsWith(buildDir) || !existsSync(filePath)) {
		filePath = join(buildDir, 'index.html');
	}

	try {
		let data = await readFile(filePath, 'utf8');
		const plan = templateKey ? planByTemplateKey(templateKey) : null;
		if (plan && basename(filePath) === 'index.html') {
			data = data.replace('</body>', `${injectedSmoke(plan)}</body>`);
		}
		res.writeHead(200, { 'Content-Type': contentType(filePath) });
		res.end(data);
	} catch {
		res.writeHead(404);
		res.end('Not found');
	}
}

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const pathname = url.pathname;

	if (pathname === '/smoke-template-work-item-seed') {
		const templateKey = url.searchParams.get('template') ?? '';
		const plan = planByTemplateKey(templateKey);
		if (!plan) {
			html(res, '<!doctype html><meta charset="utf-8">Unknown template');
			return;
		}
		html(
			res,
			`<!doctype html><meta charset="utf-8"><script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace('/workspace/${workspaceId}/projects/${plan.projectId}/issues?template_work_item_smoke=${plan.key}');
</script>`
		);
		return;
	}

	if (pathname === '/api/v1/auth/me') {
		json(res, 200, apiResult({ user: { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' } }));
		return;
	}
	if (pathname === '/api/v1/notifications/unread-count') {
		json(res, 200, apiResult({ count: 0 }));
		return;
	}
	if (pathname === '/api/v1/workspaces') {
		json(res, 200, apiResult(paginated([{ id: workspaceId, slug: 'template-work-items', name: 'Template Work Items', created_at: now, updated_at: now }])));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}`) {
		json(res, 200, apiResult({ id: workspaceId, slug: 'template-work-items', name: 'Template Work Items', role: 'owner', created_at: now, updated_at: now }));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/members`) {
		json(res, 200, apiResult(paginated([{ user_id: 'user-smoke', name: 'Smoke Admin', email: 'smoke@example.com', role: 'owner' }])));
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/labels`) {
		json(res, 200, apiResult(paginated([])));
		return;
	}
	if (pathname === '/api/v1/upload' && req.method === 'POST') {
		for await (const _chunk of req) {
			// Drain request body so XMLHttpRequest observes a completed upload.
		}
		uploadCount += 1;
		json(res, 200, apiResult({ url: `/api/v1/uploads/smoke-attachment-${uploadCount}.png`, filename: `smoke-attachment-${uploadCount}.png` }));
		return;
	}

	const projectMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)$/);
	if (projectMatch && req.method === 'GET') {
		const plan = planByProjectId(projectMatch[1]);
		json(res, 200, apiResult(plan ? projectForPlan(plan) : null));
		return;
	}

	const workflowMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)\/workflow\/effective$/);
	if (workflowMatch && req.method === 'GET') {
		json(res, 200, apiResult(workflowForPlan(planByProjectId(workflowMatch[1]))));
		return;
	}

	const sprintsMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)\/sprints$/);
	if (sprintsMatch && req.method === 'GET') {
		json(res, 200, apiResult(paginated([])));
		return;
	}

	const issuesMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)\/issues$/);
	if (issuesMatch && req.method === 'GET') {
		json(res, 200, apiResult(paginated(createdIssuesByProject[issuesMatch[1]] ?? [])));
		return;
	}
	if (issuesMatch && req.method === 'POST') {
		const projectId = issuesMatch[1];
		const plan = planByProjectId(projectId);
		const payload = await readBody(req);
		createdIssuePayloads = [...createdIssuePayloads, { template_key: plan.key, project_id: projectId, payload }];
		const existing = createdIssuesByProject[projectId] ?? [];
		const issue = {
			id: `issue-${projectId}-${existing.length + 1}`,
			project_id: projectId,
			title: payload.title,
			description: payload.description ?? '',
			status: payload.state ?? plan.firstState,
			priority: payload.priority ?? 'medium',
			reporter_id: 'user-smoke',
			created_at: now,
			updated_at: now,
			key: `${plan.projectKey}-${existing.length + 1}`,
			labels: []
		};
		createdIssuesByProject = { ...createdIssuesByProject, [projectId]: [issue, ...existing] };
		json(res, 200, apiResult(issue));
		return;
	}

	if (pathname.startsWith('/api/')) {
		json(res, 404, { code: 404, message: `unmocked ${pathname}`, data: null });
		return;
	}

	await serveStatic(req, res, url.searchParams.get('template_work_item_smoke'));
}

function runChromium(url, windowSize = '1366,900', screenshotPath = null) {
	return new Promise((resolvePromise) => {
		const args = [
			'--headless=new',
			'--disable-gpu',
			'--disable-dev-shm-usage',
			'--no-sandbox',
			'--hide-scrollbars',
			'--run-all-compositor-stages-before-draw',
			`--window-size=${windowSize}`,
			'--virtual-time-budget=16000',
			'--dump-dom',
			url
		];
		if (screenshotPath) args.push(`--screenshot=${screenshotPath}`);
		const child = spawn(chromium, args);
		let stdout = '';
		let stderr = '';
		child.stdout.on('data', (chunk) => {
			stdout += chunk.toString();
		});
		child.stderr.on('data', (chunk) => {
			stderr += chunk.toString();
		});
		const timeout = setTimeout(() => {
			stderr += '\\nChromium smoke timed out after 30s';
			child.kill('SIGKILL');
		}, 30_000);
		child.on('close', (code) => {
			clearTimeout(timeout);
			resolvePromise({ code, stdout, stderr });
		});
	});
}

if (!existsSync(chromium)) {
	throw new Error(`Chromium binary not found at ${chromium}. Set CHROMIUM_BIN to override.`);
}

const server = createServer((req, res) => {
	handler(req, res).catch((error) => {
		res.writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8' });
		res.end(error instanceof Error ? error.stack : String(error));
	});
});

server.listen(0, '127.0.0.1', async () => {
	const address = server.address();
	const port = typeof address === 'object' && address ? address.port : 0;
	if (screenshotDir) await mkdir(screenshotDir, { recursive: true });
	const desktopScreenshot = screenshotDir ? join(screenshotDir, 'template-work-items-desktop.png') : null;
	const mobileScreenshot = screenshotDir ? join(screenshotDir, 'template-work-items-mobile.png') : null;
	try {
		for (const plan of templatePlans) {
			const url = `http://127.0.0.1:${port}/smoke-template-work-item-seed?template=${plan.key}`;
			const result = await runChromium(
				url,
				'1366,900',
				plan.key === 'contract_review_default' ? desktopScreenshot : null
			);
			if (result.code !== 0) {
				throw new Error(`Chromium exited with ${result.code} for ${plan.key}\\n${result.stderr}`);
			}
			if (!result.stdout.includes(`data-template-work-item-smoke="${plan.key}"`)) {
				const errorAttr = result.stdout.match(/data-template-work-item-smoke-error="[^"]+"/)?.[0] ?? 'no smoke error attribute';
				throw new Error(`Template work-item smoke failed for ${plan.key}: ${errorAttr}`);
			}
		}
		if (screenshotDir) {
			const mobilePlan = planByTemplateKey('contract_review_default');
			const mobileUrl = `http://127.0.0.1:${port}/smoke-template-work-item-seed?template=${mobilePlan.key}`;
			const mobileResult = await runChromium(mobileUrl, '390,844', mobileScreenshot);
			if (mobileResult.code !== 0) {
				throw new Error(`Mobile Chromium exited with ${mobileResult.code} for ${mobilePlan.key}\\n${mobileResult.stderr}`);
			}
			if (!mobileResult.stdout.includes(`data-template-work-item-smoke="${mobilePlan.key}"`)) {
				const errorAttr = mobileResult.stdout.match(/data-template-work-item-smoke-error="[^"]+"/)?.[0] ?? 'no smoke error attribute';
				throw new Error(`Mobile template work-item smoke failed for ${mobilePlan.key}: ${errorAttr}`);
			}
		}
	} finally {
		server.close();
	}

	for (const plan of templatePlans) {
		const entry = createdIssuePayloads.find((item) => item.template_key === plan.key);
		if (!entry) {
			throw new Error(`Missing issue payload for ${plan.key}`);
		}
		for (const [fieldKey, fieldValue] of Object.entries(plan.fields)) {
			if (!entry.payload.description?.includes(String(fieldValue))) {
				throw new Error(`Issue payload for ${plan.key} missing ${fieldKey}: ${entry.payload.description}`);
			}
		}
		if (plan.attachment && !entry.payload.description?.includes('/api/v1/uploads/smoke-attachment-')) {
			throw new Error(`Issue payload for ${plan.key} missing uploaded attachment: ${entry.payload.description}`);
		}
	}

	if (screenshotDir) {
		console.log(`Template work-item screenshots: ${desktopScreenshot}, ${mobileScreenshot}`);
	}
	console.log('Template work-item browser smoke passed');
});

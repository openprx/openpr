#!/usr/bin/env node
import { createServer } from 'node:http';
import { mkdir, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const screenshotDir = process.env.OPENPR_PROJECT_TEMPLATE_SCREENSHOT_DIR || '';
const workspaceId = 'ws-template-smoke';
const now = '2026-05-31T12:00:00.000Z';

let createdProjectPayload = null;
let createdProjectPayloads = [];
let createdProjects = [];
let createdIssuePayload = null;
let createdIssuePayloads = [];
let createdIssues = [];
let createdIssuesByProject = {};
let aiAgents = [];
let assistantPatchPayload = null;

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

function assertIncludes(text, needle) {
	if (!text.includes(needle)) {
		throw new Error(`Expected DOM to include ${JSON.stringify(needle)}`);
	}
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

async function serveStatic(req, res, wizardMode = 'none') {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	let pathname = decodeURIComponent(url.pathname);
	if (pathname === '/') pathname = '/index.html';
	let filePath = normalize(join(buildDir, pathname));
	if (!filePath.startsWith(buildDir) || !existsSync(filePath)) {
		filePath = join(buildDir, 'index.html');
	}

	try {
		let data = await readFile(filePath, 'utf8');
		if (wizardMode === 'smoke' && basename(filePath) === 'index.html') {
			data = data.replace(
				'</body>',
				`<script>
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
function projectIdForTemplate(templateKey) {
  if (templateKey === 'contract_review_default') return 'project-created-smoke';
  return 'project-' + templateKey.replace(/_default$/, '').replaceAll('_', '-');
}
async function setScenarioField(key, value) {
  const control = await waitFor(() => document.querySelector('#scenario-field-' + key), 'scenario field ' + key);
  control.value = value;
  control.dispatchEvent(new Event(control.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
  control.dispatchEvent(new Event('input', { bubbles: true }));
}
async function createScenarioIssue(plan) {
  const createIssue = await waitFor(() => findButton('Create Issue'), 'create issue button ' + plan.data);
  createIssue.click();
  await waitFor(() => document.querySelector('#scenario-field-' + Object.keys(plan.fields)[0]), 'first scenario field ' + plan.data);
  if (plan.data === 'contract_review_default') {
    mark('data-scenario-issue-fields-seen');
  }
  for (const [key, value] of Object.entries(plan.fields)) {
    await setScenarioField(key, value);
  }
  const issueTitle = await waitFor(() => findInput('Fix login'), 'issue title ' + plan.data);
  issueTitle.value = plan.title;
  issueTitle.dispatchEvent(new Event('input', { bubbles: true }));
  findButtonExact('Create Issue').click();
  await waitFor(() => textInElements(plan.title, 'p,td,button,span'), 'created scenario issue ' + plan.data);
  mark('data-scenario-issue-create-' + plan.data);
}
async function navigateWithinApp(path) {
  const link = document.createElement('a');
  link.href = path;
  document.body.appendChild(link);
  link.click();
  link.remove();
  await waitFor(() => location.pathname === path, 'navigate ' + path);
}
function mark(name, value = 'done') {
  document.body.setAttribute(name, value);
  sessionStorage.setItem('template-smoke:' + name, value);
}
function restoreMarks() {
  for (let index = 0; index < sessionStorage.length; index += 1) {
    const key = sessionStorage.key(index);
    if (key && key.startsWith('template-smoke:data-')) {
      document.body.setAttribute(key.replace('template-smoke:', ''), sessionStorage.getItem(key) || 'done');
    }
  }
}
(async () => {
  for (const key of Object.keys(sessionStorage)) {
    if (key.startsWith('template-smoke:')) sessionStorage.removeItem(key);
  }
  const plans = [
    { template: 'Code Delivery', name: 'Code Template Smoke', key: 'CODESMK', data: 'code_delivery_default' },
    { template: 'Contract Review', name: 'Contract Template Smoke', key: 'LEGALSMK', data: 'contract_review_default' },
    { template: 'Equipment Maintenance', name: 'Maintenance Template Smoke', key: 'MAINTSMK', data: 'equipment_maintenance_default' },
    { template: 'Quality Corrective Action', name: 'Quality Template Smoke', key: 'QASMK', data: 'quality_corrective_action_default' },
    { template: 'Customer Delivery', name: 'Delivery Template Smoke', key: 'DELSMK', data: 'customer_delivery_default' }
  ];

  for (const plan of plans) {
    const create = await waitFor(() => findButton('Create Project'), 'create button ' + plan.key);
    create.click();
    const templateButton = await waitFor(() => findButton(plan.template), 'template button ' + plan.template);
    if (plan.data === 'contract_review_default') {
      mark('data-template-card-seen', 'contract_review_default');
    }
    templateButton.click();
    const name = await waitFor(() => findInput('Mobile App'), 'name input ' + plan.key);
    const key = await waitFor(() => findInput('MOBILE'), 'key input ' + plan.key);
    name.value = plan.name;
    name.dispatchEvent(new Event('input', { bubbles: true }));
    key.value = plan.key;
    key.dispatchEvent(new Event('input', { bubbles: true }));
    findButtonExact('Create').click();
    await waitFor(() => textInElements(plan.name), 'created project ' + plan.key);
    mark('data-template-created-' + plan.data);
  }

  const projectButton = await waitFor(() => findButton('Contract Template Smoke'), 'created project');
  projectButton.click();
  await waitFor(() => textInElements('Document review assistant'), 'scenario dashboard');
  mark('data-scenario-dashboard-seen');
  await waitFor(() => textInElements('Scenario Setup'), 'scenario setup');
  const modelInput = await waitFor(() => Array.from(document.querySelectorAll('input')).find((input) => input.value === 'mcp-agent'), 'assistant model input');
  modelInput.value = 'mcp-agent-smoke';
  modelInput.dispatchEvent(new Event('input', { bubbles: true }));
  const apiEndpointInput = await waitFor(() => Array.from(document.querySelectorAll('input')).find((input) => input.placeholder === 'https://...'), 'assistant endpoint input');
  apiEndpointInput.value = 'http://127.0.0.1:8090/mcp/rpc';
  apiEndpointInput.dispatchEvent(new Event('input', { bubbles: true }));
  const enableAssistant = await waitFor(() => findButtonExact('Enable'), 'enable assistant');
  enableAssistant.click();
  await waitFor(() => findButtonExact('Save assistant'), 'assistant saved');
  mark('data-scenario-assistant-enabled');
  const workLink = await waitFor(() => Array.from(document.querySelectorAll('a')).find((link) => link.textContent.includes('Open Work') || link.getAttribute('href')?.endsWith('/issues')), 'open work link');
  workLink.click();
  await createScenarioIssue({ data: 'contract_review_default', title: 'Review ACME contract', fields: { counterparty: 'ACME Manufacturing', amount: '125000', risk_level: 'high' } });
  mark('data-scenario-issue-create');
  document.body.setAttribute('data-template-smoke', 'done');
})().catch((error) => {
  document.body.setAttribute('data-template-smoke', 'failed');
  document.body.setAttribute('data-template-smoke-error', error.message);
});
</script></body>`
			);
		} else if (wizardMode === 'capture' && basename(filePath) === 'index.html') {
			data = data.replace(
				'</body>',
				`<script>
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const findButton = (text) => Array.from(document.querySelectorAll('button')).find((button) => button.textContent.includes(text));
async function waitFor(check, label) {
  for (let i = 0; i < 80; i += 1) {
    const value = check();
    if (value) return value;
    await sleep(150);
  }
  throw new Error('Timed out waiting for ' + label);
}
(async () => {
  const create = await waitFor(() => findButton('Create Project'), 'create project button');
  create.click();
  await waitFor(() => findButton('Contract Review'), 'template buttons');
  document.body.setAttribute('data-template-mobile-capture', 'done');
})().catch((error) => {
  document.body.setAttribute('data-template-mobile-capture', 'failed');
  document.body.setAttribute('data-template-mobile-capture-error', error.message);
});
</script></body>`
			);
		}
		res.writeHead(200, { 'Content-Type': contentType(filePath) });
		res.end(data);
	} catch {
		res.writeHead(404);
		res.end('Not found');
	}
}

const projectTypes = [
	{
		key: 'code_project',
		workspace_id: null,
		name: 'Code Project',
		description: 'Software delivery project',
		domain: 'software',
		default_workflow_id: null,
		enabled_capabilities: [],
		field_schema: {},
		artifact_schema: {},
		created_at: now,
		updated_at: now
	},
	{
		key: 'contract_review',
		workspace_id: null,
		name: 'Contract Review',
		description: 'Contract review workflow',
		domain: 'legal',
		default_workflow_id: null,
		enabled_capabilities: [],
		field_schema: {},
		artifact_schema: {},
		created_at: now,
		updated_at: now
	},
	{
		key: 'equipment_maintenance',
		workspace_id: null,
		name: 'Equipment Maintenance',
		description: 'Equipment maintenance workflow',
		domain: 'operations',
		default_workflow_id: null,
		enabled_capabilities: [],
		field_schema: {},
		artifact_schema: {},
		created_at: now,
		updated_at: now
	},
	{
		key: 'quality_corrective_action',
		workspace_id: null,
		name: 'Quality Corrective Action',
		description: 'Quality corrective action workflow',
		domain: 'quality',
		default_workflow_id: null,
		enabled_capabilities: [],
		field_schema: {},
		artifact_schema: {},
		created_at: now,
		updated_at: now
	},
	{
		key: 'custom_form',
		workspace_id: null,
		name: 'Customer Delivery',
		description: 'Customer delivery workflow',
		domain: 'delivery',
		default_workflow_id: null,
		enabled_capabilities: [],
		field_schema: {},
		artifact_schema: {},
		created_at: now,
		updated_at: now
	}
];

const scenarioTemplates = [
	{
		id: 'template-code',
		key: 'code_delivery_default',
		workspace_id: null,
		name: 'Code Delivery',
		description: 'AI-assisted software delivery',
		industry: 'software',
		project_type_key: 'code_project',
		audience_label: 'AI coding assistant',
		workflow_template: { states: [{ key: 'backlog' }, { key: 'done' }], default_labels: ['review'] },
		field_schema: { fields: [{ key: 'repo', label: 'Repository', type: 'text', required: true }] },
		resource_schema: { resources: [{ kind: 'repo', label: 'Repository', required: true }] },
		ai_roles: [{ key: 'coding_assistant', label: 'AI coding assistant', agent_type: 'cli', capabilities: ['code.task_context.get'] }],
		governance_policy: {},
		sample_data: {},
		created_at: now,
		updated_at: now
	},
	{
		id: 'template-contract',
		key: 'contract_review_default',
		workspace_id: null,
		name: 'Contract Review',
		description: 'Document-centered review',
		industry: 'legal',
		project_type_key: 'contract_review',
		audience_label: 'Document review assistant',
		workflow_template: {
			states: [{ key: 'intake' }, { key: 'risk_review' }, { key: 'approval' }],
			default_labels: ['legal-risk', 'approval-needed']
		},
		field_schema: {
			fields: [
				{ key: 'counterparty', label: 'Counterparty', type: 'text', required: true },
				{ key: 'amount', label: 'Amount', type: 'number' },
				{ key: 'risk_level', label: 'Risk level', type: 'select', options: ['low', 'medium', 'high'], required: true }
			]
		},
		resource_schema: { resources: [{ kind: 'document_library', label: 'Contract documents', required: true }] },
		ai_roles: [
			{ key: 'document_assistant', label: 'Document review assistant', agent_type: 'mcp', provider: 'openpr-mcp', model: 'mcp-agent', capabilities: ['documents.extract_summary', 'documents.review_risk'], writes_require_approval: true },
			{ key: 'approval_assistant', label: 'Approval assistant', agent_type: 'webhook', provider: 'openpr-webhook', model: 'external-agent', capabilities: ['approval.request'], writes_require_approval: true }
		],
		governance_policy: {},
		sample_data: {},
		created_at: now,
		updated_at: now
	},
	{
		id: 'template-maintenance',
		key: 'equipment_maintenance_default',
		workspace_id: null,
		name: 'Equipment Maintenance',
		description: 'Operations workflow for fault reports and inspection acceptance',
		industry: 'operations',
		project_type_key: 'equipment_maintenance',
		audience_label: 'Maintenance assistant',
		workflow_template: { states: [{ key: 'reported' }, { key: 'triage' }, { key: 'closed' }], default_labels: ['safety', 'inspection-needed'] },
		field_schema: {
			fields: [
				{ key: 'equipment_id', label: 'Equipment ID', type: 'text', required: true },
				{ key: 'fault_type', label: 'Fault type', type: 'text', required: true },
				{ key: 'downtime_impact', label: 'Downtime impact', type: 'select', options: ['none', 'minor', 'major', 'critical'] }
			]
		},
		resource_schema: { resources: [{ kind: 'equipment', label: 'Equipment record', required: true }] },
		ai_roles: [{ key: 'maintenance_assistant', label: 'Maintenance assistant', agent_type: 'webhook', capabilities: ['inspection.report'] }],
		governance_policy: {},
		sample_data: {},
		created_at: now,
		updated_at: now
	},
	{
		id: 'template-quality',
		key: 'quality_corrective_action_default',
		workspace_id: null,
		name: 'Quality Corrective Action',
		description: 'Quality workflow for defect classification and recheck',
		industry: 'quality',
		project_type_key: 'quality_corrective_action',
		audience_label: 'Quality audit assistant',
		workflow_template: { states: [{ key: 'detected' }, { key: 'root_cause' }, { key: 'closed' }], default_labels: ['defect', 'corrective-action'] },
		field_schema: {
			fields: [
				{ key: 'defect_type', label: 'Defect type', type: 'text', required: true },
				{ key: 'batch', label: 'Batch or order', type: 'text' },
				{ key: 'root_cause', label: 'Root cause', type: 'textarea' }
			]
		},
		resource_schema: { resources: [{ kind: 'document_library', label: 'Quality documents' }] },
		ai_roles: [{ key: 'quality_audit_assistant', label: 'Quality audit assistant', agent_type: 'mcp', capabilities: ['corrective_action.propose'] }],
		governance_policy: {},
		sample_data: {},
		created_at: now,
		updated_at: now
	},
	{
		id: 'template-delivery',
		key: 'customer_delivery_default',
		workspace_id: null,
		name: 'Customer Delivery',
		description: 'Delivery governance for customer milestones and acceptance',
		industry: 'delivery',
		project_type_key: 'custom_form',
		audience_label: 'Delivery assistant',
		workflow_template: { states: [{ key: 'planned' }, { key: 'acceptance' }, { key: 'delivered' }], default_labels: ['milestone', 'acceptance'] },
		field_schema: {
			fields: [
				{ key: 'customer', label: 'Customer', type: 'text', required: true },
				{ key: 'milestone', label: 'Milestone', type: 'text', required: true },
				{ key: 'delivery_risk', label: 'Delivery risk', type: 'select', options: ['low', 'medium', 'high'] }
			]
		},
		resource_schema: { resources: [{ kind: 'crm_account', label: 'Customer account', required: true }] },
		ai_roles: [{ key: 'delivery_assistant', label: 'Delivery assistant', agent_type: 'mcp', capabilities: ['approval.request'] }],
		governance_policy: {},
		sample_data: {},
		created_at: now,
		updated_at: now
	}
];

const embeddedContractTemplate = {
	key: 'contract_review_default',
	name: 'Contract Review',
	industry: 'legal',
	project_type_key: 'contract_review',
	audience_label: 'Document review assistant',
	workflow_template: scenarioTemplates[1].workflow_template,
	field_schema: scenarioTemplates[1].field_schema,
	resource_schema: scenarioTemplates[1].resource_schema,
	ai_roles: scenarioTemplates[1].ai_roles,
	governance_policy: scenarioTemplates[1].governance_policy,
	sample_data: scenarioTemplates[1].sample_data
};

function projectIdForTemplate(templateKey) {
	if (templateKey === 'contract_review_default') return 'project-created-smoke';
	return `project-${templateKey.replace(/_default$/, '').replaceAll('_', '-')}`;
}

function embeddedTemplate(template) {
	return {
		key: template.key,
		name: template.name,
		industry: template.industry,
		project_type_key: template.project_type_key,
		audience_label: template.audience_label,
		workflow_template: template.workflow_template,
		field_schema: template.field_schema,
		resource_schema: template.resource_schema,
		ai_roles: template.ai_roles,
		governance_policy: template.governance_policy,
		sample_data: template.sample_data
	};
}

function projectById(projectId) {
	return createdProjects.find((project) => project.id === projectId);
}

function templateForProjectId(projectId) {
	const project = projectById(projectId);
	const templateKey = project?.type_settings?.scenario_template_key;
	return scenarioTemplates.find((template) => template.key === templateKey) ?? scenarioTemplates[1];
}

function workflowForTemplate(template) {
	const states = Array.isArray(template.workflow_template?.states) ? template.workflow_template.states : [];
	return {
		id: `workflow-${template.key}`,
		name: `${template.name} workflow`,
		states: states.map((state, index) => {
			const key = String(state.key ?? `state_${index + 1}`);
			const isTerminal = Boolean(state.terminal) || ['done', 'closed', 'signed', 'delivered'].includes(key);
			return {
				key,
				display_name: String(state.name ?? key.replaceAll('_', ' ')),
				category: String(state.category ?? (isTerminal ? 'done' : 'active')),
				position: index + 1,
				is_initial: index === 0 || Boolean(state.initial),
				is_terminal: isTerminal
			};
		})
	};
}

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const pathname = url.pathname;

	if (pathname === '/smoke-auth-seed') {
		html(
			res,
			`<!doctype html><meta charset="utf-8"><script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace('/workspace/${workspaceId}/projects?template_wizard_smoke=1');
</script>`
		);
		return;
	}

	if (pathname === '/smoke-template-mobile-capture-seed') {
		html(
			res,
			`<!doctype html><meta charset="utf-8"><script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace('/workspace/${workspaceId}/projects?template_wizard_capture=1');
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
		json(res, 200, apiResult(paginated([{ id: workspaceId, slug: 'template-smoke', name: 'Template Smoke', created_at: now, updated_at: now }])));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}`) {
		json(res, 200, apiResult({ id: workspaceId, slug: 'template-smoke', name: 'Template Smoke', role: 'owner', created_at: now, updated_at: now }));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/project-types`) {
		json(res, 200, apiResult(paginated(projectTypes)));
		return;
	}

	if (pathname === '/api/v1/scenario-templates') {
		json(res, 200, apiResult(paginated(scenarioTemplates)));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/projects` && req.method === 'GET') {
		json(res, 200, apiResult(paginated(createdProjects)));
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/projects` && req.method === 'POST') {
		createdProjectPayload = await readBody(req);
		createdProjectPayloads = [...createdProjectPayloads, createdProjectPayload];
		const template = scenarioTemplates.find((item) => item.key === createdProjectPayload.scenario_template_key) ?? scenarioTemplates[1];
		const projectId = projectIdForTemplate(template.key);
		const project = {
			id: projectId,
			workspace_id: workspaceId,
			name: createdProjectPayload.name,
			key: createdProjectPayload.key,
			description: createdProjectPayload.description ?? '',
			type_key: createdProjectPayload.type_key,
			type_settings: {
				scenario_template_key: createdProjectPayload.scenario_template_key,
				scenario_template: embeddedTemplate(template)
			},
			created_at: now,
			updated_at: now,
			issue_counts: null
		};
		createdProjects = [...createdProjects.filter((item) => item.id !== project.id), project];
		if (template.key === 'contract_review_default') {
			aiAgents = [
				{
					id: 'tpl-smoke-document_assistant',
					project_id: 'project-created-smoke',
					name: 'Document review assistant',
					model: 'template-placeholder',
					provider: 'openpr-mcp',
					api_endpoint: null,
					capabilities: ['documents.extract_summary', 'documents.review_risk'],
					domain_overrides: { source: 'scenario_template' },
					max_domain_level: 'advisor',
					can_veto_human_consensus: false,
					reason_min_length: 50,
					is_active: false,
					registered_by: 'user-smoke',
					created_at: now
				},
				{
					id: 'tpl-smoke-approval_assistant',
					project_id: 'project-created-smoke',
					name: 'Approval assistant',
					model: 'template-placeholder',
					provider: 'webhook',
					api_endpoint: null,
					capabilities: ['approval.request'],
					domain_overrides: { source: 'scenario_template' },
					max_domain_level: 'advisor',
					can_veto_human_consensus: false,
					reason_min_length: 50,
					is_active: false,
					registered_by: 'user-smoke',
					created_at: now
				}
			];
		}
		json(
			res,
			200,
			apiResult(project)
		);
		return;
	}

	if (pathname === '/api/v1/projects/project-created-smoke') {
		json(res, 200, apiResult(createdProjects.find((project) => project.id === 'project-created-smoke')));
		return;
	}

	if (pathname === '/api/v1/projects/project-created-smoke/resources') {
		json(res, 200, apiResult(paginated([])));
		return;
	}

	if (pathname === '/api/v1/projects/project-created-smoke/ai-participants' && req.method === 'GET') {
		json(res, 200, apiResult(paginated(aiAgents)));
		return;
	}

	if (pathname === '/api/v1/projects/project-created-smoke/ai-participants/tpl-smoke-document_assistant' && req.method === 'PATCH') {
		assistantPatchPayload = await readBody(req);
		aiAgents = aiAgents.map((agent) =>
			agent.id === 'tpl-smoke-document_assistant'
				? { ...agent, ...assistantPatchPayload, updated_at: now }
				: agent
		);
		json(res, 200, apiResult(aiAgents.find((agent) => agent.id === 'tpl-smoke-document_assistant')));
		return;
	}

	if (pathname === '/api/v1/projects/project-created-smoke/workflow/effective') {
		json(
			res,
			200,
			apiResult({
				id: 'workflow-contract-smoke',
				name: 'Contract workflow',
				states: [
					{ key: 'intake', display_name: 'Intake', category: 'active', position: 1, is_initial: true, is_terminal: false },
					{ key: 'risk_review', display_name: 'Risk Review', category: 'active', position: 2, is_initial: false, is_terminal: false },
					{ key: 'approval', display_name: 'Approval', category: 'done', position: 3, is_initial: false, is_terminal: true }
				]
			})
		);
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

	if (pathname === '/api/v1/projects/project-created-smoke/sprints') {
		json(res, 200, apiResult(paginated([])));
		return;
	}

	if (pathname === '/api/v1/projects/project-created-smoke/issues' && req.method === 'GET') {
		json(res, 200, apiResult(paginated(createdIssues)));
		return;
	}

	if (pathname === '/api/v1/projects/project-created-smoke/issues' && req.method === 'POST') {
		createdIssuePayload = await readBody(req);
		createdIssuePayloads = [
			...createdIssuePayloads,
			{ project_id: 'project-created-smoke', template_key: 'contract_review_default', payload: createdIssuePayload }
		];
		const issue = {
			id: 'issue-created-smoke',
			project_id: 'project-created-smoke',
			title: createdIssuePayload.title,
			description: createdIssuePayload.description ?? '',
			status: createdIssuePayload.state ?? 'intake',
			priority: createdIssuePayload.priority ?? 'medium',
			reporter_id: 'user-smoke',
			created_at: now,
			updated_at: now,
			key: 'LEGALSMK-1',
			labels: []
		};
		createdIssues = [issue];
		createdIssuesByProject = { ...createdIssuesByProject, 'project-created-smoke': [issue] };
		json(res, 200, apiResult(issue));
		return;
	}

	const projectMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)$/);
	if (projectMatch && req.method === 'GET') {
		json(res, 200, apiResult(projectById(projectMatch[1])));
		return;
	}

	const projectResourcesMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)\/resources$/);
	if (projectResourcesMatch && req.method === 'GET') {
		json(res, 200, apiResult(paginated([])));
		return;
	}

	const projectWorkflowMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)\/workflow\/effective$/);
	if (projectWorkflowMatch && req.method === 'GET') {
		json(res, 200, apiResult(workflowForTemplate(templateForProjectId(projectWorkflowMatch[1]))));
		return;
	}

	const projectSprintsMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)\/sprints$/);
	if (projectSprintsMatch && req.method === 'GET') {
		json(res, 200, apiResult(paginated([])));
		return;
	}

	const projectIssuesMatch = pathname.match(/^\/api\/v1\/projects\/([^/]+)\/issues$/);
	if (projectIssuesMatch && req.method === 'GET') {
		const projectId = projectIssuesMatch[1];
		json(res, 200, apiResult(paginated(createdIssuesByProject[projectId] ?? [])));
		return;
	}

	if (projectIssuesMatch && req.method === 'POST') {
		const projectId = projectIssuesMatch[1];
		const project = projectById(projectId);
		const template = templateForProjectId(projectId);
		createdIssuePayload = await readBody(req);
		createdIssuePayloads = [
			...createdIssuePayloads,
			{ project_id: projectId, template_key: template.key, payload: createdIssuePayload }
		];
		const existingIssues = createdIssuesByProject[projectId] ?? [];
		const issue = {
			id: `issue-${projectId}-${existingIssues.length + 1}`,
			project_id: projectId,
			title: createdIssuePayload.title,
			description: createdIssuePayload.description ?? '',
			status: createdIssuePayload.state ?? workflowForTemplate(template).states[0]?.key ?? 'todo',
			priority: createdIssuePayload.priority ?? 'medium',
			reporter_id: 'user-smoke',
			created_at: now,
			updated_at: now,
			key: `${project?.key ?? 'SMOKE'}-${existingIssues.length + 1}`,
			labels: []
		};
		createdIssuesByProject = { ...createdIssuesByProject, [projectId]: [issue, ...existingIssues] };
		json(res, 200, apiResult(issue));
		return;
	}

	if (pathname.startsWith('/api/')) {
		json(res, 404, { code: 404, message: `unmocked ${pathname}`, data: null });
		return;
	}

	await serveStatic(
		req,
		res,
		url.searchParams.get('template_wizard_smoke') === '1'
			? 'smoke'
			: url.searchParams.get('template_wizard_capture') === '1'
				? 'capture'
				: 'none'
	);
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
			stderr += '\nChromium smoke timed out after 30s';
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
	const desktopScreenshot = screenshotDir ? join(screenshotDir, 'project-template-wizard-desktop.png') : null;
	const mobileScreenshot = screenshotDir ? join(screenshotDir, 'project-template-wizard-mobile.png') : null;
	const smokeUrl = `http://127.0.0.1:${port}/smoke-auth-seed`;
	const result = await runChromium(smokeUrl, '1366,900', desktopScreenshot);
	const mobileResult = screenshotDir
		? await runChromium(`http://127.0.0.1:${port}/smoke-template-mobile-capture-seed`, '390,844', mobileScreenshot)
		: { code: 0, stdout: 'data-template-mobile-capture="skipped"', stderr: '' };
	server.close();

	if (result.code !== 0) {
		throw new Error(`Chromium exited with ${result.code}\n${result.stderr}`);
	}
	if (mobileResult.code !== 0) {
		throw new Error(`Mobile Chromium exited with ${mobileResult.code}\n${mobileResult.stderr}`);
	}

	const dom = result.stdout;
	if (!dom.includes('data-template-smoke="done"')) {
		console.error(dom.match(/data-template-smoke[^\\s>]*/g)?.join('\n') ?? 'no smoke attribute');
		console.error(dom.match(/data-template-smoke-error="[^"]+"/)?.[0] ?? 'no smoke error attribute');
		console.error(`createdIssuePayload=${JSON.stringify(createdIssuePayload)}`);
		console.error(`createdIssues=${JSON.stringify(createdIssues)}`);
		console.error(dom.slice(-4000));
	}
	assertIncludes(dom, 'data-template-smoke="done"');
	assertIncludes(dom, 'data-template-card-seen="contract_review_default"');
	for (const templateKey of [
		'code_delivery_default',
		'contract_review_default',
		'equipment_maintenance_default',
		'quality_corrective_action_default',
		'customer_delivery_default'
	]) {
		assertIncludes(dom, `data-template-created-${templateKey}="done"`);
	}
	assertIncludes(dom, 'data-scenario-dashboard-seen="done"');
	assertIncludes(dom, 'data-scenario-assistant-enabled="done"');
	assertIncludes(dom, 'data-scenario-issue-fields-seen="done"');
	assertIncludes(dom, 'data-scenario-issue-create="done"');
	assertIncludes(dom, 'Contract Template Smoke');

	if (!createdProjectPayload) {
		throw new Error('Expected project creation POST payload');
	}
	const expectedTemplateKeys = [
		'code_delivery_default',
		'contract_review_default',
		'equipment_maintenance_default',
		'quality_corrective_action_default',
		'customer_delivery_default'
	];
	for (const templateKey of expectedTemplateKeys) {
		if (!createdProjectPayloads.some((payload) => payload.scenario_template_key === templateKey)) {
			throw new Error(`Missing project creation payload for ${templateKey}`);
		}
	}
	const contractPayload = createdProjectPayloads.find((payload) => payload.scenario_template_key === 'contract_review_default');
	if (!contractPayload) {
		throw new Error('Missing contract project creation payload');
	}
	if (contractPayload.type_key !== 'contract_review') {
		throw new Error(`Unexpected contract type_key: ${contractPayload.type_key}`);
	}
	if (contractPayload.key !== 'LEGALSMK') {
		throw new Error(`Unexpected contract project key: ${contractPayload.key}`);
	}
	if (!assistantPatchPayload?.is_active) {
		throw new Error(`Expected assistant activation patch, got ${JSON.stringify(assistantPatchPayload)}`);
	}
	if (assistantPatchPayload.provider !== 'openpr-mcp' || assistantPatchPayload.model !== 'mcp-agent-smoke' || assistantPatchPayload.api_endpoint !== 'http://127.0.0.1:8090/mcp/rpc') {
		throw new Error(`Expected assistant provider/model/endpoint patch, got ${JSON.stringify(assistantPatchPayload)}`);
	}
	const contractIssuePayload = createdIssuePayloads.find((entry) => entry.template_key === 'contract_review_default')?.payload;
	if (!contractIssuePayload) {
		throw new Error('Expected contract issue creation POST payload');
	}
	if (!contractIssuePayload.description?.includes('## Scenario Fields')) {
		throw new Error('Expected scenario fields markdown in issue description');
	}
	if (!contractIssuePayload.description.includes('ACME Manufacturing') || !contractIssuePayload.description.includes('Risk level')) {
		throw new Error(`Unexpected scenario field description: ${contractIssuePayload.description}`);
	}
	if (!contractIssuePayload.description.includes('high')) {
		throw new Error(`Expected selected risk level in issue description: ${contractIssuePayload.description}`);
	}

	if (screenshotDir) {
		assertIncludes(mobileResult.stdout, 'data-template-mobile-capture="done"');
	}
	if (screenshotDir) {
		console.log(`Project template wizard screenshots: ${desktopScreenshot}, ${mobileScreenshot}`);
	}
	console.log('Project template wizard browser smoke passed');
});

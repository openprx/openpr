#!/usr/bin/env node
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';

const workspaceId = 'ws-smoke';
const projectId = 'project-smoke';
const userId = 'user-smoke';
const now = '2026-05-31T12:00:00.000Z';

const project = {
	id: projectId,
	workspace_id: workspaceId,
	name: 'MCP Smoke Project',
	key: 'MCP',
	description: 'Browser smoke project for MCP admin controls',
	type_key: 'code',
	type_settings: {},
	created_at: now,
	updated_at: now,
	issue_counts: null
};

const connectors = [
	{
		id: 'connector-mcp-smoke',
		workspace_id: workspaceId,
		project_id: projectId,
		webhook_id: null,
		kind: 'mcp',
		name: 'Smoke MCP Connector',
		description: 'Project-scoped MCP server for governed AI execution',
		endpoint: 'http://127.0.0.1:18190/mcp/rpc',
		auth_policy: {
			mode: 'bearer',
			token: 'opr_smoke_secret_should_not_render'
		},
		capability_manifest: {
			mcp: {
				tools: ['invocations.report_progress', 'invocations.complete', 'check_results.create']
			}
		},
		is_active: true,
		created_by: userId,
		created_at: now,
		updated_at: now
	},
	{
		id: 'connector-webhook-smoke',
		workspace_id: workspaceId,
		project_id: null,
		webhook_id: 'webhook-smoke',
		kind: 'webhook',
		name: 'OpenPRX Webhook Bridge',
		description: 'Routes @AI work into openpr-webhook',
		endpoint: 'http://127.0.0.1:19096/webhook/openpr',
		auth_policy: { mode: 'hmac' },
		capability_manifest: { accepts: ['ai_tasks'] },
		is_active: true,
		created_by: userId,
		created_at: now,
		updated_at: now
	}
];

const invocations = [
	{
		id: 'invocation-completed-smoke',
		workspace_id: workspaceId,
		project_id: projectId,
		actor_id: userId,
		target_agent_id: null,
		source_task_id: 'task-smoke',
		trigger_kind: 'mcp',
		trigger_ref_type: 'issue',
		trigger_ref_id: 'issue-smoke',
		connector_id: 'connector-mcp-smoke',
		connector_kind: 'mcp',
		status: 'completed',
		payload: { tool: 'invocations.complete' },
		result: { status: 'success', executor: 'codex' },
		error_message: null,
		audit_chain_id: 'audit-completed-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'invocation-failed-smoke',
		workspace_id: workspaceId,
		project_id: projectId,
		actor_id: userId,
		target_agent_id: null,
		source_task_id: null,
		trigger_kind: 'check_results.create',
		trigger_ref_type: 'check_result',
		trigger_ref_id: 'check-smoke',
		connector_id: 'connector-mcp-smoke',
		connector_kind: 'mcp',
		status: 'failed',
		payload: { tool: 'check_results.create' },
		result: null,
		error_message: 'smoke failure shown in history',
		audit_chain_id: 'audit-failed-smoke',
		created_at: now,
		updated_at: now
	}
];

const invocationToolCalls = {
	'invocation-completed-smoke': [
		{
			id: 'tool-call-context-smoke',
			invocation_id: 'invocation-completed-smoke',
			workspace_id: workspaceId,
			project_id: projectId,
			actor_id: userId,
			tool_name: 'context.get_project',
			transport: 'mcp_stdio',
			status: 'succeeded',
			arguments: { project_id: projectId },
			result_summary: 'Loaded governed project context for MCP Smoke Project.',
			error_message: null,
			duration_ms: 24,
			started_at: now,
			completed_at: now,
			created_at: now
		},
		{
			id: 'tool-call-work-item-smoke',
			invocation_id: 'invocation-completed-smoke',
			workspace_id: workspaceId,
			project_id: projectId,
			actor_id: userId,
			tool_name: 'work_items.get',
			transport: 'mcp_stdio',
			status: 'succeeded',
			arguments: { work_item_id: 'issue-smoke' },
			result_summary: 'Read work item before writing the AI result.',
			error_message: null,
			duration_ms: 18,
			started_at: now,
			completed_at: now,
			created_at: now
		},
		{
			id: 'tool-call-comment-smoke',
			invocation_id: 'invocation-completed-smoke',
			workspace_id: workspaceId,
			project_id: projectId,
			actor_id: userId,
			tool_name: 'comments.create',
			transport: 'mcp_stdio',
			status: 'succeeded',
			arguments: { work_item_id: 'issue-smoke', content: 'REAL_CODEX_MCP_COMMENT_OK' },
			result_summary: 'REAL_CODEX_MCP_COMMENT_OK',
			error_message: null,
			duration_ms: 31,
			started_at: now,
			completed_at: now,
			created_at: now
		},
		{
			id: 'tool-call-complete-smoke',
			invocation_id: 'invocation-completed-smoke',
			workspace_id: workspaceId,
			project_id: projectId,
			actor_id: userId,
			tool_name: 'invocations.complete',
			transport: 'mcp_stdio',
			status: 'succeeded',
			arguments: { invocation_id: 'invocation-completed-smoke' },
			result_summary: 'real Codex used OpenPR MCP context and wrote comment',
			error_message: null,
			duration_ms: 42,
			started_at: now,
			completed_at: now,
			created_at: now
		}
	],
	'invocation-failed-smoke': []
};

function apiResult(data) {
	return JSON.stringify({ code: 0, message: 'success', data });
}

function paginated(items, page = 1, perPage = 100) {
	return {
		items,
		total: items.length,
		page,
		per_page: perPage,
		total_pages: Math.max(1, Math.ceil(items.length / perPage))
	};
}

function json(res, status, body) {
	res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });
	res.end(body);
}

function html(res, body) {
	res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
	res.end(body);
}

function contentType(filePath) {
	const ext = extname(filePath);
	if (ext === '.html') return 'text/html; charset=utf-8';
	if (ext === '.js') return 'text/javascript; charset=utf-8';
	if (ext === '.css') return 'text/css; charset=utf-8';
	if (ext === '.json') return 'application/json; charset=utf-8';
	if (ext === '.svg') return 'image/svg+xml';
	if (ext === '.png') return 'image/png';
	if (ext === '.ico') return 'image/x-icon';
	if (ext === '.woff2') return 'font/woff2';
	return 'application/octet-stream';
}

async function serveStatic(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const rawPath = decodeURIComponent(url.pathname);
	const normalized = normalize(rawPath).replace(/^(\.\.[/\\])+/, '');
	let filePath = join(buildDir, normalized === '/' ? '/index.html' : normalized);
	if (!filePath.startsWith(buildDir)) {
		res.writeHead(403);
		res.end('Forbidden');
		return;
	}
	if (!existsSync(filePath) || basename(filePath) === 'connections') {
		filePath = join(buildDir, 'index.html');
	}
	try {
		const data = await readFile(filePath);
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

	if (pathname === '/smoke-auth-seed') {
		html(
			res,
			`<!doctype html><meta charset="utf-8"><script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
location.replace('/workspace/${workspaceId}/connections?invocation_id=invocation-completed-smoke');
</script>`
		);
		return;
	}

	if (pathname === '/api/v1/auth/me') {
		json(
			res,
			200,
			apiResult({
				user: {
					id: userId,
					email: 'smoke@example.com',
					name: 'Smoke Admin',
					role: 'admin',
					status: 'active',
					created_at: now,
					updated_at: now
				}
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
						slug: 'smoke',
						name: 'Smoke Workspace',
						description: 'Smoke workspace',
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
						user_id: userId,
						workspace_id: workspaceId,
						role: 'owner',
						joined_at: now
					}
				])
			)
		);
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/projects`) {
		json(
			res,
			200,
			apiResult(paginated([project], 1, Number(url.searchParams.get('per_page') ?? 100)))
		);
		return;
	}

	if (pathname === `/api/v1/workspaces/${workspaceId}/connectors`) {
		json(res, 200, apiResult(connectors));
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}/invocations`) {
		json(
			res,
			200,
			apiResult(paginated(invocations, 1, Number(url.searchParams.get('per_page') ?? 20)))
		);
		return;
	}

	const toolCallMatch = pathname.match(/^\/api\/v1\/invocations\/([^/]+)\/tool-calls$/);
	if (toolCallMatch) {
		const invocationId = toolCallMatch[1];
		json(
			res,
			200,
			apiResult(paginated(invocationToolCalls[invocationId] ?? [], 1, Number(url.searchParams.get('per_page') ?? 100)))
		);
		return;
	}

	if (pathname === `/api/v1/projects/${projectId}/context`) {
		json(
			res,
			200,
			apiResult({
				project,
				project_type: {
					key: 'code',
					workspace_id: null,
					name: 'Code Project',
					description: 'Software delivery project',
					domain: 'software',
					default_workflow_id: null,
					enabled_capabilities: ['mcp', 'check_results', 'invocations'],
					field_schema: {},
					artifact_schema: {},
					default_connectors: [],
					created_at: now,
					updated_at: now
				},
				resources: [
					{
						id: 'resource-repo-smoke',
						project_id: projectId,
						kind: 'repo',
						name: 'openpr repository',
						locator: { path: '/opt/worker/code/openpr' },
						permission_policy: { read: true, write: false },
						sync_status: 'synced',
						created_by: userId,
						created_at: now,
						updated_at: now
					}
				],
				connectors,
				governance: null,
				workflow: null,
				recent_decisions: [],
				agent_policy: {
					project_id: projectId,
					project_type: 'code',
					capabilities: ['mcp', 'governed_checks', 'code_context', 'documents', 'approval'],
					connector_kinds: ['mcp', 'webhook'],
					action_classes: {
						read: ['context.read'],
						write: ['invocations.report_progress', 'check_results.create']
					},
					mcp: {
						writes_create_invocation: true,
						workspace_scope_required: true,
						project_context_required: true,
						tool_registry: {
							source: 'project_policy',
							groups: {
								context: ['project.context.read', 'resources.list'],
								code: [
									'code.resources.list',
									'code.directory.get',
									'code.task_context.get',
									'code.change_proposal.create'
								],
								documents: ['documents.extract_summary', 'documents.review_risk', 'approval.request'],
								invocations: ['invocations.report_progress', 'invocations.complete'],
								checks: ['check_results.create', 'proposals.create_from_result']
							},
							enabled_tools: [
								'project.context.read',
								'resources.list',
								'code.resources.list',
								'code.directory.get',
								'code.task_context.get',
								'code.change_proposal.create',
								'documents.extract_summary',
								'documents.review_risk',
								'approval.request',
								'invocations.report_progress',
								'invocations.complete',
								'check_results.create',
								'proposals.create_from_result'
							]
						}
					}
				}
			})
		);
		return;
	}

	if (pathname.startsWith('/api/')) {
		json(res, 404, JSON.stringify({ code: 404, message: `unmocked ${pathname}`, data: null }));
		return;
	}

	await serveStatic(req, res);
}

function runChromium(url) {
	return new Promise((resolvePromise) => {
		const child = spawn(chromium, [
			'--headless=new',
			'--disable-gpu',
			'--disable-dev-shm-usage',
			'--no-sandbox',
			'--hide-scrollbars',
			'--run-all-compositor-stages-before-draw',
			'--virtual-time-budget=12000',
			'--dump-dom',
			url
		]);
		let stdout = '';
		let stderr = '';
		child.stdout.on('data', (chunk) => {
			stdout += chunk.toString();
		});
		child.stderr.on('data', (chunk) => {
			stderr += chunk.toString();
		});
		child.on('close', (code) => {
			resolvePromise({ code, stdout, stderr });
		});
	});
}

function assertIncludes(dom, value) {
	if (!dom.includes(value)) {
		throw new Error(`Expected rendered DOM to include "${value}"`);
	}
}

function assertExcludes(dom, value) {
	if (dom.includes(value)) {
		throw new Error(`Expected rendered DOM not to include "${value}"`);
	}
}

if (!existsSync(buildDir)) {
	throw new Error('frontend/build does not exist. Run bun run build before smoke:mcp-admin.');
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
	const result = await runChromium(`http://127.0.0.1:${port}/smoke-auth-seed`);
	server.close();

	if (result.code !== 0) {
		throw new Error(`Chromium exited with ${result.code}\n${result.stderr}`);
	}

	const dom = result.stdout;
	assertIncludes(dom, 'Automation Connections');
	assertIncludes(dom, 'MCP Control Panel');
	assertIncludes(dom, 'MCP Smoke Project');
	assertIncludes(dom, 'Smoke MCP Connector');
	assertIncludes(dom, 'OpenPRX Webhook Bridge');
	assertIncludes(dom, 'Supported Tools');
	assertIncludes(dom, 'Accessible Resources');
	assertIncludes(dom, 'MCP Calls');
	assertIncludes(dom, 'Client Config');
	assertIncludes(dom, 'project.context.read');
	assertIncludes(dom, 'code.resources.list');
	assertIncludes(dom, 'code.change_proposal.create');
	assertIncludes(dom, 'documents.extract_summary');
	assertIncludes(dom, 'documents.review_risk');
	assertIncludes(dom, 'approval.request');
	assertIncludes(dom, 'invocations.report_progress');
	assertIncludes(dom, 'invocations.complete');
	assertIncludes(dom, 'check_results.create');
	assertIncludes(dom, 'openpr repository');
	assertIncludes(dom, 'completed');
	assertIncludes(dom, 'failed');
	assertIncludes(dom, 'Invocation Detail');
	assertIncludes(dom, 'MCP Tool Calls');
	assertIncludes(dom, '4 audited calls linked to this invocation');
	assertIncludes(dom, 'context.get_project');
	assertIncludes(dom, 'work_items.get');
	assertIncludes(dom, 'comments.create');
	assertIncludes(dom, 'REAL_CODEX_MCP_COMMENT_OK');
	assertIncludes(dom, 'real Codex used OpenPR MCP context and wrote comment');
	assertIncludes(dom, 'Bearer &lt;workspace-bot-token&gt;');
	assertExcludes(dom, 'opr_smoke_secret_should_not_render');
	assertExcludes(dom, '/auth/login');

	console.log('MCP admin browser smoke passed');
});

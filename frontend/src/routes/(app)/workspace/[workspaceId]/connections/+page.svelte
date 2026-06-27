<script lang="ts">
	import { Activity, Copy, Server, ShieldCheck, Wrench } from '@lucide/svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import {
		connectorsApi,
		type Connector,
		type ConnectorKind,
		type Invocation,
		type InvocationStatus,
		type InvocationToolCall
	} from '$lib/api/connectors';
	import { projectsApi, type Project, type ProjectContext } from '$lib/api/projects';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import { locale, t } from 'svelte-i18n';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const connectorKinds: Array<{ value: ConnectorKind; label: string }> = [
		{ value: 'webhook', label: 'Webhook' },
		{ value: 'mcp', label: 'MCP' },
		{ value: 'rest', label: 'REST' },
		{ value: 'cli', label: 'CLI' },
		{ value: 'openprx_tunnel', label: 'OpenPRX Tunnel' }
	];
	const invocationStatuses: Array<{ value: InvocationStatus | ''; label: string }> = [
		{ value: '', label: 'all' },
		{ value: 'pending', label: 'pending' },
		{ value: 'dispatched', label: 'dispatched' },
		{ value: 'running', label: 'running' },
		{ value: 'completed', label: 'completed' },
		{ value: 'failed', label: 'failed' },
		{ value: 'cancelled', label: 'cancelled' }
	];

	let loading = $state(true);
	let saving = $state(false);
	let deletingId = $state<string | null>(null);
	let togglingId = $state<string | null>(null);
	let projects = $state<Project[]>([]);
	let connectors = $state<Connector[]>([]);
	let invocations = $state<Invocation[]>([]);
	let selectedProjectId = $state('');
	let statusFilter = $state<InvocationStatus | ''>('');
	let invocationsLoading = $state(false);
	let invocationsPage = $state(1);
	let invocationsTotalPages = $state(1);
	let invocationsTotal = $state(0);
	let projectContext = $state<ProjectContext | null>(null);
	let projectContextLoading = $state(false);
	let copyingConfig = $state(false);

	let showConnectorModal = $state(false);
	let showInvocationModal = $state(false);
	let selectedInvocation = $state<Invocation | null>(null);
	let selectedInvocationToolCalls = $state<InvocationToolCall[]>([]);
	let selectedInvocationToolCallsLoading = $state(false);
	let form = $state({
		name: '',
		kind: 'webhook' as ConnectorKind,
		project_id: '',
		endpoint: '',
		description: '',
		is_active: true,
		auth_policy: '{\n  "mode": "none"\n}',
		capability_manifest: '{\n  "capabilities": []\n}'
	});

	onMount(async () => {
		await loadInitial();
	});

	async function loadInitial() {
		loading = true;
		const [projectResponse, connectorResponse] = await Promise.all([
			projectsApi.list(workspaceId, { per_page: 100 }),
			connectorsApi.list(workspaceId)
		]);

		if (projectResponse.code !== 0) {
			toast.error(projectResponse.message);
		} else {
			projects = projectResponse.data?.items ?? [];
			selectedProjectId = projects[0]?.id ?? '';
		}

		if (connectorResponse.code !== 0) {
			toast.error(connectorResponse.message);
		} else {
			connectors = connectorResponse.data ?? [];
		}

		loading = false;
		if (selectedProjectId) {
			await Promise.all([loadInvocations(1), loadProjectContext()]);
			openInitialInvocationFromUrl();
		}
	}

	function openInitialInvocationFromUrl() {
		if (typeof window === 'undefined') return;
		const invocationId = new URLSearchParams(window.location.search).get('invocation_id');
		if (!invocationId) return;
		const invocation = invocations.find((item) => item.id === invocationId);
		if (invocation) openInvocation(invocation);
	}

	async function loadConnectors() {
		const response = await connectorsApi.list(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			connectors = response.data ?? [];
		}
	}

	async function loadInvocations(pageToLoad: number = invocationsPage) {
		if (!selectedProjectId) {
			invocations = [];
			invocationsTotal = 0;
			invocationsTotalPages = 1;
			return;
		}

		invocationsLoading = true;
		const response = await connectorsApi.listInvocations(selectedProjectId, {
			status: statusFilter || undefined,
			page: pageToLoad,
			per_page: 20
		});
		if (response.code !== 0) {
			toast.error(response.message);
			invocations = [];
			invocationsTotal = 0;
			invocationsTotalPages = 1;
		} else if (response.data) {
			invocations = response.data.items ?? [];
			invocationsPage = response.data.page ?? pageToLoad;
			invocationsTotalPages = Math.max(1, response.data.total_pages ?? 1);
			invocationsTotal = response.data.total ?? 0;
		}
		invocationsLoading = false;
	}

	async function loadProjectContext() {
		if (!selectedProjectId) {
			projectContext = null;
			return;
		}

		projectContextLoading = true;
		const response = await projectsApi.getContext(selectedProjectId);
		if (response.code !== 0) {
			toast.error(response.message);
			projectContext = null;
		} else {
			projectContext = response.data ?? null;
		}
		projectContextLoading = false;
	}

	async function changeSelectedProject() {
		await Promise.all([loadInvocations(1), loadProjectContext()]);
	}

	function openCreateConnector() {
		form = {
			name: '',
			kind: 'webhook',
			project_id: selectedProjectId,
			endpoint: '',
			description: '',
			is_active: true,
			auth_policy: '{\n  "mode": "none"\n}',
			capability_manifest: '{\n  "capabilities": []\n}'
		};
		showConnectorModal = true;
	}

	async function saveConnector() {
		if (!form.name.trim()) {
			toast.error($t('connections.connectionNameRequired'));
			return;
		}

		let authPolicy: Record<string, unknown>;
		let capabilityManifest: Record<string, unknown>;
		try {
			authPolicy = parseJsonObject(form.auth_policy, $t('connections.authPolicy'));
			capabilityManifest = parseJsonObject(form.capability_manifest, $t('connections.capabilityManifest'));
		} catch (error) {
			toast.error(error instanceof Error ? error.message : $t('connections.invalidJson'));
			return;
		}

		saving = true;
		const response = await connectorsApi.create(workspaceId, {
			name: form.name.trim(),
			kind: form.kind,
			project_id: form.project_id || undefined,
			endpoint: form.endpoint.trim() || undefined,
			description: form.description.trim() || undefined,
			is_active: form.is_active,
			auth_policy: authPolicy,
			capability_manifest: capabilityManifest
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success($t('connections.createdSuccess'));
			showConnectorModal = false;
			await loadConnectors();
			await loadProjectContext();
		}
		saving = false;
	}

	async function toggleConnector(connector: Connector) {
		togglingId = connector.id;
		const response = await connectorsApi.update(workspaceId, connector.id, {
			is_active: !connector.is_active
		});
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			await loadConnectors();
			await loadProjectContext();
		}
		togglingId = null;
	}

	async function deleteConnector(connector: Connector) {
		if (!confirm($t('connections.deleteConfirm', { values: { name: connector.name } }))) return;
		deletingId = connector.id;
		const response = await connectorsApi.delete(workspaceId, connector.id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success($t('connections.deletedSuccess'));
			await loadConnectors();
			await loadProjectContext();
		}
		deletingId = null;
	}

	async function cancelInvocation(invocation: Invocation) {
		const response = await connectorsApi.cancelInvocation(invocation.id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success($t('connections.invocationCancelled'));
			await loadInvocations(invocationsPage);
		}
	}

	function openInvocation(invocation: Invocation) {
		selectedInvocation = invocation;
		selectedInvocationToolCalls = [];
		showInvocationModal = true;
		void loadInvocationToolCalls(invocation.id);
	}

	async function loadInvocationToolCalls(invocationId: string) {
		selectedInvocationToolCallsLoading = true;
		const response = await connectorsApi.listInvocationToolCalls(invocationId, { per_page: 100 });
		if (response.code !== 0) {
			toast.error(response.message);
			selectedInvocationToolCalls = [];
		} else {
			selectedInvocationToolCalls = response.data?.items ?? [];
		}
		selectedInvocationToolCallsLoading = false;
	}

	function refreshSelectedInvocationToolCalls() {
		if (!selectedInvocation) return;
		void loadInvocationToolCalls(selectedInvocation.id);
	}

	function parseJsonObject(raw: string, label: string): Record<string, unknown> {
		const parsed = JSON.parse(raw || '{}') as unknown;
		if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') {
			throw new Error($t('connections.jsonObjectRequired', { values: { label } }));
		}
		return parsed as Record<string, unknown>;
	}

	function projectName(projectId: string | null | undefined): string {
		if (!projectId) return $t('connections.workspace');
		const project = projects.find((item) => item.id === projectId);
		return project ? `${project.key} ${project.name}` : projectId.slice(0, 8);
	}

	function formatDate(value: string): string {
		return new Date(value).toLocaleString($locale === 'en' ? 'en-US' : 'zh-CN');
	}

	function formatJson(value: unknown): string {
		return JSON.stringify(value ?? {}, null, 2);
	}

	function statusClass(status: string): string {
		if (status === 'completed') return 'bg-emerald-50 text-emerald-700 ring-emerald-200';
		if (status === 'succeeded') return 'bg-emerald-50 text-emerald-700 ring-emerald-200';
		if (status === 'failed') return 'bg-red-50 text-red-700 ring-red-200';
		if (status === 'running' || status === 'dispatched') return 'bg-blue-50 text-blue-700 ring-blue-200';
		if (status === 'cancelled') return 'bg-slate-100 text-slate-600 ring-slate-200';
		return 'bg-amber-50 text-amber-700 ring-amber-200';
	}

	function kindLabel(kind: ConnectorKind): string {
		return connectorKinds.find((item) => item.value === kind)?.label ?? kind;
	}

	function invocationStatusLabel(status: InvocationStatus | ''): string {
		if (!status) return $t('connections.allStatuses');
		const keyByStatus: Record<InvocationStatus, string> = {
			pending: 'connections.statusPending',
			dispatched: 'connections.statusDispatched',
			running: 'connections.statusRunning',
			completed: 'connections.statusCompleted',
			failed: 'connections.statusFailed',
			cancelled: 'connections.statusCancelled'
		};
		return $t(keyByStatus[status]);
	}

	function statusDisplay(status: string): string {
		if (
			status === 'pending' ||
			status === 'dispatched' ||
			status === 'running' ||
			status === 'completed' ||
			status === 'failed' ||
			status === 'cancelled'
		) {
			return invocationStatusLabel(status);
		}
		return status;
	}

	function selectedProject(): Project | undefined {
		return projects.find((project) => project.id === selectedProjectId);
	}

	function scopedMcpConnectors(): Connector[] {
		return connectors.filter((connector) => connector.kind === 'mcp' && (!connector.project_id || connector.project_id === selectedProjectId));
	}

	function mcpConnectorIds(): Set<string> {
		return new Set(scopedMcpConnectors().map((connector) => connector.id));
	}

	function mcpInvocations(): Invocation[] {
		const connectorIds = mcpConnectorIds();
		return invocations.filter(
			(invocation) =>
				invocation.trigger_kind === 'mcp' ||
				invocation.connector_kind === 'mcp' ||
				(Boolean(invocation.connector_id) && connectorIds.has(invocation.connector_id as string))
		);
	}

	function failedMcpInvocations(): Invocation[] {
		return mcpInvocations().filter((invocation) => invocation.status === 'failed');
	}

	function enabledTools(): string[] {
		return projectContext?.agent_policy?.mcp?.tool_registry?.enabled_tools ?? [];
	}

	function toolGroups(): Array<{ name: string; tools: string[] }> {
		const groups = projectContext?.agent_policy?.mcp?.tool_registry?.groups ?? {};
		return Object.entries(groups).map(([name, tools]) => ({ name, tools }));
	}

	function mcpConfig(): string {
		const connector = scopedMcpConnectors()[0];
		const endpoint = connector?.endpoint?.trim() || 'http://localhost:8090/mcp/rpc';
		const project = selectedProject();
		return JSON.stringify(
			{
				mcpServers: {
					openpr: {
						type: 'http',
						url: endpoint.endsWith('/mcp/rpc') ? endpoint : `${endpoint.replace(/\/$/, '')}/mcp/rpc`,
						headers: {
							Authorization: 'Bearer <workspace-bot-token>'
						},
						env: {
							OPENPR_WORKSPACE_ID: workspaceId,
							OPENPR_PROJECT_ID: selectedProjectId || '<project-id>',
							OPENPR_PROJECT_KEY: project?.key ?? '<project-key>'
						}
					}
				}
			},
			null,
			2
		);
	}

	async function copyMcpConfig() {
		copyingConfig = true;
		try {
			await navigator.clipboard.writeText(mcpConfig());
			toast.success($t('connections.configCopied'));
		} catch {
			toast.error($t('connections.configCopyFailed'));
		}
		copyingConfig = false;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.workspaceConnections')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('connections.title')}</h1>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-400">{$t('connections.subtitle')}</p>
		</div>
		<Button onclick={openCreateConnector}>{$t('connections.newConnection')}</Button>
	</div>

	{#if loading}
		<div class="rounded-md border border-slate-200 bg-white p-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400">{$t('common.loading')}</div>
	{:else}
		<section class="space-y-3">
			<div class="flex items-center justify-between gap-3">
				<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('connections.connections')}</h2>
				<span class="text-sm text-slate-500 dark:text-slate-400">{$t('connections.total', { values: { count: connectors.length } })}</span>
			</div>

			<div class="overflow-hidden rounded-md border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900">
				{#if connectors.length === 0}
					<div class="p-6 text-sm text-slate-500 dark:text-slate-400">{$t('connections.noConnections')}</div>
				{:else}
					<div class="overflow-x-auto">
						<table class="min-w-full divide-y divide-slate-200 text-sm dark:divide-slate-700">
							<thead class="bg-slate-50 text-left text-xs font-semibold uppercase text-slate-500 dark:bg-slate-800 dark:text-slate-400">
								<tr>
									<th class="px-4 py-3">{$t('connections.name')}</th>
									<th class="px-4 py-3">{$t('connections.kind')}</th>
									<th class="px-4 py-3">{$t('connections.scope')}</th>
									<th class="px-4 py-3">{$t('connections.endpoint')}</th>
									<th class="px-4 py-3">{$t('connections.status')}</th>
									<th class="px-4 py-3 text-right">{$t('common.actions')}</th>
								</tr>
							</thead>
							<tbody class="divide-y divide-slate-200 dark:divide-slate-700">
								{#each connectors as connector}
									<tr class="align-top">
										<td class="px-4 py-3">
											<div class="font-medium text-slate-900 dark:text-slate-100">{connector.name}</div>
											{#if connector.description}
												<div class="mt-1 max-w-md text-xs text-slate-500 dark:text-slate-400">{connector.description}</div>
											{/if}
										</td>
										<td class="px-4 py-3 text-slate-700 dark:text-slate-300">{kindLabel(connector.kind)}</td>
										<td class="px-4 py-3 text-slate-700 dark:text-slate-300">{projectName(connector.project_id)}</td>
										<td class="max-w-xs truncate px-4 py-3 text-slate-500 dark:text-slate-400">{connector.endpoint || '-'}</td>
										<td class="px-4 py-3">
											<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {connector.is_active ? 'bg-emerald-50 text-emerald-700 ring-emerald-200' : 'bg-slate-100 text-slate-600 ring-slate-200'}">
												{connector.is_active ? $t('connections.active') : $t('connections.inactive')}
											</span>
										</td>
										<td class="px-4 py-3 text-right">
											<div class="inline-flex gap-2">
												<Button variant="secondary" size="sm" disabled={togglingId === connector.id} onclick={() => toggleConnector(connector)}>
													{connector.is_active ? $t('connections.disable') : $t('connections.enable')}
												</Button>
												<Button variant="danger" size="sm" disabled={deletingId === connector.id} onclick={() => deleteConnector(connector)}>{$t('connections.delete')}</Button>
											</div>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			</div>
		</section>

		<section class="space-y-3">
			<div class="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
				<div>
					<div class="flex items-center gap-2">
						<Server class="h-5 w-5 text-cyan-700 dark:text-cyan-300" aria-hidden="true" />
						<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('connections.mcpPanel')}</h2>
					</div>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-400">{selectedProject() ? `${selectedProject()?.key} ${selectedProject()?.name}` : $t('connections.noProjectSelected')}</p>
				</div>
				<Button variant="secondary" size="sm" disabled={!selectedProjectId || copyingConfig} onclick={copyMcpConfig}>
					<Copy class="mr-2 h-4 w-4" aria-hidden="true" />
					{copyingConfig ? $t('connections.copying') : $t('connections.copyConfig')}
				</Button>
			</div>

			<div class="overflow-hidden rounded-md border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900">
				{#if !selectedProjectId}
					<div class="p-6 text-sm text-slate-500 dark:text-slate-400">{$t('connections.selectProjectForMcp')}</div>
				{:else if projectContextLoading}
					<div class="p-6 text-sm text-slate-500 dark:text-slate-400">{$t('connections.loadingMcpControls')}</div>
				{:else}
					<div class="grid divide-y divide-slate-200 dark:divide-slate-700 lg:grid-cols-[1.4fr_1fr] lg:divide-x lg:divide-y-0">
						<div class="space-y-5 p-4">
							<div>
								<div class="mb-3 flex items-center justify-between gap-3">
									<div class="flex items-center gap-2">
										<Wrench class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('connections.supportedTools')}</h3>
									</div>
									<span class="text-xs text-slate-500 dark:text-slate-400">{$t('connections.enabledCount', { values: { count: enabledTools().length } })}</span>
								</div>
								{#if toolGroups().length === 0}
									<div class="text-sm text-slate-500 dark:text-slate-400">{$t('connections.noToolRegistry')}</div>
								{:else}
									<div class="space-y-3">
										{#each toolGroups() as group}
											<div class="grid gap-2 sm:grid-cols-[8rem_1fr]">
												<div class="text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">{group.name}</div>
												<div class="flex flex-wrap gap-1.5">
													{#each group.tools as tool}
														<span class="inline-flex rounded border border-slate-200 bg-slate-50 px-1.5 py-0.5 font-mono text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300">{tool}</span>
													{/each}
												</div>
											</div>
										{/each}
									</div>
								{/if}
							</div>

							<div>
								<div class="mb-3 flex items-center gap-2">
									<ShieldCheck class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
									<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('connections.accessibleResources')}</h3>
								</div>
								{#if !projectContext || projectContext.resources.length === 0}
									<div class="text-sm text-slate-500 dark:text-slate-400">{$t('connections.noResources')}</div>
								{:else}
									<div class="divide-y divide-slate-200 rounded-md border border-slate-200 dark:divide-slate-700 dark:border-slate-700">
										{#each projectContext.resources as resource}
											<div class="grid gap-1 p-3 sm:grid-cols-[8rem_1fr_auto] sm:items-center">
												<span class="font-mono text-xs text-slate-500 dark:text-slate-400">{resource.kind}</span>
												<span class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{resource.name}</span>
												<span class="text-xs text-slate-500 dark:text-slate-400">{resource.sync_status}</span>
											</div>
										{/each}
									</div>
								{/if}
							</div>
						</div>

						<div class="space-y-5 p-4">
							<div>
								<div class="mb-3 flex items-center justify-between gap-3">
									<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('connections.mcpConnectors')}</h3>
									<span class="text-xs text-slate-500 dark:text-slate-400">{$t('connections.activeScope', { values: { count: scopedMcpConnectors().length } })}</span>
								</div>
								{#if scopedMcpConnectors().length === 0}
									<div class="text-sm text-slate-500 dark:text-slate-400">{$t('connections.noMcpConnector')}</div>
								{:else}
									<div class="space-y-2">
										{#each scopedMcpConnectors() as connector}
											<div class="rounded-md border border-slate-200 p-3 dark:border-slate-700">
												<div class="flex items-center justify-between gap-3">
													<div class="min-w-0">
														<div class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{connector.name}</div>
														<div class="mt-1 truncate text-xs text-slate-500 dark:text-slate-400">{connector.endpoint || $t('connections.noEndpoint')}</div>
													</div>
													<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {connector.is_active ? 'bg-emerald-50 text-emerald-700 ring-emerald-200' : 'bg-slate-100 text-slate-600 ring-slate-200'}">
														{connector.is_active ? $t('connections.active') : $t('connections.inactive')}
													</span>
												</div>
											</div>
										{/each}
									</div>
								{/if}
							</div>

							<div>
								<div class="mb-3 flex items-center justify-between gap-3">
									<div class="flex items-center gap-2">
										<Activity class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('connections.mcpCalls')}</h3>
									</div>
									<span class="text-xs text-slate-500 dark:text-slate-400">{$t('connections.failedCount', { values: { count: failedMcpInvocations().length } })}</span>
								</div>
								{#if mcpInvocations().length === 0}
									<div class="text-sm text-slate-500 dark:text-slate-400">{$t('connections.noMcpCalls')}</div>
								{:else}
									<div class="space-y-2">
										{#each mcpInvocations().slice(0, 5) as invocation}
											<button type="button" class="block w-full rounded-md border border-slate-200 p-3 text-left hover:bg-slate-50 dark:border-slate-700 dark:hover:bg-slate-800" onclick={() => openInvocation(invocation)}>
												<div class="flex items-center justify-between gap-3">
													<span class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{invocation.trigger_kind}</span>
													<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(invocation.status)}">{statusDisplay(invocation.status)}</span>
												</div>
												<div class="mt-1 flex items-center justify-between gap-2 text-xs text-slate-500 dark:text-slate-400">
													<span class="truncate">{invocation.audit_chain_id ? `audit ${invocation.audit_chain_id.slice(0, 8)}` : invocation.id}</span>
													<span>{formatDate(invocation.created_at)}</span>
												</div>
											</button>
										{/each}
									</div>
								{/if}
							</div>

							<div>
								<div class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('connections.clientConfig')}</div>
								<pre class="max-h-52 overflow-auto rounded-md bg-slate-950 p-3 text-xs text-slate-100">{mcpConfig()}</pre>
							</div>
						</div>
					</div>
				{/if}
			</div>
		</section>

		<section class="space-y-3">
			<div class="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
				<div>
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('connections.executionHistory')}</h2>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-400">{$t('connections.invocationRecords', { values: { count: invocationsTotal } })}</p>
				</div>
				<div class="grid gap-3 sm:grid-cols-2">
					<label class="text-sm">
						<span class="mb-1 block font-medium text-slate-700 dark:text-slate-300">{$t('connections.project')}</span>
						<select bind:value={selectedProjectId} onchange={changeSelectedProject} class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
							{#each projects as project}
								<option value={project.id}>{project.key} {project.name}</option>
							{/each}
						</select>
					</label>
					<label class="text-sm">
						<span class="mb-1 block font-medium text-slate-700 dark:text-slate-300">{$t('connections.status')}</span>
						<select bind:value={statusFilter} onchange={() => loadInvocations(1)} class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
							{#each invocationStatuses as status}
								<option value={status.value}>{invocationStatusLabel(status.value)}</option>
							{/each}
						</select>
					</label>
				</div>
			</div>

			<div class="overflow-hidden rounded-md border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900">
				{#if !selectedProjectId}
					<div class="p-6 text-sm text-slate-500 dark:text-slate-400">{$t('connections.selectProjectForHistory')}</div>
				{:else if invocationsLoading}
					<div class="p-6 text-sm text-slate-500 dark:text-slate-400">{$t('connections.loadingHistory')}</div>
				{:else if invocations.length === 0}
					<div class="p-6 text-sm text-slate-500 dark:text-slate-400">{$t('connections.noInvocations')}</div>
				{:else}
					<div class="divide-y divide-slate-200 dark:divide-slate-700">
						{#each invocations as invocation}
							<div class="grid gap-3 p-4 lg:grid-cols-[1fr_auto] lg:items-center">
								<button type="button" class="min-w-0 text-left" onclick={() => openInvocation(invocation)}>
									<div class="flex flex-wrap items-center gap-2">
										<span class="font-medium text-slate-900 dark:text-slate-100">{invocation.trigger_kind}</span>
										<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(invocation.status)}">{statusDisplay(invocation.status)}</span>
										{#if invocation.connector_kind}
											<span class="text-xs text-slate-500 dark:text-slate-400">{kindLabel(invocation.connector_kind)}</span>
										{/if}
									</div>
									<div class="mt-1 truncate text-sm text-slate-500 dark:text-slate-400">
										{invocation.trigger_ref_type || 'task'} {invocation.trigger_ref_id || invocation.source_task_id || invocation.id}
									</div>
								</button>
								<div class="flex items-center gap-3 lg:justify-end">
									<span class="text-sm text-slate-500 dark:text-slate-400">{formatDate(invocation.created_at)}</span>
									{#if invocation.status === 'pending' || invocation.status === 'dispatched' || invocation.status === 'running'}
										<Button variant="secondary" size="sm" onclick={() => cancelInvocation(invocation)}>{$t('common.cancel')}</Button>
									{/if}
								</div>
							</div>
						{/each}
					</div>
				{/if}
			</div>

			{#if selectedProjectId && invocationsTotalPages > 1}
				<div class="flex items-center justify-end gap-2">
					<Button variant="secondary" size="sm" disabled={invocationsPage <= 1} onclick={() => loadInvocations(invocationsPage - 1)}>{$t('connections.previous')}</Button>
					<span class="text-sm text-slate-500 dark:text-slate-400">{invocationsPage} / {invocationsTotalPages}</span>
					<Button variant="secondary" size="sm" disabled={invocationsPage >= invocationsTotalPages} onclick={() => loadInvocations(invocationsPage + 1)}>{$t('connections.next')}</Button>
				</div>
			{/if}
		</section>
	{/if}
</div>

<Modal bind:open={showConnectorModal} title={$t('connections.newConnection')}>
	<div class="space-y-4">
		<Input label={$t('connections.name')} bind:value={form.name} placeholder="Document review bot" />
		<label class="block text-sm">
			<span class="mb-1 block font-medium text-slate-700 dark:text-slate-300">{$t('connections.kind')}</span>
			<select bind:value={form.kind} class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
				{#each connectorKinds as kind}
					<option value={kind.value}>{kind.label}</option>
				{/each}
			</select>
		</label>
		<label class="block text-sm">
			<span class="mb-1 block font-medium text-slate-700 dark:text-slate-300">{$t('connections.projectScope')}</span>
			<select bind:value={form.project_id} class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
				<option value="">{$t('connections.workspace')}</option>
				{#each projects as project}
					<option value={project.id}>{project.key} {project.name}</option>
				{/each}
			</select>
		</label>
		<Input label={$t('connections.endpoint')} bind:value={form.endpoint} placeholder="https://example.com/agent" />
		<Input label={$t('connections.description')} bind:value={form.description} placeholder="Handles review and callback execution" />
		<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
			<input type="checkbox" bind:checked={form.is_active} class="h-4 w-4 rounded border-slate-300" />
			{$t('connections.active')}
		</label>
		<label class="block text-sm">
			<span class="mb-1 block font-medium text-slate-700 dark:text-slate-300">{$t('connections.authPolicy')}</span>
			<textarea bind:value={form.auth_policy} rows="5" class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 font-mono text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"></textarea>
		</label>
		<label class="block text-sm">
			<span class="mb-1 block font-medium text-slate-700 dark:text-slate-300">{$t('connections.capabilityManifest')}</span>
			<textarea bind:value={form.capability_manifest} rows="5" class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 font-mono text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"></textarea>
		</label>
		<div class="flex justify-end gap-2">
			<Button variant="secondary" onclick={() => (showConnectorModal = false)}>{$t('common.cancel')}</Button>
			<Button disabled={saving} onclick={saveConnector}>{saving ? $t('connections.saving') : $t('connections.create')}</Button>
		</div>
	</div>
</Modal>

<Modal bind:open={showInvocationModal} title={$t('connections.invocationDetail')} maxWidthClass="max-w-4xl">
	{#if selectedInvocation}
		<div class="space-y-4 text-sm">
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<div class="font-medium text-slate-700 dark:text-slate-300">{$t('connections.status')}</div>
					<div class="mt-1"><span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(selectedInvocation.status)}">{statusDisplay(selectedInvocation.status)}</span></div>
				</div>
				<div>
					<div class="font-medium text-slate-700 dark:text-slate-300">{$t('connections.trigger')}</div>
					<div class="mt-1 text-slate-600 dark:text-slate-400">{selectedInvocation.trigger_kind}</div>
				</div>
				<div>
					<div class="font-medium text-slate-700 dark:text-slate-300">{$t('connections.connector')}</div>
					<div class="mt-1 text-slate-600 dark:text-slate-400">{selectedInvocation.connector_kind || '-'}</div>
				</div>
				<div>
					<div class="font-medium text-slate-700 dark:text-slate-300">{$t('connections.created')}</div>
					<div class="mt-1 text-slate-600 dark:text-slate-400">{formatDate(selectedInvocation.created_at)}</div>
				</div>
			</div>
			<div>
				<div class="mb-1 font-medium text-slate-700 dark:text-slate-300">{$t('connections.payload')}</div>
				<pre class="max-h-60 overflow-auto rounded-md bg-slate-950 p-3 text-xs text-slate-100">{formatJson(selectedInvocation.payload)}</pre>
			</div>
			<div>
				<div class="mb-1 font-medium text-slate-700 dark:text-slate-300">{$t('connections.result')}</div>
				<pre class="max-h-60 overflow-auto rounded-md bg-slate-950 p-3 text-xs text-slate-100">{formatJson(selectedInvocation.result)}</pre>
			</div>
			<div>
				<div class="mb-2 flex items-center justify-between gap-3">
					<div>
						<div class="font-medium text-slate-700 dark:text-slate-300">{$t('connections.toolCalls')}</div>
						<div class="text-xs text-slate-500 dark:text-slate-400">{$t('connections.auditedCalls', { values: { count: selectedInvocationToolCalls.length } })}</div>
					</div>
					<Button variant="secondary" size="sm" onclick={refreshSelectedInvocationToolCalls}>{$t('connections.refresh')}</Button>
				</div>
				{#if selectedInvocationToolCallsLoading}
					<div class="rounded-md border border-slate-200 p-3 text-slate-500 dark:border-slate-700 dark:text-slate-400">{$t('connections.loadingToolCalls')}</div>
				{:else if selectedInvocationToolCalls.length === 0}
					<div class="rounded-md border border-slate-200 p-3 text-slate-500 dark:border-slate-700 dark:text-slate-400">{$t('connections.noAuditedToolCalls')}</div>
				{:else}
					<div class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700">
						{#each selectedInvocationToolCalls as call}
							<div class="border-b border-slate-200 p-3 last:border-b-0 dark:border-slate-700">
								<div class="flex flex-wrap items-center justify-between gap-2">
									<div class="min-w-0">
										<div class="truncate font-mono text-xs font-semibold text-slate-900 dark:text-slate-100">{call.tool_name}</div>
										<div class="mt-1 text-xs text-slate-500 dark:text-slate-400">{call.transport} · {call.duration_ms}ms · {formatDate(call.started_at)}</div>
									</div>
									<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(call.status)}">{statusDisplay(call.status)}</span>
								</div>
								{#if call.result_summary}
									<div class="mt-2 rounded-md bg-slate-50 p-2 text-xs text-slate-700 dark:bg-slate-800 dark:text-slate-300">{call.result_summary}</div>
								{/if}
								{#if call.error_message}
									<div class="mt-2 rounded-md bg-red-50 p-2 text-xs text-red-700 dark:bg-red-950/40 dark:text-red-300">{call.error_message}</div>
								{/if}
							</div>
						{/each}
					</div>
				{/if}
			</div>
			{#if selectedInvocation.error_message}
				<div class="rounded-md border border-red-200 bg-red-50 p-3 text-red-700">{selectedInvocation.error_message}</div>
			{/if}
		</div>
	{/if}
</Modal>

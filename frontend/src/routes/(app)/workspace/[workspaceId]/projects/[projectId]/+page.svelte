<script lang="ts">
	import { FileText, Power } from '@lucide/svelte';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import {
		projectsApi,
		type Project,
		type ProjectResource,
		type ProjectResourceKind
	} from '$lib/api/projects';
	import { issuesApi, type Issue } from '$lib/api/issues';
	import { aiAgentsApi, type AiAgent } from '$lib/api/ai-agents';
	import { importExportApi } from '$lib/api/import-export';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';
	import { requireRouteParam } from '$lib/utils/route-params';
	import {
		getProjectScenarioTemplate,
		getScenarioFields,
		scenarioItemLabel,
		templateArray
	} from '$lib/utils/scenario-template';
	import {
		scenarioTemplateAudience,
		scenarioTemplateIndustry,
		scenarioTemplateName
	} from '$lib/utils/project-type-i18n';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam($page.params.projectId, 'projectId');
	const locatorPlaceholder = 'Locator, e.g. {"url":"https://..."} or /path/to/repo';

	let project = $state<Project | null>(null);
	let resources = $state<ProjectResource[]>([]);
	let recentIssues = $state<Issue[]>([]);
	let loading = $state(true);
	let exporting = $state(false);
	let importing = $state(false);
	let resourceSubmitting = $state(false);
	let aiAgents = $state<AiAgent[]>([]);
	let assistantConfigDrafts = $state<
		Record<string, { provider: string; model: string; api_endpoint: string }>
	>({});
	let updatingAssistantId = $state<string | null>(null);
	let deletingResourceId = $state<string | null>(null);
	let editingResourceId = $state<string | null>(null);
	let resourceEditDrafts = $state<
		Record<
			string,
			{ kind: ProjectResourceKind; name: string; locator: string; sync_status: string }
		>
	>({});
	let resourceForm = $state({
		kind: 'repo' as ProjectResourceKind,
		name: '',
		locator: ''
	});

	const scenarioTemplate = $derived(getProjectScenarioTemplate(project));
	const scenarioFields = $derived(getScenarioFields(scenarioTemplate));
	const scenarioWorkflowStates = $derived(
		templateArray(scenarioTemplate, 'workflow_template', 'states')
	);
	const scenarioRequiredResources = $derived(
		templateArray(scenarioTemplate, 'resource_schema', 'resources')
	);

	onMount(async () => {
		const loadedProject = await loadProject();
		if (!loadedProject) {
			loading = false;
			return;
		}

		if (loadedProject.type_key === 'custom_form') {
			await goto(`/workspace/${workspaceId}/projects/${projectId}/forms`, { replaceState: true });
			return;
		}

		await Promise.all([loadRecentIssues(), loadResources(), loadAiAgents()]);
		loading = false;
	});

	async function loadProject(): Promise<Project | null> {
		const response = await projectsApi.get(projectId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			project = response.data;
			return response.data;
		}
		return null;
	}

	async function loadRecentIssues() {
		const response = await issuesApi.list(projectId, {
			page: 1,
			per_page: 5,
			sort_by: 'updated_at',
			sort_order: 'desc'
		});
		if (response.data) {
			recentIssues = response.data.items ?? [];
		}
	}

	async function loadResources() {
		const response = await projectsApi.listResources(projectId);
		if (response.code === 0 && response.data) {
			resources = response.data.items ?? [];
		}
	}

	async function loadAiAgents() {
		const response = await aiAgentsApi.list(projectId);
		if (response.code === 0 && response.data) {
			aiAgents = response.data.items ?? [];
		}
	}

	function isRecord(value: unknown): value is Record<string, unknown> {
		return typeof value === 'object' && value !== null && !Array.isArray(value);
	}

	function scenarioValue(item: unknown, key: string, fallback = ''): string {
		if (!isRecord(item)) return fallback;
		const value = item[key];
		return typeof value === 'string' && value.trim() ? value.trim() : fallback;
	}

	function scenarioArrayValue(item: unknown, key: string): string[] {
		if (!isRecord(item) || !Array.isArray(item[key])) return [];
		return item[key].filter(
			(value): value is string => typeof value === 'string' && value.trim().length > 0
		);
	}

	function scenarioBooleanValue(item: unknown, key: string): boolean {
		return isRecord(item) ? Boolean(item[key]) : false;
	}

	function eventInputValue(event: Event): string {
		return event.currentTarget instanceof HTMLInputElement ? event.currentTarget.value : '';
	}

	function assistantRoles(): unknown[] {
		return scenarioTemplate?.ai_roles ?? [];
	}

	function assistantForRole(role: unknown): AiAgent | undefined {
		const roleKey = scenarioValue(role, 'key');
		const roleLabel = scenarioItemLabel(role, roleKey);
		return aiAgents.find((agent) => {
			const roleMeta = agent.domain_overrides?.role;
			const metaKey = isRecord(roleMeta) && typeof roleMeta.key === 'string' ? roleMeta.key : '';
			return metaKey === roleKey || agent.name === roleLabel || agent.id.endsWith(roleKey);
		});
	}

	function assistantProviderForRole(role: unknown): string {
		return scenarioValue(role, 'provider', scenarioValue(role, 'agent_type', 'assistant'));
	}

	function assistantModelForRole(role: unknown): string {
		return scenarioValue(role, 'model', 'configured-by-operator');
	}

	function assistantDraft(assistant: AiAgent, role: unknown) {
		return (
			assistantConfigDrafts[assistant.id] ?? {
				provider: assistant.provider || assistantProviderForRole(role),
				model:
					assistant.model === 'template-placeholder'
						? assistantModelForRole(role)
						: assistant.model || assistantModelForRole(role),
				api_endpoint: assistant.api_endpoint ?? scenarioValue(role, 'api_endpoint')
			}
		);
	}

	function setAssistantDraft(
		assistant: AiAgent,
		field: 'provider' | 'model' | 'api_endpoint',
		value: string,
		role: unknown
	) {
		assistantConfigDrafts = {
			...assistantConfigDrafts,
			[assistant.id]: {
				...assistantDraft(assistant, role),
				[field]: value
			}
		};
	}

	async function saveAssistantConfig(role: unknown) {
		const assistant = assistantForRole(role);
		if (!assistant) return;
		const draft = assistantDraft(assistant, role);
		const provider = draft.provider.trim();
		const model = draft.model.trim();
		if (!provider || !model) {
			toast.error(get(t)('project.assistantConfigRequired'));
			return;
		}
		updatingAssistantId = assistant.id;
		const response = await aiAgentsApi.update(projectId, assistant.id, {
			provider,
			model,
			api_endpoint: draft.api_endpoint.trim() || undefined,
			is_active: true
		});
		updatingAssistantId = null;
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		toast.success(get(t)('project.assistantSaved'));
		await loadAiAgents();
	}

	function parseLocator(raw: string): Record<string, unknown> {
		const trimmed = raw.trim();
		if (!trimmed) {
			return {};
		}
		try {
			const parsed = JSON.parse(trimmed) as unknown;
			if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
				return parsed as Record<string, unknown>;
			}
			throw new Error('locator must be an object');
		} catch {
			return { value: trimmed };
		}
	}

	function formatLocator(locator: Record<string, unknown>): string {
		const keys = Object.keys(locator ?? {});
		if (keys.length === 0) {
			return $t('resources.noLocator');
		}
		if (typeof locator.url === 'string') return locator.url;
		if (typeof locator.path === 'string') return locator.path;
		if (typeof locator.value === 'string') return locator.value;
		return JSON.stringify(locator);
	}

	function resourceKindLabel(kind: ProjectResourceKind): string {
		const labels: Record<ProjectResourceKind, string> = {
			repo: $t('resources.kindRepo'),
			directory: $t('resources.kindDirectory'),
			document_library: $t('resources.kindDocumentLibrary'),
			crm_account: $t('resources.kindCrmAccount'),
			erp_order: $t('resources.kindErpOrder'),
			equipment: $t('resources.kindEquipment'),
			site: $t('resources.kindSite'),
			custom: $t('resources.kindCustom')
		};
		return labels[kind] ?? kind;
	}

	function resourceDraft(resource: ProjectResource) {
		return (
			resourceEditDrafts[resource.id] ?? {
				kind: resource.kind,
				name: resource.name,
				locator: formatLocator(resource.locator),
				sync_status: resource.sync_status
			}
		);
	}

	function setResourceDraft(
		resource: ProjectResource,
		field: 'kind' | 'name' | 'locator' | 'sync_status',
		value: string
	) {
		resourceEditDrafts = {
			...resourceEditDrafts,
			[resource.id]: {
				...resourceDraft(resource),
				[field]: field === 'kind' ? (value as ProjectResourceKind) : value
			}
		};
	}

	function startEditResource(resource: ProjectResource) {
		editingResourceId = resource.id;
		resourceEditDrafts = {
			...resourceEditDrafts,
			[resource.id]: {
				kind: resource.kind,
				name: resource.name,
				locator: formatLocator(resource.locator),
				sync_status: resource.sync_status
			}
		};
	}

	async function handleUpdateResource(resource: ProjectResource) {
		const draft = resourceDraft(resource);
		if (!draft.name.trim()) {
			toast.error('Resource name is required');
			return;
		}
		resourceSubmitting = true;
		const response = await projectsApi.updateResource(projectId, resource.id, {
			kind: draft.kind,
			name: draft.name.trim(),
			locator: parseLocator(draft.locator),
			sync_status: draft.sync_status.trim() || 'manual'
		});
		resourceSubmitting = false;
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		if (response.data) {
			resources = resources.map((item) => (item.id === response.data?.id ? response.data : item));
		}
		editingResourceId = null;
		toast.success('Project resource updated');
	}

	async function handleCreateResource() {
		if (!resourceForm.name.trim()) {
			toast.error('Resource name is required');
			return;
		}

		resourceSubmitting = true;
		const response = await projectsApi.createResource(projectId, {
			kind: resourceForm.kind,
			name: resourceForm.name.trim(),
			locator: parseLocator(resourceForm.locator)
		});
		resourceSubmitting = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		resourceForm = { kind: 'repo', name: '', locator: '' };
		toast.success('Project resource added');
		await loadResources();
	}

	async function handleDeleteResource(resource: ProjectResource) {
		deletingResourceId = resource.id;
		const response = await projectsApi.deleteResource(projectId, resource.id);
		deletingResourceId = null;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		resources = resources.filter((item) => item.id !== resource.id);
		toast.success('Project resource deleted');
	}

	async function handleExportProject() {
		exporting = true;
		const response = await importExportApi.exportProject(projectId);
		if (response.code !== 0 || !response.data) {
			toast.error(response.message || get(t)('project.exportFailed'));
			exporting = false;
			return;
		}

		const payload = JSON.stringify(response.data, null, 2);
		const blob = new Blob([payload], { type: 'application/json' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${project?.key ?? 'project'}-export.json`;
		document.body.append(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);

		toast.success(get(t)('project.exportSuccess'));
		exporting = false;
	}

	async function handleImportFile(event: Event) {
		const target = event.currentTarget as HTMLInputElement;
		const file = target.files?.[0];
		if (!file) {
			return;
		}

		importing = true;
		try {
			const content = await file.text();
			const payload = JSON.parse(content) as unknown;
			const response = await importExportApi.importProject(workspaceId, payload);
			if (response.code !== 0) {
				toast.error(response.message);
			} else {
				toast.success(get(t)('project.importSuccess'));
			}
		} catch {
			toast.error(get(t)('project.importInvalidFile'));
		} finally {
			target.value = '';
			importing = false;
		}
	}
</script>

<div class="max-w-7xl mx-auto">
	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		</div>
	{:else if project}
		<div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<div class="mb-2 flex items-center gap-3">
					<span
						class="rounded bg-slate-100 dark:bg-slate-800 px-2 py-1 text-sm text-slate-600 dark:text-slate-300"
						>{project.key}</span
					>
					<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{project.name}</h1>
				</div>
				{#if project.description}
					<p class="text-slate-600 dark:text-slate-300">{project.description}</p>
				{/if}
			</div>
			<div class="flex flex-wrap gap-2">
				<Button variant="secondary" loading={exporting} onclick={handleExportProject}
					>{$t('project.exportJson')}</Button
				>
				<label
					class="inline-flex cursor-pointer items-center justify-center rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700"
				>
					{importing ? $t('project.importingJson') : $t('project.importJson')}
					<input
						type="file"
						accept="application/json"
						class="hidden"
						disabled={importing}
						onchange={handleImportFile}
					/>
				</label>
			</div>
		</div>

		<div class="mb-8 grid gap-5 lg:grid-cols-[minmax(0,2fr)_minmax(280px,1fr)]">
			<section class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-900">
				<div class="mb-4">
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('project.projectManagement')}
					</h2>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
						{$t('project.projectManagementDesc')}
					</p>
				</div>
				<div class="grid gap-3 md:grid-cols-3">
					<a
						href={`/workspace/${workspaceId}/projects/${projectId}/issues`}
						class="rounded-md border border-slate-200 p-4 transition-colors hover:border-blue-200 hover:bg-blue-50 dark:border-slate-700 dark:hover:bg-slate-800"
					>
						<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.workItems')}
						</h3>
						<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('project.workItemsDesc')}</p>
					</a>
					<a
						href={`/workspace/${workspaceId}/projects/${projectId}/board`}
						class="rounded-md border border-slate-200 p-4 transition-colors hover:border-blue-200 hover:bg-blue-50 dark:border-slate-700 dark:hover:bg-slate-800"
					>
						<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.boardView')}
						</h3>
						<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('project.boardViewDesc')}</p>
					</a>
					<a
						href={`/workspace/${workspaceId}/projects/${projectId}/cycles`}
						class="rounded-md border border-slate-200 p-4 transition-colors hover:border-blue-200 hover:bg-blue-50 dark:border-slate-700 dark:hover:bg-slate-800"
					>
						<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.sprintPlan')}
						</h3>
						<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('project.sprintPlanDesc')}</p>
					</a>
				</div>
			</section>

			<section class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-900">
				<div class="mb-4">
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('project.businessData')}
					</h2>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
						{$t('project.businessDataDesc')}
					</p>
				</div>
				<a
					href={`/workspace/${workspaceId}/projects/${projectId}/forms`}
					class="flex items-start gap-3 rounded-md border border-slate-200 p-4 transition-colors hover:border-blue-200 hover:bg-blue-50 dark:border-slate-700 dark:hover:bg-slate-800"
				>
					<FileText class="mt-0.5 h-5 w-5 text-blue-600 dark:text-blue-300" aria-hidden="true" />
					<span>
						<span class="block text-sm font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.forms')}
						</span>
						<span class="mt-1 block text-sm text-slate-600 dark:text-slate-300">
							{$t('project.formsDesc')}
						</span>
					</span>
				</a>
			</section>
		</div>

		{#if scenarioTemplate}
			<div
				class="mb-8 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-700 dark:bg-slate-900"
			>
				<div class="mb-5 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
					<div>
						<p class="text-xs font-medium uppercase tracking-wide text-blue-600 dark:text-blue-300">
							{scenarioTemplateIndustry(scenarioTemplate.industry, $t)}
						</p>
						<h2 class="mt-1 text-lg font-semibold text-slate-900 dark:text-slate-100">
							{scenarioTemplateName(scenarioTemplate, $t)}
						</h2>
						<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
							{scenarioTemplateAudience(scenarioTemplate.audience_label, scenarioTemplate.key, $t)}
						</p>
					</div>
					<a
						href={`/workspace/${workspaceId}/projects/${projectId}/issues`}
						class="inline-flex items-center justify-center rounded-md border border-slate-200 px-3 py-2 text-sm font-medium text-slate-700 transition-colors hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
					>
						{$t('project.openScenarioWork')}
					</a>
				</div>

				<div class="grid gap-3 md:grid-cols-4">
					<div class="rounded-lg bg-slate-50 p-4 dark:bg-slate-950">
						<p class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
							{$t('project.workflow')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-slate-900 dark:text-slate-100">
							{scenarioWorkflowStates.length}
						</p>
					</div>
					<div class="rounded-lg bg-slate-50 p-4 dark:bg-slate-950">
						<p class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
							{$t('project.fields')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-slate-900 dark:text-slate-100">
							{scenarioFields.length}
						</p>
					</div>
					<div class="rounded-lg bg-slate-50 p-4 dark:bg-slate-950">
						<p class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
							{$t('project.resources')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-slate-900 dark:text-slate-100">
							{scenarioRequiredResources.length}
						</p>
					</div>
					<div class="rounded-lg bg-slate-50 p-4 dark:bg-slate-950">
						<p class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
							{$t('project.assistants')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-slate-900 dark:text-slate-100">
							{scenarioTemplate.ai_roles.length}
						</p>
					</div>
				</div>

				<div class="mt-5 grid gap-5 lg:grid-cols-3">
					<div>
						<h3 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.scenarioFields')}
						</h3>
						<div class="space-y-2">
							{#each scenarioFields.slice(0, 5) as field}
								<div
									class="flex items-center justify-between rounded-md border border-slate-200 px-3 py-2 text-sm dark:border-slate-700"
								>
									<span class="text-slate-700 dark:text-slate-300">{field.label}</span>
									<span class="text-xs text-slate-500 dark:text-slate-400">{field.type}</span>
								</div>
							{/each}
						</div>
					</div>
					<div>
						<h3 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.requiredResources')}
						</h3>
						<div class="space-y-2">
							{#each scenarioRequiredResources.slice(0, 5) as item, index}
								<div
									class="rounded-md border border-slate-200 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:text-slate-300"
								>
									{scenarioItemLabel(item, `Resource ${index + 1}`)}
								</div>
							{/each}
						</div>
					</div>
				</div>
			</div>

			<div
				class="mb-8 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-700 dark:bg-slate-900"
			>
				<div class="mb-5 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
					<div>
						<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.scenarioSetup')}
						</h2>
						<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
							{scenarioTemplateAudience(scenarioTemplate.audience_label, scenarioTemplate.key, $t)}
						</p>
					</div>
				</div>

				<div class="grid gap-5">
					<div>
						<h3 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">
							{$t('project.assistantSetup')}
						</h3>
						{#if assistantRoles().length === 0}
							<p
								class="rounded-md border border-dashed border-slate-300 p-4 text-sm text-slate-500 dark:border-slate-700 dark:text-slate-400"
							>
								{$t('project.noAssistantRoles')}
							</p>
						{:else}
							<div class="space-y-3">
								{#each assistantRoles() as role (scenarioValue(role, 'key', scenarioItemLabel(role, 'assistant')))}
									{@const assistant = assistantForRole(role)}
									<div class="rounded-md border border-slate-200 p-4 dark:border-slate-700">
										<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
											<div class="min-w-0">
												<div class="flex flex-wrap items-center gap-2">
													<h4 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
														{scenarioItemLabel(role, $t('project.assistant'))}
													</h4>
													<span
														class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300"
														>{scenarioValue(role, 'agent_type', 'assistant')}</span
													>
													<span
														class="rounded px-2 py-0.5 text-xs font-medium {assistant?.is_active
															? 'bg-emerald-50 text-emerald-700'
															: 'bg-amber-50 text-amber-700'}"
													>
														{assistant?.is_active ? $t('project.active') : $t('project.needsSetup')}
													</span>
												</div>
												<div class="mt-2 flex flex-wrap gap-1.5">
													{#each scenarioArrayValue(role, 'capabilities') as capability}
														<span
															class="rounded border border-slate-200 bg-slate-50 px-1.5 py-0.5 font-mono text-xs text-slate-600 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300"
															>{capability}</span
														>
													{/each}
												</div>
												{#if scenarioBooleanValue(role, 'writes_require_approval')}
													<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
														{$t('project.writesRequireApproval')}
													</p>
												{/if}
												{#if assistant}
													{@const draft = assistantDraft(assistant, role)}
													<div class="mt-3 grid gap-2 sm:grid-cols-3">
														<label
															class="block text-xs font-medium text-slate-600 dark:text-slate-300"
														>
															{$t('project.provider')}
															<input
																class="mt-1 block w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																value={draft.provider}
																placeholder={assistantProviderForRole(role)}
																oninput={(event) =>
																	setAssistantDraft(
																		assistant,
																		'provider',
																		eventInputValue(event),
																		role
																	)}
															/>
														</label>
														<label
															class="block text-xs font-medium text-slate-600 dark:text-slate-300"
														>
															{$t('project.model')}
															<input
																class="mt-1 block w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																value={draft.model}
																placeholder={assistantModelForRole(role)}
																oninput={(event) =>
																	setAssistantDraft(
																		assistant,
																		'model',
																		eventInputValue(event),
																		role
																	)}
															/>
														</label>
														<label
															class="block text-xs font-medium text-slate-600 dark:text-slate-300"
														>
															{$t('project.apiEndpoint')}
															<input
																class="mt-1 block w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																value={draft.api_endpoint}
																placeholder="https://..."
																oninput={(event) =>
																	setAssistantDraft(
																		assistant,
																		'api_endpoint',
																		eventInputValue(event),
																		role
																	)}
															/>
														</label>
													</div>
												{/if}
											</div>
											<Button
												size="sm"
												variant={assistant?.is_active ? 'secondary' : 'primary'}
												disabled={!assistant || updatingAssistantId === assistant.id}
												onclick={() => saveAssistantConfig(role)}
											>
												<Power class="mr-2 h-4 w-4" aria-hidden="true" />
												{assistant?.is_active
													? $t('project.saveAssistant')
													: $t('project.enableAssistant')}
											</Button>
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</div>

				</div>
			</div>
		{/if}

		<div
			class="mb-8 rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6"
		>
			<div class="mb-4 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
				<div>
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('resources.title')}
					</h2>
					<p class="text-sm text-slate-600 dark:text-slate-300">
						{$t('resources.description')}
					</p>
				</div>
			</div>

			<div class="mb-5 grid grid-cols-1 gap-3 md:grid-cols-[160px_1fr_1.3fr_auto]">
				<select
					bind:value={resourceForm.kind}
					class="rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					aria-label={$t('resources.resourceKind')}
				>
					<option value="repo">{$t('resources.kindRepo')}</option>
					<option value="directory">{$t('resources.kindDirectory')}</option>
					<option value="document_library">{$t('resources.kindDocumentLibrary')}</option>
					<option value="crm_account">{$t('resources.kindCrmAccount')}</option>
					<option value="erp_order">{$t('resources.kindErpOrder')}</option>
					<option value="equipment">{$t('resources.kindEquipment')}</option>
					<option value="site">{$t('resources.kindSite')}</option>
					<option value="custom">{$t('resources.kindCustom')}</option>
				</select>
				<input
					bind:value={resourceForm.name}
					placeholder={$t('resources.name')}
					class="rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
				/>
				<input
					bind:value={resourceForm.locator}
					placeholder={locatorPlaceholder}
					class="rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
				/>
				<Button loading={resourceSubmitting} onclick={handleCreateResource}>{$t('resources.add')}</Button>
			</div>

			{#if resources.length === 0}
				<p
					class="rounded-md border border-dashed border-slate-300 dark:border-slate-700 p-4 text-sm text-slate-500 dark:text-slate-400"
				>
					{$t('resources.empty')}
				</p>
			{:else}
				<div class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700">
					<table class="min-w-full divide-y divide-slate-200 dark:divide-slate-700">
						<thead class="bg-slate-50 dark:bg-slate-800">
							<tr>
								<th
									class="px-4 py-2 text-left text-xs font-medium uppercase text-slate-500 dark:text-slate-400"
									>{$t('resources.kind')}</th
								>
								<th
									class="px-4 py-2 text-left text-xs font-medium uppercase text-slate-500 dark:text-slate-400"
									>{$t('resources.name')}</th
								>
								<th
									class="px-4 py-2 text-left text-xs font-medium uppercase text-slate-500 dark:text-slate-400"
									>{$t('resources.locator')}</th
								>
								<th
									class="px-4 py-2 text-right text-xs font-medium uppercase text-slate-500 dark:text-slate-400"
									>{$t('common.actions')}</th
								>
							</tr>
						</thead>
						<tbody class="divide-y divide-slate-200 dark:divide-slate-700">
							{#each resources as resource (resource.id)}
								{@const editingResource = editingResourceId === resource.id}
								{@const draft = resourceDraft(resource)}
								<tr>
									<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">
										{#if editingResource}
											<select
												value={draft.kind}
												onchange={(event) =>
													setResourceDraft(resource, 'kind', event.currentTarget.value)}
												class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											>
												<option value="repo">{$t('resources.kindRepo')}</option>
												<option value="directory">{$t('resources.kindDirectory')}</option>
												<option value="document_library">{$t('resources.kindDocumentLibrary')}</option>
												<option value="crm_account">{$t('resources.kindCrmAccount')}</option>
												<option value="erp_order">{$t('resources.kindErpOrder')}</option>
												<option value="equipment">{$t('resources.kindEquipment')}</option>
												<option value="site">{$t('resources.kindSite')}</option>
												<option value="custom">{$t('resources.kindCustom')}</option>
											</select>
										{:else}
											{resourceKindLabel(resource.kind)}
										{/if}
									</td>
									<td class="px-4 py-3 text-sm font-medium text-slate-900 dark:text-slate-100">
										{#if editingResource}
											<input
												value={draft.name}
												oninput={(event) =>
													setResourceDraft(resource, 'name', event.currentTarget.value)}
												class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											/>
										{:else}
											{resource.name}
										{/if}
									</td>
									<td class="max-w-md px-4 py-3 text-sm text-slate-600 dark:text-slate-300">
										{#if editingResource}
											<input
												value={draft.locator}
												oninput={(event) =>
													setResourceDraft(resource, 'locator', event.currentTarget.value)}
												class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											/>
										{:else}
											<span class="block truncate">{formatLocator(resource.locator)}</span>
										{/if}
									</td>
									<td class="px-4 py-3 text-right">
										{#if editingResource}
											<div class="flex justify-end gap-2">
												<Button
													size="sm"
													variant="secondary"
													onclick={() => (editingResourceId = null)}>{$t('common.cancel')}</Button
												>
												<Button
													size="sm"
													loading={resourceSubmitting}
													onclick={() => handleUpdateResource(resource)}>{$t('common.save')}</Button
												>
											</div>
										{:else}
											<div class="flex justify-end gap-3">
												<button
													type="button"
													onclick={() => startEditResource(resource)}
													class="text-sm text-blue-600 hover:text-blue-700"
												>
													{$t('common.edit')}
												</button>
												<button
													type="button"
													disabled={deletingResourceId === resource.id}
													onclick={() => handleDeleteResource(resource)}
													class="text-sm text-red-600 hover:text-red-700 disabled:cursor-not-allowed disabled:opacity-60"
												>
													{$t('common.delete')}
												</button>
											</div>
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>

		<div
			class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6"
		>
			<h2 class="mb-4 text-lg font-semibold text-slate-900 dark:text-slate-100">
				{$t('project.recentUpdates')}
			</h2>
			{#if recentIssues.length === 0}
				<p class="text-slate-500 dark:text-slate-400">{$t('project.noRecentWorkItems')}</p>
			{:else}
				<div class="space-y-3">
					{#each recentIssues as issue}
						<a
							href={`/workspace/${workspaceId}/projects/${projectId}/issues/${issue.id}`}
							class="block rounded-md p-3 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
						>
							<div class="space-y-1.5">
								<div class="flex items-center gap-2">
									<span class="font-mono text-sm text-slate-500 dark:text-slate-400"
										>{issue.key}</span
									>
									<span class="text-sm font-medium text-slate-900 dark:text-slate-100"
										>{issue.title}</span
									>
									<span
										class="ml-auto rounded px-2 py-1 text-xs {issue.status === 'done'
											? 'bg-green-100 text-green-700'
											: 'bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-300'}"
									>
										{issue.status}
									</span>
								</div>
								{#if issue.labels?.length}
									<div class="flex flex-wrap gap-1">
										{#each issue.labels as label}
											<span
												class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium"
												style={`background-color: ${label.color}20; color: ${label.color}; border: 1px solid ${label.color}40;`}
											>
												{label.name}
											</span>
										{/each}
									</div>
								{/if}
							</div>
						</a>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
</div>

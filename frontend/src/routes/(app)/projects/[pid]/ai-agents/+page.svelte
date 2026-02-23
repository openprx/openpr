<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { authStore } from '$lib/stores/auth';
	import { isAdminUser } from '$lib/utils/auth';
	import { aiAgentsApi, type AiAgent, type AiAgentStats, type AiLevel } from '$lib/api/ai-agents';
	import { projectsApi } from '$lib/api/projects';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import Input from '$lib/components/Input.svelte';

	const projectId = $derived($page.params.pid || '');
	const canManage = $derived(isAdminUser($authStore.user));

	let loading = $state(false);
	let saving = $state(false);
	let projectName = $state('');
	let agents = $state<AiAgent[]>([]);
	let statsMap = $state<Record<string, AiAgentStats>>({});
	let showCreate = $state(false);
	let showEdit = $state(false);
	let editTarget = $state<AiAgent | null>(null);

	let createForm = $state({
		id: '',
		name: '',
		model: '',
		provider: '',
		api_endpoint: '',
		capabilities: 'code_analysis, test_generation',
		domain_overrides: '',
		max_domain_level: 'voter' as AiLevel,
		can_veto_human_consensus: false,
		reason_min_length: 50
	});

	let editForm = $state({
		name: '',
		model: '',
		provider: '',
		api_endpoint: '',
		capabilities: '',
		domain_overrides: '',
		max_domain_level: 'voter' as AiLevel,
		can_veto_human_consensus: false,
		reason_min_length: 50,
		is_active: true
	});

	onMount(() => {
		void loadAll();
	});

	async function loadAll() {
		loading = true;
		const [projectRes, listRes] = await Promise.all([projectsApi.get(projectId), aiAgentsApi.list(projectId)]);
		if (projectRes.code === 0 && projectRes.data) {
			projectName = projectRes.data.name;
		}
		if (listRes.code !== 0 || !listRes.data) {
			toast.error(listRes.message || get(t)('aiAgents.loadFailed'));
			agents = [];
			loading = false;
			return;
		}
		agents = listRes.data.items;
		await loadStats();
		loading = false;
	}

	async function loadStats() {
		const entries = await Promise.all(
			agents.map(async (agent) => {
				const res = await aiAgentsApi.stats(projectId, agent.id);
				return [agent.id, res.code === 0 && res.data ? res.data : null] as const;
			})
		);

		statsMap = Object.fromEntries(entries.filter((item) => item[1]).map((item) => [item[0], item[1]])) as Record<string, AiAgentStats>;
	}

	function parseCapabilities(value: string): string[] {
		return value
			.split(',')
			.map((item) => item.trim())
			.filter(Boolean);
	}

	function parseDomainOverrides(value: string): { ok: true; value: Record<string, string> | undefined } | { ok: false } {
		const trimmed = value.trim();
		if (!trimmed) return { ok: true, value: undefined };
		try {
			const parsed = JSON.parse(trimmed) as unknown;
			if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
				toast.error(get(t)('aiAgents.domainOverridesInvalid'));
				return { ok: false };
			}
			return {
				ok: true,
				value: Object.fromEntries(
					Object.entries(parsed as Record<string, unknown>).map(([k, v]) => [k, String(v)])
				)
			};
		} catch {
			toast.error(get(t)('aiAgents.domainOverridesInvalid'));
			return { ok: false };
		}
	}

	async function createAgent() {
		if (!canManage) return;
		const parsedDomainOverrides = parseDomainOverrides(createForm.domain_overrides);
		if (!parsedDomainOverrides.ok) return;

		saving = true;
		const payload = {
			id: createForm.id.trim(),
			name: createForm.name.trim(),
			model: createForm.model.trim(),
			provider: createForm.provider.trim(),
			api_endpoint: createForm.api_endpoint.trim() || undefined,
			capabilities: parseCapabilities(createForm.capabilities),
			domain_overrides: parsedDomainOverrides.value,
			max_domain_level: createForm.max_domain_level,
			can_veto_human_consensus: createForm.can_veto_human_consensus,
			reason_min_length: Number(createForm.reason_min_length)
		};

		const res = await aiAgentsApi.create(projectId, payload);
		saving = false;
		if (res.code !== 0) {
			toast.error(res.message || get(t)('aiAgents.createFailed'));
			return;
		}
		toast.success(get(t)('aiAgents.createSuccess'));
		showCreate = false;
		await loadAll();
	}

	function openEdit(agent: AiAgent) {
		editTarget = agent;
		editForm = {
			name: agent.name,
			model: agent.model,
			provider: agent.provider,
			api_endpoint: agent.api_endpoint ?? '',
			capabilities: agent.capabilities.join(', '),
			domain_overrides: agent.domain_overrides ? JSON.stringify(agent.domain_overrides) : '',
			max_domain_level: agent.max_domain_level,
			can_veto_human_consensus: agent.can_veto_human_consensus,
			reason_min_length: agent.reason_min_length,
			is_active: agent.is_active
		};
		showEdit = true;
	}

	async function saveEdit() {
		if (!editTarget || !canManage) return;
		const parsedDomainOverrides = parseDomainOverrides(editForm.domain_overrides);
		if (!parsedDomainOverrides.ok) return;

		saving = true;
		const res = await aiAgentsApi.update(projectId, editTarget.id, {
			name: editForm.name.trim(),
			model: editForm.model.trim(),
			provider: editForm.provider.trim(),
			api_endpoint: editForm.api_endpoint.trim() || undefined,
			capabilities: parseCapabilities(editForm.capabilities),
			domain_overrides: parsedDomainOverrides.value,
			max_domain_level: editForm.max_domain_level,
			can_veto_human_consensus: editForm.can_veto_human_consensus,
			reason_min_length: Number(editForm.reason_min_length),
			is_active: editForm.is_active
		});
		saving = false;
		if (res.code !== 0) {
			toast.error(res.message || get(t)('aiAgents.updateFailed'));
			return;
		}
		toast.success(get(t)('aiAgents.updateSuccess'));
		showEdit = false;
		editTarget = null;
		await loadAll();
	}

	async function removeAgent(agentId: string) {
		if (!canManage) return;
		if (!confirm(get(t)('aiAgents.removeConfirm'))) {
			return;
		}
		const res = await aiAgentsApi.delete(projectId, agentId);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('aiAgents.removeFailed'));
			return;
		}
		toast.success(get(t)('aiAgents.removeSuccess'));
		await loadAll();
	}

	function levelLabel(level: string): string {
		return get(t)(`trustCommon.level.${level}`) || level;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.aiAgents')}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900">
		<div class="mb-4 flex flex-wrap items-center justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('aiAgents.title')}</h1>
				<p class="text-sm text-slate-500">{$t('aiAgents.subtitle', { values: { project: projectName || projectId } })}</p>
			</div>
			{#if canManage}
				<Button onclick={() => (showCreate = true)}>{$t('aiAgents.create')}</Button>
			{/if}
		</div>

		{#if loading}
			<div class="py-8 text-center text-slate-500">{$t('common.loading')}</div>
		{:else if agents.length === 0}
			<div class="py-8 text-center text-slate-500">{$t('aiAgents.empty')}</div>
		{:else}
			<div class="grid grid-cols-1 gap-3 lg:grid-cols-2">
				{#each agents as agent (agent.id)}
					<div class="rounded-lg border border-slate-200 p-4 dark:border-slate-700">
						<div class="flex items-center justify-between gap-3">
							<div>
								<div class="flex items-center gap-2">
									<span class="text-lg">🤖</span>
									<h2 class="font-semibold text-slate-900 dark:text-slate-100">{agent.name}</h2>
								</div>
								<p class="text-xs text-slate-500">{agent.id}</p>
							</div>
							<span class={`rounded px-2 py-0.5 text-xs ${agent.is_active ? 'bg-emerald-100 text-emerald-700' : 'bg-slate-100 text-slate-700'}`}>{agent.is_active ? $t('aiAgents.active') : $t('aiAgents.inactive')}</span>
						</div>

						<div class="mt-3 grid grid-cols-2 gap-2 text-sm text-slate-600 dark:text-slate-300">
							<div>{$t('aiAgents.model')}: {agent.model}</div>
							<div>{$t('aiAgents.provider')}: {agent.provider}</div>
							<div>{$t('aiAgents.maxLevel')}: {levelLabel(agent.max_domain_level)}</div>
							<div>{$t('aiAgents.reasonMinLength')}: {agent.reason_min_length}</div>
						</div>

						<div class="mt-3 text-xs text-slate-500">{$t('aiAgents.capabilities')}: {agent.capabilities.join(', ') || '-'}</div>
						<div class="mt-2 text-xs text-slate-500">{$t('aiAgents.domainOverrides')}: {agent.domain_overrides ? JSON.stringify(agent.domain_overrides) : $t('aiAgents.autoByTrust')}</div>
						<div class="mt-2 text-xs text-slate-500">{$t('aiAgents.canVetoHumanConsensus')}: {agent.can_veto_human_consensus ? $t('common.confirm') : $t('common.cancel')}</div>

						{#if statsMap[agent.id]}
							<div class="mt-3 rounded border border-slate-200 bg-slate-50 p-3 text-xs text-slate-600 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300">
								<div>{$t('aiAgents.statsVotes')}: {statsMap[agent.id].total_votes}</div>
								<div>{$t('aiAgents.statsComments')}: {statsMap[agent.id].total_comments}</div>
								<div>{$t('aiAgents.statsLastActive')}: {statsMap[agent.id].last_active_at ? new Date(statsMap[agent.id].last_active_at || '').toLocaleString() : '-'}</div>
							</div>
						{/if}

						{#if canManage}
							<div class="mt-3 flex gap-2">
								<Button variant="secondary" size="sm" onclick={() => openEdit(agent)}>{$t('common.edit')}</Button>
								<Button variant="danger" size="sm" onclick={() => removeAgent(agent.id)}>{$t('common.delete')}</Button>
							</div>
						{/if}
						<div class="mt-2">
							<a href={`/ai-agents/${agent.id}/learning`} class="text-xs text-blue-600 hover:underline">
								{$t('governanceExt.learningEntry')}
							</a>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<Modal bind:open={showCreate} title={$t('aiAgents.createTitle')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			void createAgent();
		}}
		class="space-y-3"
	>
		<Input label={$t('aiAgents.agentId')} bind:value={createForm.id} required />
		<Input label={$t('aiAgents.name')} bind:value={createForm.name} required />
		<Input label={$t('aiAgents.model')} bind:value={createForm.model} required />
		<Input label={$t('aiAgents.provider')} bind:value={createForm.provider} required />
		<Input label={$t('aiAgents.apiEndpoint')} bind:value={createForm.api_endpoint} />
		<Input label={$t('aiAgents.capabilitiesInput')} bind:value={createForm.capabilities} required />
		<Input label={$t('aiAgents.domainOverridesInput')} bind:value={createForm.domain_overrides} />

		<div>
			<label for="create-max-level" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('aiAgents.maxLevel')}</label>
			<select id="create-max-level" bind:value={createForm.max_domain_level} class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
				<option value="observer">{levelLabel('observer')}</option>
				<option value="advisor">{levelLabel('advisor')}</option>
				<option value="voter">{levelLabel('voter')}</option>
				<option value="vetoer">{levelLabel('vetoer')}</option>
				<option value="autonomous">{levelLabel('autonomous')}</option>
			</select>
		</div>

		<Input type="number" label={$t('aiAgents.reasonMinLength')} bind:value={createForm.reason_min_length} />
		<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
			<input type="checkbox" bind:checked={createForm.can_veto_human_consensus} />
			{$t('aiAgents.canVetoHumanConsensus')}
		</label>

		<div class="flex justify-end gap-2">
			<Button variant="secondary" onclick={() => (showCreate = false)}>{$t('common.cancel')}</Button>
			<Button type="submit" loading={saving}>{$t('common.create')}</Button>
		</div>
	</form>
</Modal>

<Modal bind:open={showEdit} title={$t('aiAgents.editTitle')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			void saveEdit();
		}}
		class="space-y-3"
	>
		<Input label={$t('aiAgents.name')} bind:value={editForm.name} required />
		<Input label={$t('aiAgents.model')} bind:value={editForm.model} required />
		<Input label={$t('aiAgents.provider')} bind:value={editForm.provider} required />
		<Input label={$t('aiAgents.apiEndpoint')} bind:value={editForm.api_endpoint} />
		<Input label={$t('aiAgents.capabilitiesInput')} bind:value={editForm.capabilities} required />
		<Input label={$t('aiAgents.domainOverridesInput')} bind:value={editForm.domain_overrides} />

		<div>
			<label for="edit-max-level" class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('aiAgents.maxLevel')}</label>
			<select id="edit-max-level" bind:value={editForm.max_domain_level} class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
				<option value="observer">{levelLabel('observer')}</option>
				<option value="advisor">{levelLabel('advisor')}</option>
				<option value="voter">{levelLabel('voter')}</option>
				<option value="vetoer">{levelLabel('vetoer')}</option>
				<option value="autonomous">{levelLabel('autonomous')}</option>
			</select>
		</div>

		<Input type="number" label={$t('aiAgents.reasonMinLength')} bind:value={editForm.reason_min_length} />
		<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
			<input type="checkbox" bind:checked={editForm.can_veto_human_consensus} />
			{$t('aiAgents.canVetoHumanConsensus')}
		</label>
		<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
			<input type="checkbox" bind:checked={editForm.is_active} />
			{$t('aiAgents.active')}
		</label>

		<div class="flex justify-end gap-2">
			<Button variant="secondary" onclick={() => (showEdit = false)}>{$t('common.cancel')}</Button>
			<Button type="submit" loading={saving}>{$t('common.save')}</Button>
		</div>
	</form>
</Modal>

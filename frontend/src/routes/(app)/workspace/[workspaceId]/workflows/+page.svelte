<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import {
		workflowsApi,
		type WorkflowSummary,
		type WorkflowDetail,
		type WorkflowStateResponse
	} from '$lib/api/workflows';
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import Card from '$lib/components/Card.svelte';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');

	let workflows = $state<WorkflowSummary[]>([]);
	let loading = $state(true);
	let showCreateModal = $state(false);
	let showEditModal = $state(false);
	let showStatesModal = $state(false);
	let editingWorkflow = $state<WorkflowSummary | null>(null);
	let managingWorkflow = $state<WorkflowDetail | null>(null);
	let saving = $state(false);
	let createForm = $state({ name: '', description: '' });
	let editForm = $state({ name: '', description: '' });
	let showStateForm = $state(false);
	let editingState = $state<WorkflowStateResponse | null>(null);
	let stateForm = $state({
		key: '',
		display_name: '',
		category: 'active',
		color: '#94a3b8',
		is_initial: false,
		is_terminal: false
	});
	let savingState = $state(false);
	let draggingStateId = $state<string | null>(null);
	let dropTargetIdx = $state<number | null>(null);

	onMount(async () => {
		await loadWorkflows();
	});

	async function loadWorkflows() {
		loading = true;
		const response = await workflowsApi.listByWorkspace(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			workflows = response.data.items ?? [];
		}
		loading = false;
	}

	function openCreateModal() {
		createForm = { name: '', description: '' };
		showCreateModal = true;
	}

	async function handleCreate() {
		if (!createForm.name.trim()) {
			toast.error(get(t)('workflow.nameRequired'));
			return;
		}
		saving = true;
		const response = await workflowsApi.create(workspaceId, {
			name: createForm.name.trim(),
			description: createForm.description.trim() || undefined
		});
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('workflow.createSuccess'));
			showCreateModal = false;
			await loadWorkflows();
		}
		saving = false;
	}

	function openEditModal(wf: WorkflowSummary) {
		editingWorkflow = wf;
		editForm = { name: wf.name, description: wf.description };
		showEditModal = true;
	}

	async function handleEdit() {
		if (!editingWorkflow || !editForm.name.trim()) {
			toast.error(get(t)('workflow.nameRequired'));
			return;
		}
		saving = true;
		const response = await workflowsApi.update(editingWorkflow.id, {
			name: editForm.name.trim(),
			description: editForm.description.trim() || undefined
		});
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('workflow.updateSuccess'));
			showEditModal = false;
			await loadWorkflows();
		}
		saving = false;
	}

	async function handleDelete(wf: WorkflowSummary) {
		if (!confirm(get(t)('workflow.deleteConfirm', { values: { name: wf.name } }))) return;
		const response = await workflowsApi.delete(wf.id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('workflow.deleteSuccess'));
			await loadWorkflows();
		}
	}

	async function openStatesModal(wf: WorkflowSummary) {
		const response = await workflowsApi.get(wf.id);
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		managingWorkflow = response.data;
		showStateForm = false;
		editingState = null;
		stateForm = { key: '', display_name: '', category: 'active', color: '#94a3b8', is_initial: false, is_terminal: false };
		showStatesModal = true;
	}

	function openAddStateForm() {
		editingState = null;
		stateForm = { key: '', display_name: '', category: 'active', color: '#94a3b8', is_initial: false, is_terminal: false };
		showStateForm = true;
	}

	function openEditStateForm(state: WorkflowStateResponse) {
		editingState = state;
		stateForm = {
			key: state.key,
			display_name: state.display_name,
			category: state.category,
			color: state.color ?? '#94a3b8',
			is_initial: state.is_initial,
			is_terminal: state.is_terminal
		};
		showStateForm = true;
	}

	function cancelStateForm() {
		showStateForm = false;
		editingState = null;
	}

	async function handleSaveState() {
		if (!managingWorkflow) return;
		if (!stateForm.display_name.trim()) {
			toast.error(get(t)('workflow.displayNameRequired'));
			return;
		}
		savingState = true;

		if (editingState) {
			const response = await workflowsApi.updateState(editingState.id, {
				display_name: stateForm.display_name.trim(),
				category: stateForm.category,
				color: stateForm.color,
				is_initial: stateForm.is_initial,
				is_terminal: stateForm.is_terminal
			});
			if (response.code !== 0) {
				toast.error(response.message);
			} else {
				toast.success(get(t)('workflow.stateUpdateSuccess'));
				showStateForm = false;
				editingState = null;
				await refreshManagedWorkflow();
			}
		} else {
			if (!stateForm.key.trim()) {
				toast.error(get(t)('workflow.stateKeyRequired'));
				savingState = false;
				return;
			}
			const response = await workflowsApi.createState(managingWorkflow.id, {
				key: stateForm.key.trim(),
				display_name: stateForm.display_name.trim(),
				category: stateForm.category,
				color: stateForm.color,
				is_initial: stateForm.is_initial,
				is_terminal: stateForm.is_terminal
			});
			if (response.code !== 0) {
				toast.error(response.message);
			} else {
				toast.success(get(t)('workflow.stateAddSuccess'));
				showStateForm = false;
				await refreshManagedWorkflow();
			}
		}

		savingState = false;
	}

	async function handleDeleteState(state: WorkflowStateResponse) {
		if (!confirm(get(t)('workflow.stateDeleteConfirm', { values: { name: state.display_name } }))) return;
		const response = await workflowsApi.deleteState(state.id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('workflow.stateDeleteSuccess'));
			await refreshManagedWorkflow();
		}
	}

	async function refreshManagedWorkflow() {
		if (!managingWorkflow) return;
		const response = await workflowsApi.get(managingWorkflow.id);
		if (response.code === 0 && response.data) {
			managingWorkflow = response.data;
		}
	}

	function handleDragStart(stateId: string) {
		draggingStateId = stateId;
	}

	function handleDragEnd() {
		draggingStateId = null;
		dropTargetIdx = null;
	}

	function handleDragOver(e: DragEvent) {
		e.preventDefault();
	}

	function handleDragEnter(idx: number) {
		dropTargetIdx = idx;
	}

	function handleDrop(e: DragEvent, targetIdx: number) {
		e.preventDefault();
		if (!draggingStateId || !managingWorkflow) return;
		const states = [...managingWorkflow.states];
		const fromIdx = states.findIndex((s) => s.id === draggingStateId);
		if (fromIdx === targetIdx) {
			draggingStateId = null;
			dropTargetIdx = null;
			return;
		}
		const [moved] = states.splice(fromIdx, 1);
		states.splice(targetIdx, 0, moved);
		managingWorkflow = { ...managingWorkflow, states };
		draggingStateId = null;
		dropTargetIdx = null;
		workflowsApi.reorderStates(managingWorkflow.id, states.map((s) => s.id));
	}
</script>

<div class="mx-auto max-w-5xl space-y-6 p-6">
	<div class="flex items-center justify-between">
		<div>
			<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('workflow.title')}</h1>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
				{$t('workflow.subtitle')}
			</p>
		</div>
		<Button onclick={openCreateModal}>{$t('workflow.newWorkflow')}</Button>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-slate-500 dark:text-slate-400">
			{$t('workflow.loading')}
		</div>
	{:else if workflows.length === 0}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-10 text-center text-slate-500 dark:text-slate-400">
			{$t('workflow.noWorkflows')}
		</div>
	{:else}
		<div class="space-y-4">
			{#each workflows as wf (wf.id)}
				<Card>
					<div class="flex items-start justify-between gap-4">
						<div class="min-w-0 flex-1">
							<div class="flex items-center gap-2 flex-wrap">
								<span class="text-lg font-semibold text-slate-900 dark:text-slate-100">{wf.name}</span>
								{#if wf.is_system_default}
									<span class="inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200">
										{$t('workflow.systemDefault')}
									</span>
								{:else}
									<span class="inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300">
										{$t('workflow.custom')}
									</span>
								{/if}
							</div>
							{#if wf.description}
								<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{wf.description}</p>
							{/if}
							<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">{$t('workflow.stateCount', { values: { count: wf.state_count } })}</p>
						</div>
						<div class="flex items-center gap-2 shrink-0">
							<Button variant="secondary" size="sm" onclick={() => openStatesModal(wf)}>
								{$t('workflow.manageStates')}
							</Button>
							<Button
								variant="secondary"
								size="sm"
								disabled={wf.is_system_default}
								onclick={() => openEditModal(wf)}
							>
								{$t('common.edit')}
							</Button>
							<Button
								variant="danger"
								size="sm"
								disabled={wf.is_system_default}
								onclick={() => handleDelete(wf)}
							>
								{$t('common.delete')}
							</Button>
						</div>
					</div>
				</Card>
			{/each}
		</div>
	{/if}
</div>

<!-- Create Workflow Modal -->
<Modal bind:open={showCreateModal} title={$t('workflow.newWorkflow')}>
	<form onsubmit={(e) => { e.preventDefault(); handleCreate(); }} class="space-y-4">
		<Input label={$t('workflow.name')} placeholder={$t('workflow.namePlaceholder')} bind:value={createForm.name} required />
		<div class="space-y-1">
			<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="create-wf-description">
				{$t('workflow.description')}
			</label>
			<textarea
				id="create-wf-description"
				class="block w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 resize-y"
				placeholder={$t('workflow.descriptionPlaceholder')}
				bind:value={createForm.description}
				rows={3}
			></textarea>
		</div>
	</form>
	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showCreateModal = false)}>{$t('common.cancel')}</Button>
		<Button loading={saving} onclick={handleCreate}>{$t('common.create')}</Button>
	{/snippet}
</Modal>

<!-- Edit Workflow Modal -->
<Modal bind:open={showEditModal} title={$t('workflow.editWorkflow')}>
	<form onsubmit={(e) => { e.preventDefault(); handleEdit(); }} class="space-y-4">
		<Input label={$t('workflow.name')} placeholder={$t('workflow.namePlaceholder')} bind:value={editForm.name} required />
		<div class="space-y-1">
			<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="edit-wf-description">
				{$t('workflow.description')}
			</label>
			<textarea
				id="edit-wf-description"
				class="block w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 dark:placeholder-slate-500 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 resize-y"
				placeholder={$t('workflow.descriptionPlaceholder')}
				bind:value={editForm.description}
				rows={3}
			></textarea>
		</div>
	</form>
	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showEditModal = false)}>{$t('common.cancel')}</Button>
		<Button loading={saving} onclick={handleEdit}>{$t('common.save')}</Button>
	{/snippet}
</Modal>

<!-- Manage States Modal -->
<Modal bind:open={showStatesModal} title={managingWorkflow ? $t('workflow.manageStatesTitle', { values: { name: managingWorkflow.name } }) : $t('workflow.manageStates')} maxWidthClass="max-w-3xl">
	{#if managingWorkflow}
		<div class="space-y-4">
			{#if managingWorkflow.states.length === 0}
				<p class="text-sm text-slate-500 dark:text-slate-400">{$t('workflow.noStates')}</p>
			{:else}
				<div class="overflow-x-auto rounded-md border border-slate-200 dark:border-slate-700">
					<table class="w-full text-left text-sm">
						<thead>
							<tr class="bg-slate-50 dark:bg-slate-950">
								<th class="px-3 py-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400 w-10"></th>
								<th class="px-3 py-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400 w-8">{$t('workflow.color')}</th>
								<th class="px-3 py-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">{$t('workflow.stateKey')}</th>
								<th class="px-3 py-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">{$t('workflow.displayName')}</th>
								<th class="px-3 py-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">{$t('workflow.category')}</th>
								<th class="px-3 py-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">{$t('workflow.flags')}</th>
								<th class="px-3 py-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400 text-right">{$t('workflow.actions')}</th>
							</tr>
						</thead>
						<tbody>
							{#each managingWorkflow.states as state, idx (state.id)}
								<tr
									class={`border-b border-slate-200 dark:border-slate-700 transition-colors ${draggingStateId === state.id ? 'opacity-40' : ''} ${dropTargetIdx === idx ? 'bg-blue-50 dark:bg-blue-900/20' : 'bg-white dark:bg-slate-900'}`}
									draggable={true}
									ondragstart={() => handleDragStart(state.id)}
									ondragend={handleDragEnd}
									ondragover={handleDragOver}
									ondragenter={() => handleDragEnter(idx)}
									ondrop={(e) => handleDrop(e, idx)}
								>
									<td class="px-3 py-2 text-slate-400 cursor-grab select-none text-center" title={$t('workflow.dragToReorder')}>
										<span class="text-lg leading-none">⠿</span>
									</td>
									<td class="px-3 py-2">
										<span
											class="inline-block h-3 w-3 rounded-full"
											style={`background-color: ${state.color ?? '#94a3b8'}`}
										></span>
									</td>
									<td class="px-3 py-2 font-mono text-slate-500 dark:text-slate-400">{state.key}</td>
									<td class="px-3 py-2 text-slate-900 dark:text-slate-100">{state.display_name}</td>
									<td class="px-3 py-2">
										{#if state.category === 'done'}
											<span class="inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium bg-green-100 text-green-700 dark:bg-green-900/50 dark:text-green-300">
												{$t('workflow.categoryDone')}
											</span>
										{:else}
											<span class="inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium bg-blue-100 text-blue-700 dark:bg-blue-900/50 dark:text-blue-300">
												{$t('workflow.categoryActive')}
											</span>
										{/if}
									</td>
									<td class="px-3 py-2">
										<div class="flex gap-1 flex-wrap">
											{#if state.is_initial}
												<span class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium bg-amber-100 text-amber-700 dark:bg-amber-900/50 dark:text-amber-300">
													{$t('workflow.initial')}
												</span>
											{/if}
											{#if state.is_terminal}
												<span class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium bg-purple-100 text-purple-700 dark:bg-purple-900/50 dark:text-purple-300">
													{$t('workflow.terminal')}
												</span>
											{/if}
										</div>
									</td>
									<td class="px-3 py-2 text-right">
										<div class="flex items-center justify-end gap-1">
											<Button variant="ghost" size="sm" onclick={() => openEditStateForm(state)}>{$t('common.edit')}</Button>
											<Button variant="danger" size="sm" onclick={() => handleDeleteState(state)}>{$t('common.delete')}</Button>
										</div>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}

			{#if !showStateForm}
				<div>
					<Button variant="secondary" size="sm" onclick={openAddStateForm}>+ {$t('workflow.addState')}</Button>
				</div>
			{:else}
				<div class="rounded-md border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 p-4 space-y-3">
					<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
						{editingState ? $t('workflow.editState') : $t('workflow.addState')}
					</h3>
					<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
						{#if !editingState}
							<div class="space-y-1">
								<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="state-key">
									{$t('workflow.stateKey')} <span class="text-red-500">*</span>
								</label>
								<input
									id="state-key"
									type="text"
									placeholder={$t('workflow.stateKeyPlaceholder')}
									bind:value={stateForm.key}
									class="block w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
							</div>
						{/if}
						<div class="space-y-1">
							<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="state-display-name">
								{$t('workflow.displayName')} <span class="text-red-500">*</span>
							</label>
							<input
								id="state-display-name"
								type="text"
								placeholder={$t('workflow.displayNamePlaceholder')}
								bind:value={stateForm.display_name}
								class="block w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder:text-slate-400 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
						</div>
						<div class="space-y-1">
							<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="state-category">
								{$t('workflow.category')}
							</label>
							<select
								id="state-category"
								bind:value={stateForm.category}
								class="block w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								<option value="active">{$t('workflow.categoryActive')}</option>
								<option value="done">{$t('workflow.categoryDone')}</option>
							</select>
						</div>
						<div class="space-y-1">
							<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="state-color">
								{$t('workflow.color')}
							</label>
							<input
								id="state-color"
								type="color"
								bind:value={stateForm.color}
								class="block h-9 w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-1 py-1 cursor-pointer focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
						</div>
					</div>
					<div class="flex gap-4">
						<label class="inline-flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300 cursor-pointer">
							<input
								type="checkbox"
								bind:checked={stateForm.is_initial}
								class="h-4 w-4 rounded border-slate-300 text-blue-600 focus:ring-blue-500"
							/>
							{$t('workflow.isInitial')}
						</label>
						<label class="inline-flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300 cursor-pointer">
							<input
								type="checkbox"
								bind:checked={stateForm.is_terminal}
								class="h-4 w-4 rounded border-slate-300 text-blue-600 focus:ring-blue-500"
							/>
							{$t('workflow.isTerminal')}
						</label>
					</div>
					<div class="flex gap-2">
						<Button variant="secondary" size="sm" onclick={cancelStateForm}>{$t('common.cancel')}</Button>
						<Button size="sm" loading={savingState} onclick={handleSaveState}>{$t('common.save')}</Button>
					</div>
				</div>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showStatesModal = false)}>{$t('common.close')}</Button>
	{/snippet}
</Modal>

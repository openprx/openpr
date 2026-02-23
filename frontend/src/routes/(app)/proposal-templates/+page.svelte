<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import {
		proposalTemplatesApi,
		type ProposalTemplate,
		type CreateProposalTemplateInput,
		type UpdateProposalTemplateInput
	} from '$lib/api/proposal-templates';
	import { toast } from '$lib/stores/toast';
	import { projectOptionsStore } from '$lib/stores/project-options';
	import { groupProjectOptionsByWorkspace } from '$lib/utils/project-options';

	const projects = $derived($projectOptionsStore.items);
	const groupedProjects = $derived.by(() => groupProjectOptionsByWorkspace(projects));
	let selectedProjectId = $state('');
	let templates = $state<ProposalTemplate[]>([]);
	let loading = $state(true);
	let saving = $state(false);

	let editingId = $state('');
	let formName = $state('');
	let formDescription = $state('');
	let formTemplateType = $state('governance');
	let formContentText = $state('{\n  "title": "",\n  "proposal_type": "governance",\n  "content": "",\n  "domains": ["governance"]\n}');
	let formIsDefault = $state(false);
	let formIsActive = $state(true);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		await projectOptionsStore.ensureLoaded();
		if (projects.length > 0) {
			selectedProjectId = projects[0].id;
			await loadTemplates();
		}
		loading = false;
	}

	async function loadTemplates() {
		if (!selectedProjectId) {
			templates = [];
			return;
		}
		const res = await proposalTemplatesApi.list({ project_id: selectedProjectId });
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('proposalTemplates.loadFailed'));
			templates = [];
			return;
		}
		templates = res.data.items ?? [];
	}

	function resetForm() {
		editingId = '';
		formName = '';
		formDescription = '';
		formTemplateType = 'governance';
		formContentText =
			'{\n  "title": "",\n  "proposal_type": "governance",\n  "content": "",\n  "domains": ["governance"]\n}';
		formIsDefault = false;
		formIsActive = true;
	}

	function startCreate() {
		resetForm();
	}

	function startEdit(item: ProposalTemplate) {
		editingId = item.id;
		formName = item.name;
		formDescription = item.description || '';
		formTemplateType = item.template_type || 'governance';
		formContentText = JSON.stringify(item.content ?? {}, null, 2);
		formIsDefault = item.is_default;
		formIsActive = item.is_active;
	}

	function parseContent(): Record<string, unknown> | null {
		try {
			const parsed = JSON.parse(formContentText);
			if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
				toast.error(get(t)('proposalTemplates.contentInvalid'));
				return null;
			}
			return parsed as Record<string, unknown>;
		} catch {
			toast.error(get(t)('proposalTemplates.contentInvalid'));
			return null;
		}
	}

	async function submitForm() {
		if (!selectedProjectId) {
			toast.error(get(t)('proposalTemplates.projectRequired'));
			return;
		}
		if (formName.trim().length < 2) {
			toast.error(get(t)('proposalTemplates.nameRequired'));
			return;
		}
		const parsedContent = parseContent();
		if (!parsedContent) {
			return;
		}

		saving = true;
		if (editingId) {
			const payload: UpdateProposalTemplateInput = {
				name: formName.trim(),
				description: formDescription.trim() || undefined,
				template_type: formTemplateType.trim(),
				content: parsedContent,
				is_default: formIsDefault,
				is_active: formIsActive
			};
			const res = await proposalTemplatesApi.update(editingId, payload);
			saving = false;
			if (res.code !== 0) {
				toast.error(res.message || get(t)('proposalTemplates.updateFailed'));
				return;
			}
			toast.success(get(t)('proposalTemplates.updateSuccess'));
		} else {
			const payload: CreateProposalTemplateInput = {
				project_id: selectedProjectId,
				name: formName.trim(),
				description: formDescription.trim() || undefined,
				template_type: formTemplateType.trim(),
				content: parsedContent,
				is_default: formIsDefault,
				is_active: formIsActive
			};
			const res = await proposalTemplatesApi.create(payload);
			saving = false;
			if (res.code !== 0) {
				toast.error(res.message || get(t)('proposalTemplates.createFailed'));
				return;
			}
			toast.success(get(t)('proposalTemplates.createSuccess'));
		}

		resetForm();
		await loadTemplates();
	}

	async function removeTemplate(item: ProposalTemplate) {
		if (!window.confirm(get(t)('proposalTemplates.deleteConfirm', { values: { name: item.name } }))) {
			return;
		}
		const res = await proposalTemplatesApi.delete(item.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('proposalTemplates.deleteFailed'));
			return;
		}
		toast.success(get(t)('proposalTemplates.deleteSuccess'));
		await loadTemplates();
	}

	async function setDefault(item: ProposalTemplate) {
		const res = await proposalTemplatesApi.update(item.id, { is_default: true });
		if (res.code !== 0) {
			toast.error(res.message || get(t)('proposalTemplates.updateFailed'));
			return;
		}
		toast.success(get(t)('proposalTemplates.setDefaultSuccess'));
		await loadTemplates();
	}

	async function onProjectChange() {
		resetForm();
		await loadTemplates();
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.proposalTemplates')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('proposalTemplates.title')}</h1>
		<p class="mt-1 text-sm text-slate-500">{$t('proposalTemplates.subtitle')}</p>
	</div>

	<div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
		<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800 lg:col-span-2">
			<div class="mb-3 flex items-center justify-between gap-3">
				<div class="flex-1">
					<label for="proposal-template-project" class="mb-1 block text-sm text-slate-600">{$t('impactReview.project')}</label>
					<select id="proposal-template-project" bind:value={selectedProjectId} onchange={onProjectChange} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
						{#each groupedProjects as group}
							<optgroup label={group.workspaceName}>
								{#each group.items as project}
									<option value={project.id}>{project.name}</option>
								{/each}
							</optgroup>
						{/each}
					</select>
				</div>
				<button type="button" onclick={startCreate} class="rounded-md border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900">
					{$t('proposalTemplates.new')}
				</button>
			</div>

			{#if loading}
				<p class="text-sm text-slate-500">{$t('common.loading')}</p>
			{:else if templates.length === 0}
				<p class="text-sm text-slate-500">{$t('proposalTemplates.empty')}</p>
			{:else}
				<div class="space-y-2">
					{#each templates as item}
						<div class="rounded-md border border-slate-200 p-3 dark:border-slate-600">
							<div class="mb-2 flex flex-wrap items-center gap-2">
								<p class="text-sm font-medium text-slate-900 dark:text-slate-100">{item.name}</p>
								<span class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600">{item.template_type}</span>
								{#if item.is_default}
									<span class="rounded bg-emerald-100 px-2 py-0.5 text-xs text-emerald-700">{$t('proposalTemplates.default')}</span>
								{/if}
								{#if !item.is_active}
									<span class="rounded bg-amber-100 px-2 py-0.5 text-xs text-amber-700">{$t('proposalTemplates.inactive')}</span>
								{/if}
							</div>
							{#if item.description}
								<p class="mb-2 text-xs text-slate-500">{item.description}</p>
							{/if}
							<div class="flex flex-wrap gap-2">
								<button type="button" onclick={() => startEdit(item)} class="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900">
									{$t('common.edit')}
								</button>
								{#if !item.is_default}
									<button type="button" onclick={() => setDefault(item)} class="rounded border border-emerald-300 px-2 py-1 text-xs text-emerald-700 hover:bg-emerald-50">
										{$t('proposalTemplates.setDefault')}
									</button>
								{/if}
								<button type="button" onclick={() => removeTemplate(item)} class="rounded border border-red-300 px-2 py-1 text-xs text-red-700 hover:bg-red-50">
									{$t('common.delete')}
								</button>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</div>

		<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
			<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">
				{editingId ? $t('proposalTemplates.editTitle') : $t('proposalTemplates.createTitle')}
			</h2>
			<div class="space-y-3">
				<div>
					<label for="proposal-template-name" class="mb-1 block text-sm text-slate-600">{$t('proposalTemplates.name')}</label>
					<input id="proposal-template-name" bind:value={formName} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
				</div>
				<div>
					<label for="proposal-template-description" class="mb-1 block text-sm text-slate-600">{$t('proposalTemplates.description')}</label>
					<input id="proposal-template-description" bind:value={formDescription} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
				</div>
				<div>
					<label for="proposal-template-type" class="mb-1 block text-sm text-slate-600">{$t('proposalTemplates.templateType')}</label>
					<input id="proposal-template-type" bind:value={formTemplateType} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
				</div>
				<div>
					<label for="proposal-template-content" class="mb-1 block text-sm text-slate-600">{$t('proposalTemplates.content')}</label>
					<textarea id="proposal-template-content" bind:value={formContentText} rows="12" class="w-full rounded-md border border-slate-300 px-3 py-2 font-mono text-xs dark:border-slate-600 dark:bg-slate-900"></textarea>
				</div>
				<div class="grid grid-cols-2 gap-3">
					<label class="flex items-center gap-2 text-xs text-slate-600">
						<input type="checkbox" bind:checked={formIsDefault} />
						{$t('proposalTemplates.default')}
					</label>
					<label class="flex items-center gap-2 text-xs text-slate-600">
						<input type="checkbox" bind:checked={formIsActive} />
						{$t('proposalTemplates.active')}
					</label>
				</div>
				<div class="flex gap-2">
					<button type="button" onclick={submitForm} class="flex-1 rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700" disabled={saving}>
						{saving ? $t('common.saving') : $t('common.save')}
					</button>
					<button type="button" onclick={resetForm} class="rounded-md border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900">
						{$t('common.cancel')}
					</button>
				</div>
			</div>
		</div>
	</div>
</div>

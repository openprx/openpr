<script lang="ts">
	import { goto } from '$app/navigation';
	import { tick } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { apiClient } from '$lib/api/client';
	import { proposalTemplatesApi, type ProposalTemplate } from '$lib/api/proposal-templates';
	import {
		proposalsApi,
		type ProposalType,
		type VotingRule,
		type CycleTemplate
	} from '$lib/api/proposals';
	import {
		groupProjectOptionsByWorkspace,
		loadProjectOptions,
		type ProjectOption
	} from '$lib/utils/project-options';
	import { toast } from '$lib/stores/toast';
	import { renderMarkdown } from '$lib/utils/markdown';
	import {
		isAllowedUploadMime,
		MAX_UPLOAD_SIZE_BYTES,
		mediaMarkdown,
		UPLOAD_ACCEPT_ATTR
	} from '$lib/utils/upload';
	import { onMount } from 'svelte';

	let title = $state('');
	let proposalType = $state<ProposalType>('feature');
	let domains = $state('');
	let content = $state('');
	let contentMode = $state<'edit' | 'preview'>('edit');
	let votingRule = $state<VotingRule>('simple_majority');
	let cycleTemplate = $state<CycleTemplate>('fast');
	let submitting = $state(false);
	let loadingTemplates = $state(false);
	let lastAutoContent = $state('');
	let projectOptions = $state<ProjectOption[]>([]);
	let selectedProjectId = $state('');
	let templateOptions = $state<ProposalTemplate[]>([]);
	let selectedTemplateId = $state('');
	let contentTextareaEl = $state<HTMLTextAreaElement | null>(null);
	let uploadingContentImage = $state(false);
	const groupedProjectOptions = $derived.by(() => groupProjectOptionsByWorkspace(projectOptions));

	const recommendedCycleByType: Record<ProposalType, CycleTemplate> = {
		feature: 'rapid',
		priority: 'rapid',
		bugfix: 'rapid',
		architecture: 'standard',
		resource: 'standard',
		governance: 'critical'
	};

	const proposalTypeOptions: Array<{ value: ProposalType; labelKey: string }> = [
		{ value: 'feature', labelKey: 'governance.type.feature' },
		{ value: 'architecture', labelKey: 'governance.type.architecture' },
		{ value: 'priority', labelKey: 'governance.type.priority' },
		{ value: 'resource', labelKey: 'governance.type.resource' },
		{ value: 'governance', labelKey: 'governance.type.governance' },
		{ value: 'bugfix', labelKey: 'governance.type.bugfix' }
	];

	const votingRuleOptions: Array<{ value: VotingRule; labelKey: string }> = [
		{ value: 'simple_majority', labelKey: 'governance.votingRule.simple_majority' },
		{ value: 'absolute_majority', labelKey: 'governance.votingRule.absolute_majority' },
		{ value: 'consensus', labelKey: 'governance.votingRule.consensus' }
	];

	const cycleTemplateOptions: Array<{ value: CycleTemplate; labelKey: string }> = [
		{ value: 'rapid', labelKey: 'governance.cycleTemplate.rapid' },
		{ value: 'fast', labelKey: 'governance.cycleTemplate.fast' },
		{ value: 'standard', labelKey: 'governance.cycleTemplate.standard' },
		{ value: 'critical', labelKey: 'governance.cycleTemplate.critical' }
	];

	function getProposalTemplate(type: ProposalType): string {
		return get(t)(`governance.template.${type}`);
	}

	function applyTypePreset(type: ProposalType): void {
		cycleTemplate = recommendedCycleByType[type];
		const nextTemplate = getProposalTemplate(type);
		if (!content.trim() || content === lastAutoContent) {
			content = nextTemplate;
		}
		lastAutoContent = nextTemplate;
	}

	function handleTypeChange(): void {
		applyTypePreset(proposalType);
	}

	function appendContentImage(url: string, mimeType: string): void {
		const markdown = mediaMarkdown(url, mimeType);
		if (!contentTextareaEl) {
			const suffix = content.trim().length > 0 ? '\n\n' : '';
			content = `${content}${suffix}${markdown}`;
			return;
		}

		const current = content;
		const start = contentTextareaEl.selectionStart ?? current.length;
		const end = contentTextareaEl.selectionEnd ?? start;
		content = `${current.slice(0, start)}${markdown}${current.slice(end)}`;

		void tick().then(() => {
			if (!contentTextareaEl) {
				return;
			}
			const cursor = start + markdown.length;
			contentTextareaEl.selectionStart = cursor;
			contentTextareaEl.selectionEnd = cursor;
			contentTextareaEl.focus();
		});
	}

	async function uploadContentImage(file: File): Promise<void> {
		if (!isAllowedUploadMime(file.type)) {
			toast.error(get(t)('toast.uploadTypeFail'));
			return;
		}
		if (file.size > MAX_UPLOAD_SIZE_BYTES) {
			toast.error(get(t)('toast.uploadSizeFail'));
			return;
		}

		uploadingContentImage = true;
		const formData = new FormData();
		formData.append('file', file);
		const headers = new Headers();
		const token = apiClient.getToken();
		if (token) {
			headers.set('Authorization', `Bearer ${token}`);
		}

		try {
			const response = await fetch('/api/v1/upload', {
				method: 'POST',
				headers,
				body: formData
			});
			const result = (await response.json()) as {
				code?: number;
				message?: string;
				data?: { url?: string };
			};
			if (result.code === 0 && result.data?.url) {
				appendContentImage(result.data.url, file.type);
				toast.success(get(t)('toast.uploadSuccess'));
			} else {
				toast.error(result.message || get(t)('toast.uploadFail'));
			}
		} catch {
			toast.error(get(t)('toast.uploadNetworkFail'));
		} finally {
			uploadingContentImage = false;
		}
	}

	function handleContentPaste(event: ClipboardEvent): void {
		const items = event.clipboardData?.items;
		if (!items) {
			return;
		}
		for (const item of items) {
			if (!isAllowedUploadMime(item.type)) {
				continue;
			}
			const file = item.getAsFile();
			if (!file) {
				return;
			}
			event.preventDefault();
			void uploadContentImage(file);
			return;
		}
	}

	async function handleContentImageSelect(event: Event): Promise<void> {
		const input = event.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) {
			return;
		}
		await uploadContentImage(file);
		input.value = '';
	}

	$effect(() => {
		if (!lastAutoContent) {
			applyTypePreset(proposalType);
		}
	});

	onMount(() => {
		void initTemplates();
	});

	async function initTemplates() {
		projectOptions = await loadProjectOptions(true);
		if (projectOptions.length === 0) {
			return;
		}
		selectedProjectId = projectOptions[0].id;
		await loadTemplatesForProject();
	}

	async function loadTemplatesForProject() {
		if (!selectedProjectId) {
			templateOptions = [];
			selectedTemplateId = '';
			return;
		}
		loadingTemplates = true;
		const res = await proposalTemplatesApi.list({ project_id: selectedProjectId, is_active: true });
		loadingTemplates = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('proposalTemplates.loadFailed'));
			templateOptions = [];
			selectedTemplateId = '';
			return;
		}
		templateOptions = res.data.items ?? [];
		const defaultTemplate = templateOptions.find((item) => item.is_default);
		if (defaultTemplate) {
			selectedTemplateId = defaultTemplate.id;
			applyTemplate(defaultTemplate);
		}
	}

	function onProjectChange() {
		void loadTemplatesForProject();
	}

	function onTemplateChange() {
		if (!selectedTemplateId) {
			return;
		}
		const selected = templateOptions.find((item) => item.id === selectedTemplateId);
		if (!selected) {
			return;
		}
		applyTemplate(selected);
	}

	function getTemplateString(contentObject: Record<string, unknown>, key: string): string {
		const value = contentObject[key];
		return typeof value === 'string' ? value : '';
	}

	function getTemplateDomains(contentObject: Record<string, unknown>): string[] {
		const raw = contentObject.domains;
		if (!Array.isArray(raw)) {
			return [];
		}
		return raw.map((item) => String(item)).filter(Boolean);
	}

	function applyTemplate(template: ProposalTemplate) {
		const data = template.content ?? {};
		const templateTitle = getTemplateString(data, 'title');
		const templateType = getTemplateString(data, 'proposal_type');
		const templateContent = getTemplateString(data, 'content');
		const templateVotingRule = getTemplateString(data, 'voting_rule');
		const templateCycleTemplate = getTemplateString(data, 'cycle_template');
		const templateDomains = getTemplateDomains(data);

		if (templateTitle) {
			title = templateTitle;
		}
		if (
			templateType &&
			['feature', 'architecture', 'priority', 'resource', 'governance', 'bugfix'].includes(templateType)
		) {
			proposalType = templateType as ProposalType;
		}
		if (templateContent) {
			content = templateContent;
			lastAutoContent = templateContent;
		}
		if (
			templateVotingRule &&
			['simple_majority', 'absolute_majority', 'consensus'].includes(templateVotingRule)
		) {
			votingRule = templateVotingRule as VotingRule;
		}
		if (templateCycleTemplate && ['rapid', 'fast', 'standard', 'critical'].includes(templateCycleTemplate)) {
			cycleTemplate = templateCycleTemplate as CycleTemplate;
		}
		if (templateDomains.length > 0) {
			domains = templateDomains.join(', ');
		}
	}

	async function submit() {
		if (title.trim().length < 10 || title.trim().length > 200) {
			toast.error(get(t)('governance.validation.titleLength'));
			return;
		}
		if (content.trim().length < 50) {
			toast.error(get(t)('governance.validation.contentLength'));
			return;
		}

		const domainList = domains
			.split(',')
			.map((item) => item.trim())
			.filter(Boolean);
		if (domainList.length === 0) {
			toast.error(get(t)('governance.validation.domainsRequired'));
			return;
		}

		submitting = true;
		const res = await proposalsApi.create({
			title: title.trim(),
			proposal_type: proposalType,
			content: content.trim(),
			domains: domainList,
			voting_rule: votingRule,
			cycle_template: cycleTemplate,
			template_id: selectedTemplateId || undefined
		});
		submitting = false;

		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governance.createFailed'));
			return;
		}

		const createdId = String((res.data as Record<string, unknown>).id ?? '');
		toast.success(get(t)('governance.createSuccess'));
		if (createdId) {
			goto(`/proposals/${createdId}`);
		} else {
			goto('/proposals');
		}
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.proposalNew')}</title>
</svelte:head>

<div class="mx-auto max-w-3xl">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="mb-4 text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governance.createTitle')}</h1>
		<div class="space-y-4">
			<div>
				<label for="proposal-title" class="mb-1 block text-sm text-slate-600">{$t('governance.field.title')}</label>
				<input
					id="proposal-title"
					type="text"
					bind:value={title}
					placeholder={$t('governance.titlePlaceholder')}
					class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"
				/>
			</div>

			<div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
				<div>
					<label for="proposal-template-project" class="mb-1 block text-sm text-slate-600">{$t('proposalTemplates.project')}</label>
					<select
						id="proposal-template-project"
						bind:value={selectedProjectId}
						onchange={onProjectChange}
						class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"
					>
						<option value="">{`- ${$t('proposalTemplates.project')} -`}</option>
						{#each groupedProjectOptions as group}
							<optgroup label={group.workspaceName}>
								{#each group.items as project}
									<option value={project.id}>{project.name}</option>
								{/each}
							</optgroup>
						{/each}
					</select>
				</div>
				<div>
					<label for="proposal-type" class="mb-1 block text-sm text-slate-600">{$t('governance.field.type')}</label>
					<select id="proposal-type" bind:value={proposalType} onchange={handleTypeChange} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
						{#each proposalTypeOptions as option}
							<option value={option.value}>{$t(option.labelKey)}</option>
						{/each}
					</select>
				</div>
				<div>
					<label for="proposal-voting-rule" class="mb-1 block text-sm text-slate-600">{$t('governance.field.votingRule')}</label>
					<select id="proposal-voting-rule" bind:value={votingRule} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
						{#each votingRuleOptions as option}
							<option value={option.value}>{$t(option.labelKey)}</option>
						{/each}
					</select>
					<p class="mt-1 text-xs text-slate-500">{$t(`governance.votingRuleDescription.${votingRule}`)}</p>
				</div>
				<div>
					<label for="proposal-cycle-template" class="mb-1 block text-sm text-slate-600">{$t('governance.field.cycleTemplate')}</label>
					<select id="proposal-cycle-template" bind:value={cycleTemplate} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
						{#each cycleTemplateOptions as option}
							<option value={option.value}>{$t(option.labelKey)}</option>
						{/each}
					</select>
					<p class="mt-1 text-xs text-slate-500">
						{$t('governance.recommendedCycle', { values: { cycle: $t(`governance.cycleTemplate.${recommendedCycleByType[proposalType]}`) } })}
					</p>
					<p class="mt-1 text-xs text-slate-500">{$t(`governance.cycleDescription.${cycleTemplate}`)}</p>
				</div>
			</div>

			<div>
				<label for="proposal-template-id" class="mb-1 block text-sm text-slate-600">{$t('proposalTemplates.applyTemplate')}</label>
				<select id="proposal-template-id" bind:value={selectedTemplateId} onchange={onTemplateChange} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" disabled={loadingTemplates || templateOptions.length === 0}>
					<option value="">{`- ${$t('proposalTemplates.none')} -`}</option>
					{#each templateOptions as template}
						<option value={template.id}>{template.name}{template.is_default ? ` (${ $t('proposalTemplates.default') })` : ''}</option>
					{/each}
				</select>
			</div>

			<div>
				<label for="proposal-domains" class="mb-1 block text-sm text-slate-600">{$t('governance.field.domains')}</label>
				<input
					id="proposal-domains"
					type="text"
					bind:value={domains}
					placeholder={$t('governance.domainsPlaceholder')}
					class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"
				/>
			</div>

			<div>
				<label for="proposal-content" class="mb-1 block text-sm text-slate-600">{$t('governance.field.content')}</label>
				<div class="mb-2 inline-flex overflow-hidden rounded-md border border-slate-300 text-xs dark:border-slate-600">
					<button type="button" class={`px-2 py-1 ${contentMode === 'edit' ? 'bg-blue-600 text-white' : 'bg-white text-slate-600 dark:bg-slate-900 dark:text-slate-300'}`} onclick={() => (contentMode = 'edit')}>
						{$t('issue.editMode')}
					</button>
					<button type="button" class={`px-2 py-1 ${contentMode === 'preview' ? 'bg-blue-600 text-white' : 'bg-white text-slate-600 dark:bg-slate-900 dark:text-slate-300'}`} onclick={() => (contentMode = 'preview')}>
						{$t('issue.previewMode')}
					</button>
				</div>
				{#if contentMode === 'edit'}
					<textarea
						id="proposal-content"
						bind:value={content}
						bind:this={contentTextareaEl}
						rows="12"
						placeholder={$t('governance.contentPlaceholder')}
						class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"
						onpaste={handleContentPaste}
					></textarea>
					<div class="mt-2">
						<label class="inline-flex cursor-pointer items-center gap-1 text-sm text-blue-600 hover:text-blue-800 dark:text-blue-400 dark:hover:text-blue-300">
							<span>{uploadingContentImage ? $t('search.searching') : $t('issue.uploadImage')}</span>
							<input type="file" accept={UPLOAD_ACCEPT_ATTR} class="hidden" onchange={handleContentImageSelect} disabled={uploadingContentImage} />
						</label>
					</div>
				{:else}
					<div class="markdown-body min-h-[280px] rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
						{#if content.trim()}
							{@html renderMarkdown(content)}
						{:else}
							<p class="text-slate-400">{$t('issue.noDescriptionPreview')}</p>
						{/if}
					</div>
				{/if}
			</div>

			<div class="flex items-center justify-end gap-2">
				<a href="/proposals" class="rounded-md border border-slate-300 px-4 py-2 text-sm">{$t('common.cancel')}</a>
				<button
					type="button"
					onclick={submit}
					disabled={submitting}
					class="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-60"
				>
					{submitting ? $t('common.processing') : $t('governance.saveDraft')}
				</button>
			</div>
		</div>
	</div>
</div>

<style>
	:global(.markdown-body img) {
		max-width: 100%;
		height: auto;
		border-radius: 0.5rem;
	}
</style>

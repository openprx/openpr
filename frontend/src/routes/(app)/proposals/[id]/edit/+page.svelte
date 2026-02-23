<script lang="ts">
	import { onMount } from 'svelte';
	import { tick } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { apiClient } from '$lib/api/client';
	import { authStore } from '$lib/stores/auth';
	import {
		proposalsApi,
		type Proposal,
		type ProposalType,
		type VotingRule,
		type CycleTemplate
	} from '$lib/api/proposals';
	import { toast } from '$lib/stores/toast';
	import { renderMarkdown } from '$lib/utils/markdown';

	const proposalId = $derived($page.params.id || '');
	const meId = $derived($authStore.user?.id || '');

	let loading = $state(true);
	let saving = $state(false);
	let proposal = $state<Proposal | null>(null);

	let title = $state('');
	let proposalType = $state<ProposalType>('feature');
	let domains = $state('');
	let content = $state('');
	let contentMode = $state<'edit' | 'preview'>('edit');
	let votingRule = $state<VotingRule>('simple_majority');
	let cycleTemplate = $state<CycleTemplate>('fast');
	let contentTextareaEl = $state<HTMLTextAreaElement | null>(null);
	let uploadingContentImage = $state(false);

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

	onMount(async () => {
		await loadProposal();
	});

	async function loadProposal() {
		if (!proposalId) {
			goto('/proposals');
			return;
		}
		const res = await proposalsApi.get(proposalId);
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governance.notFound'));
			goto('/proposals');
			return;
		}
		proposal = res.data.proposal;
		if (proposal.status !== 'draft') {
			toast.error(get(t)('governance.draftOnlyEditable'));
			goto(`/proposals/${proposal.id}`);
			return;
		}
		if (!meId || proposal.author_id !== meId) {
			toast.error(get(t)('governance.authorOnlyEdit'));
			goto(`/proposals/${proposal.id}`);
			return;
		}
		title = proposal.title;
		proposalType = proposal.proposal_type;
		domains = proposal.domains.join(', ');
		content = proposal.content;
		votingRule = proposal.voting_rule;
		cycleTemplate = proposal.cycle_template;
		loading = false;
	}

	async function submit() {
		if (!proposal) return;
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

		saving = true;
		const res = await proposalsApi.update(proposal.id, {
			title: title.trim(),
			proposal_type: proposalType,
			content: content.trim(),
			domains: domainList,
			voting_rule: votingRule,
			cycle_template: cycleTemplate
		});
		saving = false;

		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governance.updateFailed'));
			return;
		}
		toast.success(get(t)('governance.updateSuccess'));
		goto(`/proposals/${proposal.id}`);
	}

	function isAllowedImage(type: string): boolean {
		return ['image/png', 'image/jpeg', 'image/jpg', 'image/gif', 'image/webp'].includes(type);
	}

	function appendContentImage(url: string): void {
		const markdown = `![image](${url})`;
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
		if (!isAllowedImage(file.type)) {
			toast.error(get(t)('toast.uploadTypeFail'));
			return;
		}
		if (file.size > 10 * 1024 * 1024) {
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
				appendContentImage(result.data.url);
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
			if (!item.type.startsWith('image/')) {
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
</script>

{#if loading}
	<div class="mx-auto max-w-3xl rounded-lg border border-slate-200 bg-white p-8 text-center text-slate-500 dark:border-slate-700 dark:bg-slate-800">{$t('governance.loading')}</div>
{:else}
	<div class="mx-auto max-w-3xl">
		<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
			<h1 class="mb-4 text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governance.editTitle')}</h1>
			<div class="space-y-4">
				<div>
					<label for="proposal-title" class="mb-1 block text-sm text-slate-600">{$t('governance.field.title')}</label>
					<input id="proposal-title" type="text" bind:value={title} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
				</div>

				<div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
					<div>
						<label for="proposal-type" class="mb-1 block text-sm text-slate-600">{$t('governance.field.type')}</label>
						<select id="proposal-type" bind:value={proposalType} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
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
					</div>
				</div>

				<div>
					<label for="proposal-domains" class="mb-1 block text-sm text-slate-600">{$t('governance.field.domains')}</label>
					<input id="proposal-domains" type="text" bind:value={domains} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
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
							class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"
							onpaste={handleContentPaste}
						></textarea>
						<div class="mt-2">
							<label class="inline-flex cursor-pointer items-center gap-1 text-sm text-blue-600 hover:text-blue-800 dark:text-blue-400 dark:hover:text-blue-300">
								<span>{uploadingContentImage ? $t('search.searching') : $t('issue.uploadImage')}</span>
								<input type="file" accept="image/png,image/jpeg,image/gif,image/webp" class="hidden" onchange={handleContentImageSelect} disabled={uploadingContentImage} />
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
					<a href={`/proposals/${proposalId}`} class="rounded-md border border-slate-300 px-4 py-2 text-sm">{$t('common.cancel')}</a>
					<button type="button" onclick={submit} disabled={saving} class="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-60">
						{saving ? $t('common.saving') : $t('governance.saveDraft')}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}

<style>
	:global(.markdown-body img) {
		max-width: 100%;
		height: auto;
		border-radius: 0.5rem;
	}
</style>

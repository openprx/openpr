<script lang="ts">
	import { labelsApi } from '$lib/api/labels';
	import { membersApi, type WorkspaceMember } from '$lib/api/members';
	import {
		issuesApi,
		type Issue,
		type IssuePriority,
		type IssueStatus
	} from '$lib/api/issues';
	import { sprintsApi, type Sprint } from '$lib/api/sprints';
	import { toast } from '$lib/stores/toast';
	import { renderMarkdown } from '$lib/utils/markdown';
	import {
		appendAttachmentsMarkdown,
		ISSUE_ATTACHMENT_ACCEPT,
		ISSUE_ATTACHMENT_MAX_SIZE_BYTES,
		type UploadedAttachment
	} from '$lib/utils/attachments';
	import { t } from 'svelte-i18n';
	import Button from './Button.svelte';
	import FileUpload from './FileUpload.svelte';
	import Input from './Input.svelte';
	import Modal from './Modal.svelte';
	import Tag from './Tag.svelte';

	interface LabelOption {
		id: string;
		name: string;
		color: string;
	}

	interface Props {
		open?: boolean;
		workspaceId: string;
		projectId: string;
		initialStatus?: IssueStatus;
		initialPriority?: IssuePriority;
		initialSprintId?: string;
		onCreated?: (issue: Issue) => void;
	}

	let {
		open = $bindable(false),
		workspaceId,
		projectId,
		initialStatus = 'backlog',
		initialPriority = 'medium',
		initialSprintId = '',
		onCreated
	}: Props = $props();

	let loadingOptions = $state(false);
	let creating = $state(false);
	let descriptionTab = $state<'edit' | 'preview'>('edit');
	let members = $state<WorkspaceMember[]>([]);
	let sprints = $state<Sprint[]>([]);
	let labels = $state<LabelOption[]>([]);
	let selectedLabelIds = $state<string[]>([]);
	let attachments = $state<UploadedAttachment[]>([]);

	let form = $state({
		title: '',
		description: '',
		status: 'backlog' as IssueStatus,
		priority: 'medium' as IssuePriority,
		assignee_id: '',
		sprint_id: ''
	});

	let wasOpen = false;
	$effect(() => {
		if (open && !wasOpen) {
			resetForm();
			void loadOptions();
		}
		wasOpen = open;
	});

	function resetForm(): void {
		form = {
			title: '',
			description: '',
			status: initialStatus,
			priority: initialPriority,
			assignee_id: '',
			sprint_id: initialSprintId
		};
		selectedLabelIds = [];
		attachments = [];
		descriptionTab = 'edit';
	}

	async function loadOptions(): Promise<void> {
		loadingOptions = true;

		const [memberRes, sprintRes, labelRes] = await Promise.all([
			membersApi.list(workspaceId),
			sprintsApi.list(projectId),
			labelsApi.list(workspaceId)
		]);

		if (memberRes.code === 0 && memberRes.data) {
			members = memberRes.data.items ?? [];
		} else {
			toast.error(memberRes.message || $t('issue.loadMembersFailed'));
		}

		if (sprintRes.code === 0 && sprintRes.data) {
			sprints = sprintRes.data.items ?? [];
		} else {
			toast.error(sprintRes.message || $t('issue.loadSprintsFailed'));
		}

		if (labelRes.code === 0 && labelRes.data) {
			labels = (labelRes.data.items ?? []).map((item) => ({
				id: String(item.id),
				name: String(item.name),
				color: String(item.color || '#64748b')
			}));
		} else {
			toast.error(labelRes.message || $t('issue.loadLabelsFailed'));
		}

		loadingOptions = false;
	}

	function toggleLabel(labelId: string): void {
		if (selectedLabelIds.includes(labelId)) {
			selectedLabelIds = selectedLabelIds.filter((id) => id !== labelId);
			return;
		}
		selectedLabelIds = [...selectedLabelIds, labelId];
	}

	async function handleCreate(): Promise<void> {
		if (creating) {
			return;
		}
		if (!form.title.trim()) {
			toast.error($t('issue.enterTitle'));
			return;
		}

		creating = true;
		const description = appendAttachmentsMarkdown(form.description, attachments).trim();
		const createRes = await issuesApi.create(projectId, {
			title: form.title.trim(),
			description: description || undefined,
			status: form.status,
			priority: form.priority,
			assignee_id: form.assignee_id || undefined,
			sprint_id: form.sprint_id || undefined
		});

		if (createRes.code !== 0 || !createRes.data) {
			toast.error(createRes.message || $t('issue.createFailed'));
			creating = false;
			return;
		}

		if (selectedLabelIds.length > 0) {
			const labelRequests = selectedLabelIds.map((labelId) =>
				labelsApi.addToIssue(createRes.data!.id, labelId)
			);
			const labelResults = await Promise.all(labelRequests);
			const failed = labelResults.find((result) => result.code !== 0);
			if (failed) {
				toast.error(failed.message || $t('issue.addLabelPartialFail'));
			}
		}

		const selectedLabels = labels
			.filter((label) => selectedLabelIds.includes(label.id))
			.map((label) => ({
				id: label.id,
				name: label.name,
				color: label.color
			}));

		const createdIssue: Issue = {
			...createRes.data,
			labels: selectedLabels.length > 0 ? selectedLabels : createRes.data.labels
		};

		onCreated?.(createdIssue);
		toast.success($t('issue.createSuccess'));
		open = false;
		resetForm();
		creating = false;
	}

	function getMemberOptionLabel(member: WorkspaceMember): string {
		const base = member.name || member.email;
		return member.entity_type === 'bot' ? `[Bot] ${base}` : base;
	}
</script>

<Modal bind:open title={$t('issue.create')} maxWidthClass="max-w-2xl">
	{#if loadingOptions}
		<div class="space-y-3">
			<div class="h-10 animate-pulse rounded-md bg-slate-100 dark:bg-slate-800"></div>
			<div class="h-56 animate-pulse rounded-md bg-slate-100 dark:bg-slate-800"></div>
			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div class="h-10 animate-pulse rounded-md bg-slate-100 dark:bg-slate-800"></div>
				<div class="h-10 animate-pulse rounded-md bg-slate-100 dark:bg-slate-800"></div>
			</div>
		</div>
	{:else}
		<form class="space-y-5" onsubmit={(event) => { event.preventDefault(); handleCreate(); }}>
			<Input label={$t('issue.title')} required bind:value={form.title} placeholder={$t('issue.titleExample')} />

			<div class="space-y-2">
				<div class="flex items-center justify-between">
					<label class="text-sm font-medium text-slate-700 dark:text-slate-300" for="issueDescription">{$t('issue.description')}</label>
					<div class="inline-flex rounded-md border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 p-0.5">
						<button
							type="button"
							class={`rounded px-2 py-1 text-xs ${descriptionTab === 'edit'
								? 'bg-white dark:bg-slate-900 text-slate-900 dark:text-slate-100 shadow-sm dark:shadow-slate-900/50'
								: 'text-slate-500 hover:text-slate-700 dark:text-slate-300'}`}
							onclick={() => (descriptionTab = 'edit')}
						>
							{$t('issue.editMode')}
						</button>
						<button
							type="button"
							class={`rounded px-2 py-1 text-xs ${descriptionTab === 'preview'
								? 'bg-white dark:bg-slate-900 text-slate-900 dark:text-slate-100 shadow-sm dark:shadow-slate-900/50'
								: 'text-slate-500 hover:text-slate-700 dark:text-slate-300'}`}
							onclick={() => (descriptionTab = 'preview')}
						>
							{$t('issue.previewMode')}
						</button>
					</div>
				</div>

				{#if descriptionTab === 'edit'}
					<textarea
						id="issueDescription"
						bind:value={form.description}
						class="min-h-[220px] w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 placeholder:text-slate-400 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:placeholder-slate-500"
						placeholder={$t('issue.descriptionPlaceholder')}
					></textarea>
					<FileUpload
						bind:value={attachments}
						accept={ISSUE_ATTACHMENT_ACCEPT}
						maxSize={ISSUE_ATTACHMENT_MAX_SIZE_BYTES}
						multiple
					/>
				{:else}
					<div class="markdown-body dark:prose-invert min-h-[220px] rounded-md border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 px-3 py-2 text-sm text-slate-700 dark:text-slate-300">
						{#if form.description.trim()}
							{@html renderMarkdown(form.description)}
						{:else}
							<p class="text-slate-400">{$t('issue.noDescriptionPreview')}</p>
						{/if}
					</div>
				{/if}
			</div>

			<div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
				<div>
					<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="issueStatus">{$t('issue.status')}</label>
					<select
						id="issueStatus"
						bind:value={form.status}
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					>
						<option value="backlog">{$t('issue.backlog')}</option>
						<option value="todo">{$t('issue.todo')}</option>
						<option value="in_progress">{$t('issue.inProgress')}</option>
						<option value="done">{$t('issue.done')}</option>
					</select>
				</div>

				<div>
					<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="issuePriority">{$t('issue.priority')}</label>
					<select
						id="issuePriority"
						bind:value={form.priority}
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					>
						<option value="low">{$t('issue.low')}</option>
						<option value="medium">{$t('issue.medium')}</option>
						<option value="high">{$t('issue.high')}</option>
						<option value="urgent">{$t('issue.urgent')}</option>
					</select>
				</div>

				<div>
					<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="issueAssignee">{$t('issue.assignee')}</label>
					<select
						id="issueAssignee"
						bind:value={form.assignee_id}
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					>
						<option value="">{$t('common.unassigned')}</option>
						{#each members as member (member.user_id)}
							<option value={member.user_id}>{getMemberOptionLabel(member)}</option>
						{/each}
					</select>
				</div>

				<div>
					<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="issueSprint">Sprint</label>
					<select
						id="issueSprint"
						bind:value={form.sprint_id}
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					>
						<option value="">{$t('issue.noSprint')}</option>
						{#each sprints as sprint (sprint.id)}
							<option value={sprint.id}>{sprint.name}</option>
						{/each}
					</select>
				</div>
			</div>

			<fieldset class="space-y-2">
				<legend class="block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('issue.labels')}</legend>
				{#if labels.length === 0}
					<p class="rounded-md border border-dashed border-slate-300 dark:border-slate-600 bg-slate-50 dark:bg-slate-950 px-3 py-2 text-sm text-slate-500 dark:text-slate-400">
						{$t('issue.noWorkspaceLabels')}
					</p>
				{:else}
					<div class="max-h-36 space-y-1 overflow-y-auto rounded-md border border-slate-200 dark:border-slate-700 p-2">
						{#each labels as label (label.id)}
							<label
								class="flex cursor-pointer items-center justify-between rounded px-2 py-1 text-sm hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
							>
								<div class="flex items-center gap-2">
									<input
										type="checkbox"
										checked={selectedLabelIds.includes(label.id)}
										onchange={() => toggleLabel(label.id)}
									/>
									<Tag label={label.name} color={label.color} />
								</div>
							</label>
						{/each}
					</div>
				{/if}
			</fieldset>
		</form>
	{/if}

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (open = false)}>{$t('common.cancel')}</Button>
		<Button loading={creating} onclick={handleCreate}>{$t('issue.create')}</Button>
	{/snippet}
</Modal>

<style>
	:global(.markdown-body img) {
		max-width: 100%;
		height: auto;
		border-radius: 0.375rem;
		margin: 0.5rem 0;
	}

	:global(.markdown-body p) {
		margin-top: 0.5rem;
		margin-bottom: 0.5rem;
	}

	:global(.markdown-body code) {
		background: #e2e8f0;
		border-radius: 0.25rem;
		padding: 0.125rem 0.25rem;
	}
</style>

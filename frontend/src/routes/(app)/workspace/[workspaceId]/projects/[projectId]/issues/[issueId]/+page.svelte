<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount, tick } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { activityApi, type Activity } from '$lib/api/activity';
	import { commentsApi, type Comment } from '$lib/api/comments';
	import { issuesApi, type Issue, type IssuePriority, type IssueStatus } from '$lib/api/issues';
	import { labelsApi, type Label } from '$lib/api/labels';
	import { sprintsApi, type Sprint } from '$lib/api/sprints';
	import { membersApi, type WorkspaceMember } from '$lib/api/members';
	import { authStore } from '$lib/stores/auth';
	import { toast } from '$lib/stores/toast';
	import { renderMarkdown } from '$lib/utils/markdown';
	import { requireRouteParam } from '$lib/utils/route-params';
	import {
		appendAttachmentsMarkdown,
		extractMarkdownAttachments,
		ISSUE_ATTACHMENT_ACCEPT,
		ISSUE_ATTACHMENT_MAX_SIZE_BYTES,
		type UploadedAttachment
	} from '$lib/utils/attachments';
	import Button from '$lib/components/Button.svelte';
	import { isImageUploadMime, isAllowedUploadMime } from '$lib/utils/upload';
	import { apiClient } from '$lib/api/client';
	import FileUpload from '$lib/components/FileUpload.svelte';
	import Input from '$lib/components/Input.svelte';
	import Select from '$lib/components/Select.svelte';
	import Tag from '$lib/components/Tag.svelte';
	import Textarea from '$lib/components/Textarea.svelte';
	import BotIcon from '$lib/components/BotIcon.svelte';

	type SelectOption = {
		label: string;
		value: string;
		disabled?: boolean;
	};

	const workspaceId = $derived(requireRouteParam($page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam($page.params.projectId, 'projectId'));
	const issueId = $derived(requireRouteParam($page.params.issueId, 'issueId'));

	const statusOptions: SelectOption[] = [
		{ label: get(t)('issue.backlog'), value: 'backlog' },
		{ label: get(t)('issue.todo'), value: 'todo' },
		{ label: get(t)('issue.inProgress'), value: 'in_progress' },
		{ label: get(t)('issue.done'), value: 'done' }
	];

	const priorityOptions: SelectOption[] = [
		{ label: get(t)('issue.low'), value: 'low' },
		{ label: get(t)('issue.medium'), value: 'medium' },
		{ label: get(t)('issue.high'), value: 'high' },
		{ label: get(t)('issue.urgent'), value: 'urgent' }
	];

	let loading = $state(true);
	let issue = $state<Issue | null>(null);
	let comments = $state<Comment[]>([]);
	let activities = $state<Activity[]>([]);
	let projectLabels = $state<Label[]>([]);
	let issueLabels = $state<Label[]>([]);
	let sprints = $state<Sprint[]>([]);
	let workspaceMembers = $state<WorkspaceMember[]>([]);
	let memberNameMap = $state<Record<string, string>>({});
	let memberEntityTypeMap = $state<Record<string, string>>({});

	let editingIssue = $state(false);
	let issueForm = $state({
		title: '',
		description: ''
	});
	let savingIssue = $state(false);
	let deletingIssue = $state(false);
	let issueDescriptionAttachments = $state<UploadedAttachment[]>([]);

	let statusValue = $state<IssueStatus>('backlog');
	let priorityValue = $state<IssuePriority>('medium');
	let assigneeValue = $state('');
	let sprintValue = $state('');
	let updatingField = $state<'status' | 'priority' | 'assignee' | 'sprint' | null>(null);

	let selectedLabelId = $state('');
	let labelSaving = $state(false);
	let newLabelName = $state('');
	let newLabelColor = $state('#2563eb');
	let creatingLabel = $state(false);

	let newComment = $state('');
	let commentSubmitting = $state(false);
	let editingCommentId = $state<string | null>(null);
	let editingCommentContent = $state('');
	let commentBusyId = $state<string | null>(null);
	let mentionedUsers = $state<string[]>([]);
	let showMentionDropdown = $state(false);
	let mentionFilter = $state('');
	let mentionAnchorIndex = $state<number | null>(null);
	let commentTextareaEl = $state<HTMLTextAreaElement | null>(null);
	let editingMentionedUsers = $state<string[]>([]);
	let showEditMentionDropdown = $state(false);
	let editMentionFilter = $state('');
	let editMentionAnchorIndex = $state<number | null>(null);
	let editTextareaEl = $state<HTMLTextAreaElement | null>(null);
	let commentAttachments = $state<UploadedAttachment[]>([]);

	onMount(async () => {
		await loadPageData();
	});

	const currentUserId = $derived($authStore.user?.id ?? '');

	const assigneeOptions = $derived.by<SelectOption[]>(() => {
		const opts = Object.entries(memberNameMap)
			.sort(([, a], [, b]) => a.localeCompare(b))
			.map(([id, name]) => ({
				label: memberEntityTypeMap[id] === 'bot' ? `[Bot] ${name}` : name,
				value: id
			}));
		return [{ label: get(t)('common.unassigned'), value: '' }, ...opts];
	});

	const availableLabelOptions = $derived.by<SelectOption[]>(() => {
		const selected = new Set((issueLabels || []).map((item) => item.id));
		return projectLabels
			.filter((item) => !selected.has(item.id))
			.map((item) => ({ label: item.name, value: item.id }));
	});

	const sprintOptions = $derived.by<SelectOption[]>(() => [
		{ label: get(t)('issue.noSprint'), value: '' },
		...sprints.map((sprint) => ({ label: sprint.name, value: sprint.id }))
	]);

	const currentSprintName = $derived.by(() => {
		const sprintId = issue?.sprint_id;
		if (!sprintId) {
			return get(t)('issue.noSprint');
		}
		return sprints.find((sprint) => sprint.id === sprintId)?.name ?? get(t)('issue.noSprint');
	});

	const mentionCandidates = $derived.by(() =>
		workspaceMembers.filter((member) => member.user_id !== currentUserId)
	);

	const filteredMentionMembers = $derived.by(() => {
		const keyword = mentionFilter.trim().toLowerCase();
		return mentionCandidates
			.filter((member) => {
				if (!keyword) {
					return true;
				}
				return (
					member.name.toLowerCase().includes(keyword) ||
					member.email.toLowerCase().includes(keyword)
				);
			})
			.slice(0, 8);
	});

	const filteredEditMentionMembers = $derived.by(() => {
		const keyword = editMentionFilter.trim().toLowerCase();
		return mentionCandidates
			.filter((member) => {
				if (!keyword) {
					return true;
				}
				return (
					member.name.toLowerCase().includes(keyword) ||
					member.email.toLowerCase().includes(keyword)
				);
			})
			.slice(0, 8);
	});

	const existingIssueAttachments = $derived.by(() => extractMarkdownAttachments(issueForm.description));

	async function loadPageData() {
		loading = true;

		const [issueResponse, commentsResponse, activityResponse, labelsResponse, issueLabelsResponse, sprintsResponse] = await Promise.all([
			issuesApi.get(issueId),
			commentsApi.list(issueId),
			activityApi.list(issueId),
			labelsApi.list(workspaceId),
			labelsApi.listForIssue(issueId),
			sprintsApi.list(projectId)
		]);

		if (issueResponse.code !== 0) {
			toast.error(issueResponse.message);
		} else if (issueResponse.data) {
			issue = issueResponse.data;
			issueForm = {
				title: issueResponse.data.title,
				description: issueResponse.data.description ?? ''
			};
			statusValue = issueResponse.data.status;
			priorityValue = issueResponse.data.priority;
			assigneeValue = issueResponse.data.assignee_id ?? '';
			sprintValue = issueResponse.data.sprint_id ?? '';
			issueLabels = extractIssueLabels(issueResponse.data);
		}

		if (commentsResponse.code !== 0) {
			toast.error(commentsResponse.message);
		} else if (commentsResponse.data) {
			comments = [...(commentsResponse.data.items ?? [])].sort(sortByCreatedAsc);
		}

		if (activityResponse.code !== 0) {
			toast.error(activityResponse.message);
		} else if (activityResponse.data) {
			activities = [...(activityResponse.data.items ?? [])].sort(sortByCreatedDesc);
		}

		if (labelsResponse.code !== 0) {
			toast.error(labelsResponse.message);
		} else if (labelsResponse.data) {
			projectLabels = labelsResponse.data.items ?? [];
		}



		if (issueLabelsResponse.code === 0 && issueLabelsResponse.data) {
			issueLabels = issueLabelsResponse.data.items ?? [];
		}

		if (sprintsResponse.code !== 0) {
			toast.error(sprintsResponse.message);
		} else if (sprintsResponse.data) {
			sprints = sprintsResponse.data.items ?? [];
		}

		loading = false;

		// Load workspace members for assignee names
		membersApi.list(workspaceId).then((res) => {
			if (res.code === 0 && res.data) {
				workspaceMembers = res.data.items ?? [];
				const map: Record<string, string> = {};
				const entityMap: Record<string, string> = {};
				for (const member of (res.data.items ?? [])) {
					const fallback = member.user_id.substring(0, 8);
					map[member.user_id] = member.name || member.email || fallback;
					entityMap[member.user_id] = member.entity_type || 'human';
				}
				memberNameMap = map;
				memberEntityTypeMap = entityMap;
			}
		}).catch(() => {});
	}

	function extractIssueLabels(data: Issue): Label[] {
		const ext = data as Issue & { issue_labels?: Label[] };
		if (Array.isArray(ext.labels)) {
			return ext.labels.map((label) => ({
				id: label.id,
				name: label.name,
				color: label.color,
				project_id: issue?.project_id ?? data.project_id,
				created_at: ''
			}));
		}
		if (Array.isArray(ext.issue_labels)) {
			return ext.issue_labels;
		}
		return [];
	}

	function sortByCreatedAsc(a: { created_at: string }, b: { created_at: string }): number {
		return new Date(a.created_at).getTime() - new Date(b.created_at).getTime();
	}

	function sortByCreatedDesc(a: { created_at: string }, b: { created_at: string }): number {
		return new Date(b.created_at).getTime() - new Date(a.created_at).getTime();
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function formatIssueCode(data: Issue): string {
		if (data.key?.trim()) {
			return data.key;
		}
		if (/^\d+$/.test(data.id)) {
			return `#${data.id}`;
		}
		return `#${data.id.slice(0, 8)}`;
	}

	async function updateIssue(data: {
		title?: string;
		description?: string;
		status?: IssueStatus;
		priority?: IssuePriority;
		assignee_id?: string;
		sprint_id?: string;
	}): Promise<boolean> {
		if (!issue) {
			return false;
		}

		const response = await issuesApi.update(issue.id, data);
		if (response.code !== 0) {
			toast.error(response.message);
			return false;
		}

		if (response.data) {
			issue = response.data;
			issueLabels = extractIssueLabels(response.data);
			issueForm = {
				title: response.data.title,
				description: response.data.description ?? ''
			};
			statusValue = response.data.status;
			priorityValue = response.data.priority;
			assigneeValue = response.data.assignee_id ?? '';
			sprintValue = response.data.sprint_id ?? '';
		}

		return true;
	}

	async function saveIssueEdit() {
		if (!issue || savingIssue) {
			return;
		}
		const title = issueForm.title.trim();
		if (!title) {
			toast.error(get(t)('issue.titleRequired'));
			return;
		}

		savingIssue = true;
		const description = appendAttachmentsMarkdown(
			issueForm.description,
			issueDescriptionAttachments
		).trim();
		const success = await updateIssue({
			title,
			description: description || undefined
		});
		savingIssue = false;
		if (success) {
			editingIssue = false;
			issueDescriptionAttachments = [];
			toast.success(get(t)('issue.updated'));
		}
	}

	function toggleIssueEditMode() {
		editingIssue = !editingIssue;
		issueDescriptionAttachments = [];
	}

	async function handleStatusChange() {
		if (!issue || updatingField) {
			return;
		}
		if (issue.status === statusValue) {
			return;
		}

		updatingField = 'status';
		const previous = issue.status;
		const success = await updateIssue({ status: statusValue });
		if (!success) {
			statusValue = previous;
		} else {
			toast.success(get(t)('issue.statusUpdated'));
		}
		updatingField = null;
	}

	async function handlePriorityChange() {
		if (!issue || updatingField) {
			return;
		}
		if (issue.priority === priorityValue) {
			return;
		}

		updatingField = 'priority';
		const previous = issue.priority;
		const success = await updateIssue({ priority: priorityValue });
		if (!success) {
			priorityValue = previous;
		} else {
			toast.success(get(t)('issue.priorityUpdated'));
		}
		updatingField = null;
	}

	async function handleAssigneeChange() {
		if (!issue || updatingField) {
			return;
		}
		if ((issue.assignee_id ?? '') === assigneeValue) {
			return;
		}

		updatingField = 'assignee';
		const previous = issue.assignee_id ?? '';
		const success = await updateIssue({ assignee_id: assigneeValue || undefined });
		if (!success) {
			assigneeValue = previous;
		} else {
			toast.success(get(t)('issue.assigneeUpdated'));
		}
		updatingField = null;
	}

	async function handleSprintChange() {
		if (!issue || updatingField) {
			return;
		}
		if ((issue.sprint_id ?? '') === sprintValue) {
			return;
		}

		updatingField = 'sprint';
		const previous = issue.sprint_id ?? '';
		const success = await updateIssue({ sprint_id: sprintValue || undefined });
		if (!success) {
			sprintValue = previous;
		} else {
			toast.success(get(t)('issue.sprintUpdated'));
		}
		updatingField = null;
	}

	async function handleCloseIssue() {
		if (!issue || issue.status === 'done') {
			return;
		}

		statusValue = 'done';
		await handleStatusChange();
	}

	async function handleDeleteIssue() {
		if (!issue || deletingIssue) {
			return;
		}

		if (!confirm(get(t)('issue.deleteConfirm'))) {
			return;
		}

		deletingIssue = true;
		const response = await issuesApi.delete(issue.id);
		deletingIssue = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		toast.success(get(t)('issue.deleted'));
		goto(`/workspace/${workspaceId}/projects/${projectId}/issues`);
	}

	async function handleAddLabel() {
		if (!issue || !selectedLabelId || labelSaving) {
			return;
		}

		labelSaving = true;
		const response = await labelsApi.addToIssue(issue.id, selectedLabelId);
		labelSaving = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		const selectedLabel = projectLabels.find((item) => item.id === selectedLabelId);
		if (selectedLabel && !issueLabels?.some((item) => item.id === selectedLabel.id)) {
			issueLabels = [...(issueLabels || []), selectedLabel];
		}
		selectedLabelId = '';
		toast.success(get(t)('issue.labelAdded'));
	}

	async function handleRemoveLabel(labelId: string) {
		if (!issue || labelSaving) {
			return;
		}

		labelSaving = true;
		const response = await labelsApi.removeFromIssue(issue.id, labelId);
		labelSaving = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		issueLabels = (issueLabels || []).filter((item) => item.id !== labelId);
		toast.success(get(t)('issue.labelRemoved'));
	}

	async function handleCreateLabel() {
		if (creatingLabel) {
			return;
		}
		const name = newLabelName.trim();
		if (!name) {
			toast.error(get(t)('issue.labelNameRequired'));
			return;
		}

		creatingLabel = true;
		const response = await labelsApi.create(workspaceId, {
			name,
			color: newLabelColor
		});
		creatingLabel = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		if (response.data) {
			projectLabels = [...projectLabels, response.data];
			newLabelName = '';
			selectedLabelId = response.data.id;
			toast.success(get(t)('issue.labelCreated'));
		}
	}

	function canManageComment(comment: Comment): boolean {
		return Boolean(currentUserId && currentUserId === comment.user_id);
	}

	function parseMentionAtCursor(text: string, cursorPos: number): { start: number | null; filter: string } {
		const beforeCursor = text.slice(0, cursorPos);
		const atMatch = beforeCursor.match(/(^|\s)@([^\s@]*)$/);
		if (!atMatch) {
			return { start: null, filter: '' };
		}
		return {
			start: beforeCursor.lastIndexOf('@'),
			filter: atMatch[2] ?? ''
		};
	}

	function extractMentionIds(text: string): string[] {
		const content = text.toLowerCase();
		const ids: string[] = [];
		const seen = new Set<string>();

		for (const member of mentionCandidates) {
			const marker = `@${member.name}`.toLowerCase();
			if (!content.includes(marker) || seen.has(member.user_id)) {
				continue;
			}
			seen.add(member.user_id);
			ids.push(member.user_id);
		}

		return ids;
	}

	function handleNewCommentInput(event: Event) {
		const textarea = event.target as HTMLTextAreaElement;
		commentTextareaEl = textarea;
		const mentionState = parseMentionAtCursor(textarea.value, textarea.selectionStart ?? 0);
		mentionedUsers = extractMentionIds(textarea.value);
		mentionAnchorIndex = mentionState.start;
		mentionFilter = mentionState.filter;
		showMentionDropdown = mentionState.start !== null;
	}

	function handleEditCommentInput(event: Event) {
		const textarea = event.target as HTMLTextAreaElement;
		editTextareaEl = textarea;
		const mentionState = parseMentionAtCursor(textarea.value, textarea.selectionStart ?? 0);
		editingMentionedUsers = extractMentionIds(textarea.value);
		editMentionAnchorIndex = mentionState.start;
		editMentionFilter = mentionState.filter;
		showEditMentionDropdown = mentionState.start !== null;
	}

	function insertMention(
		textarea: HTMLTextAreaElement,
		currentText: string,
		anchorIndex: number | null,
		member: WorkspaceMember
	): { nextText: string; cursorPos: number } {
		const cursorPos = textarea.selectionStart ?? currentText.length;
		const atStart = anchorIndex ?? currentText.lastIndexOf('@', Math.max(0, cursorPos - 1));
		if (atStart < 0) {
			return { nextText: currentText, cursorPos };
		}

		const before = currentText.slice(0, atStart);
		const after = currentText.slice(cursorPos);
		const mentionText = `@${member.name} `;
		const nextText = `${before}${mentionText}${after}`;
		const nextCursorPos = before.length + mentionText.length;
		return { nextText, cursorPos: nextCursorPos };
	}

	async function selectMention(member: WorkspaceMember) {
		if (!commentTextareaEl) {
			return;
		}
		const inserted = insertMention(commentTextareaEl, newComment, mentionAnchorIndex, member);
		newComment = inserted.nextText;
		await tick();
		commentTextareaEl.focus();
		commentTextareaEl.setSelectionRange(inserted.cursorPos, inserted.cursorPos);
		mentionedUsers = extractMentionIds(inserted.nextText);
		showMentionDropdown = false;
		mentionFilter = '';
		mentionAnchorIndex = null;
	}

	async function selectEditMention(member: WorkspaceMember) {
		if (!editTextareaEl) {
			return;
		}
		const inserted = insertMention(
			editTextareaEl,
			editingCommentContent,
			editMentionAnchorIndex,
			member
		);
		editingCommentContent = inserted.nextText;
		await tick();
		editTextareaEl.focus();
		editTextareaEl.setSelectionRange(inserted.cursorPos, inserted.cursorPos);
		editingMentionedUsers = extractMentionIds(inserted.nextText);
		showEditMentionDropdown = false;
		editMentionFilter = '';
		editMentionAnchorIndex = null;
	}

	async function pasteUpload(
		file: File,
		target: 'description' | 'comment'
	): Promise<void> {
		const formData = new FormData();
		formData.append('file', file);
		try {
			const res = await fetch('/api/v1/upload', {
				method: 'POST',
				headers: { Authorization: `Bearer ${apiClient.getToken()}` },
				body: formData
			});
			const result = await res.json();
			const url = result?.data?.url ?? result?.url;
			if (!url) {
				toast.error(get(t)('toast.uploadFail'));
				return;
			}
			const mime = file.type;
			let md: string;
			if (isImageUploadMime(mime)) {
				md = `![image](${url})`;
			} else if (mime.startsWith('video/')) {
				md = `<video controls src="${url}"></video>`;
			} else {
				md = `[${file.name}](${url})`;
			}
			if (target === 'description') {
				const suffix = issueForm.description.endsWith('\n') || !issueForm.description ? '' : '\n';
				issueForm.description = `${issueForm.description}${suffix}${md}\n`;
			} else {
				const suffix = newComment.endsWith('\n') || !newComment ? '' : '\n';
				newComment = `${newComment}${suffix}${md}\n`;
			}
			toast.success(get(t)('toast.uploadSuccess'));
		} catch {
			toast.error(get(t)('toast.uploadNetworkFail'));
		}
	}

	function handleDescriptionPaste(event: ClipboardEvent) {
		const items = event.clipboardData?.items;
		if (!items) return;
		for (const item of items) {
			if (item.kind === 'file' && isAllowedUploadMime(item.type)) {
				event.preventDefault();
				const file = item.getAsFile();
				if (file) void pasteUpload(file, 'description');
				return;
			}
		}
	}

	function handleCommentPaste(event: ClipboardEvent) {
		const items = event.clipboardData?.items;
		if (!items) return;
		for (const item of items) {
			if (item.kind === 'file' && isAllowedUploadMime(item.type)) {
				event.preventDefault();
				const file = item.getAsFile();
				if (file) void pasteUpload(file, 'comment');
				return;
			}
		}
	}

	function handleDescriptionUpload(event: CustomEvent<{ url: string; filename: string }>) {
		const { url, filename } = event.detail;
		const att = issueDescriptionAttachments.find((a) => a.url === url);
		const mime = att?.mimeType ?? '';
		if (isImageUploadMime(mime)) {
			const md = `![${filename}](${url})`;
			const suffix = issueForm.description.endsWith('\n') || !issueForm.description ? '' : '\n';
			issueForm.description = `${issueForm.description}${suffix}${md}\n`;
		} else if (mime.startsWith('video/')) {
			const md = `<video controls src="${url}"></video>`;
			const suffix = issueForm.description.endsWith('\n') || !issueForm.description ? '' : '\n';
			issueForm.description = `${issueForm.description}${suffix}${md}\n`;
		}
	}

	function handleCommentUpload(event: CustomEvent<{ url: string; filename: string }>) {
		const { url, filename } = event.detail;
		const att = commentAttachments.find((a) => a.url === url);
		const mime = att?.mimeType ?? '';
		if (isImageUploadMime(mime)) {
			const md = `![${filename}](${url})`;
			const suffix = newComment.endsWith('\n') || !newComment ? '' : '\n';
			newComment = `${newComment}${suffix}${md}\n`;
		} else if (mime.startsWith('video/')) {
			const md = `<video controls src="${url}"></video>`;
			const suffix = newComment.endsWith('\n') || !newComment ? '' : '\n';
			newComment = `${newComment}${suffix}${md}\n`;
		}
	}

	async function handleCreateComment() {
		if (commentSubmitting) {
			return;
		}

		const content = appendAttachmentsMarkdown(newComment, commentAttachments).trim();
		if (!content) {
			return;
		}

		commentSubmitting = true;
		const mentionIds = extractMentionIds(newComment);
		const response = await commentsApi.create(issueId, {
			content,
			mentions: mentionIds.length > 0 ? mentionIds : undefined
		});
		commentSubmitting = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		if (response.data) {
			comments = [...comments, response.data].sort(sortByCreatedAsc);
			newComment = '';
			commentAttachments = [];
			mentionedUsers = [];
			showMentionDropdown = false;
			mentionFilter = '';
			mentionAnchorIndex = null;
			toast.success(get(t)('issue.commentAdded'));
		}
	}

	function startEditComment(comment: Comment) {
		if (!canManageComment(comment)) {
			return;
		}
		editingCommentId = comment.id;
		editingCommentContent = comment.content;
		editingMentionedUsers = extractMentionIds(comment.content);
	}

	function cancelEditComment() {
		editingCommentId = null;
		editingCommentContent = '';
		editingMentionedUsers = [];
		showEditMentionDropdown = false;
		editMentionFilter = '';
		editMentionAnchorIndex = null;
		editTextareaEl = null;
	}

	async function handleUpdateComment() {
		if (!editingCommentId || !editingCommentContent.trim() || commentBusyId) {
			return;
		}

		commentBusyId = editingCommentId;
		const mentionIds = extractMentionIds(editingCommentContent);
		const response = await commentsApi.update(editingCommentId, {
			content: editingCommentContent.trim(),
			mentions: mentionIds.length > 0 ? mentionIds : undefined
		});
		commentBusyId = null;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		if (response.data) {
			comments = comments
				.map((item) => (item.id === response.data?.id ? response.data : item))
				.sort(sortByCreatedAsc);
			cancelEditComment();
			toast.success(get(t)('issue.commentUpdated'));
		}
	}

	async function handleDeleteComment(id: string) {
		if (commentBusyId) {
			return;
		}
		if (!confirm(get(t)('issue.commentDeleteConfirm'))) {
			return;
		}

		commentBusyId = id;
		const response = await commentsApi.delete(id);
		commentBusyId = null;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		comments = comments.filter((item) => item.id !== id);
		if (editingCommentId === id) {
			cancelEditComment();
		}
		toast.success(get(t)('issue.commentDeleted'));
	}

	function formatStatus(status: string): string {
		const map: Record<string, string> = {
			backlog: get(t)('issue.backlog'),
			todo: get(t)('issue.todo'),
			in_progress: get(t)('issue.inProgress'),
			done: get(t)('issue.done')
		};
		return map[status] || status;
	}

	function formatPriority(priority: string): string {
		const map: Record<string, string> = {
			low: get(t)('issue.low'),
			medium: get(t)('issue.medium'),
			high: get(t)('issue.high'),
			urgent: get(t)('issue.urgent')
		};
		return map[priority] || priority;
	}

	function getMemberDisplay(userId: string | undefined): string {
		if (!userId) {
			return '';
		}
		return memberNameMap[userId] || userId.substring(0, 8);
	}

	function renderCommentContent(content: string): string {
		const rendered = renderMarkdown(content);
		return rendered.replace(
			/(^|[\s>])@([^\s@<]+)/g,
			(_matched, prefix: string, mention: string) =>
				`${prefix}<span class=\"comment-mention\">@${mention}</span>`
		);
	}

	function isBotMember(member: WorkspaceMember): boolean {
		return member.entity_type === 'bot';
	}

	function getSprintDisplay(sprintId: string | undefined): string {
		if (!sprintId) {
			return '';
		}
		return sprints.find((item) => item.id === sprintId)?.name || sprintId.substring(0, 8);
	}

	function getActivityAuthor(activity: Activity): string {
		return activity.author_name || getMemberDisplay(activity.user_id) || get(t)('common.user');
	}

	function formatActivity(activity: Activity): string {
		const detail = activity.detail || {};
		const from = typeof detail.from === 'string' ? detail.from : '';
		const to = typeof detail.to === 'string' ? detail.to : '';
		const toName = typeof detail.to_name === 'string' ? detail.to_name : '';
		const labelName = typeof detail.label_name === 'string' ? detail.label_name : '';

		switch (activity.action) {
			case 'assigned':
				if (to) {
					return get(t)('activity.assigned', { values: { name: toName || getMemberDisplay(to) } });
				}
				return get(t)('activity.unassigned');
			case 'status_changed':
				return get(t)('activity.statusChanged', { values: { from: formatStatus(from), to: formatStatus(to) } });
			case 'priority_changed':
				return get(t)('activity.priorityChanged', { values: { from: formatPriority(from), to: formatPriority(to) } });
			case 'sprint_changed':
				if (to) {
					return get(t)('activity.sprintChanged', { values: { name: toName || getSprintDisplay(to) } });
				}
				return get(t)('activity.sprintRemoved');
			case 'label_added':
				return get(t)('activity.labelAdded', { values: { name: labelName } });
			case 'label_removed':
				return get(t)('activity.labelRemoved', { values: { name: labelName } });
			case 'comment_added':
				return get(t)('activity.commentAdded');
			case 'comment_edited':
				return get(t)('activity.commentEdited');
			case 'comment_deleted': {
				const preview =
					typeof detail.content_preview === 'string' ? detail.content_preview.trim() : '';
				return preview
					? get(t)('activity.commentDeletedWithPreview', { values: { preview } })
					: get(t)('activity.commentDeleted');
			}
			default:
				return activity.action || get(t)('common.unknownAction');
		}
	}
</script>

<div class="mx-auto max-w-7xl pb-44 md:pb-0">
	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-10">
			<div class="flex items-center justify-center gap-3 text-slate-500 dark:text-slate-400">
				<svg class="h-5 w-5 animate-spin" viewBox="0 0 24 24" fill="none">
					<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="3"></circle>
					<path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v3a5 5 0 00-5 5H4z"></path>
				</svg>
				<span>{$t('issue.loadingDetail')}</span>
			</div>
		</div>
	{:else if !issue}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-10 text-center text-slate-500 dark:text-slate-400">
			{$t('issue.notFound')}
		</div>
	{:else}
		<div class="space-y-4">
			<div class="flex flex-col gap-3 rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:flex-row sm:items-center sm:justify-between">
				<div class="text-sm text-slate-600 dark:text-slate-300">
					<a href="/workspace" class="hover:text-blue-600">{$t('nav.myWorkspace')}</a>
					<span class="mx-1 text-slate-400">/</span>
					<a href="/workspace/{workspaceId}/projects" class="hover:text-blue-600">{$t('project.title')}</a>
					<span class="mx-1 text-slate-400">/</span>
					<a href="/workspace/{workspaceId}/projects/{projectId}/issues" class="hover:text-blue-600">{$t('issue.list')}</a>
					<span class="mx-1 text-slate-400">/</span>
					<span class="font-medium text-slate-900 dark:text-slate-100">{formatIssueCode(issue)}</span>
				</div>
				<div class="flex flex-wrap gap-2">
					<Button variant="ghost" size="sm" onclick={toggleIssueEditMode}>
						{editingIssue ? $t('issue.cancelEdit') : $t('common.edit')}
					</Button>
					<Button variant="danger" size="sm" onclick={handleDeleteIssue} loading={deletingIssue}>
						{$t('common.delete')}
					</Button>
					<Button variant="secondary" size="sm" onclick={handleCloseIssue} disabled={issue.status === 'done'}>
						{$t('issue.close')}
					</Button>
				</div>
			</div>

			<div class="grid grid-cols-1 gap-4 lg:grid-cols-10">
				<div class="space-y-4 lg:col-span-7">
					<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
						<p class="mb-2 text-xs font-mono text-slate-500 dark:text-slate-400">{formatIssueCode(issue)}</p>
						{#if issue.proposal_id || issue.governance_exempt}
							<div class="mb-3 flex flex-wrap items-center gap-2">
								{#if issue.proposal_id}
									<a
										href={`/proposals/${issue.proposal_id}`}
										class="inline-flex items-center rounded-full border border-blue-200 bg-blue-50 px-2 py-0.5 text-xs font-medium text-blue-700"
									>
										{$t('issue.sourceProposal')}: {issue.proposal_id}
									</a>
								{/if}
								{#if issue.governance_exempt}
									<span
										class="inline-flex items-center rounded-full border border-amber-200 bg-amber-50 px-2 py-0.5 text-xs font-medium text-amber-700"
										title={issue.governance_exempt_reason || ''}
									>
										{$t('issue.governanceExempt')}
									</span>
								{/if}
							</div>
						{/if}
						{#if editingIssue}
							<div class="space-y-3">
								<Input label={$t('issue.title')} bind:value={issueForm.title} maxlength={200} required />
								<Textarea
									label={$t('issue.addDescription')}
									bind:value={issueForm.description}
									rows={8}
									placeholder={$t('issue.descriptionInputPlaceholder')}
									onpaste={handleDescriptionPaste}
								/>
								<FileUpload
									bind:value={issueDescriptionAttachments}
									accept={ISSUE_ATTACHMENT_ACCEPT}
									maxSize={ISSUE_ATTACHMENT_MAX_SIZE_BYTES}
									multiple
									on:upload={handleDescriptionUpload}
								/>
								{#if existingIssueAttachments.length > 0}
									<div class="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm dark:border-slate-700 dark:bg-slate-900">
										<p class="mb-1 font-medium text-slate-700 dark:text-slate-200">{$t('issue.existingAttachments')}</p>
										<div class="space-y-1">
											{#each existingIssueAttachments as attachment (attachment.url)}
												<a
													href={attachment.url}
													target="_blank"
													rel="noreferrer"
													class="block truncate text-blue-600 hover:text-blue-800 dark:text-blue-400 dark:hover:text-blue-300"
												>
													{attachment.isImage ? '🖼️' : '📎'} {attachment.filename}
												</a>
											{/each}
										</div>
									</div>
								{/if}
								<div class="flex gap-2">
									<Button onclick={saveIssueEdit} loading={savingIssue}>{$t('issue.saveChanges')}</Button>
									<Button variant="ghost" onclick={toggleIssueEditMode}>{$t('common.cancel')}</Button>
								</div>
							</div>
						{:else}
							<h1 class="mb-4 text-2xl font-bold text-slate-900 dark:text-slate-100">{issue.title}</h1>
							<div class="rounded-md border border-slate-200 bg-slate-50 p-4 dark:border-slate-700 dark:bg-slate-900">
								{#if issue.description?.trim()}
									<div class="prose prose-sm max-w-none markdown-body text-sm text-slate-700 dark:prose-invert dark:text-slate-300">
										{@html renderMarkdown(issue.description)}
									</div>
								{:else}
									<p class="text-sm text-slate-500 dark:text-slate-400">{$t('issue.noDescriptionPreview')}</p>
								{/if}
							</div>
						{/if}
					</div>

					<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
						<h2 class="mb-4 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('issue.comments')}（{comments.length}）</h2>
						<div class="space-y-4">
							{#if comments.length === 0}
								<p class="text-sm text-slate-500 dark:text-slate-400">{$t('issue.noComments')}</p>
							{:else}
								{#each comments as comment (comment.id)}
									<div class="rounded-lg border border-slate-200 p-4 dark:border-slate-700">
										<div class="mb-2 flex items-start justify-between gap-3">
											<div>
												<p class="text-sm font-medium text-slate-900 dark:text-slate-100">{comment.author_name || comment.user_id}</p>
												<p class="text-xs text-slate-500 dark:text-slate-400">{formatDate(comment.created_at)}</p>
											</div>
											{#if canManageComment(comment)}
												<div class="flex gap-1">
													<Button variant="ghost" size="sm" onclick={() => startEditComment(comment)}>
														{$t('common.edit')}
													</Button>
													<Button
														variant="ghost"
														size="sm"
														onclick={() => handleDeleteComment(comment.id)}
														disabled={commentBusyId === comment.id}
													>
														{$t('common.delete')}
													</Button>
												</div>
											{/if}
										</div>

										{#if editingCommentId === comment.id}
											<div class="space-y-2 relative">
												{#if showEditMentionDropdown && filteredEditMentionMembers.length > 0}
													<div class="absolute bottom-full left-0 z-50 mb-1 max-h-48 w-64 overflow-y-auto rounded-lg border border-slate-200 bg-white shadow-lg dark:shadow-slate-900/50 dark:border-slate-700 dark:bg-slate-800">
														{#each filteredEditMentionMembers as member (member.user_id)}
															<button
																class="w-full px-3 py-2 text-left text-sm hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-700"
																onclick={() => selectEditMention(member)}
															>
																<span class="font-medium inline-flex items-center gap-1">
																	{#if isBotMember(member)}
																		<BotIcon class="h-3.5 w-3.5 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
																	{/if}
																	<span>{member.name}</span>
																</span>
																<span class="ml-1 text-slate-400 dark:text-slate-400">{member.email}</span>
															</button>
														{/each}
													</div>
												{/if}
												<Textarea
													bind:value={editingCommentContent}
													rows={4}
													maxlength={5000}
													oninput={handleEditCommentInput}
												/>
												<div class="flex gap-2">
													<Button
														size="sm"
														onclick={handleUpdateComment}
														disabled={commentBusyId === comment.id}
													>
														{$t('common.save')}
													</Button>
													<Button variant="ghost" size="sm" onclick={cancelEditComment}>{$t('common.cancel')}</Button>
												</div>
											</div>
										{:else}
											<div class="prose prose-sm max-w-none markdown-body text-sm text-slate-700 dark:prose-invert dark:text-slate-300">
												{@html renderCommentContent(comment.content)}
											</div>
										{/if}
									</div>
								{/each}
							{/if}
						</div>
					</div>
				</div>

				<div class="space-y-4 lg:col-span-3">
					<div class="space-y-3 rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
						<Select
							label={$t('common.status')}
							options={statusOptions}
							bind:value={statusValue}
							onchange={handleStatusChange}
							disabled={updatingField !== null}
						/>
						<Select
							label={$t('common.priority')}
							options={priorityOptions}
							bind:value={priorityValue}
							onchange={handlePriorityChange}
							disabled={updatingField !== null}
						/>
						<Select
							label={$t('common.assignee')}
							options={assigneeOptions}
							bind:value={assigneeValue}
							onchange={handleAssigneeChange}
							disabled={updatingField !== null}
						/>
						<div class="space-y-1">
							<p class="text-sm font-medium text-slate-700 dark:text-slate-300">Sprint</p>
							{#if currentSprintName && currentSprintName !== $t('issue.noSprint')}
								<span class="inline-flex items-center rounded-full px-3 py-1 text-sm font-medium bg-indigo-50 text-indigo-700 border border-indigo-200">
									<svg class="mr-1.5 h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 4v16m0-14h10l-2 4 2 4H5"></path>
									</svg>
									{currentSprintName}
								</span>
							{:else}
								<span class="text-sm text-slate-400">{$t('issue.noLinkedSprint')}</span>
							{/if}
						</div>

						<div class="space-y-2">
							<p class="text-sm font-medium text-slate-700 dark:text-slate-300">{$t('issue.labels')}</p>
							<div class="flex flex-wrap gap-2">
								{#if issueLabels.length === 0}
									<span class="text-xs text-slate-500 dark:text-slate-400">{$t('issue.noLabels')}</span>
								{:else}
									{#each issueLabels as label (label.id)}
										<Tag
											label={label.name}
											color={label.color}
											removable
											onremove={() => handleRemoveLabel(label.id)}
										/>
									{/each}
								{/if}
							</div>
							<div class="flex gap-2">
								<div class="flex-1">
									<Select
										options={availableLabelOptions}
										bind:value={selectedLabelId}
										placeholder={$t('issue.selectLabel')}
										disabled={labelSaving || availableLabelOptions.length === 0}
									/>
								</div>
								<Button size="sm" onclick={handleAddLabel} disabled={!selectedLabelId || labelSaving}>
									{$t('common.create')}
								</Button>
							</div>
							<div class="grid grid-cols-1 gap-2">
								<Input bind:value={newLabelName} placeholder={$t('issue.newLabelName')} maxlength={50} />
								<div class="flex gap-2">
									<input
										type="color"
										bind:value={newLabelColor}
										class="h-9 w-12 rounded border border-slate-300 bg-white dark:border-slate-600 dark:bg-slate-800"
									/>
									<Button size="sm" variant="ghost" onclick={handleCreateLabel} loading={creatingLabel}>
										{$t('issue.createLabel')}
									</Button>
								</div>
							</div>
						</div>
					</div>

					<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
						<h3 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('issue.activity')}</h3>
						<div class="space-y-3">
							{#if activities.length === 0}
								<p class="text-xs text-slate-500 dark:text-slate-400">{$t('issue.noActivity')}</p>
							{:else}
								{#each activities as activity, index (activity.id)}
									<div class="flex items-start gap-3 py-2">
										<div class="flex min-h-10 flex-col items-center">
											<div class="h-2 w-2 rounded-full bg-blue-500"></div>
											<div class="w-0.5 flex-1 bg-slate-200 dark:bg-slate-700" class:opacity-0={index === activities.length - 1}></div>
										</div>
										<div>
											<p class="text-sm text-slate-700 dark:text-slate-300">
												<span class="font-medium">{getActivityAuthor(activity)}</span>
												{' '}
												{formatActivity(activity)}
											</p>
											<p class="text-xs text-slate-400 dark:text-slate-400">{formatDate(activity.created_at)}</p>
										</div>
									</div>
								{/each}
							{/if}
						</div>
					</div>
				</div>
			</div>

			<div class="fixed inset-x-0 bottom-0 z-20 border-t border-slate-200 bg-white/95 p-3 backdrop-blur dark:border-slate-700 dark:bg-slate-900/95 md:static md:z-auto md:border-0 md:bg-transparent md:p-0">
				<div class="relative mx-auto max-w-7xl rounded-lg border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-800 md:p-4">
					{#if showMentionDropdown && filteredMentionMembers.length > 0}
						<div class="absolute bottom-full left-3 z-50 mb-1 max-h-48 w-64 overflow-y-auto rounded-lg border border-slate-200 bg-white shadow-lg dark:shadow-slate-900/50 dark:border-slate-700 dark:bg-slate-800">
							{#each filteredMentionMembers as member (member.user_id)}
								<button
									class="w-full px-3 py-2 text-left text-sm hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-700"
									onclick={() => selectMention(member)}
								>
									<span class="font-medium inline-flex items-center gap-1">
										{#if isBotMember(member)}
											<BotIcon class="h-3.5 w-3.5 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
										{/if}
										<span>{member.name}</span>
									</span>
									<span class="ml-1 text-slate-400 dark:text-slate-400">{member.email}</span>
								</button>
							{/each}
						</div>
					{/if}
					<Textarea
						label={$t('issue.addComment')}
						bind:value={newComment}
						rows={3}
						maxlength={5000}
						placeholder={$t('issue.commentPlaceholder')}
						oninput={handleNewCommentInput}
						onpaste={handleCommentPaste}
					/>
					<div class="mt-2 flex items-start justify-between gap-3">
						<div class="min-w-0 flex-1">
							<FileUpload
								compact
								bind:value={commentAttachments}
								accept={ISSUE_ATTACHMENT_ACCEPT}
								maxSize={ISSUE_ATTACHMENT_MAX_SIZE_BYTES}
								multiple
								on:upload={handleCommentUpload}
							/>
						</div>
						<Button
							onclick={handleCreateComment}
							disabled={!newComment.trim() && commentAttachments.length === 0}
							loading={commentSubmitting}
						>
							{$t('issue.submitComment')}
						</Button>
					</div>
				</div>
			</div>
		</div>
	{/if}
</div>

<style>
	:global(.markdown-body h1),
	:global(.markdown-body h2),
	:global(.markdown-body h3) {
		font-weight: 600;
		margin-top: 0.75rem;
		margin-bottom: 0.5rem;
	}

	:global(.markdown-body p) {
		margin: 0.5rem 0;
	}

	:global(.markdown-body img) {
		max-width: 100%;
		height: auto;
		border-radius: 0.375rem;
		display: block;
		margin: 0.5rem 0;
	}

	:global(.markdown-body pre) {
		border-radius: 0.375rem;
		padding: 0.75rem;
		background: rgb(15 23 42);
		color: rgb(248 250 252);
		overflow: auto;
	}

	:global(.markdown-body code) {
		font-size: 0.85em;
	}

	:global(.markdown-body ul),
	:global(.markdown-body ol) {
		margin: 0.5rem 0;
		padding-left: 1.25rem;
	}

	:global(.markdown-body .comment-mention) {
		color: rgb(37 99 235);
		font-weight: 600;
	}
</style>

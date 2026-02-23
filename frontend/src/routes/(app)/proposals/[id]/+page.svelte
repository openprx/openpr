<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { authStore } from '$lib/stores/auth';
	import { apiClient } from '$lib/api/client';
	import { membersApi, type WorkspaceMember } from '$lib/api/members';
	import {
		proposalsApi,
		type Proposal,
		type ProposalComment,
		type ProposalStatus,
		type ProposalType,
		type VoteChoice,
		type ProposalIssueSearchResult
	} from '$lib/api/proposals';
	import {
		impactReviewsApi,
		type ImpactReviewDetail,
		type ReviewRating,
		type ReviewStatus
	} from '$lib/api/impact-reviews';
	import { toast } from '$lib/stores/toast';
	import { renderMarkdown } from '$lib/utils/markdown';

	type MentionCandidate = {
		id: string;
		name: string;
		avatar?: string;
	};

	const proposalId = $derived($page.params.id || '');

	let loading = $state(true);
	let proposal = $state<Proposal | null>(null);
	let decisionId = $state('');
	let tally = $state({ yes: 0, no: 0, abstain: 0 });
	let comments = $state<ProposalComment[]>([]);
	let links = $state<Array<{ issue_id: string; issue_title: string; issue_state: string }>>([]);
	let newComment = $state('');
	let newCommentMode = $state<'edit' | 'preview'>('edit');
	let voteReason = $state('');
	let issueSearch = $state('');
	let issueResults = $state<ProposalIssueSearchResult[]>([]);
	let searchingIssues = $state(false);
	let nowTs = $state(Date.now());
	let impactReviewDetail = $state<ImpactReviewDetail | null>(null);
	let loadingImpactReview = $state(false);
	let impactRating = $state<ReviewRating | ''>('');
	let impactStatus = $state<ReviewStatus>('collecting');
	let impactAchievements = $state('');
	let impactLessons = $state('');
	let impactGoalAchievementsText = $state('[]');
	let impactSaving = $state(false);
	let commentTextareaEl = $state<HTMLTextAreaElement | null>(null);
	let showMentionDropdown = $state(false);
	let mentionFilter = $state('');
	let mentionAnchorIndex = $state<number | null>(null);
	let uploadingCommentImage = $state(false);
	let workspaceMembers = $state<WorkspaceMember[]>([]);

	const proposalPageTitle = $derived.by(() => {
		if (proposal?.title?.trim()) {
			return $t('pageTitle.proposalDetail', { values: { title: proposal.title.trim() } });
		}
		return $t('pageTitle.proposalDetailFallback');
	});

	const workspaceId = $derived.by(() => {
		if ($page.params.workspaceId) {
			return $page.params.workspaceId;
		}
		const fromQuery = $page.url.searchParams.get('workspaceId') || $page.url.searchParams.get('workspace_id');
		if (fromQuery) {
			return fromQuery;
		}
		const fromPath = $page.url.pathname.match(/^\/workspace\/([^/]+)/);
		return fromPath?.[1] ?? '';
	});

	onMount(() => {
		void loadAll();
		void loadWorkspaceMembers();
		const timer = window.setInterval(() => {
			nowTs = Date.now();
		}, 1000);
		return () => window.clearInterval(timer);
	});

	async function loadWorkspaceMembers() {
		try {
			const response = await membersApi.searchUsers('_', { limit: 50 });
			if (response.code === 0 && response.data?.items) {
				workspaceMembers = response.data.items.map((u: { id: string; email: string; name: string }) => ({
					user_id: u.id,
					email: u.email,
					name: u.name || u.email,
					role: 'member' as const,
					joined_at: ''
				}));
				return;
			}
		} catch (_) { /* fallback below */ }
		workspaceMembers = [];
	}

	async function loadAll() {
		if (!proposalId) {
			goto('/proposals');
			return;
		}
		loading = true;
		const [detailRes, commentsRes, linksRes] = await Promise.all([
			proposalsApi.get(proposalId),
			proposalsApi.listComments(proposalId),
			proposalsApi.listIssues(proposalId)
		]);

		if (detailRes.code !== 0 || !detailRes.data) {
			toast.error(detailRes.message || get(t)('governance.notFound'));
			goto('/proposals');
			return;
		}

		proposal = detailRes.data.proposal;
		decisionId = detailRes.data.decision_id || '';
		tally = detailRes.data.tally;
		comments = commentsRes.data?.items ?? [];
		links = (linksRes.data?.items ?? []) as Array<{ issue_id: string; issue_title: string; issue_state: string }>;
		await loadImpactReview();
		loading = false;
	}

	function syncImpactForm() {
		impactRating = impactReviewDetail?.review.rating || '';
		impactStatus = impactReviewDetail?.review.status || 'collecting';
		impactAchievements = impactReviewDetail?.review.achievements || '';
		impactLessons = impactReviewDetail?.review.lessons || '';
		impactGoalAchievementsText = JSON.stringify(impactReviewDetail?.review.goal_achievements || [], null, 2);
	}

	async function loadImpactReview() {
		if (!proposalId) {
			impactReviewDetail = null;
			return;
		}
		loadingImpactReview = true;
		const res = await impactReviewsApi.getByProposal(proposalId);
		loadingImpactReview = false;
		if (res.code === 0 && res.data) {
			impactReviewDetail = res.data;
			syncImpactForm();
			return;
		}
		impactReviewDetail = null;
	}

	const meId = $derived($authStore.user?.id || '');
	const meName = $derived($authStore.user?.name || '');
	const meAvatar = $derived($authStore.user?.avatar_url || '');
	const isAuthor = $derived(Boolean(proposal && meId && proposal.author_id === meId));

	const authorMetaMap = $derived.by<Record<string, { name: string; avatar?: string }>>(() => {
		const map: Record<string, { name: string; avatar?: string }> = {};
		for (const comment of comments) {
			if (!comment.author_id) {
				continue;
			}
			map[comment.author_id] = {
				name: comment.author_name || comment.author_id.slice(0, 8),
				avatar: comment.author_avatar
			};
		}
		if (meId) {
			map[meId] = {
				name: meName || map[meId]?.name || meId.slice(0, 8),
				avatar: meAvatar || map[meId]?.avatar
			};
		}
		if (proposal?.author_id && !map[proposal.author_id]) {
			map[proposal.author_id] = {
				name: proposal.author_id.slice(0, 8)
			};
		}
		return map;
	});

	const proposalAuthorName = $derived.by(() => {
		if (!proposal) {
			return '';
		}
		return authorMetaMap[proposal.author_id]?.name || proposal.author_id.slice(0, 8);
	});

	const mentionCandidates = $derived.by<MentionCandidate[]>(() => {
		return workspaceMembers
			.filter((member) => Boolean(member.user_id) && member.user_id !== meId)
			.map((member) => ({
				id: member.user_id,
				name: member.name || member.email || member.user_id.slice(0, 8)
			}))
			.slice(0, 20);
	});

	const filteredMentionCandidates = $derived.by(() => {
		const keyword = mentionFilter.trim().toLowerCase();
		return mentionCandidates
			.filter((member) => !keyword || member.name.toLowerCase().includes(keyword))
			.slice(0, 8);
	});

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
			if (!content.includes(marker) || seen.has(member.id)) {
				continue;
			}
			seen.add(member.id);
			ids.push(member.id);
		}
		return ids;
	}

	function handleNewCommentInput(event: Event) {
		const textarea = event.target as HTMLTextAreaElement;
		commentTextareaEl = textarea;
		const mentionState = parseMentionAtCursor(textarea.value, textarea.selectionStart ?? 0);
		mentionAnchorIndex = mentionState.start;
		mentionFilter = mentionState.filter;
		showMentionDropdown = mentionState.start !== null;
	}

	async function selectMention(member: MentionCandidate) {
		if (!commentTextareaEl) {
			return;
		}
		const cursorPos = commentTextareaEl.selectionStart ?? newComment.length;
		const atStart = mentionAnchorIndex ?? newComment.lastIndexOf('@', Math.max(0, cursorPos - 1));
		if (atStart < 0) {
			return;
		}
		const before = newComment.slice(0, atStart);
		const after = newComment.slice(cursorPos);
		const mentionText = `@${member.name} `;
		newComment = `${before}${mentionText}${after}`;
		const nextCursorPos = before.length + mentionText.length;
		await tick();
		commentTextareaEl?.focus();
		commentTextareaEl?.setSelectionRange(nextCursorPos, nextCursorPos);
		showMentionDropdown = false;
		mentionFilter = '';
		mentionAnchorIndex = null;
	}

	function isAllowedImage(type: string): boolean {
		return ['image/png', 'image/jpeg', 'image/jpg', 'image/gif', 'image/webp'].includes(type);
	}

	function appendCommentImage(url: string): void {
		const markdown = `![image](${url})`;
		if (!commentTextareaEl) {
			const suffix = newComment.trim().length > 0 ? '\n\n' : '';
			newComment = `${newComment}${suffix}${markdown}`;
			return;
		}

		const current = newComment;
		const start = commentTextareaEl.selectionStart ?? current.length;
		const end = commentTextareaEl.selectionEnd ?? start;
		newComment = `${current.slice(0, start)}${markdown}${current.slice(end)}`;

		void tick().then(() => {
			if (!commentTextareaEl) {
				return;
			}
			const cursor = start + markdown.length;
			commentTextareaEl.selectionStart = cursor;
			commentTextareaEl.selectionEnd = cursor;
			commentTextareaEl.focus();
		});
	}

	async function uploadCommentImage(file: File): Promise<void> {
		if (!isAllowedImage(file.type)) {
			toast.error(get(t)('toast.uploadTypeFail'));
			return;
		}
		if (file.size > 10 * 1024 * 1024) {
			toast.error(get(t)('toast.uploadSizeFail'));
			return;
		}

		uploadingCommentImage = true;
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
				appendCommentImage(result.data.url);
				toast.success(get(t)('toast.uploadSuccess'));
			} else {
				toast.error(result.message || get(t)('toast.uploadFail'));
			}
		} catch {
			toast.error(get(t)('toast.uploadNetworkFail'));
		} finally {
			uploadingCommentImage = false;
		}
	}

	function handleCommentPaste(event: ClipboardEvent): void {
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
			void uploadCommentImage(file);
			return;
		}
	}

	async function handleCommentImageSelect(event: Event): Promise<void> {
		const input = event.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) {
			return;
		}
		await uploadCommentImage(file);
		input.value = '';
	}

	function commentAuthorName(comment: ProposalComment): string {
		return comment.author_name || authorMetaMap[comment.author_id]?.name || comment.author_id.slice(0, 8);
	}

	function commentAvatarText(comment: ProposalComment): string {
		const name = commentAuthorName(comment);
		return name.charAt(0).toUpperCase() || '?';
	}

	function renderCommentContent(content: string): string {
		const rendered = renderMarkdown(content);
		return rendered.replace(
			/(^|[\s>])@([^\s@<]+)/g,
			(_matched, prefix: string, mention: string) =>
				`${prefix}<span class="comment-mention">@${mention}</span>`
		);
	}

	async function actSubmit() {
		if (!proposal) return;
		const res = await proposalsApi.submit(proposal.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.submitFailed'));
			return;
		}
		toast.success(get(t)('governance.discussionSubmitted'));
		await loadAll();
	}

	async function actStartVoting() {
		if (!proposal) return;
		const res = await proposalsApi.startVoting(proposal.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.startVotingFailed'));
			return;
		}
		toast.success(get(t)('governance.votingStarted'));
		await loadAll();
	}

	async function actArchive() {
		if (!proposal) return;
		const res = await proposalsApi.archive(proposal.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.archiveFailed'));
			return;
		}
		toast.success(get(t)('governance.archived'));
		await loadAll();
	}

	async function actDeleteDraft() {
		if (!proposal) return;
		const yes = window.confirm(get(t)('governance.draftDeleteConfirm'));
		if (!yes) return;
		const res = await proposalsApi.delete(proposal.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.deleteFailed'));
			return;
		}
		toast.success(get(t)('governance.draftDeleted'));
		goto('/proposals');
	}

	async function vote(choice: VoteChoice) {
		if (!proposal) return;
		const res = await proposalsApi.vote(proposal.id, choice, voteReason || undefined);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.voteFailed'));
			return;
		}
		toast.success(get(t)('governance.voteSuccess'));
		voteReason = '';
		await loadAll();
	}

	async function withdrawVote() {
		if (!proposal) return;
		const res = await proposalsApi.withdrawVote(proposal.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.withdrawVoteFailed'));
			return;
		}
		toast.success(get(t)('governance.voteWithdrawn'));
		await loadAll();
	}

	async function addComment() {
		if (!proposal || !newComment.trim()) return;
		const mentionIds = extractMentionIds(newComment);
		const res = await proposalsApi.comment(
			proposal.id,
			newComment.trim(),
			'general',
			mentionIds.length > 0 ? mentionIds : undefined
		);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.commentAddFailed'));
			return;
		}
		newComment = '';
		showMentionDropdown = false;
		mentionFilter = '';
		mentionAnchorIndex = null;
		toast.success(get(t)('governance.commentAdded'));
		await loadAll();
	}

	async function searchIssues() {
		const q = issueSearch.trim();
		if (!q) {
			issueResults = [];
			return;
		}
		searchingIssues = true;
		const res = await proposalsApi.searchIssues(q);
		searchingIssues = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governance.issueSearchFailed'));
			return;
		}
		issueResults = res.data;
	}

	async function linkIssue(issueId: string) {
		if (!proposal) return;
		const res = await proposalsApi.linkIssue(proposal.id, issueId);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governance.issueLinkFailed'));
			return;
		}
		toast.success(get(t)('governance.issueLinkSuccess'));
		issueResults = [];
		issueSearch = '';
		await loadAll();
	}

	function statusTone(status: ProposalStatus): string {
		switch (status) {
			case 'draft':
				return 'bg-slate-100 text-slate-700';
			case 'open':
				return 'bg-blue-100 text-blue-700';
			case 'voting':
				return 'bg-amber-100 text-amber-700';
			case 'approved':
				return 'bg-emerald-100 text-emerald-700';
			case 'rejected':
				return 'bg-red-100 text-red-700';
			case 'vetoed':
				return 'bg-rose-100 text-rose-700';
			case 'archived':
				return 'bg-slate-200 text-slate-700';
		}
	}

	function statusLabel(status: ProposalStatus): string {
		return get(t)(`governance.status.${status}`);
	}

	function typeLabel(type: ProposalType): string {
		return get(t)(`governance.type.${type}`);
	}

	function discussionHours(cycle: Proposal['cycle_template']): number {
		switch (cycle) {
			case 'rapid':
				return 1;
			case 'standard':
				return 72;
			case 'critical':
				return 168;
			default:
				return 24;
		}
	}

	function getDeadline(value: Proposal): { label: string; at?: string } {
		if (value.status === 'voting') {
			return { label: get(t)('governance.votingDeadlineLabel'), at: value.voting_ended_at };
		}
		if (value.status === 'open' && value.submitted_at) {
			const base = new Date(value.submitted_at).getTime();
			if (Number.isFinite(base)) {
				return {
					label: get(t)('governance.discussionDeadlineLabel'),
					at: new Date(base + discussionHours(value.cycle_template) * 3600 * 1000).toISOString()
				};
			}
		}
		return { label: '', at: undefined };
	}

	function countdownText(target?: string): string {
		if (!target) return '-';
		const end = new Date(target).getTime();
		if (!Number.isFinite(end)) return '-';
		const diff = end - nowTs;
		if (diff <= 0) return get(t)('governance.countdown.ended');
		const totalSeconds = Math.floor(diff / 1000);
		const days = Math.floor(totalSeconds / 86400);
		const hours = Math.floor((totalSeconds % 86400) / 3600);
		const minutes = Math.floor((totalSeconds % 3600) / 60);
		const chunks: string[] = [];
		if (days > 0) chunks.push(`${days}d`);
		if (hours > 0 || days > 0) chunks.push(`${hours}h`);
		chunks.push(`${minutes}m`);
		return get(t)('governance.countdown.remaining', { values: { time: chunks.join(' ') } });
	}

	const progressStages = ['draft', 'open', 'voting', 'final'] as const;

	function stageIndex(status: ProposalStatus): number {
		switch (status) {
			case 'draft':
				return 0;
			case 'open':
				return 1;
			case 'voting':
				return 2;
			default:
				return 3;
		}
	}

	function stageLabel(stage: (typeof progressStages)[number], status: ProposalStatus): string {
		if (stage === 'final') {
			return status === 'approved'
				? get(t)('governance.progress.approved')
				: get(t)('governance.progress.rejected');
		}
		return get(t)(`governance.progress.${stage}`);
	}

	async function createImpactReview() {
		if (!proposal) return;
		const res = await impactReviewsApi.createForProposal(proposal.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('impactReview.createFailed'));
			return;
		}
		toast.success(get(t)('impactReview.createSuccess'));
		await loadImpactReview();
	}

	async function saveImpactReview() {
		if (!proposal || !impactReviewDetail) return;
		let parsedGoalAchievements: unknown = [];
		try {
			parsedGoalAchievements = JSON.parse(impactGoalAchievementsText || '[]');
		} catch {
			toast.error(get(t)('impactReview.goalAchievementsInvalid'));
			return;
		}

		impactSaving = true;
		const res = await impactReviewsApi.updateByProposal(proposal.id, {
			status: impactStatus,
			rating: impactRating || undefined,
			goal_achievements: parsedGoalAchievements,
			achievements: impactAchievements.trim() || undefined,
			lessons: impactLessons.trim() || undefined
		});
		impactSaving = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('impactReview.updateFailed'));
			return;
		}
		impactReviewDetail = res.data;
		syncImpactForm();
		toast.success(get(t)('impactReview.updateSuccess'));
	}
</script>

<svelte:head>
	<title>{proposalPageTitle}</title>
</svelte:head>

{#if loading}
	<div class="mx-auto max-w-6xl rounded-lg border border-slate-200 bg-white p-8 text-center text-slate-500 dark:border-slate-700 dark:bg-slate-800">
		{$t('governance.loading')}
	</div>
{:else if proposal}
	<div class="mx-auto grid max-w-6xl grid-cols-1 gap-4 lg:grid-cols-3">
		<div class="space-y-4 lg:col-span-2">
			<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
				<div class="mb-3 flex items-start justify-between gap-3">
					<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{proposal.title}</h1>
					<span class={`rounded px-2 py-1 text-xs font-medium ${statusTone(proposal.status)}`}>{statusLabel(proposal.status)}</span>
				</div>
				<div class="mb-4 text-sm text-slate-500">
					{typeLabel(proposal.proposal_type)} · {proposalAuthorName} · {new Date(proposal.created_at).toLocaleString()}
				</div>
				<div class="mb-4 space-y-3">
					<div class="grid grid-cols-4 gap-2">
						{#each progressStages as stage, idx}
							<div class="space-y-1">
								<div class={`h-1.5 rounded ${idx <= stageIndex(proposal.status) ? 'bg-blue-600' : 'bg-slate-200 dark:bg-slate-700'}`}></div>
								<p class={`text-xs ${idx === stageIndex(proposal.status) ? 'font-medium text-blue-700 dark:text-blue-300' : 'text-slate-500'}`}>
									{stageLabel(stage, proposal.status)}
								</p>
							</div>
						{/each}
					</div>
					{#if getDeadline(proposal).at}
						<div class="text-xs text-slate-500">
							<p>{getDeadline(proposal).label}: {countdownText(getDeadline(proposal).at)}</p>
							<p>{$t('governance.deadlineAtLabel')}: {new Date(getDeadline(proposal).at ?? '').toLocaleString()}</p>
						</div>
					{/if}
				</div>
				<div class="prose prose-slate max-w-none dark:prose-invert">
					{@html renderMarkdown(proposal.content)}
				</div>
			</div>

			<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
				<h2 class="mb-3 text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('governance.discussion')}</h2>
				<div class="space-y-3">
					{#if comments.length === 0}
						<p class="text-sm text-slate-500">{$t('governance.noComments')}</p>
					{:else}
						{#each comments as comment}
							<div class="rounded-md border border-slate-200 p-3 dark:border-slate-600">
								<div class="mb-2 flex items-start gap-3">
									{#if comment.author_avatar}
										<img src={comment.author_avatar} alt={commentAuthorName(comment)} class="h-8 w-8 rounded-full object-cover" />
									{:else}
										<div class="flex h-8 w-8 items-center justify-center rounded-full bg-slate-200 text-xs font-semibold text-slate-700 dark:bg-slate-700 dark:text-slate-100">
											{commentAvatarText(comment)}
										</div>
									{/if}
									<div>
										<div class="text-xs text-slate-500">{commentAuthorName(comment)} · {comment.comment_type} · {new Date(comment.created_at).toLocaleString()}</div>
									</div>
								</div>
								<div class="prose prose-sm max-w-none markdown-body text-sm text-slate-700 dark:prose-invert dark:text-slate-300">
									{@html renderCommentContent(comment.content)}
								</div>
							</div>
						{/each}
					{/if}
				</div>
				<div class="relative mt-3 space-y-2">
					{#if showMentionDropdown && filteredMentionCandidates.length > 0}
						<div class="absolute bottom-full left-0 z-20 mb-1 max-h-48 w-64 overflow-y-auto rounded-lg border border-slate-200 bg-white shadow-lg dark:border-slate-700 dark:bg-slate-800">
							{#each filteredMentionCandidates as member (member.id)}
								<button type="button" class="w-full px-3 py-2 text-left text-sm hover:bg-slate-100 dark:hover:bg-slate-700" onclick={() => selectMention(member)}>
									{member.name}
								</button>
							{/each}
						</div>
					{/if}
					<div class="mb-1 inline-flex overflow-hidden rounded-md border border-slate-300 text-xs dark:border-slate-600">
						<button type="button" class={`px-2 py-1 ${newCommentMode === 'edit' ? 'bg-blue-600 text-white' : 'bg-white text-slate-600 dark:bg-slate-900 dark:text-slate-300'}`} onclick={() => (newCommentMode = 'edit')}>
							{$t('issue.editMode')}
						</button>
						<button type="button" class={`px-2 py-1 ${newCommentMode === 'preview' ? 'bg-blue-600 text-white' : 'bg-white text-slate-600 dark:bg-slate-900 dark:text-slate-300'}`} onclick={() => (newCommentMode = 'preview')}>
							{$t('issue.previewMode')}
						</button>
					</div>
					{#if newCommentMode === 'edit'}
						<textarea
							bind:value={newComment}
							bind:this={commentTextareaEl}
							rows="4"
							placeholder={$t('governance.commentPlaceholder')}
							class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"
							oninput={handleNewCommentInput}
							onpaste={handleCommentPaste}
						></textarea>
					{:else}
						<div class="min-h-24 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 dark:border-slate-700 dark:bg-slate-900">
							{#if newComment.trim()}
								<div class="prose prose-sm max-w-none markdown-body text-sm text-slate-700 dark:prose-invert dark:text-slate-300">
									{@html renderMarkdown(newComment)}
								</div>
							{:else}
								<p class="text-sm text-slate-400">{$t('issue.noDescriptionPreview')}</p>
							{/if}
						</div>
					{/if}
					<div class="flex items-center justify-between">
						<label class="inline-flex cursor-pointer items-center gap-1 text-sm text-blue-600 hover:text-blue-800 dark:text-blue-400 dark:hover:text-blue-300">
							<span>{uploadingCommentImage ? $t('search.searching') : $t('issue.uploadImage')}</span>
							<input type="file" accept="image/png,image/jpeg,image/gif,image/webp" class="hidden" onchange={handleCommentImageSelect} disabled={uploadingCommentImage} />
						</label>
						<button type="button" onclick={addComment} class="rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700" disabled={!newComment.trim()}>
							{$t('governance.commentSubmit')}
						</button>
					</div>
				</div>
			</div>

			<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
				<div class="mb-3 flex items-center justify-between gap-3">
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('impactReview.title')}</h2>
					<a href={`/proposals/${proposal.id}/review`} class="text-sm text-blue-600 hover:underline">
						{$t('impactReview.detailLink')}
					</a>
				</div>
				{#if loadingImpactReview}
					<p class="text-sm text-slate-500">{$t('common.loading')}</p>
				{:else if !impactReviewDetail}
					<p class="text-sm text-slate-500">{$t('impactReview.notCreated')}</p>
					{#if proposal.status === 'approved'}
						<button type="button" onclick={createImpactReview} class="mt-3 rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700">
							{$t('impactReview.create')}
						</button>
					{/if}
				{:else}
					<div class="mb-3 grid grid-cols-1 gap-2 text-xs text-slate-500 md:grid-cols-4">
						<p>{$t('impactReview.reviewId')}: {impactReviewDetail.review.id}</p>
						<p>{$t('impactReview.scheduledAt')}: {impactReviewDetail.review.scheduled_at ? new Date(impactReviewDetail.review.scheduled_at).toLocaleString() : '-'}</p>
						<p>{$t('impactReview.conductedAt')}: {impactReviewDetail.review.conducted_at ? new Date(impactReviewDetail.review.conducted_at).toLocaleString() : '-'}</p>
						<p>{$t('impactReview.trustApplied')}: {impactReviewDetail.review.trust_score_applied ? $t('impactReview.yes') : $t('impactReview.no')}</p>
					</div>
					<div class="space-y-3">
						<div class="grid grid-cols-1 gap-3 md:grid-cols-2">
							<div>
								<label for="proposal-impact-status" class="mb-1 block text-sm text-slate-600">{$t('common.status')}</label>
								<select id="proposal-impact-status" bind:value={impactStatus} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
									<option value="pending">{$t('impactReview.status.pending')}</option>
									<option value="collecting">{$t('impactReview.status.collecting')}</option>
									<option value="completed">{$t('impactReview.status.completed')}</option>
									<option value="skipped">{$t('impactReview.status.skipped')}</option>
								</select>
							</div>
							<div>
								<label for="proposal-impact-rating" class="mb-1 block text-sm text-slate-600">{$t('impactReview.rating')}</label>
								<select id="proposal-impact-rating" bind:value={impactRating} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
									<option value="">{`- ${$t('impactReview.ratingAll')} -`}</option>
									<option value="S">S</option>
									<option value="A">A</option>
									<option value="B">B</option>
									<option value="C">C</option>
									<option value="F">F</option>
								</select>
							</div>
						</div>
						<div>
							<label for="proposal-impact-goal-achievements" class="mb-1 block text-sm text-slate-600">{$t('impactReview.goalAchievements')}</label>
							<textarea id="proposal-impact-goal-achievements" bind:value={impactGoalAchievementsText} rows="5" class="w-full rounded-md border border-slate-300 px-3 py-2 font-mono text-xs dark:border-slate-600 dark:bg-slate-900"></textarea>
						</div>
						<div>
							<label for="proposal-impact-achievements" class="mb-1 block text-sm text-slate-600">{$t('impactReview.achievements')}</label>
							<textarea id="proposal-impact-achievements" bind:value={impactAchievements} rows="3" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"></textarea>
						</div>
						<div>
							<label for="proposal-impact-lessons" class="mb-1 block text-sm text-slate-600">{$t('impactReview.lessons')}</label>
							<textarea id="proposal-impact-lessons" bind:value={impactLessons} rows="3" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900"></textarea>
						</div>
						<div class="flex justify-end">
							<button type="button" onclick={saveImpactReview} class="rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700" disabled={impactSaving}>
								{impactSaving ? $t('common.saving') : $t('common.save')}
							</button>
						</div>
					</div>
				{/if}
			</div>
		</div>

		<div class="space-y-4">
			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h3 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governance.actions')}</h3>
				<div class="space-y-2">
					{#if proposal.status === 'draft' && isAuthor}
						<a href={`/proposals/${proposal.id}/edit`} class="block w-full rounded-md border border-slate-300 px-3 py-2 text-center text-sm hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900">{$t('governance.editDraft')}</a>
						<button type="button" onclick={actDeleteDraft} class="w-full rounded-md bg-red-600 px-3 py-2 text-sm text-white hover:bg-red-700">{$t('governance.deleteDraft')}</button>
						<button type="button" onclick={actSubmit} class="w-full rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700">{$t('governance.submitDiscussion')}</button>
					{/if}
					{#if proposal.status === 'open' && isAuthor}
						<button type="button" onclick={actStartVoting} class="w-full rounded-md bg-amber-600 px-3 py-2 text-sm text-white hover:bg-amber-700">{$t('governance.startVoting')}</button>
					{/if}
					{#if proposal.status === 'voting'}
						<p class="text-xs text-slate-500">{$t('governance.votingInProgress')}</p>
					{/if}
					{#if (proposal.status === 'approved' || proposal.status === 'rejected' || proposal.status === 'vetoed') && isAuthor}
						<button type="button" onclick={actArchive} class="w-full rounded-md bg-slate-700 px-3 py-2 text-sm text-white hover:bg-slate-800">{$t('governance.archiveAction')}</button>
					{/if}
					{#if proposal.status === 'archived'}
						<p class="text-xs text-slate-500">{$t('governance.archivedStatus')}</p>
					{/if}
				</div>
			</div>

			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h3 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governance.voting')}</h3>
				<div class="mb-2 grid grid-cols-3 gap-2 text-xs">
					<div class="rounded bg-emerald-50 px-2 py-1 text-emerald-700">{$t('governance.voteChoice.yes')}: {tally.yes}</div>
					<div class="rounded bg-red-50 px-2 py-1 text-red-700">{$t('governance.voteChoice.no')}: {tally.no}</div>
					<div class="rounded bg-slate-100 px-2 py-1 text-slate-700">{$t('governance.voteChoice.abstain')}: {tally.abstain}</div>
				</div>
				{#if proposal.status === 'voting'}
					<textarea bind:value={voteReason} rows="2" placeholder={$t('governance.voteReasonPlaceholder')} class="mb-2 w-full rounded-md border border-slate-300 px-2 py-1 text-sm dark:border-slate-600 dark:bg-slate-900"></textarea>
					<div class="grid grid-cols-3 gap-2">
						<button type="button" onclick={() => vote('yes')} class="rounded bg-emerald-600 px-2 py-2 text-xs text-white hover:bg-emerald-700">{$t('governance.voteButton.yes')}</button>
						<button type="button" onclick={() => vote('no')} class="rounded bg-red-600 px-2 py-2 text-xs text-white hover:bg-red-700">{$t('governance.voteButton.no')}</button>
						<button type="button" onclick={() => vote('abstain')} class="rounded bg-slate-600 px-2 py-2 text-xs text-white hover:bg-slate-700">{$t('governance.voteButton.abstain')}</button>
					</div>
					<button type="button" onclick={withdrawVote} class="mt-2 w-full rounded border border-slate-300 px-2 py-2 text-xs text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-900">{$t('governance.withdrawVote')}</button>
				{:else}
					<p class="text-xs text-slate-500">{$t('governance.votingOnlyHint')}</p>
				{/if}
			</div>

			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h3 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governance.issueLinks')}</h3>
				<div class="space-y-2">
					{#if links.length === 0}
						<p class="text-xs text-slate-500">{$t('governance.noIssueLinks')}</p>
					{:else}
						{#each links as link}
							<div class="rounded border border-slate-200 px-2 py-1 text-xs dark:border-slate-600">
								#{link.issue_id.slice(0, 8)} · {link.issue_title} · {link.issue_state}
							</div>
						{/each}
					{/if}
				</div>
				{#if proposal.status === 'approved' && isAuthor}
					<div class="mt-2 space-y-2">
						<div class="flex gap-2">
							<input bind:value={issueSearch} onkeydown={(e) => e.key === 'Enter' && searchIssues()} placeholder={$t('governance.searchIssuePlaceholder')} class="w-full rounded-md border border-slate-300 px-2 py-1 text-xs dark:border-slate-600 dark:bg-slate-900" />
							<button type="button" onclick={searchIssues} class="rounded-md bg-blue-600 px-3 py-1 text-xs text-white hover:bg-blue-700">{$t('common.search')}</button>
						</div>
						{#if searchingIssues}
							<p class="text-xs text-slate-500">{$t('governance.searchingIssues')}</p>
						{:else if issueResults.length > 0}
							<div class="space-y-1">
								{#each issueResults as issue}
									<div class="flex items-center justify-between rounded border border-slate-200 px-2 py-1 text-xs dark:border-slate-600">
										<span class="truncate pr-2">{issue.title} · {issue.state}</span>
										<button type="button" onclick={() => linkIssue(issue.id)} class="rounded bg-slate-800 px-2 py-1 text-white hover:bg-slate-900">{$t('governance.select')}</button>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				{/if}
			</div>

			<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-800">
				<h3 class="mb-2 text-sm font-semibold text-slate-900 dark:text-slate-100">{$t('governance.decisions.title')}</h3>
				{#if decisionId}
					<p class="text-xs text-slate-600 dark:text-slate-300">{$t('governance.decisions.linkedDecision', { values: { id: decisionId } })}</p>
				{:else}
					<p class="text-xs text-slate-500">{$t('governance.decisions.none')}</p>
				{/if}
				<div class="mt-3 flex flex-wrap gap-2">
					<a
						href={`/proposals/${proposal.id}/review`}
						class="inline-flex rounded-md border border-slate-300 px-3 py-1.5 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900"
					>
						{$t('impactReview.detailLink')}
					</a>
					<a
						href={`/proposals/${proposal.id}/veto`}
						class="inline-flex rounded-md border border-slate-300 px-3 py-1.5 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-900"
					>
						{$t('vetoDetail.title')}
					</a>
				</div>
				<a href={`/proposals/${proposal.id}/chain`} class="mt-3 inline-block text-xs text-blue-600 hover:underline">
					{$t('governanceExt.viewChain')}
				</a>
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

	:global(.markdown-body .comment-mention) {
		color: #2563eb;
		font-weight: 600;
	}
</style>

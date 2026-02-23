<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onDestroy, tick } from 'svelte';
	import { searchApi, type SearchResult } from '$lib/api/search';
	import { toast } from '$lib/stores/toast';
	import { previewText } from '$lib/utils/search';
	import { t } from 'svelte-i18n';

	interface Props {
		open?: boolean;
	}

	let { open = $bindable(false) }: Props = $props();

	let query = $state('');
	let results = $state<SearchResult[]>([]);
	let loading = $state(false);
	let selectedIndex = $state(0);
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let inputEl = $state<HTMLInputElement | null>(null);

	function clearPendingSearch() {
		if (debounceTimer) {
			clearTimeout(debounceTimer);
			debounceTimer = null;
		}
	}

	async function runSearch(keyword: string) {
		loading = true;
		const response = await searchApi.search(keyword);
		if (response.code === 0 && response.data) {
			results = response.data.results ?? [];
		} else {
			results = [];
		}
		selectedIndex = 0;
		loading = false;
	}

	function handleInput() {
		clearPendingSearch();
		const keyword = query.trim();

		if (!keyword) {
			results = [];
			selectedIndex = 0;
			loading = false;
			return;
		}

		debounceTimer = setTimeout(() => {
			void runSearch(keyword);
		}, 300);
	}

	function getItemTitle(item: SearchResult): string {
		if (item.type === 'project') {
			return item.name;
		}
		if (item.type === 'comment') {
			return $t('issue.comments');
		}
		return item.title;
	}

	function getItemPreview(item: SearchResult): string {
		if (item.type === 'comment') {
			return previewText(item.body, 100);
		}
		return previewText(item.description, 100);
	}

	function normalizeId(value: unknown): string {
		if (typeof value !== 'string') {
			return '';
		}
		const next = value.trim();
		if (!next || next === 'undefined' || next === 'null') {
			return '';
		}
		return next;
	}

	function getItemHref(item: SearchResult): string | null {
		const raw = item as unknown as Record<string, unknown>;
		const workspaceId = normalizeId(raw.workspace_id ?? raw.workspaceId ?? $page.params.workspaceId);

		if (!workspaceId) {
			return null;
		}

		if (item.type === 'issue') {
			const projectId = normalizeId(raw.project_id ?? raw.projectId);
			const issueId = normalizeId(item.id);
			return projectId && issueId ? `/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}` : null;
		}
		if (item.type === 'project') {
			const projectId = normalizeId(item.id);
			return projectId ? `/workspace/${workspaceId}/projects/${projectId}` : null;
		}
		const projectId = normalizeId(raw.project_id ?? raw.projectId);
		const issueId = normalizeId(raw.issue_id ?? raw.issueId);
		return projectId && issueId ? `/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}` : null;
	}

	function closeModal() {
		open = false;
	}

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			closeModal();
		}
	}

	function handleDialogKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') {
			event.preventDefault();
			closeModal();
		}
	}

	function navigateToResult(item: SearchResult) {
		const href = getItemHref(item);
		if (!href) {
			toast.error($t('search.missingWorkspace'));
			return;
		}
		closeModal();
		goto(href);
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'ArrowDown') {
			event.preventDefault();
			selectedIndex = Math.min(selectedIndex + 1, Math.max(results.length - 1, 0));
			return;
		}

		if (event.key === 'ArrowUp') {
			event.preventDefault();
			selectedIndex = Math.max(selectedIndex - 1, 0);
			return;
		}

		if (event.key === 'Enter') {
			const selected = results[selectedIndex];
			if (selected) {
				event.preventDefault();
				navigateToResult(selected);
			}
			return;
		}

		if (event.key === 'Escape') {
			event.preventDefault();
			closeModal();
		}
	}

	$effect(() => {
		if (!open) {
			clearPendingSearch();
			loading = false;
			query = '';
			results = [];
			selectedIndex = 0;
			return;
		}

		void tick().then(() => {
			inputEl?.focus();
		});
	});

	onDestroy(() => {
		clearPendingSearch();
	});
</script>

{#if open}
	<div
		class="fixed inset-0 z-50 flex items-start justify-center bg-black/50 px-4 pt-[18vh] dark:bg-black/70"
		onclick={handleBackdropClick}
		onkeydown={handleDialogKeydown}
		role="dialog"
		tabindex="-1"
		aria-modal="true"
	>
		<div
			class="w-full max-w-2xl overflow-hidden rounded-xl border border-slate-200 bg-white shadow-2xl dark:border-slate-700 dark:bg-slate-800 dark:shadow-slate-900/50"
		>
			<div class="flex items-center border-b border-slate-200 px-4 dark:border-slate-700">
				<svg class="h-5 w-5 text-slate-400 dark:text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="m21 21-4.35-4.35M10 18a8 8 0 1 1 0-16 8 8 0 0 1 0 16Z"
					></path>
				</svg>
				<input
					bind:this={inputEl}
					type="text"
					bind:value={query}
					oninput={handleInput}
					onkeydown={handleKeydown}
					placeholder={$t('search.placeholder')}
					class="flex-1 bg-transparent px-3 py-4 text-sm text-slate-900 outline-none placeholder:text-slate-400 dark:text-slate-100 dark:placeholder-slate-500"
				/>
			</div>

			<div class="max-h-96 overflow-y-auto p-2">
				{#if loading}
					<p class="py-4 text-center text-sm text-slate-400 dark:text-slate-400">{$t('search.searching')}</p>
				{:else if !query.trim()}
					<p class="py-4 text-center text-sm text-slate-400 dark:text-slate-400">{$t('search.startTyping')}</p>
				{:else if results.length === 0}
					<p class="py-4 text-center text-sm text-slate-400 dark:text-slate-400">{$t('search.noResults')}</p>
				{:else}
					{#each results as item, index (item.type + '-' + item.id)}
						<button
							type="button"
							class="w-full rounded-lg px-3 py-2 text-left text-sm {index === selectedIndex
								? 'bg-blue-50 dark:bg-blue-500/20'
								: 'hover:bg-slate-50 dark:bg-slate-950 dark:hover:bg-slate-700'}"
							onclick={() => navigateToResult(item)}
						>
							<div class="flex items-center justify-between gap-3">
								<div class="font-medium text-slate-900 dark:text-slate-100">{getItemTitle(item)}</div>
								<span class="text-xs uppercase text-slate-400 dark:text-slate-400">{item.type}</span>
							</div>
							<div class="line-clamp-1 text-xs text-slate-400 dark:text-slate-400">{getItemPreview(item)}</div>
						</button>
					{/each}
				{/if}
			</div>
		</div>
	</div>
{/if}

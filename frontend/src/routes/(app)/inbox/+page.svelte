<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { issuesApi } from '$lib/api/issues';
	import { notificationsApi, type Notification, type NotificationType } from '$lib/api/notifications';
	import { projectsApi } from '$lib/api/projects';
	import { toast } from '$lib/stores/toast';
	import { timeAgo } from '$lib/utils/timeago';

	let notifications = $state<Notification[]>([]);
	let loading = $state(true);
	let errorMessage = $state('');
	let operatingId = $state<string | null>(null);
	let markingAll = $state(false);

	const issueRouteCache = new Map<string, string>();

	onMount(async () => {
		await loadNotifications();
	});

	const unreadNotifications = $derived.by(() =>
		notifications
			.filter((item) => !item.is_read)
			.sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
	);

	const readNotifications = $derived.by(() =>
		notifications
			.filter((item) => item.is_read)
			.sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
	);

	async function loadNotifications() {
		loading = true;
		errorMessage = '';
		const response = await notificationsApi.list();

		if (response.code !== 0) {
			notifications = [];
			errorMessage = response.message;
		} else {
			notifications = response.data?.items ?? [];
		}

		loading = false;
	}

	function getNotificationIcon(type?: NotificationType): string {
		const iconMap: Record<string, string> = {
			mention: 'M15 10l4.55-2.28A1 1 0 0121 8.62v6.76a1 1 0 01-1.45.9L15 14m-8 5a4 4 0 01-4-4V9a4 4 0 014-4h4a4 4 0 014 4v6a4 4 0 01-4 4H7z',
			mentioned: 'M15 10l4.55-2.28A1 1 0 0121 8.62v6.76a1 1 0 01-1.45.9L15 14m-8 5a4 4 0 01-4-4V9a4 4 0 014-4h4a4 4 0 014 4v6a4 4 0 01-4 4H7z',
			assignment: 'M16 7V4H8v3M5 7h14l-1 13H6L5 7z',
			comment_reply: 'M8 10h8M8 14h5m8-3a9 9 0 11-4.18-7.6A9 9 0 0121 11.5z',
			issue_update: 'M4.93 4.93A10 10 0 1119.07 19.07M20 4v5h-5',
			project_update: 'M3 11l18-5-5 18-2-7-7-2z',
			info: 'M12 8h.01M11 12h2v4h-2m9-4a10 10 0 11-20 0 10 10 0 0120 0z'
		};
		return (type && iconMap[type]) || iconMap.info;
	}

	function formatRelativeTime(dateText: string): string {
		return timeAgo(dateText);
	}

	function getNotificationTitle(notification: Notification): string {
		if (notification.title.startsWith('notification.')) {
			return get(t)(notification.title);
		}

		switch (notification.type) {
			case 'assignment':
				return get(t)('notification.issueAssignedTitle');
			case 'mentioned':
			case 'mention':
				return get(t)('notification.mentionedTitle');
			default:
				return notification.title;
		}
	}

	function getNotificationContent(notification: Notification): string {
		if (notification.content.startsWith('issue_assigned:')) {
			const issue = notification.content.substring('issue_assigned:'.length);
			return get(t)('notification.issueAssignedContent', { values: { issue } });
		}

		if (notification.content.startsWith('mentioned_by:')) {
			const actor = notification.content.substring('mentioned_by:'.length);
			return get(t)('notification.mentionedContent', { values: { actor } });
		}

		switch (notification.type) {
			case 'assignment':
				return get(t)('notification.issueAssignedContentFallback');
			case 'mentioned':
			case 'mention':
				return get(t)('notification.mentionedContentFallback');
			default:
				return notification.content;
		}
	}

	async function markRead(id: string) {
		operatingId = id;
		const response = await notificationsApi.markRead(id);
		operatingId = null;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		notifications = notifications.map((item) =>
			item.id === id ? { ...item, is_read: true } : item
		);
	}

	async function markAllRead() {
		if (markingAll || unreadNotifications.length === 0) return;
		markingAll = true;

		const response = await notificationsApi.markAllRead();
		markingAll = false;

	if (response.code === 0) {
			toast.success(get(t)('notification.markAllRead'));
			await loadNotifications();
			return;
		}

		toast.error(response.message);
	}

	async function deleteNotification(id: string) {
		operatingId = id;
		const response = await notificationsApi.delete(id);
		operatingId = null;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		notifications = notifications.filter((item) => item.id !== id);
	}

	async function resolveIssueRoute(issueId: string): Promise<string | null> {
		const cached = issueRouteCache.get(issueId);
		if (cached) {
			return cached;
		}

		const issueResponse = await issuesApi.get(issueId);
		if ((issueResponse.code !== 0) || !issueResponse.data) {
			return null;
		}

		const projectResponse = await projectsApi.get(issueResponse.data.project_id);
		if ((projectResponse.code !== 0) || !projectResponse.data) {
			return null;
		}

		const route = `/workspace/${projectResponse.data.workspace_id}/projects/${projectResponse.data.id}/issues/${issueId}`;
		issueRouteCache.set(issueId, route);
		return route;
	}

	async function goToIssue(notification: Notification) {
		if (!notification.related_issue_id) {
			return;
		}

		const route = await resolveIssueRoute(notification.related_issue_id);
		if (!route) {
			toast.error(get(t)('notification.cannotLocateIssue'));
			return;
		}

		if (!notification.is_read) {
			await markRead(notification.id);
		}

		goto(route);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:p-5">
		<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('notification.title')}</h1>
				<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('notification.unreadReadCount', { values: { unread: unreadNotifications.length, read: readNotifications.length } })}</p>
			</div>
			<div class="flex flex-wrap gap-2">
				<button
					onclick={markAllRead}
					disabled={unreadNotifications.length === 0 || markingAll}
					class="rounded-md bg-blue-600 px-3 py-2 text-sm text-white hover:bg-blue-700 disabled:opacity-60"
				>
					{markingAll ? $t('common.processing') : $t('notification.markAllRead')}
				</button>
				<button
					type="button"
					class="rounded-md border border-slate-300 dark:border-slate-600 px-3 py-2 text-sm text-slate-600 dark:text-slate-300"
					disabled
				>
					{$t('common.settings')}
				</button>
			</div>
		</div>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-8 text-center text-slate-500 dark:text-slate-400">
			<div class="mx-auto mb-3 h-6 w-6 animate-spin rounded-full border-2 border-blue-600 border-t-transparent"></div>
			{$t('notification.loading')}
		</div>
	{:else if errorMessage}
		<div class="rounded-lg border border-red-200 bg-red-50 p-6 text-center">
			<p class="mb-2 text-sm font-medium text-red-700">{$t('notification.loadFailed')}</p>
			<p class="mb-4 text-sm text-red-600">{errorMessage}</p>
			<button onclick={loadNotifications} class="rounded-md bg-red-600 px-4 py-2 text-sm text-white hover:bg-red-700">
				{$t('common.retry')}
			</button>
		</div>
	{:else if notifications.length === 0}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-12 text-center">
			<div class="mb-3 flex justify-center text-slate-400">
				<svg class="h-10 w-10" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 17h5l-1.4-1.4a2 2 0 01-.6-1.44V11a6 6 0 00-4-5.66V5a2 2 0 10-4 0v.34A6 6 0 006 11v3.16a2 2 0 01-.6 1.44L4 17h5m6 0a3 3 0 01-6 0"></path>
				</svg>
			</div>
			<p class="text-sm text-slate-600 dark:text-slate-300">{$t('notification.noNotifications')}</p>
		</div>
	{:else}
		<section class="space-y-3">
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('notification.unread')} ({unreadNotifications.length})</h2>
			{#if unreadNotifications.length === 0}
				<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-5 text-sm text-slate-500 dark:text-slate-400">{$t('notification.noUnread')}</div>
			{:else}
				<div class="space-y-3">
					{#each unreadNotifications as item (item.id)}
						<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:p-5">
							<div class="flex items-start justify-between gap-3">
								<div class="flex min-w-0 gap-3">
									<span class="mt-2 inline-block h-2.5 w-2.5 shrink-0 rounded-full bg-blue-500"></span>
									<div class="inline-flex h-8 w-8 items-center justify-center rounded-md bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-300">
										<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d={getNotificationIcon(item.type)}></path>
										</svg>
									</div>
									<div class="min-w-0">
										<p class="font-medium text-slate-900 dark:text-slate-100">{getNotificationTitle(item)}</p>
										<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{getNotificationContent(item)}</p>
										{#if item.issue_title}
											<p class="mt-1 text-sm font-medium text-blue-600 dark:text-blue-400">📋 {item.issue_title}</p>
										{/if}
									</div>
								</div>
								<span class="shrink-0 text-xs text-slate-500 dark:text-slate-400">{formatRelativeTime(item.created_at)}</span>
							</div>
							<div class="mt-4 flex flex-wrap gap-2">
								{#if item.related_issue_id}
									<button
										onclick={() => goToIssue(item)}
										class="rounded-md border border-slate-300 dark:border-slate-600 px-3 py-1.5 text-sm text-slate-700 dark:text-slate-300 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
									>
										{$t('notification.viewIssue')}
									</button>
								{/if}
								<button
									onclick={() => markRead(item.id)}
									disabled={operatingId === item.id}
									class="rounded-md border border-blue-200 px-3 py-1.5 text-sm text-blue-700 hover:bg-blue-50 disabled:opacity-60"
								>
									{operatingId === item.id ? $t('common.processing') : $t('notification.markRead')}
								</button>
								<button
									onclick={() => deleteNotification(item.id)}
									disabled={operatingId === item.id}
									class="rounded-md border border-red-200 px-3 py-1.5 text-sm text-red-700 hover:bg-red-50 disabled:opacity-60"
								>
									{$t('common.delete')}
								</button>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section class="space-y-3">
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('notification.read')} ({readNotifications.length})</h2>
			{#if readNotifications.length === 0}
				<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-5 text-sm text-slate-500 dark:text-slate-400">{$t('notification.noRead')}</div>
			{:else}
				<div class="space-y-3">
					{#each readNotifications as item (item.id)}
						<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 sm:p-5 opacity-80">
							<div class="flex items-start justify-between gap-3">
								<div class="flex min-w-0 gap-3">
									<div class="inline-flex h-8 w-8 items-center justify-center rounded-md bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-300">
										<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d={getNotificationIcon(item.type)}></path>
										</svg>
									</div>
									<div class="min-w-0">
										<p class="font-medium text-slate-900 dark:text-slate-100">{getNotificationTitle(item)}</p>
										<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{getNotificationContent(item)}</p>
										{#if item.issue_title}
											<p class="mt-1 text-sm font-medium text-blue-600 dark:text-blue-400">📋 {item.issue_title}</p>
										{/if}
									</div>
								</div>
								<span class="shrink-0 text-xs text-slate-500 dark:text-slate-400">{formatRelativeTime(item.created_at)}</span>
							</div>
							<div class="mt-4 flex flex-wrap gap-2">
								{#if item.related_issue_id}
									<button
										onclick={() => goToIssue(item)}
										class="rounded-md border border-slate-300 dark:border-slate-600 px-3 py-1.5 text-sm text-slate-700 dark:text-slate-300 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950"
									>
										{$t('notification.viewIssue')}
									</button>
								{/if}
								<button
									onclick={() => deleteNotification(item.id)}
									disabled={operatingId === item.id}
									class="rounded-md border border-red-200 px-3 py-1.5 text-sm text-red-700 hover:bg-red-50 disabled:opacity-60"
								>
									{$t('common.delete')}
								</button>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>
	{/if}
</div>

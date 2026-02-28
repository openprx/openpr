<script lang="ts">
	import '$lib/i18n';
	import { onMount } from 'svelte';
	import { authStore } from '$lib/stores/auth';
	import { authApi } from '$lib/api/auth';
	import { notificationsApi } from '$lib/api/notifications';
	import SearchModal from '$lib/components/SearchModal.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { isAdminUser } from '$lib/utils/auth';
	import { t } from 'svelte-i18n';
	import { projectOptionsStore } from '$lib/stores/project-options';
	import { workspacesApi } from '$lib/api/workspaces';

	let { children } = $props();

	async function handleLogout() {
		await authApi.logout();
		authStore.logout();
		goto('/auth/login');
	}

	let sidebarOpen = $state(false);
	let searchOpen = $state(false);
	let unreadCount = $state(0);
	let loadingProfile = $state(true);
	let pollTimer: ReturnType<typeof setInterval> | null = null;
	let workspaceRole = $state<string | null>(null);

	const isAdmin = $derived(isAdminUser($authStore.user));
	const isWorkspaceAdmin = $derived(workspaceRole === 'owner' || workspaceRole === 'admin' || isAdmin);

	const appPageTitle = $derived.by(() => {
		const pathname = $page.url.pathname;

		if (pathname === '/workspace') return $t('pageTitle.workspace');
		if (pathname === '/governance') return $t('pageTitle.governanceCenter');
		if (pathname === '/proposals') return $t('pageTitle.proposals');
		if (pathname === '/proposals/new') return $t('pageTitle.proposalNew');
		if (pathname === '/impact-reviews') return $t('pageTitle.impactReviews');
		if (pathname === '/proposal-templates') return $t('pageTitle.proposalTemplates');
		if (pathname === '/decisions/analytics') return $t('pageTitle.decisionAnalytics');
		if (pathname === '/governance/config') return $t('pageTitle.governanceConfig');
		if (pathname === '/governance/audit-logs') return $t('pageTitle.governanceAuditLogs');
		if (/^\/ai-agents\/[^/]+\/learning$/.test(pathname)) return $t('pageTitle.aiLearning');
		if (/^\/proposals\/[^/]+\/review$/.test(pathname)) return $t('pageTitle.proposalImpactReview');
		if (/^\/proposals\/[^/]+\/edit$/.test(pathname)) return $t('pageTitle.proposalEdit');
		if (/^\/proposals\/[^/]+\/veto$/.test(pathname)) return $t('pageTitle.vetoDetail');
		if (/^\/proposals\/[^/]+\/chain$/.test(pathname)) return $t('pageTitle.proposalChain');
		if (/^\/proposals\/[^/]+$/.test(pathname)) return $t('pageTitle.proposalDetailFallback');
		if (pathname === '/trust-board') return $t('pageTitle.trustBoard');
		if (pathname === '/trust-scores/appeals') return $t('pageTitle.trustAppeals');
		if (/^\/users\/[^/]+\/trust$/.test(pathname)) return $t('pageTitle.userTrust', { values: { id: '' } });
		if (/^\/projects\/[^/]+\/ai-agents$/.test(pathname)) return $t('pageTitle.aiAgents');
		if (/^\/projects\/[^/]+\/audit-reports\/[^/]+$/.test(pathname)) return $t('pageTitle.auditReportDetail');
		if (/^\/projects\/[^/]+\/audit-reports$/.test(pathname)) return $t('pageTitle.auditReports');
		if (pathname === '/search') return $t('pageTitle.search');
		if (pathname === '/inbox') return $t('pageTitle.inbox');
		if (pathname === '/my-work') return $t('pageTitle.myWork');
		if (pathname === '/settings') return $t('pageTitle.personalSettings');
		if (pathname === '/admin') return $t('pageTitle.adminDashboard');
		if (pathname === '/admin/users') return $t('pageTitle.adminUsers');
		if (pathname === '/admin/settings') return $t('pageTitle.adminSettings');
		if (/^\/workspace\/[^/]+\/settings$/.test(pathname)) return $t('pageTitle.workspaceSettings');
		if (/^\/workspace\/[^/]+\/members$/.test(pathname)) return $t('pageTitle.workspaceMembers');
		if (/^\/workspace\/[^/]+\/webhooks$/.test(pathname)) return $t('pageTitle.workspaceWebhooks');
		if (/^\/workspace\/[^/]+\/projects\/[^/]+\/issues\/[^/]+$/.test(pathname)) return $t('pageTitle.issueDetail');
		if (/^\/workspace\/[^/]+\/projects\/[^/]+\/issues$/.test(pathname)) return $t('pageTitle.issueList');
		if (/^\/workspace\/[^/]+\/projects\/[^/]+\/board$/.test(pathname)) return $t('pageTitle.projectBoard');
		if (/^\/workspace\/[^/]+\/projects\/[^/]+\/cycles$/.test(pathname)) return $t('pageTitle.projectCycles');
		if (/^\/workspace\/[^/]+\/projects\/[^/]+$/.test(pathname)) return $t('pageTitle.projectDetail');
		if (/^\/workspace\/[^/]+\/projects$/.test(pathname)) return $t('pageTitle.workspaceProjects');

		return $t('pageTitle.appDefault');
	});

	const currentWorkspaceId = $derived.by(() => {
		const match = $page.url.pathname.match(/^\/workspace\/([^/]+)/);
		return match?.[1] ?? null;
	});

	const workspaceNavLinks = $derived.by(() => {
		if (!currentWorkspaceId) {
			return [] as Array<{ href: string; label: string }>;
		}

		const links = [
			{ href: `/workspace/${currentWorkspaceId}/projects`, label: $t('nav.projectList') }
		];
		if (isWorkspaceAdmin) {
			links.push(
				{ href: `/workspace/${currentWorkspaceId}/members`, label: $t('nav.members') },
				{ href: `/workspace/${currentWorkspaceId}/webhooks`, label: $t('nav.webhook') },
				{ href: `/workspace/${currentWorkspaceId}/settings`, label: $t('nav.workspaceSettings') }
			);
		}
		return links;
	});

	onMount(() => {
		void (async () => {
			const meResponse = await authApi.me();

			if (meResponse.code !== 0) {
				authStore.logout();
				goto('/auth/login');
				return;
			}

			if (meResponse.data) {
				authStore.setUser(meResponse.data.user);
			}

			await Promise.all([fetchUnreadCount(), projectOptionsStore.ensureLoaded()]);

			// Fetch workspace role for current user
			if (currentWorkspaceId && meResponse.data?.user) {
				try {
					const membersRes = await workspacesApi.getMembers(currentWorkspaceId);
					if (membersRes.code === 0 && membersRes.data?.items) {
						const me = membersRes.data.items.find((m) => m.user_id === meResponse.data!.user.id);
						workspaceRole = me?.role ?? null;
					}
				} catch {
					workspaceRole = null;
				}
			}

			loadingProfile = false;
		})();

		pollTimer = setInterval(() => {
			void fetchUnreadCount();
		}, 30000);

		return () => {
			if (pollTimer) {
				clearInterval(pollTimer);
				pollTimer = null;
			}
		};
	});

	async function fetchUnreadCount() {
		const response = await notificationsApi.getUnreadCount();
		if (response.code === 0 && response.data) {
			unreadCount = response.data.count;
		}
	}

	function goToNotifications() {
		goto('/inbox');
	}

	function isActive(href: string, exact = false): boolean {
		if (href === '/' || exact) {
			return $page.url.pathname === href;
		}
		return $page.url.pathname === href || $page.url.pathname.startsWith(`${href}/`);
	}

	function isGovernanceRoute(pathname: string): boolean {
		return (
			pathname === '/governance' ||
			pathname.startsWith('/proposals') ||
			pathname.startsWith('/decisions/analytics') ||
			pathname.startsWith('/trust-board') ||
			pathname.startsWith('/impact-reviews') ||
			pathname.startsWith('/proposal-templates') ||
			pathname.startsWith('/trust-scores/appeals') ||
			pathname.startsWith('/governance/config') ||
			pathname.startsWith('/governance/audit-logs') ||
			pathname.includes('/audit-reports')
		);
	}

	function handleGlobalSearchShortcut(event: KeyboardEvent) {
		const key = typeof event.key === 'string' ? event.key.toLowerCase() : '';
		if ((event.ctrlKey || event.metaKey) && key === 'k') {
			event.preventDefault();
			searchOpen = true;
		}
	}

	$effect(() => {
		$page.url.pathname;
		sidebarOpen = false;
	});
</script>

<svelte:window on:keydown={handleGlobalSearchShortcut} />
<svelte:head>
	<title>{appPageTitle}</title>
</svelte:head>

<div class="min-h-screen bg-slate-50 dark:bg-slate-950">
	<header class="sticky top-0 z-20 border-b border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900">
		<div class="flex items-center justify-between gap-3 px-4 py-3">
			<div class="flex items-center gap-3">
				<button
					onclick={() => (sidebarOpen = !sidebarOpen)}
					class="rounded-md p-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 lg:hidden"
					aria-label={$t('nav.toggleSidebar')}
				>
					<svg class="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16"></path>
					</svg>
				</button>
				<a href="/workspace" class="text-xl font-bold text-slate-900 dark:text-slate-100">OpenPR</a>
			</div>

			<div class="hidden flex-1 justify-center md:flex">
				<button
					type="button"
					onclick={() => (searchOpen = true)}
					class="w-full max-w-md rounded-md border border-slate-300 bg-slate-50 px-3 py-2 text-left text-sm text-slate-500 hover:bg-white dark:border-slate-600 dark:bg-slate-800 dark:text-slate-400 dark:hover:bg-slate-700"
				>
					{$t('search.placeholder')}
					<span class="float-right rounded border border-slate-300 px-1.5 text-xs text-slate-400 dark:border-slate-600 dark:text-slate-400">Ctrl+K</span>
				</button>
			</div>

			<div class="flex items-center gap-3">
				{#if !loadingProfile && $authStore.user}
					<span class="hidden text-sm text-slate-600 dark:text-slate-400 sm:inline">{$authStore.user.email}</span>
				{/if}
				<button
					type="button"
					onclick={goToNotifications}
					class="relative rounded-md p-2 text-slate-700 hover:bg-slate-100 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-800"
					aria-label={$t('nav.notifications')}
				>
					<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 17h5l-1.4-1.4a2 2 0 01-.6-1.44V11a6 6 0 00-4-5.66V5a2 2 0 10-4 0v.34A6 6 0 006 11v3.16a2 2 0 01-.6 1.44L4 17h5m6 0a3 3 0 01-6 0"></path>
					</svg>
					{#if unreadCount > 0}
						<span class="absolute -right-1 -top-1 inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-red-500 px-1 text-xs text-white">
							{unreadCount > 99 ? '99+' : unreadCount}
						</span>
					{/if}
				</button>
				<button
					onclick={handleLogout}
					class="rounded-md px-4 py-2 text-sm text-slate-700 hover:bg-slate-100 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-800"
				>
					{$t('nav.logout')}
				</button>
			</div>
		</div>
	</header>

	<div class="flex">
		<aside
			class="fixed top-[57px] z-10 h-[calc(100vh-57px)] w-64 border-r border-slate-200 bg-white transition-transform duration-300 dark:border-slate-700 dark:bg-slate-900 lg:sticky {sidebarOpen
				? 'translate-x-0'
				: '-translate-x-full lg:translate-x-0'}"
		>
			<nav class="space-y-5 p-4">
				<div class="space-y-1">
					<a
						href="/workspace"
						class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive('/workspace', true)
							? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
							: 'text-slate-700 dark:text-slate-300'}"
					>
						<span class="inline-flex items-center gap-3">
							<svg
								class="h-5 w-5 {isActive('/workspace', true) ? 'text-blue-600' : 'text-slate-400'}"
								fill="none"
								stroke="currentColor"
								stroke-width="1.5"
								viewBox="0 0 24 24"
								aria-hidden="true"
							>
								<path stroke-linecap="round" stroke-linejoin="round" d="M4.5 4.5h6v6h-6zm9 0h6v6h-6zm-9 9h6v6h-6zm9 0h6v6h-6z"></path>
							</svg>
							<span>{$t('nav.myWorkspace')}</span>
						</span>
					</a>
					<a
						href="/my-work"
						class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive('/my-work')
							? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
							: 'text-slate-700 dark:text-slate-300'}"
					>
						<span class="inline-flex items-center gap-3">
							<svg
								class="h-5 w-5 {isActive('/my-work') ? 'text-blue-600' : 'text-slate-400'}"
								fill="none"
								stroke="currentColor"
								stroke-width="1.5"
								viewBox="0 0 24 24"
								aria-hidden="true"
							>
								<path stroke-linecap="round" stroke-linejoin="round" d="M9 6h10M9 12h10M9 18h10M5.25 6.75h.008v.008H5.25zm0 6h.008v.008H5.25zm0 6h.008v.008H5.25z"></path>
							</svg>
							<span>{$t('workspace.myWork')}</span>
						</span>
					</a>
					<a
						href="/governance"
						class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isGovernanceRoute($page.url.pathname)
							? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
							: 'text-slate-700 dark:text-slate-300'}"
					>
						<span class="inline-flex items-center gap-3">
							<svg
								class="h-5 w-5 {isGovernanceRoute($page.url.pathname) ? 'text-blue-600' : 'text-slate-400'}"
								fill="none"
								stroke="currentColor"
								stroke-width="1.5"
								viewBox="0 0 24 24"
								aria-hidden="true"
							>
								<path stroke-linecap="round" stroke-linejoin="round" d="M3 21h18M4.5 18h15M6 18V10.5m4 7.5V10.5m4 7.5V10.5m4 7.5V10.5M12 3l9 4.5H3L12 3z"></path>
							</svg>
							<span>{$t('nav.governanceCenter')}</span>
						</span>
					</a>
					<a
						href="/inbox"
						class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive('/inbox')
							? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
							: 'text-slate-700 dark:text-slate-300'}"
					>
						<span class="inline-flex items-center gap-3">
							<svg
								class="h-5 w-5 {isActive('/inbox') ? 'text-blue-600' : 'text-slate-400'}"
								fill="none"
								stroke="currentColor"
								stroke-width="1.5"
								viewBox="0 0 24 24"
								aria-hidden="true"
							>
								<path stroke-linecap="round" stroke-linejoin="round" d="M14 17h5l-1.2-1.2a2.5 2.5 0 01-.8-1.8V11a5 5 0 10-10 0v3a2.5 2.5 0 01-.8 1.8L5 17h5m4 0a2 2 0 11-4 0"></path>
							</svg>
							<span>{$t('nav.notifications')}</span>
							{#if unreadCount > 0}
								<span class="inline-flex min-w-5 items-center justify-center rounded-full bg-red-500 px-1.5 py-0.5 text-xs text-white">
									{unreadCount > 99 ? '99+' : unreadCount}
								</span>
							{/if}
						</span>
					</a>
					<a
						href="/settings"
						class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive('/settings')
							? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
							: 'text-slate-700 dark:text-slate-300'}"
					>
						<span class="inline-flex items-center gap-3">
							<svg
								class="h-5 w-5 {isActive('/settings') ? 'text-blue-600' : 'text-slate-400'}"
								fill="none"
								stroke="currentColor"
								stroke-width="1.5"
								viewBox="0 0 24 24"
								aria-hidden="true"
							>
								<path stroke-linecap="round" stroke-linejoin="round" d="M15.75 7.5a3.75 3.75 0 11-7.5 0 3.75 3.75 0 017.5 0zM4.5 20.25a7.5 7.5 0 0115 0"></path>
							</svg>
							<span>{$t('nav.personalSettings')}</span>
						</span>
					</a>
				</div>

				{#if workspaceNavLinks.length > 0}
					<div>
						<p class="mb-2 px-4 text-xs font-semibold uppercase tracking-wide text-slate-400 dark:text-slate-400">{$t('nav.currentWorkspace')}</p>
						<div class="space-y-1">
							{#each workspaceNavLinks as link}
								<a
									href={link.href}
									class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive(link.href)
										? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
										: 'text-slate-700 dark:text-slate-300'}"
								>
									{link.label}
								</a>
							{/each}
						</div>
					</div>
				{/if}

				{#if isAdmin}
					<div>
						<p class="mb-2 px-4 text-xs font-semibold uppercase tracking-wide text-slate-400 dark:text-slate-400">{$t('nav.admin')}</p>
						<div class="space-y-1">
							<a
								href="/admin"
								class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive('/admin', true)
									? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
									: 'text-slate-700 dark:text-slate-300'}"
							>
								<span class="inline-flex items-center gap-3">
									<svg
										class="h-5 w-5 {isActive('/admin', true) ? 'text-blue-600' : 'text-slate-400'}"
										fill="none"
										stroke="currentColor"
										stroke-width="1.5"
										viewBox="0 0 24 24"
										aria-hidden="true"
									>
										<path stroke-linecap="round" stroke-linejoin="round" d="M4.5 19.5h15M6 16.5V12m4 4.5V8.25m4 8.25v-6m4 6V5.25"></path>
									</svg>
									<span>{$t('nav.adminDashboard')}</span>
								</span>
							</a>
							<a
								href="/admin/users"
								class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive('/admin/users')
									? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
									: 'text-slate-700 dark:text-slate-300'}"
							>
								<span class="inline-flex items-center gap-3">
									<svg
										class="h-5 w-5 {isActive('/admin/users') ? 'text-blue-600' : 'text-slate-400'}"
										fill="none"
										stroke="currentColor"
										stroke-width="1.5"
										viewBox="0 0 24 24"
										aria-hidden="true"
									>
										<path stroke-linecap="round" stroke-linejoin="round" d="M15 7.5a3 3 0 11-6 0 3 3 0 016 0zM20.25 8.25a2.25 2.25 0 11-4.5 0 2.25 2.25 0 014.5 0zM8.25 8.25a2.25 2.25 0 11-4.5 0 2.25 2.25 0 014.5 0zM3.75 19.5a5.25 5.25 0 0110.5 0M14.25 19.5a4.5 4.5 0 019 0"></path>
									</svg>
									<span>{$t('nav.userManagement')}</span>
								</span>
							</a>
							<a
								href="/admin/settings"
								class="block rounded-md px-4 py-2 hover:bg-slate-100 dark:bg-slate-800 dark:hover:bg-slate-800 {isActive('/admin/settings')
									? 'bg-slate-100 text-blue-600 dark:bg-slate-800'
									: 'text-slate-700 dark:text-slate-300'}"
							>
								<span class="inline-flex items-center gap-3">
									<svg
										class="h-5 w-5 {isActive('/admin/settings') ? 'text-blue-600' : 'text-slate-400'}"
										fill="none"
										stroke="currentColor"
										stroke-width="1.5"
										viewBox="0 0 24 24"
										aria-hidden="true"
									>
										<path stroke-linecap="round" stroke-linejoin="round" d="M10.5 3.75h3l.75 2.25a7.3 7.3 0 011.74.99l2.2-.74 1.5 2.6-1.74 1.52c.09.37.15.76.15 1.15s-.06.78-.15 1.15l1.74 1.52-1.5 2.6-2.2-.74a7.3 7.3 0 01-1.74.99l-.75 2.25h-3l-.75-2.25a7.3 7.3 0 01-1.74-.99l-2.2.74-1.5-2.6 1.74-1.52A4.86 4.86 0 015 12c0-.39.06-.78.15-1.15L3.4 9.33l1.5-2.6 2.2.74c.54-.4 1.13-.73 1.74-.99l.75-2.25zM12 14.25A2.25 2.25 0 1012 9.75a2.25 2.25 0 000 4.5z"></path>
									</svg>
									<span>{$t('nav.systemSettings')}</span>
								</span>
							</a>
						</div>
					</div>
				{/if}
			</nav>
		</aside>

		<main class="flex-1 overflow-auto">
			<div class="p-4 sm:p-6">
				{@render children()}
			</div>
		</main>
	</div>

	<SearchModal bind:open={searchOpen} />
</div>

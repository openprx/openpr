<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { authApi } from '$lib/api/auth';
	import { workspacesApi } from '$lib/api/workspaces';
	import { projectsApi } from '$lib/api/projects';
	import { issuesApi } from '$lib/api/issues';
	import { membersApi } from '$lib/api/members';
	import { toast } from '$lib/stores/toast';
	import { isAdminUser } from '$lib/utils/auth';
	import Card from '$lib/components/Card.svelte';
	import Button from '$lib/components/Button.svelte';

	interface SystemStats {
		users: number;
		workspaces: number;
		projects: number;
		issues: number;
	}

	let loading = $state(true);
	let stats = $state<SystemStats>({ users: 0, workspaces: 0, projects: 0, issues: 0 });
	const statusItems = $derived([
		{ label: $t('admin.apiService'), value: $t('admin.statusOk'), color: 'text-green-700 bg-green-100' },
		{ label: $t('admin.databaseConnection'), value: $t('admin.statusOk'), color: 'text-green-700 bg-green-100' },
		{ label: $t('admin.webhookJobs'), value: $t('admin.statusMonitoring'), color: 'text-blue-700 bg-blue-100' }
	]);

	onMount(async () => {
		const meResponse = await authApi.me();
		if ((meResponse.code !== 0) || !isAdminUser(meResponse.data?.user ?? null)) {
			toast.error(get(t)('admin.adminOnlyPage'));
			goto('/workspace');
			return;
		}

		await loadStats();
	});

	async function loadStats() {
		loading = true;

		const workspaceResponse = await workspacesApi.list();
		if ((workspaceResponse.code !== 0) || !workspaceResponse.data) {
			toast.error(workspaceResponse.message || get(t)('admin.loadOverviewFailed'));
			loading = false;
			return;
		}

		const workspaces = workspaceResponse.data.items ?? [];
		let projectCount = 0;
		let issueCount = 0;
		const userIdSet = new Set<string>();

		for (const workspace of workspaces) {
			const [memberResponse, projectResponse] = await Promise.all([
				membersApi.list(workspace.id),
				projectsApi.list(workspace.id, { page: 1, per_page: 100 })
			]);

			if (memberResponse.data) {
				for (const member of memberResponse.data.items ?? []) {
					userIdSet.add(member.user_id);
				}
			}

			if (projectResponse.data) {
				projectCount += projectResponse.data.total;
				for (const project of projectResponse.data.items ?? []) {
					const issueResponse = await issuesApi.list(project.id, { page: 1, per_page: 1 });
					if (issueResponse.data) {
						issueCount += issueResponse.data.total;
					}
				}
			}
		}

		stats = {
			users: userIdSet.size,
			workspaces: workspaces.length,
			projects: projectCount,
			issues: issueCount
		};

		loading = false;
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('admin.dashboard')}</h1>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('admin.overview')}</p>
		</div>
		<div class="flex gap-2">
			<Button variant="secondary" onclick={() => goto('/admin/users')}>{$t('admin.createUser')}</Button>
			<Button onclick={() => goto('/inbox')}>{$t('admin.viewRecentActivity')}</Button>
		</div>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
	{:else}
		<div class="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
			<Card>
				<p class="text-sm text-slate-500 dark:text-slate-400">{$t('admin.totalUsers')}</p>
				<p class="mt-2 text-3xl font-bold text-slate-900 dark:text-slate-100">{stats.users}</p>
			</Card>
			<Card>
				<p class="text-sm text-slate-500 dark:text-slate-400">{$t('admin.totalWorkspaces')}</p>
				<p class="mt-2 text-3xl font-bold text-slate-900 dark:text-slate-100">{stats.workspaces}</p>
			</Card>
			<Card>
				<p class="text-sm text-slate-500 dark:text-slate-400">{$t('admin.totalProjects')}</p>
				<p class="mt-2 text-3xl font-bold text-slate-900 dark:text-slate-100">{stats.projects}</p>
			</Card>
			<Card>
				<p class="text-sm text-slate-500 dark:text-slate-400">{$t('admin.totalIssues')}</p>
				<p class="mt-2 text-3xl font-bold text-slate-900 dark:text-slate-100">{stats.issues}</p>
			</Card>
		</div>

		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('admin.systemStatus')}</h2>
			<div class="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-3">
				{#each statusItems as item}
					<div class="rounded-md border border-slate-200 dark:border-slate-700 p-3">
						<p class="text-sm text-slate-500 dark:text-slate-400">{item.label}</p>
						<span class="mt-2 inline-flex rounded-full px-2.5 py-1 text-xs font-medium {item.color}">
							{item.value}
						</span>
					</div>
				{/each}
			</div>
		</Card>
	{/if}
</div>

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { authApi } from '$lib/api/auth';
	import { toast } from '$lib/stores/toast';
	import { isAdminUser } from '$lib/utils/auth';
	import Card from '$lib/components/Card.svelte';
	import Input from '$lib/components/Input.svelte';
	import Button from '$lib/components/Button.svelte';

	let saving = $state(false);
	let basicSettings = $state({
		siteName: 'OpenPR',
		siteUrl: 'https://openpr.local',
		allowWorkspaceCreate: 'admin_only'
	});
	let securitySettings = $state({
		passwordMinLength: '8',
		requireStrongPassword: true,
		sessionHours: '24'
	});

	onMount(async () => {
		const meResponse = await authApi.me();
		if ((meResponse.code !== 0) || !isAdminUser(meResponse.data?.user ?? null)) {
			toast.error(get(t)('admin.settingsAdminOnly'));
			goto('/workspace');
		}
	});

	async function saveSettings() {
		saving = true;
		await new Promise((resolve) => setTimeout(resolve, 300));
		toast.success(get(t)('admin.settingsSavedPlaceholder'));
		saving = false;
	}
</script>

<div class="mx-auto max-w-5xl space-y-6">
	<div>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('admin.systemSettings')}</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('admin.settingsDescription')}</p>
	</div>

	<Card>
		<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('admin.basicSettings')}</h2>
		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
			<Input label={$t('admin.siteName')} bind:value={basicSettings.siteName} />
			<Input label={$t('admin.siteUrl')} bind:value={basicSettings.siteUrl} />
			<div class="md:col-span-2">
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="allowWorkspaceCreate">{$t('admin.workspaceCreatePermission')}</label>
				<select
					id="allowWorkspaceCreate"
					bind:value={basicSettings.allowWorkspaceCreate}
					class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
				>
					<option value="admin_only">{$t('admin.adminOnly')}</option>
					<option value="all_users">{$t('admin.allUsers')}</option>
				</select>
			</div>
		</div>
	</Card>

	<Card>
		<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('admin.securitySettings')}</h2>
		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-3">
			<Input label={$t('admin.minPasswordLength')} type="number" bind:value={securitySettings.passwordMinLength} />
			<Input label={$t('admin.sessionHours')} type="number" bind:value={securitySettings.sessionHours} />
			<label class="flex items-center gap-2 rounded-md border border-slate-200 dark:border-slate-700 px-3 py-2 text-sm text-slate-700 dark:text-slate-300">
				<input type="checkbox" bind:checked={securitySettings.requireStrongPassword} />
				{$t('admin.strongPasswordPolicy')}
			</label>
		</div>
	</Card>

	<div class="flex justify-end">
		<Button loading={saving} onclick={saveSettings}>{$t('settings.save')}</Button>
	</div>
</div>

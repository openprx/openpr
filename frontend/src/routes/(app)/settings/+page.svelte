<script lang="ts">
	import { setLocale } from '$lib/i18n';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { authApi, type User } from '$lib/api/auth';
	import { authStore } from '$lib/stores/auth';
	import { toast } from '$lib/stores/toast';
	import { setTheme, theme, type Theme } from '$lib/theme';
	import Card from '$lib/components/Card.svelte';
	import Input from '$lib/components/Input.svelte';
	import Button from '$lib/components/Button.svelte';
	import { locale, t } from 'svelte-i18n';

	let loading = $state(true);
	let savingProfile = $state(false);
	let savingPassword = $state(false);
	let savingNotifications = $state(false);
	let currentTheme = $state<Theme>('light');

	let profileForm = $state({
		name: '',
		email: '',
		avatarUrl: ''
	});
	let passwordForm = $state({
		currentPassword: '',
		newPassword: '',
		confirmPassword: ''
	});
	let notificationPrefs = $state({
		emailNotification: true,
		mentionOnly: false,
		dailyDigest: true
	});

	onMount(async () => {
		currentTheme = get(theme);
		const response = await authApi.me();
		if (response.code !== 0) {
			toast.error(response.message);
			loading = false;
			return;
		}

		if (response.data) {
			const user = response.data.user as User;
			profileForm = {
				name: user.name || '',
				email: user.email || '',
				avatarUrl: user.avatar_url || ''
			};
			authStore.setUser(user);
		}

		loading = false;
	});

	async function saveProfile() {
		savingProfile = true;
		await new Promise((resolve) => setTimeout(resolve, 300));
		if ($authStore.user) {
			authStore.setUser({ ...$authStore.user, name: profileForm.name, email: profileForm.email });
		}
		toast.success($t('settings.profileSaved'));
		savingProfile = false;
	}

	async function updatePassword() {
		if (!passwordForm.currentPassword || !passwordForm.newPassword) {
			toast.error($t('settings.passwordIncomplete'));
			return;
		}
		if (passwordForm.newPassword.length < 8) {
			toast.error($t('settings.passwordLength'));
			return;
		}
		if (passwordForm.newPassword !== passwordForm.confirmPassword) {
			toast.error($t('settings.passwordMismatch'));
			return;
		}

		savingPassword = true;
		await new Promise((resolve) => setTimeout(resolve, 300));
		passwordForm = { currentPassword: '', newPassword: '', confirmPassword: '' };
		toast.success($t('settings.passwordSaved'));
		savingPassword = false;
	}

	async function saveNotificationPrefs() {
		savingNotifications = true;
		await new Promise((resolve) => setTimeout(resolve, 200));
		toast.success($t('settings.notificationsSaved'));
		savingNotifications = false;
	}

	function handleThemeChange(event: Event) {
		const next = (event.target as HTMLSelectElement).value as Theme;
		currentTheme = next;
		setTheme(next);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('settings.title')}</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-400">{$t('settings.subtitle')}</p>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-400">{$t('common.loading')}</div>
	{:else}
		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('settings.profile')}</h2>
			<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
				<Input label={$t('admin.name')} bind:value={profileForm.name} />
				<Input label={$t('auth.email')} type="email" bind:value={profileForm.email} />
				<div class="md:col-span-2">
					<Input label={$t('settings.avatarUrl')} type="url" bind:value={profileForm.avatarUrl} placeholder="https://..." />
				</div>
			</div>
			<div class="mt-4">
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="language">{$t('settings.language')}</label>
				<select
					id="language"
					class="block w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
					onchange={(e) => setLocale((e.target as HTMLSelectElement).value)}
					value={$locale}
				>
					<option value="zh">{$t('settings.zh')}</option>
					<option value="en">{$t('settings.en')}</option>
				</select>
			</div>
			<div class="mt-4">
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="theme">{$t('settings.theme')}</label>
				<select
					id="theme"
					class="block w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
					onchange={handleThemeChange}
					value={currentTheme}
				>
					<option value="light">{$t('settings.themeLight')}</option>
					<option value="dark">{$t('settings.themeDark')}</option>
					<option value="system">{$t('settings.themeSystem')}</option>
				</select>
			</div>
			<div class="mt-4 flex justify-end">
				<Button loading={savingProfile} onclick={saveProfile}>{$t('settings.saveProfile')}</Button>
			</div>
		</Card>

		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('settings.changePassword')}</h2>
			<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-3">
				<Input type="password" label={$t('settings.currentPassword')} bind:value={passwordForm.currentPassword} />
				<Input type="password" label={$t('settings.newPassword')} bind:value={passwordForm.newPassword} />
				<Input type="password" label={$t('settings.confirmPassword')} bind:value={passwordForm.confirmPassword} />
			</div>
			<div class="mt-4 flex justify-end">
				<Button loading={savingPassword} onclick={updatePassword}>{$t('settings.updatePassword')}</Button>
			</div>
		</Card>

		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('settings.notifications')}</h2>
			<div class="mt-4 space-y-3 text-sm text-slate-700 dark:text-slate-300">
				<label class="flex items-center gap-2">
					<input type="checkbox" bind:checked={notificationPrefs.emailNotification} />
					{$t('settings.emailNotifications')}
				</label>
				<label class="flex items-center gap-2">
					<input type="checkbox" bind:checked={notificationPrefs.mentionOnly} />
					{$t('settings.mentionsOnly')}
				</label>
				<label class="flex items-center gap-2">
					<input type="checkbox" bind:checked={notificationPrefs.dailyDigest} />
					{$t('settings.dailyDigest')}
				</label>
			</div>
			<div class="mt-4 flex justify-end">
				<Button loading={savingNotifications} onclick={saveNotificationPrefs}>{$t('settings.savePrefs')}</Button>
			</div>
		</Card>
	{/if}
</div>

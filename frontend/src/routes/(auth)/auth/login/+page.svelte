<script lang="ts">
	import '$lib/i18n';
	import { goto } from '$app/navigation';
	import { authApi } from '$lib/api/auth';
	import { authStore } from '$lib/stores/auth';
	import { toast } from '$lib/stores/toast';
	import { t } from 'svelte-i18n';

	let email = '';
	let password = '';
	let loading = false;

	async function handleLogin() {
		if (!email || !password) {
			toast.error($t('auth.required'));
			return;
		}

		loading = true;
		const response = await authApi.login({ email, password });

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			authStore.setUser(response.data.user);
			toast.success($t('auth.loginSuccess'));
			goto('/workspace');
		}

		loading = false;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.login')}</title>
</svelte:head>

<div class="min-h-screen flex items-center justify-center bg-slate-50 px-4 dark:bg-slate-950">
	<div class="max-w-md w-full space-y-8">
		<div class="text-center">
			<h1 class="text-3xl font-bold text-slate-900 dark:text-slate-100">OpenPR</h1>
			<p class="mt-2 text-sm text-slate-600 dark:text-slate-400">{$t('auth.subtitle')}</p>
		</div>

		<form class="mt-8 space-y-6 rounded-lg bg-white p-8 shadow dark:bg-slate-800" on:submit|preventDefault={handleLogin}>
			<div class="space-y-4">
				<div>
					<label for="email" class="block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('auth.email')}</label>
					<input
						id="email"
						type="email"
						bind:value={email}
						required
						class="mt-1 block w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-slate-900 shadow-sm dark:shadow-slate-900/50 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
						placeholder={$t('placeholders.emailPlaceholder')}
					/>
				</div>

				<div>
					<label for="password" class="block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('auth.password')}</label>
					<input
						id="password"
						type="password"
						bind:value={password}
						required
						class="mt-1 block w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-slate-900 shadow-sm dark:shadow-slate-900/50 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
						placeholder={$t('placeholders.passwordPlaceholder')}
					/>
				</div>
			</div>

			<button
				type="submit"
				disabled={loading}
				class="w-full flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm dark:shadow-slate-900/50 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 dark:focus:ring-offset-slate-900 focus:ring-blue-500 dark:focus:ring-blue-400 disabled:opacity-50 disabled:cursor-not-allowed"
			>
				{loading ? $t('auth.loginLoading') : $t('auth.login')}
			</button>
		</form>
	</div>
</div>

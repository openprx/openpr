<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { workspacesApi, type Workspace } from '$lib/api/workspaces';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import Card from '$lib/components/Card.svelte';
	import Input from '$lib/components/Input.svelte';
	import Textarea from '$lib/components/Textarea.svelte';
	import Button from '$lib/components/Button.svelte';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');

	let loading = $state(true);
	let saving = $state(false);
	let deleting = $state(false);
	let workspace = $state<Workspace | null>(null);
	let form = $state({ name: '', slug: '', description: '' });

	onMount(async () => {
		await loadWorkspace();
	});

	async function loadWorkspace() {
		loading = true;
		const response = await workspacesApi.get(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			workspace = response.data;
			form = {
				name: response.data.name,
				slug: response.data.slug,
				description: response.data.description ?? ''
			};
		}
		loading = false;
	}

	async function saveWorkspace() {
		if (!form.name.trim()) {
			toast.error(get(t)('workspace.enterName'));
			return;
		}
		saving = true;
		const response = await workspacesApi.update(workspaceId, {
			name: form.name.trim(),
			description: form.description.trim() || undefined
		});
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('workspace.settingsSaved'));
			await loadWorkspace();
		}
		saving = false;
	}

	async function handleDeleteWorkspace() {
		if (!workspace) return;
		if (!confirm(get(t)('workspace.deleteConfirmIrreversible', { values: { name: workspace.name } }))) {
			return;
		}

		deleting = true;
		const response = await workspacesApi.delete(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
			deleting = false;
			return;
		}

		toast.success(get(t)('workspace.deleteSuccess'));
		goto('/workspace');
	}
</script>

<div class="mx-auto max-w-5xl space-y-6">
	<div>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('workspace.settingsTitle')}</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('workspace.settingsSubtitle')}</p>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
	{:else}
		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('workspace.basicInfo')}</h2>
			<div class="mt-4 grid grid-cols-1 gap-4">
				<Input label={$t('workspace.name')} bind:value={form.name} required />
				<Input label="Slug" bind:value={form.slug} readonly helperText={$t('workspace.slugReadonlyHint')} />
				<Textarea label={$t('workspace.description')} bind:value={form.description} rows={4} />
			</div>
			<div class="mt-4 flex justify-end">
				<Button loading={saving} onclick={saveWorkspace}>{$t('settings.save')}</Button>
			</div>
		</Card>

		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('workspace.labelManagement')}</h2>
			<p class="mt-2 text-sm text-slate-600 dark:text-slate-300">{$t('workspace.labelManagementDesc')}</p>
			<div class="mt-4">
				<Button variant="secondary" onclick={() => goto(`/workspace/${workspaceId}/projects`)}>
					{$t('workspace.goToProjectList')}
				</Button>
			</div>
		</Card>

		<Card>
			<h2 class="text-lg font-semibold text-red-700">{$t('workspace.dangerZone')}</h2>
			<p class="mt-2 text-sm text-slate-600 dark:text-slate-300">{$t('workspace.deleteDesc')}</p>
			<div class="mt-4">
				<Button variant="danger" loading={deleting} onclick={handleDeleteWorkspace}>{$t('workspace.deleteWorkspace')}</Button>
			</div>
		</Card>
	{/if}
</div>

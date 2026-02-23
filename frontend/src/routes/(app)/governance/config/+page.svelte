<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { governanceApi } from '$lib/api/governance';
	import { toast } from '$lib/stores/toast';
	import { projectOptionsStore } from '$lib/stores/project-options';
	import { groupProjectOptionsByWorkspace } from '$lib/utils/project-options';

	const projects = $derived($projectOptionsStore.items);
	const groupedProjects = $derived.by(() => groupProjectOptionsByWorkspace(projects));
	let selectedProjectId = $state('');
	let loading = $state(true);
	let saving = $state(false);

	let reviewRequired = $state(true);
	let autoReviewDays = $state(30);
	let reviewReminderDays = $state(7);
	let auditReportCron = $state('0 0 1 * *');
	let trustUpdateMode = $state('auto');
	let configText = $state('{}');

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		await projectOptionsStore.ensureLoaded();
		if (projects.length > 0) {
			selectedProjectId = projects[0].id;
			await loadConfig();
		}
		loading = false;
	}

	async function loadConfig() {
		if (!selectedProjectId) {
			return;
		}
		const res = await governanceApi.getConfig(selectedProjectId);
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceConfig.loadFailed'));
			return;
		}
		reviewRequired = res.data.review_required;
		autoReviewDays = res.data.auto_review_days;
		reviewReminderDays = res.data.review_reminder_days;
		auditReportCron = res.data.audit_report_cron;
		trustUpdateMode = res.data.trust_update_mode;
		configText = JSON.stringify(res.data.config ?? {}, null, 2);
	}

	function parseConfig(): Record<string, unknown> | null {
		try {
			const parsed = JSON.parse(configText);
			if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
				toast.error(get(t)('governanceConfig.configInvalid'));
				return null;
			}
			return parsed as Record<string, unknown>;
		} catch {
			toast.error(get(t)('governanceConfig.configInvalid'));
			return null;
		}
	}

	async function saveConfig() {
		if (!selectedProjectId) {
			return;
		}
		const parsedConfig = parseConfig();
		if (!parsedConfig) {
			return;
		}
		saving = true;
		const res = await governanceApi.updateConfig({
			project_id: selectedProjectId,
			review_required: reviewRequired,
			auto_review_days: autoReviewDays,
			review_reminder_days: reviewReminderDays,
			audit_report_cron: auditReportCron.trim(),
			trust_update_mode: trustUpdateMode.trim(),
			config: parsedConfig
		});
		saving = false;
		if (res.code !== 0) {
			toast.error(res.message || get(t)('governanceConfig.updateFailed'));
			return;
		}
		toast.success(get(t)('governanceConfig.updateSuccess'));
	}

	async function onProjectChange() {
		await loadConfig();
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.governanceConfig')}</title>
</svelte:head>

<div class="mx-auto max-w-4xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceConfig.title')}</h1>
		<p class="mt-1 text-sm text-slate-500">{$t('governanceConfig.subtitle')}</p>
	</div>

	<div class="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-800">
		{#if loading}
			<p class="text-sm text-slate-500">{$t('common.loading')}</p>
		{:else}
			<div class="space-y-4">
				<div>
					<label for="governance-config-project" class="mb-1 block text-sm text-slate-600">{$t('impactReview.project')}</label>
					<select id="governance-config-project" bind:value={selectedProjectId} onchange={onProjectChange} class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900">
						{#each groupedProjects as group}
							<optgroup label={group.workspaceName}>
								{#each group.items as project}
									<option value={project.id}>{project.name}</option>
								{/each}
							</optgroup>
						{/each}
					</select>
				</div>

				<div class="grid grid-cols-1 gap-4 md:grid-cols-2">
					<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-200">
						<input id="governance-config-review-required" type="checkbox" bind:checked={reviewRequired} />
						{$t('governanceConfig.reviewRequired')}
					</label>
					<div>
						<label for="governance-config-auto-review-days" class="mb-1 block text-sm text-slate-600">{$t('governanceConfig.autoReviewDays')}</label>
						<input id="governance-config-auto-review-days" bind:value={autoReviewDays} type="number" min="0" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
					</div>
					<div>
						<label for="governance-config-review-reminder-days" class="mb-1 block text-sm text-slate-600">{$t('governanceConfig.reviewReminderDays')}</label>
						<input id="governance-config-review-reminder-days" bind:value={reviewReminderDays} type="number" min="0" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
					</div>
					<div>
						<label for="governance-config-audit-report-cron" class="mb-1 block text-sm text-slate-600">{$t('governanceConfig.auditReportCron')}</label>
						<input id="governance-config-audit-report-cron" bind:value={auditReportCron} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
					</div>
					<div class="md:col-span-2">
						<label for="governance-config-trust-update-mode" class="mb-1 block text-sm text-slate-600">{$t('governanceConfig.trustUpdateMode')}</label>
						<input id="governance-config-trust-update-mode" bind:value={trustUpdateMode} type="text" class="w-full rounded-md border border-slate-300 px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900" />
					</div>
				</div>

				<div>
					<label for="governance-config-json" class="mb-1 block text-sm text-slate-600">{$t('governanceConfig.configJson')}</label>
					<textarea id="governance-config-json" bind:value={configText} rows="10" class="w-full rounded-md border border-slate-300 px-3 py-2 font-mono text-xs dark:border-slate-600 dark:bg-slate-900"></textarea>
				</div>

				<div class="flex justify-end">
					<button type="button" onclick={saveConfig} class="rounded-md bg-blue-600 px-4 py-2 text-sm text-white hover:bg-blue-700" disabled={saving}>
						{saving ? $t('common.saving') : $t('common.save')}
					</button>
				</div>
			</div>
		{/if}
	</div>
</div>

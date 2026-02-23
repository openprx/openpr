<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { authStore } from '$lib/stores/auth';
	import { vetoApi, type VetoerScope } from '$lib/api/veto';
	import { isAdminUser } from '$lib/utils/auth';
	import { trustApi, type Appeal, type AppealStatus } from '$lib/api/trust';
	import { toast } from '$lib/stores/toast';
	import Button from '$lib/components/Button.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import Input from '$lib/components/Input.svelte';
	import Textarea from '$lib/components/Textarea.svelte';

	let myVetoScopeKeys = $state<Set<string>>(new Set());

	const isProjectAdmin = $derived(isAdminUser($authStore.user));
	const canReview = $derived(isProjectAdmin || myVetoScopeKeys.size > 0);

	let loading = $state(false);
	let saving = $state(false);
	let showCreate = $state(false);
	let statusFilter = $state<'all' | AppealStatus>('all');
	let mineOnly = $state(true);
	let items = $state<Appeal[]>([]);
	let reviewingId = $state<number | null>(null);

	let createForm = $state({
		log_id: '',
		reason: '',
		evidence: ''
	});

	let reviewForm = $state({
		status: 'accepted' as 'accepted' | 'rejected',
		review_note: ''
	});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		await loadMyVetoScopes();
		const res = await trustApi.listAppeals({ status: statusFilter, mine: mineOnly });
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('appeals.loadFailed'));
			items = [];
			loading = false;
			return;
		}
		items = res.data.items;
		loading = false;
	}

	async function loadMyVetoScopes() {
		const meId = $authStore.user?.id;
		if (!meId) {
			myVetoScopeKeys = new Set();
			return;
		}

		const res = await vetoApi.listVetoers();
		if (res.code !== 0 || !res.data) {
			myVetoScopeKeys = new Set();
			return;
		}

		const next = new Set<string>();
		for (const row of res.data.items || []) {
			if (row.user_id === meId && row.project_id && row.domain) {
				next.add(`${row.project_id}:${row.domain}`);
			}
		}
		myVetoScopeKeys = next;
	}

	function canReviewAppeal(item: Appeal): boolean {
		if (isProjectAdmin) {
			return true;
		}
		if (!item.project_id || !item.domain) {
			return false;
		}
		return myVetoScopeKeys.has(`${item.project_id}:${item.domain}`);
	}

	function statusTone(status: AppealStatus): string {
		switch (status) {
			case 'accepted':
				return 'bg-emerald-100 text-emerald-700';
			case 'rejected':
				return 'bg-red-100 text-red-700';
			default:
				return 'bg-amber-100 text-amber-700';
		}
	}

	async function createAppeal() {
		const logId = Number(createForm.log_id);
		if (!Number.isInteger(logId) || logId <= 0 || !createForm.reason.trim()) {
			toast.error(get(t)('appeals.invalidCreateForm'));
			return;
		}

		saving = true;
		let evidence: unknown = undefined;
		if (createForm.evidence.trim()) {
			try {
				evidence = JSON.parse(createForm.evidence);
			} catch {
				toast.error(get(t)('appeals.invalidEvidence'));
				saving = false;
				return;
			}
		}

		const res = await trustApi.createAppeal({
			log_id: logId,
			reason: createForm.reason.trim(),
			evidence
		});
		saving = false;
		if (res.code !== 0) {
			toast.error(res.message || get(t)('appeals.createFailed'));
			return;
		}
		toast.success(get(t)('appeals.createSuccess'));
		showCreate = false;
		createForm = { log_id: '', reason: '', evidence: '' };
		await load();
	}

	async function reviewAppeal(item: Appeal, status: 'accepted' | 'rejected') {
		reviewingId = item.id;
		const res = await trustApi.updateAppeal(item.id, {
			status,
			review_note: reviewForm.review_note.trim() || undefined
		});
		reviewingId = null;
		if (res.code !== 0) {
			toast.error(res.message || get(t)('appeals.reviewFailed'));
			return;
		}
		toast.success(get(t)('appeals.reviewSuccess'));
		reviewForm = { status: 'accepted', review_note: '' };
		await load();
	}

	async function removeAppeal(item: Appeal) {
		if (!confirm(get(t)('appeals.deleteConfirm'))) {
			return;
		}
		const res = await trustApi.deleteAppeal(item.id);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('appeals.deleteFailed'));
			return;
		}
		toast.success(get(t)('appeals.deleteSuccess'));
		await load();
	}

	function appealStatusLabel(status: AppealStatus): string {
		return get(t)(`appeals.status.${status}`) || status;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.trustAppeals')}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900">
		<div class="mb-4 flex flex-wrap items-center justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-slate-900 dark:text-slate-100">{$t('appeals.title')}</h1>
				<p class="text-sm text-slate-500">{$t('appeals.subtitle')}</p>
			</div>
			<Button onclick={() => (showCreate = true)}>{$t('appeals.create')}</Button>
		</div>

		<div class="mb-4 flex flex-wrap gap-2">
			<select bind:value={statusFilter} onchange={load} class="rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
				<option value="all">{$t('appeals.status.all')}</option>
				<option value="pending">{$t('appeals.status.pending')}</option>
				<option value="accepted">{$t('appeals.status.accepted')}</option>
				<option value="rejected">{$t('appeals.status.rejected')}</option>
			</select>
			<label class="inline-flex items-center gap-2 rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100">
				<input type="checkbox" bind:checked={mineOnly} onchange={load} />
				{$t('appeals.mineOnly')}
			</label>
		</div>

		{#if loading}
			<div class="py-8 text-center text-slate-500">{$t('common.loading')}</div>
		{:else if items.length === 0}
			<div class="py-8 text-center text-slate-500">{$t('appeals.empty')}</div>
		{:else}
			<div class="space-y-2">
				{#each items as item (item.id)}
					<div class="rounded-lg border border-slate-200 p-3 dark:border-slate-700">
						<div class="flex flex-wrap items-start justify-between gap-3">
							<div class="min-w-0 flex-1">
								<div class="flex items-center gap-2">
									<span class="font-medium text-slate-900 dark:text-slate-100">{$t('appeals.logId', { values: { id: item.log_id } })}</span>
									<span class={`rounded px-2 py-0.5 text-xs ${statusTone(item.status)}`}>{appealStatusLabel(item.status)}</span>
								</div>
								<p class="mt-1 text-sm text-slate-700 dark:text-slate-300">{item.reason}</p>
								<p class="mt-1 text-xs text-slate-500">{$t('appeals.meta', { values: { appellant: item.appellant_id, created: new Date(item.created_at).toLocaleString() } })}</p>
								{#if item.review_note}
									<p class="mt-1 text-xs text-slate-500">{$t('appeals.reviewNote')}: {item.review_note}</p>
								{/if}
							</div>

							<div class="flex flex-wrap gap-2">
								{#if item.status === 'pending' && canReviewAppeal(item)}
									<Button size="sm" onclick={() => reviewAppeal(item, 'accepted')} loading={reviewingId === item.id}>{$t('appeals.accept')}</Button>
									<Button size="sm" variant="danger" onclick={() => reviewAppeal(item, 'rejected')} loading={reviewingId === item.id}>{$t('appeals.reject')}</Button>
								{/if}
								{#if item.status === 'pending' && item.appellant_id === $authStore.user?.id}
									<Button size="sm" variant="secondary" onclick={() => removeAppeal(item)}>{$t('common.delete')}</Button>
								{/if}
							</div>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<Modal bind:open={showCreate} title={$t('appeals.createTitle')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			void createAppeal();
		}}
		class="space-y-3"
	>
		<Input type="number" label={$t('appeals.logIdInput')} bind:value={createForm.log_id} required />
		<Textarea label={$t('appeals.reasonInput')} bind:value={createForm.reason} rows={4} required />
		<Textarea label={$t('appeals.evidenceInput')} bind:value={createForm.evidence} rows={3} hint={$t('appeals.evidenceHint')} />
		{#if canReview}
			<Textarea label={$t('appeals.reviewNote')} bind:value={reviewForm.review_note} rows={2} />
		{/if}
		<div class="flex justify-end gap-2">
			<Button variant="secondary" onclick={() => (showCreate = false)}>{$t('common.cancel')}</Button>
			<Button type="submit" loading={saving}>{$t('common.submit')}</Button>
		</div>
	</form>
</Modal>

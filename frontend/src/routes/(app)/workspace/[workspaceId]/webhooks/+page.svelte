<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { membersApi, type WorkspaceMember } from '$lib/api/members';
	import {
		webhooksApi,
		type Webhook,
		type CreateWebhookData,
		type WebhookDelivery
	} from '$lib/api/webhooks';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import Button from '$lib/components/Button.svelte';
	import BotIcon from '$lib/components/BotIcon.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const eventOptions = ['issue.created', 'issue.updated', 'comment.created'];

	let loading = $state(true);
	let submitting = $state(false);
	let deletingId = $state<string | null>(null);
	let webhooks = $state<Webhook[]>([]);
	let botUsers = $state<WorkspaceMember[]>([]);

	let showModal = $state(false);
	let editingWebhook = $state<Webhook | null>(null);
	let botUserId = $state('');
	let form = $state({
		url: '',
		events: ['issue.created'] as string[],
		is_active: true,
		secret: ''
	});

	let showDeliveriesModal = $state(false);
	let deliveriesLoading = $state(false);
	let selectedWebhook = $state<Webhook | null>(null);
	let deliveries = $state<WebhookDelivery[]>([]);
	let deliveriesPage = $state(1);
	let deliveriesPerPage = 20;
	let deliveriesTotal = $state(0);
	let deliveriesTotalPages = $state(1);

	let showDeliveryDetailModal = $state(false);
	let deliveryDetailLoading = $state(false);
	let selectedDelivery = $state<WebhookDelivery | null>(null);
	let deliveryTab = $state<'request' | 'response'>('request');

	onMount(async () => {
		await Promise.all([loadWebhooks(), loadBots()]);
	});

	async function loadWebhooks() {
		loading = true;
		const response = await webhooksApi.list(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			webhooks = response.data.items ?? [];
		}
		loading = false;
	}

	function openCreate() {
		editingWebhook = null;
		form = { url: '', events: ['issue.created'], is_active: true, secret: '' };
		botUserId = '';
		showModal = true;
	}

	function openEdit(webhook: Webhook) {
		editingWebhook = webhook;
		form = {
			url: webhook.url,
			events: webhook.events,
			is_active: webhook.is_active ?? webhook.active ?? true,
			secret: webhook.secret ?? ''
		};
		botUserId = webhook.bot_user_id ?? '';
		showModal = true;
	}

	function editWebhook(webhook: Webhook) {
		openEdit(webhook);
	}

	async function loadBots() {
		const response = await membersApi.list(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		botUsers = (response.data?.items ?? []).filter((member) => member.entity_type === 'bot');
	}

	function toggleEvent(eventName: string) {
		if (form.events.includes(eventName)) {
			form.events = form.events.filter((value) => value !== eventName);
		} else {
			form.events = [...form.events, eventName];
		}
	}

	async function saveWebhook() {
		if (!form.url.trim()) {
			toast.error(get(t)('webhook.enterUrl'));
			return;
		}
		if (form.events.length === 0) {
			toast.error(get(t)('webhook.selectOneEvent'));
			return;
		}

		submitting = true;
		const payload: CreateWebhookData = {
			url: form.url.trim(),
			events: form.events,
			is_active: form.is_active,
			active: form.is_active,
			secret: form.secret.trim() || undefined,
			bot_user_id: botUserId || undefined
		};

		const response = editingWebhook
			? await webhooksApi.update(workspaceId, editingWebhook.id, payload)
			: await webhooksApi.create(workspaceId, payload);

		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)(editingWebhook ? 'webhook.updated' : 'webhook.created'));
			showModal = false;
			await loadWebhooks();
		}

		submitting = false;
	}

	async function deleteWebhook(webhook: Webhook) {
		if (!confirm(get(t)('webhook.deleteConfirm', { values: { url: webhook.url } }))) {
			return;
		}

		deletingId = webhook.id;
		const response = await webhooksApi.delete(workspaceId, webhook.id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('webhook.deleted'));
			await loadWebhooks();
		}
		deletingId = null;
	}

	async function viewDeliveries(webhook: Webhook) {
		selectedWebhook = webhook;
		deliveriesPage = 1;
		deliveries = [];
		deliveriesTotal = 0;
		deliveriesTotalPages = 1;
		showDeliveriesModal = true;
		await loadDeliveries(1);
	}

	async function loadDeliveries(pageToLoad: number = deliveriesPage) {
		if (!selectedWebhook) return;

		deliveriesLoading = true;
		const response = await webhooksApi.listDeliveries(
			workspaceId,
			selectedWebhook.id,
			pageToLoad,
			deliveriesPerPage
		);
		if (response.code !== 0) {
			toast.error(response.message);
			deliveries = [];
			deliveriesTotal = 0;
			deliveriesTotalPages = 1;
		} else if (response.data) {
			deliveries = response.data.items ?? [];
			deliveriesTotal = response.data.total ?? 0;
			deliveriesPage = response.data.page ?? pageToLoad;
			deliveriesTotalPages = Math.max(1, response.data.total_pages ?? 1);
		}
		deliveriesLoading = false;
	}

	async function openDeliveryDetail(delivery: WebhookDelivery) {
		if (!selectedWebhook) return;
		showDeliveryDetailModal = true;
		deliveryDetailLoading = true;
		deliveryTab = 'request';
		selectedDelivery = delivery;

		const response = await webhooksApi.getDelivery(workspaceId, selectedWebhook.id, delivery.id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			selectedDelivery = response.data;
		}

		deliveryDetailLoading = false;
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function getBotName(botId: string | null | undefined): string {
		if (!botId) return '';
		const bot = botUsers.find((item) => item.user_id === botId);
		return bot?.name || botId.slice(0, 8);
	}

	function getBotType(botId: string | null | undefined): string {
		if (!botId) return '';
		const bot = botUsers.find((item) => item.user_id === botId);
		if (bot?.agent_type === 'openclaw') return get(t)('agentTypes.openclaw');
		if (bot?.agent_type === 'webhook') return get(t)('agentTypes.webhook');
		if (bot?.agent_type === 'custom') return get(t)('agentTypes.custom');
		return get(t)('admin.botBadge');
	}

	function getDeliveryStatus(delivery: WebhookDelivery): {
		label: string;
		className: string;
	} {
		if (delivery.response_status === null) {
			return {
				label: `❌ ${delivery.error ?? 'network error'}`,
				className: 'text-red-600'
			};
		}
		if (delivery.response_status >= 200 && delivery.response_status <= 299) {
			return {
				label: `✅ ${delivery.response_status}`,
				className: 'text-green-600'
			};
		}
		if (delivery.response_status >= 400 && delivery.response_status <= 499) {
			return {
				label: `⚠️ ${delivery.response_status}`,
				className: 'text-amber-600'
			};
		}
		if (delivery.response_status >= 500) {
			return {
				label: `❌ ${delivery.response_status}`,
				className: 'text-red-600'
			};
		}
		return {
			label: String(delivery.response_status),
			className: 'text-slate-600 dark:text-slate-300'
		};
	}

	function formatDuration(durationMs: number | null): string {
		if (durationMs === null || durationMs === undefined) return '--';
		return `${durationMs}ms`;
	}

	function getPageNumbers(current: number, totalCount: number): Array<number | '...'> {
		if (totalCount <= 7) {
			return Array.from({ length: totalCount }, (_, i) => i + 1);
		}

		const pages = new Set<number>([1, totalCount, current - 1, current, current + 1]);
		if (current <= 3) {
			pages.add(2);
			pages.add(3);
		}
		if (current >= totalCount - 2) {
			pages.add(totalCount - 1);
			pages.add(totalCount - 2);
		}

		const sorted = [...pages].filter((p) => p >= 1 && p <= totalCount).sort((a, b) => a - b);
		const result: Array<number | '...'> = [];
		for (let i = 0; i < sorted.length; i += 1) {
			const pageNum = sorted[i];
			const prev = sorted[i - 1];
			if (i > 0 && pageNum - prev > 1) {
				result.push('...');
			}
			result.push(pageNum);
		}
		return result;
	}

	function goToDeliveriesPage(pageNum: number) {
		if (pageNum < 1 || pageNum > deliveriesTotalPages || pageNum === deliveriesPage) {
			return;
		}
		deliveriesPage = pageNum;
		void loadDeliveries(pageNum);
	}

	function formatJson(value: unknown): string {
		if (value === null || value === undefined) return '';
		try {
			return JSON.stringify(value, null, 2);
		} catch {
			return String(value);
		}
	}

	function formatHeaders(headers: Record<string, unknown> | null): string {
		if (!headers) return '';
		return Object.entries(headers)
			.map(([key, value]) => `${key}: ${String(value)}`)
			.join('\n');
	}

	function formatResponseBody(body: string | null): string {
		if (!body) return '';
		try {
			const parsed = JSON.parse(body);
			return JSON.stringify(parsed, null, 2);
		} catch {
			return body;
		}
	}

	function buildRequestText(delivery: WebhookDelivery | null): string {
		if (!delivery || !selectedWebhook) return '';
		const payloadText = formatJson(delivery.payload);
		const headerText = formatHeaders(delivery.request_headers);
		return `POST ${selectedWebhook.url}\n${headerText}\n\n${payloadText}`;
	}

	function buildResponseText(delivery: WebhookDelivery | null): string {
		if (!delivery) return '';
		if (delivery.response_status === null) {
			return `ERROR\n\n${delivery.error ?? 'Network error'}`;
		}
		return `HTTP ${delivery.response_status}\n\n${formatResponseBody(delivery.response_body)}`;
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('webhook.title')}</h1>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('webhook.subtitle')}</p>
		</div>
		<Button onclick={openCreate}>{$t('webhook.create')}</Button>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
	{:else if webhooks.length === 0}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-10 text-center text-slate-500 dark:text-slate-400">{$t('webhook.empty')}</div>
	{:else}
		<div class="overflow-hidden rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900">
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-slate-200 dark:divide-slate-700">
					<thead class="bg-slate-50 dark:bg-slate-950">
						<tr>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('webhook.url')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('webhook.events')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('webhook.linkedBot')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.status')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.createdAt')}</th>
							<th class="px-4 py-3 text-right text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-200 dark:divide-slate-700 bg-white dark:bg-slate-900">
						{#each webhooks as webhook (webhook.id)}
							<tr>
								<td class="max-w-xs px-4 py-3 text-sm text-slate-900 dark:text-slate-100">{webhook.url}</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{webhook.events.join(', ')}</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">
									{#if webhook.bot_user_id}
										<div class="inline-flex items-center gap-1.5 rounded-full bg-cyan-100 px-2 py-0.5 text-xs font-medium text-cyan-700 dark:bg-cyan-900/30 dark:text-cyan-300">
											<BotIcon class="h-3.5 w-3.5" title={$t('admin.botBadge')} />
											<span>{getBotName(webhook.bot_user_id)} ({getBotType(webhook.bot_user_id)})</span>
										</div>
									{:else}
										<span class="text-slate-400">{$t('webhook.noBot')}</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-sm">
									<span class="inline-flex rounded-full px-2.5 py-1 text-xs font-medium {(webhook.is_active ?? webhook.active)
										? 'bg-green-100 text-green-700'
										: 'bg-slate-200 text-slate-700 dark:text-slate-300'}">
										{(webhook.is_active ?? webhook.active) ? 'active' : 'inactive'}
									</span>
								</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{formatDate(webhook.created_at)}</td>
								<td class="px-4 py-3 text-right">
									<div class="flex items-center justify-end gap-3">
										<button class="text-sm text-slate-500 hover:text-blue-600 transition-colors" onclick={() => viewDeliveries(webhook)}>
											{$t('webhook.records')}
										</button>
										<button class="text-sm text-slate-500 hover:text-blue-600 transition-colors" onclick={() => editWebhook(webhook)}>
											{$t('common.edit')}
										</button>
										<button
											class="text-sm text-slate-500 hover:text-red-600 transition-colors disabled:opacity-60"
											disabled={deletingId === webhook.id}
											onclick={() => deleteWebhook(webhook)}
										>
											{$t('common.delete')}
										</button>
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</div>
	{/if}
</div>

<Modal bind:open={showModal} title={editingWebhook ? $t('webhook.edit') : $t('webhook.create')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			saveWebhook();
		}}
		class="space-y-4"
	>
		<Input type="url" label="Webhook URL" bind:value={form.url} required placeholder={$t('placeholders.webhookUrl')} />
		<Input label={$t('webhook.secretOptional')} bind:value={form.secret} />
		<div>
			<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="botUserId">{$t('webhook.linkedBot')}</label>
			<select
				id="botUserId"
				bind:value={botUserId}
				class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			>
				<option value="">{$t('webhook.noBot')}</option>
				{#each botUsers as bot (bot.user_id)}
					<option value={bot.user_id}>{bot.name} ({getBotType(bot.user_id)})</option>
				{/each}
			</select>
		</div>
		<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
			<input type="checkbox" class="h-4 w-4 rounded border-slate-300 text-blue-600 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800" bind:checked={form.is_active} />
			{$t('webhook.enable')}
		</label>

		<div>
			<p class="mb-2 text-sm font-medium text-slate-700 dark:text-slate-300">{$t('webhook.eventTypes')}</p>
			<div class="space-y-2">
				{#each eventOptions as eventName}
					<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
						<input
							type="checkbox"
							class="h-4 w-4 rounded border-slate-300 text-blue-600 focus:ring-blue-500 dark:focus:ring-blue-400 dark:border-slate-600 dark:bg-slate-800"
							checked={form.events.includes(eventName)}
							onchange={() => toggleEvent(eventName)}
						/>
						{eventName}
					</label>
				{/each}
			</div>
		</div>

		<div class="flex justify-end gap-2 pt-2">
			<Button variant="secondary" onclick={() => (showModal = false)}>{$t('common.cancel')}</Button>
			<Button type="submit" loading={submitting}>{editingWebhook ? $t('common.save') : $t('common.create')}</Button>
		</div>
	</form>
</Modal>

<Modal
	bind:open={showDeliveriesModal}
	title={selectedWebhook ? `${$t('webhook.deliveries')} · ${selectedWebhook.url}` : $t('webhook.deliveries')}
	maxWidthClass="max-w-4xl"
	onclose={() => {
		showDeliveryDetailModal = false;
		selectedDelivery = null;
	}}
>
	<div class="space-y-4">
		{#if deliveriesLoading}
			<div class="rounded-md border border-slate-200 dark:border-slate-700 p-4 text-sm text-slate-500 dark:text-slate-400">{$t('webhook.loadingDeliveries')}</div>
		{:else if deliveries.length === 0}
			<div class="rounded-md border border-slate-200 dark:border-slate-700 p-4 text-sm text-slate-500 dark:text-slate-400">{$t('webhook.noDeliveries')}</div>
		{:else}
			<div class="overflow-x-auto rounded-lg border border-slate-200 dark:border-slate-700">
				<table class="min-w-full divide-y divide-slate-200 dark:divide-slate-700 text-sm">
					<thead class="bg-slate-50 dark:bg-slate-950">
						<tr>
							<th class="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('webhook.deliveryTime')}</th>
							<th class="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('webhook.deliveryEvent')}</th>
							<th class="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('webhook.deliveryStatus')}</th>
							<th class="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('webhook.deliveryDuration')}</th>
							<th class="px-3 py-2 text-right text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-200 dark:divide-slate-700 bg-white dark:bg-slate-900">
						{#each deliveries as delivery (delivery.id)}
							<tr>
								<td class="px-3 py-2 text-slate-700 dark:text-slate-300">{formatDate(delivery.created_at)}</td>
								<td class="px-3 py-2 text-slate-700 dark:text-slate-300">{delivery.event_type}</td>
								<td class={`px-3 py-2 font-medium ${getDeliveryStatus(delivery).className}`}>{getDeliveryStatus(delivery).label}</td>
								<td class="px-3 py-2 text-slate-700 dark:text-slate-300">{formatDuration(delivery.duration_ms)}</td>
								<td class="px-3 py-2 text-right">
									<button class="text-sm text-slate-500 hover:text-blue-600 transition-colors" onclick={() => openDeliveryDetail(delivery)}>
										{$t('webhook.deliveryDetail')}
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			<div class="flex items-center justify-between text-sm">
				<div class="text-slate-500 dark:text-slate-400">
					{$t('webhook.deliveryPagination', { values: { total: deliveriesTotal, page: deliveriesPage, pages: deliveriesTotalPages } })}
				</div>
				<div class="flex items-center gap-1">
					<button
						class="rounded border border-slate-300 dark:border-slate-600 px-2 py-1 text-slate-600 dark:text-slate-300 disabled:opacity-50"
						disabled={deliveriesPage <= 1}
						onclick={() => goToDeliveriesPage(deliveriesPage - 1)}
					>
						{$t('webhook.prevPage')}
					</button>
					{#each getPageNumbers(deliveriesPage, deliveriesTotalPages) as pageNum}
						{#if pageNum === '...'}
							<span class="px-2 py-1 text-slate-400">...</span>
						{:else}
							<button
								class={`rounded px-2 py-1 ${pageNum === deliveriesPage
									? 'bg-blue-600 text-white'
									: 'border border-slate-300 dark:border-slate-600 text-slate-600 dark:text-slate-300'}`}
								onclick={() => goToDeliveriesPage(pageNum)}
							>
								{pageNum}
							</button>
						{/if}
					{/each}
					<button
						class="rounded border border-slate-300 dark:border-slate-600 px-2 py-1 text-slate-600 dark:text-slate-300 disabled:opacity-50"
						disabled={deliveriesPage >= deliveriesTotalPages}
						onclick={() => goToDeliveriesPage(deliveriesPage + 1)}
					>
						{$t('webhook.nextPage')}
					</button>
				</div>
			</div>
		{/if}
	</div>
</Modal>

<Modal
	bind:open={showDeliveryDetailModal}
	title={selectedDelivery ? `${$t('webhook.deliveryDetailTitle')} · ${selectedDelivery.id.slice(0, 8)}` : $t('webhook.deliveryDetailTitle')}
	maxWidthClass="max-w-4xl"
>
	{#if deliveryDetailLoading}
		<div class="text-sm text-slate-500 dark:text-slate-400">{$t('webhook.loadingDetail')}</div>
	{:else if selectedDelivery}
		<div class="space-y-4">
			<div class="flex items-center gap-2 border-b border-slate-200 dark:border-slate-700 pb-2">
				<button
					class={`px-3 py-1 text-sm rounded-md transition-colors ${deliveryTab === 'request'
						? 'bg-blue-600 text-white'
						: 'text-slate-500 hover:text-blue-600'}`}
					onclick={() => (deliveryTab = 'request')}
				>
					{$t('webhook.request')}
				</button>
				<button
					class={`px-3 py-1 text-sm rounded-md transition-colors ${deliveryTab === 'response'
						? 'bg-blue-600 text-white'
						: 'text-slate-500 hover:text-blue-600'}`}
					onclick={() => (deliveryTab = 'response')}
				>
					{$t('webhook.response')}
				</button>
			</div>

			{#if deliveryTab === 'request'}
				<pre class="max-h-[480px] overflow-auto rounded-lg bg-slate-900 p-4 text-green-400 font-mono text-xs sm:text-sm whitespace-pre-wrap">{buildRequestText(selectedDelivery)}</pre>
			{:else}
				<pre class="max-h-[480px] overflow-auto rounded-lg bg-slate-900 p-4 text-green-400 font-mono text-xs sm:text-sm whitespace-pre-wrap">{buildResponseText(selectedDelivery)}</pre>
			{/if}
		</div>
	{/if}
</Modal>

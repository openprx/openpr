<script lang="ts">
	import { Activity, Bot, Cable, CheckCircle2, ChevronDown, ChevronUp, PlugZap, RefreshCw, RotateCcw, Route, Webhook, XCircle } from '@lucide/svelte';
	import { onMount } from 'svelte';
	import {
		connectorsApi,
		type Connector,
		type EventInbox,
		type Invocation,
		type InvocationStatus,
		type InvocationToolCall
	} from '$lib/api/connectors';
	import type { FormField, UniversalForm } from '$lib/api/forms';
	import Button from '$lib/components/Button.svelte';
	import { toast } from '$lib/stores/toast';
	import { t } from 'svelte-i18n';

	interface Props {
		form: UniversalForm;
		printJobCount?: number;
	}

	let { form, printJobCount = 0 }: Props = $props();

	let loading = $state(false);
	let actionLoading = $state(false);
	let routingSavingConnectorId = $state<string | null>(null);
	let expandedInvocationId = $state<string | null>(null);
	let formInboxLoading = $state(false);
	let inboxLoadingInvocationIds = $state<Record<string, boolean>>({});
	let inboxReplayingIds = $state<Record<string, boolean>>({});
	let formInboxRows = $state<EventInbox[]>([]);
	let inboxByInvocation = $state<Record<string, EventInbox[]>>({});
	let toolCallsLoadingInvocationIds = $state<Record<string, boolean>>({});
	let toolCallsByInvocation = $state<Record<string, InvocationToolCall[]>>({});
	let connectors = $state<Connector[]>([]);
	let invocations = $state<Invocation[]>([]);
	let subscriptionDrafts = $state<Record<string, SubscriptionDraft>>({});

	type RoutingCondition = {
		field: string;
		operator: string;
		value: string;
	};

	type SubscriptionDraft = {
		events: string[];
		routingKey: string;
		conditions: RoutingCondition[];
	};

	const formEventOptions = [
		'form.record.created',
		'form.record.updated',
		'form.record.deleted',
		'form.record.restored'
	];
	const routingOperators = ['equals', 'not_equals', 'contains', 'not_empty'];
	const formFields = $derived(parseFormFields(form.schema));

	const formConnectors = $derived(
		connectors.filter((connector) => !connector.project_id || connector.project_id === form.project_id)
	);
	const activeConnectors = $derived(formConnectors.filter((connector) => connector.is_active));
	const formInvocations = $derived(invocations.filter((invocation) => invocationBelongsToForm(invocation)));
	const latestInvocation = $derived(formInvocations[0] ?? null);
	const failedInvocation = $derived(formInvocations.find((invocation) => invocation.status === 'failed') ?? null);
	const activeInvocation = $derived(
		formInvocations.find((invocation) => ['pending', 'dispatched', 'running'].includes(invocation.status)) ?? null
	);
	const rows = $derived([
		{
			key: 'mcp',
			icon: Bot,
			title: $t('forms.automation.mcp'),
			value: 'forms.*, form_records.*, events.tail'
		},
		{
			key: 'webhooks',
			icon: Webhook,
			title: $t('forms.automation.webhooks'),
			value: 'business_events'
		},
		{
			key: 'plugins',
			icon: PlugZap,
			title: $t('forms.automation.plugins'),
			value: form.key
		},
		{
			key: 'connectors',
			icon: Cable,
			title: $t('forms.automation.connectors'),
			value: `${activeConnectors.length}/${formConnectors.length || 0}`
		}
	]);

	onMount(() => {
		void loadAutomation();
	});

	$effect(() => {
		if (form.id) void loadAutomation();
	});

	function invocationBelongsToForm(invocation: Invocation): boolean {
		return (
			(invocation.trigger_ref_type === 'form' && invocation.trigger_ref_id === form.id) ||
			invocation.payload?.form_id === form.id ||
			invocation.payload?.form_key === form.key
		);
	}

	function isRecord(value: unknown): value is Record<string, unknown> {
		return Boolean(value && typeof value === 'object' && !Array.isArray(value));
	}

	function asStringArray(value: unknown): string[] {
		return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
	}

	function parseFormFields(schema: Record<string, unknown>): FormField[] {
		return Array.isArray(schema.fields)
			? schema.fields.filter((field): field is FormField => isRecord(field) && typeof field.key === 'string')
			: [];
	}

	function formRoutingConfig(manifest: Record<string, unknown>): Record<string, unknown> {
		const routing = isRecord(manifest.routing) ? manifest.routing : {};
		const forms = isRecord(routing.forms) ? routing.forms : {};
		const route = forms[form.key];
		return isRecord(route) ? route : {};
	}

	function connectorSubscribedEvents(connector: Connector): string[] {
		const manifest = connector.capability_manifest ?? {};
		const route = formRoutingConfig(manifest);
		const routeEvents = asStringArray(route.events);
		if (routeEvents.length > 0) return routeEvents;
		const manifestForms = asStringArray(manifest.forms);
		if (manifestForms.length > 0 && !manifestForms.includes(form.key)) return [];
		return asStringArray(manifest.events).filter((event) => formEventOptions.includes(event));
	}

	function connectorRoutingKey(connector: Connector): string {
		const manifest = connector.capability_manifest ?? {};
		const route = formRoutingConfig(manifest);
		return typeof route.routing_key === 'string' ? route.routing_key : '';
	}

	function connectorRoutingConditions(connector: Connector): RoutingCondition[] {
		const manifest = connector.capability_manifest ?? {};
		const route = formRoutingConfig(manifest);
		const rawConditions = Array.isArray(route.conditions) ? route.conditions : [];
		return rawConditions
			.filter((condition): condition is Record<string, unknown> => isRecord(condition))
			.map((condition) => ({
				field: typeof condition.field === 'string' ? condition.field : '',
				operator: typeof condition.operator === 'string' ? condition.operator : 'equals',
				value: typeof condition.value === 'string' ? condition.value : ''
			}))
			.filter((condition) => condition.field);
	}

	function syncSubscriptionDrafts(items: Connector[]) {
		subscriptionDrafts = Object.fromEntries(
			items.map((connector) => [
				connector.id,
				{
					events: connectorSubscribedEvents(connector),
					routingKey: connectorRoutingKey(connector),
					conditions: connectorRoutingConditions(connector)
				}
			])
		);
	}

	function emptySubscriptionDraft(connector?: Connector): SubscriptionDraft {
		return {
			events: connector ? connectorSubscribedEvents(connector) : [],
			routingKey: connector ? connectorRoutingKey(connector) : '',
			conditions: connector ? connectorRoutingConditions(connector) : []
		};
	}

	function subscriptionDraft(connector: Connector): SubscriptionDraft {
		return (
			subscriptionDrafts[connector.id] ?? emptySubscriptionDraft(connector)
		);
	}

	function setConnectorEvent(connectorId: string, eventName: string, checked: boolean) {
		const current = subscriptionDrafts[connectorId] ?? emptySubscriptionDraft();
		const nextEvents = checked
			? Array.from(new Set([...current.events, eventName]))
			: current.events.filter((event) => event !== eventName);
		subscriptionDrafts = {
			...subscriptionDrafts,
			[connectorId]: { ...current, events: nextEvents }
		};
	}

	function setConnectorRoutingKey(connectorId: string, routingKey: string) {
		const current = subscriptionDrafts[connectorId] ?? emptySubscriptionDraft();
		subscriptionDrafts = {
			...subscriptionDrafts,
			[connectorId]: { ...current, routingKey }
		};
	}

	function addRoutingCondition(connectorId: string) {
		const current = subscriptionDrafts[connectorId] ?? emptySubscriptionDraft();
		const field = formFields[0]?.key ?? '';
		subscriptionDrafts = {
			...subscriptionDrafts,
			[connectorId]: {
				...current,
				conditions: [...current.conditions, { field, operator: 'equals', value: '' }]
			}
		};
	}

	function updateRoutingCondition(connectorId: string, index: number, patch: Partial<RoutingCondition>) {
		const current = subscriptionDrafts[connectorId] ?? emptySubscriptionDraft();
		subscriptionDrafts = {
			...subscriptionDrafts,
			[connectorId]: {
				...current,
				conditions: current.conditions.map((condition, conditionIndex) =>
					conditionIndex === index ? { ...condition, ...patch } : condition
				)
			}
		};
	}

	function removeRoutingCondition(connectorId: string, index: number) {
		const current = subscriptionDrafts[connectorId] ?? emptySubscriptionDraft();
		subscriptionDrafts = {
			...subscriptionDrafts,
			[connectorId]: {
				...current,
				conditions: current.conditions.filter((_, conditionIndex) => conditionIndex !== index)
			}
		};
	}

	function buildCapabilityManifest(connector: Connector): Record<string, unknown> {
		const draft = subscriptionDraft(connector);
		const manifest = { ...(connector.capability_manifest ?? {}) };
		const existingForms = asStringArray(manifest.forms).filter((key) => key !== form.key);
		const existingEvents = asStringArray(manifest.events).filter((event) => !formEventOptions.includes(event));
		if (draft.events.length > 0) {
			manifest.forms = Array.from(new Set([...existingForms, form.key]));
			manifest.events = Array.from(new Set([...existingEvents, ...draft.events]));
		} else {
			manifest.forms = existingForms;
			manifest.events = existingEvents;
		}
		const routing = isRecord(manifest.routing) ? { ...manifest.routing } : {};
		const forms = isRecord(routing.forms) ? { ...routing.forms } : {};
		if (draft.events.length > 0) {
			forms[form.key] = {
				events: draft.events,
				routing_key: draft.routingKey.trim(),
				conditions: draft.conditions
					.map((condition) => ({
						field: condition.field.trim(),
						operator: condition.operator.trim() || 'equals',
						value: condition.value.trim()
					}))
					.filter((condition) => condition.field && condition.operator)
			};
		} else {
			delete forms[form.key];
		}
		manifest.routing = { ...routing, forms };
		return manifest;
	}

	async function saveConnectorRouting(connector: Connector) {
		routingSavingConnectorId = connector.id;
		const response = await connectorsApi.update(form.workspace_id, connector.id, {
			capability_manifest: buildCapabilityManifest(connector)
		});
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
		} else {
			connectors = connectors.map((item) => (item.id === response.data?.id ? response.data : item));
			syncSubscriptionDrafts(connectors.map((item) => (item.id === response.data?.id ? response.data : item)));
			toast.success($t('forms.automation.routingSaved'));
		}
		routingSavingConnectorId = null;
	}

	function statusClass(status: string): string {
		switch (status) {
			case 'completed':
			case 'succeeded':
				return 'bg-emerald-50 text-emerald-700 ring-emerald-200 dark:bg-emerald-950/30 dark:text-emerald-300 dark:ring-emerald-800';
			case 'failed':
				return 'bg-red-50 text-red-700 ring-red-200 dark:bg-red-950/30 dark:text-red-300 dark:ring-red-800';
			case 'running':
			case 'dispatched':
				return 'bg-blue-50 text-blue-700 ring-blue-200 dark:bg-blue-950/30 dark:text-blue-300 dark:ring-blue-800';
			case 'cancelled':
				return 'bg-slate-100 text-slate-600 ring-slate-200 dark:bg-slate-800 dark:text-slate-300 dark:ring-slate-700';
			default:
				return 'bg-amber-50 text-amber-700 ring-amber-200 dark:bg-amber-950/30 dark:text-amber-300 dark:ring-amber-800';
		}
	}

	function formatDate(value: string): string {
		try {
			return new Intl.DateTimeFormat(undefined, {
				month: 'short',
				day: 'numeric',
				hour: '2-digit',
				minute: '2-digit'
			}).format(new Date(value));
		} catch {
			return value;
		}
	}

	function connectorLabel(invocation: Invocation): string {
		const connector = connectors.find((item) => item.id === invocation.connector_id);
		return connector?.name ?? invocation.connector_kind ?? invocation.connector_id ?? $t('forms.automation.noConnector');
	}

	function connectorEndpoint(connector: Connector): string {
		return connector.endpoint || connector.id;
	}

	function connectorAuthMode(connector: Connector): string {
		const mode = connector.auth_policy?.mode;
		if (typeof mode === 'string' && mode.length > 0) return mode;
		return Object.keys(connector.auth_policy ?? {}).length > 0 ? 'custom' : 'none';
	}

	function connectorLastInvocation(connector: Connector): Invocation | null {
		return formInvocations.find((invocation) => invocation.connector_id === connector.id) ?? null;
	}

	function connectorLastStatus(connector: Connector): string {
		return connectorLastInvocation(connector)?.status ?? '';
	}

	function connectorLastUpdatedAt(connector: Connector): string {
		return connectorLastInvocation(connector)?.updated_at ?? connector.updated_at;
	}

	function connectorLastError(connector: Connector): string | null {
		return connectorLastInvocation(connector)?.error_message ?? null;
	}

	function invocationDelivery(invocation: Invocation | null): Record<string, unknown> | null {
		const result = invocation?.result;
		if (!isRecord(result)) return null;
		return isRecord(result.connector_delivery) ? result.connector_delivery : null;
	}

	function connectorLastDelivery(connector: Connector): Record<string, unknown> | null {
		return invocationDelivery(connectorLastInvocation(connector));
	}

	function deliveryText(delivery: Record<string, unknown>, key: string): string {
		const value = delivery[key];
		if (typeof value === 'number') return value.toString();
		return typeof value === 'string' && value.length > 0 ? value : '-';
	}

	function invocationPayloadText(invocation: Invocation, key: string): string {
		const payload = invocation.payload;
		if (!isRecord(payload)) return '-';
		const value = payload[key];
		if (typeof value === 'string' && value.length > 0) return value;
		if (key === 'business_event_id' && isRecord(payload.envelope)) {
			const eventId = payload.envelope.event_id;
			return typeof eventId === 'string' && eventId.length > 0 ? eventId : '-';
		}
		return '-';
	}

	function inboxRowIds(rows: EventInbox[]): string {
		return rows.length > 0 ? rows.map((row) => row.id).join(', ') : '-';
	}

	function connectorSubscribedEventsLabel(connector: Connector): string {
		const events = connectorSubscribedEvents(connector);
		return events.length > 0 ? events.join(', ') : $t('forms.automation.noSubscribedEvents');
	}

	function formatJson(value: unknown): string {
		if (value === null || value === undefined) return '{}';
		try {
			return JSON.stringify(value, null, 2);
		} catch {
			return String(value);
		}
	}

	function invocationReceipt(invocation: Invocation): Record<string, unknown> | null {
		const result = invocation.result;
		if (!isRecord(result)) return null;
		if (isRecord(result.receipt)) return result.receipt;
		if (
			typeof result.receipt_status === 'string' ||
			typeof result.status === 'string' ||
			typeof result.idempotency_key === 'string' ||
			typeof result.inbox_status === 'string' ||
			typeof result.event_type === 'string'
		) {
			return result;
		}
		return null;
	}

	function receiptText(receipt: Record<string, unknown>, key: string): string {
		const value = receipt[key];
		return typeof value === 'string' && value.length > 0 ? value : '-';
	}

	function receiptStatusText(invocation: Invocation, receipt: Record<string, unknown>): string {
		const receiptStatus = receiptText(receipt, 'receipt_status');
		if (receiptStatus !== '-') return receiptStatus;
		const status = receiptText(receipt, 'status');
		return status !== '-' ? status : invocation.status;
	}

	function receiptSourceText(receipt: Record<string, unknown>): string {
		const source = [receiptText(receipt, 'source_kind'), receiptText(receipt, 'source_id')].filter((value) => value !== '-');
		return source.length > 0 ? source.join(':') : '-';
	}

	function invocationInbox(invocationId: string): EventInbox[] {
		return inboxByInvocation[invocationId] ?? [];
	}

	function inboxRowSource(row: EventInbox): string {
		return [row.source_kind, row.source_id].filter(Boolean).join(':') || row.source_kind;
	}

	function inboxRowInvocationId(row: EventInbox): string | null {
		const invocationId = row.payload?.invocation_id;
		return typeof invocationId === 'string' && invocationId.length > 0 ? invocationId : null;
	}

	function mergeInboxRow(row: EventInbox) {
		formInboxRows = formInboxRows.map((item) => (item.id === row.id ? row : item));
		const invocationId = inboxRowInvocationId(row);
		if (invocationId && inboxByInvocation[invocationId]) {
			inboxByInvocation = {
				...inboxByInvocation,
				[invocationId]: inboxByInvocation[invocationId].map((item) => (item.id === row.id ? row : item))
			};
		}
	}

	function invocationToolCalls(invocationId: string): InvocationToolCall[] {
		return toolCallsByInvocation[invocationId] ?? [];
	}

	async function toggleInvocationDetails(invocationId: string) {
		if (expandedInvocationId === invocationId) {
			expandedInvocationId = null;
			return;
		}
		expandedInvocationId = invocationId;
		await Promise.all([
			inboxByInvocation[invocationId] ? Promise.resolve() : loadInvocationInbox(invocationId),
			toolCallsByInvocation[invocationId] ? Promise.resolve() : loadInvocationToolCalls(invocationId)
		]);
	}

	async function loadInvocationInbox(invocationId: string) {
		inboxLoadingInvocationIds = { ...inboxLoadingInvocationIds, [invocationId]: true };
		const response = await connectorsApi.listInvocationInbox(invocationId, { per_page: 10 });
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			inboxByInvocation = {
				...inboxByInvocation,
				[invocationId]: response.data?.items ?? []
			};
		}
		inboxLoadingInvocationIds = { ...inboxLoadingInvocationIds, [invocationId]: false };
	}

	async function loadInvocationToolCalls(invocationId: string) {
		toolCallsLoadingInvocationIds = { ...toolCallsLoadingInvocationIds, [invocationId]: true };
		const response = await connectorsApi.listInvocationToolCalls(invocationId, { per_page: 10 });
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toolCallsByInvocation = {
				...toolCallsByInvocation,
				[invocationId]: response.data?.items ?? []
			};
		}
		toolCallsLoadingInvocationIds = { ...toolCallsLoadingInvocationIds, [invocationId]: false };
	}

	async function loadFormInbox() {
		formInboxLoading = true;
		const response = await connectorsApi.listFormInbox(form.id, { per_page: 10 });
		if (response.code !== 0) {
			toast.error(response.message);
			formInboxRows = [];
		} else {
			formInboxRows = response.data?.items ?? [];
		}
		formInboxLoading = false;
	}

	async function replayInbox(invocationId: string, inboxId: string) {
		inboxReplayingIds = { ...inboxReplayingIds, [inboxId]: true };
		const response = await connectorsApi.replayInvocationInbox(invocationId, inboxId);
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
		} else {
			const current = invocationInbox(invocationId);
			inboxByInvocation = {
				...inboxByInvocation,
				[invocationId]: current.map((row) => (row.id === response.data?.id ? response.data : row))
			};
			mergeInboxRow(response.data);
			toast.success($t('forms.automation.inboxReplayQueued'));
		}
		inboxReplayingIds = { ...inboxReplayingIds, [inboxId]: false };
	}

	async function replayFormInbox(inboxId: string) {
		inboxReplayingIds = { ...inboxReplayingIds, [inboxId]: true };
		const response = await connectorsApi.replayFormInbox(form.id, inboxId);
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
		} else {
			mergeInboxRow(response.data);
			toast.success($t('forms.automation.inboxReplayQueued'));
		}
		inboxReplayingIds = { ...inboxReplayingIds, [inboxId]: false };
	}

	async function loadAutomation() {
		loading = true;
		const [connectorResponse, invocationResponse, formInboxResponse] = await Promise.all([
			connectorsApi.list(form.workspace_id, { project_id: form.project_id }),
			connectorsApi.listInvocations(form.project_id, { per_page: 20 }),
			connectorsApi.listFormInbox(form.id, { per_page: 10 })
		]);
		if (connectorResponse.code !== 0) {
			toast.error(connectorResponse.message);
			connectors = [];
		} else {
			connectors = connectorResponse.data ?? [];
			syncSubscriptionDrafts(connectors);
		}
		if (invocationResponse.code !== 0) {
			toast.error(invocationResponse.message);
			invocations = [];
		} else {
			invocations = invocationResponse.data?.items ?? [];
		}
		if (formInboxResponse.code !== 0) {
			toast.error(formInboxResponse.message);
			formInboxRows = [];
		} else {
			formInboxRows = formInboxResponse.data?.items ?? [];
		}
		loading = false;
	}

	async function createFormInvocation(source?: Invocation) {
		actionLoading = true;
		const connector = source?.connector_id
			? activeConnectors.find((item) => item.id === source.connector_id)
			: activeConnectors[0];
		const response = await connectorsApi.createInvocation(form.project_id, {
			connector_id: connector?.id,
			trigger_kind: 'manual',
			trigger_ref_type: 'form',
			trigger_ref_id: form.id,
			payload: {
				form_id: form.id,
				form_key: form.key,
				form_name: form.name,
				source: source ? 'retry' : 'automation_panel'
			}
		});
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success($t('forms.automation.invocationCreated'));
			await loadAutomation();
		}
		actionLoading = false;
	}

	async function reportReceipt(invocation: Invocation, status: 'completed' | 'failed') {
		actionLoading = true;
		const response = await connectorsApi.reportReceipt(invocation.id, {
			status,
			idempotency_key: `${form.id}:${invocation.id}:${status}`,
			payload: {
				form_id: form.id,
				form_key: form.key,
				receipt_source: 'automation_panel'
			},
			error_message: status === 'failed' ? 'Manual automation failure from form panel' : undefined
		});
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success($t(status === 'failed' ? 'forms.automation.receiptFailed' : 'forms.automation.receiptCompleted'));
			await loadAutomation();
			if (expandedInvocationId === invocation.id) {
				await loadInvocationInbox(invocation.id);
			}
		}
		actionLoading = false;
	}
</script>

<section class="rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900" data-form-automation-panel={form.id}>
	<div class="flex flex-wrap items-start justify-between gap-3 border-b border-slate-200 px-4 py-3 dark:border-slate-700">
		<div>
			<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">{$t('forms.automation.title')}</h3>
			<p class="mt-0.5 font-mono text-xs text-slate-500 dark:text-slate-400">{form.key}</p>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<Button size="sm" variant="secondary" loading={loading} onclick={loadAutomation}>
				<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.automation.refresh')}
			</Button>
			<span data-automation-create-invocation={form.id}>
				<Button size="sm" loading={actionLoading} onclick={() => createFormInvocation()}>
					<Activity class="mr-2 h-4 w-4" aria-hidden="true" />
					{$t('forms.automation.createInvocation')}
				</Button>
			</span>
		</div>
	</div>

	<div class="grid gap-3 p-4 md:grid-cols-2 xl:grid-cols-4">
		{#each rows as row}
			{@const Icon = row.icon}
			<div class="rounded-md border border-slate-200 p-3 dark:border-slate-700" data-automation-summary={row.key}>
				<div class="flex items-center gap-2 text-sm font-medium text-slate-950 dark:text-slate-100">
					<Icon class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
					{row.title}
				</div>
				<p class="mt-2 break-words font-mono text-xs text-slate-500 dark:text-slate-400">{row.value}</p>
			</div>
		{/each}
	</div>

	<section class="border-t border-slate-200 px-4 py-3 dark:border-slate-700" data-automation-global-inbox={form.id}>
		<div class="flex flex-wrap items-center justify-between gap-2">
			<div>
				<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">{$t('forms.automation.globalInbox')}</h4>
				<p class="mt-0.5 font-mono text-xs text-slate-500 dark:text-slate-400">{form.key}</p>
			</div>
			<Button size="sm" variant="secondary" loading={formInboxLoading} onclick={loadFormInbox}>
				<RefreshCw class="mr-1 h-4 w-4" aria-hidden="true" />
				{$t('forms.automation.refresh')}
			</Button>
		</div>
		{#if formInboxLoading && formInboxRows.length === 0}
			<div class="mt-3 text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		{:else if formInboxRows.length === 0}
			<div class="mt-3 text-sm text-slate-500 dark:text-slate-400">{$t('forms.automation.noGlobalInboxRows')}</div>
		{:else}
			<div class="mt-3 grid gap-2 lg:grid-cols-2">
				{#each formInboxRows as row}
					<div class="rounded-md border border-slate-200 bg-slate-50 p-2 text-xs dark:border-slate-700 dark:bg-slate-950" data-automation-global-inbox-row={row.id}>
						<div class="flex flex-wrap items-start justify-between gap-2">
							<div class="min-w-0">
								<div class="flex flex-wrap items-center gap-2">
									<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(row.status)}" data-automation-global-inbox-row-status={row.id}>
										{row.status}
									</span>
									<span class="break-all font-mono text-slate-700 dark:text-slate-200" data-automation-global-inbox-row-event={row.id}>{row.event_type}</span>
								</div>
								<div class="mt-1 break-all font-mono text-slate-500 dark:text-slate-400" data-automation-global-inbox-row-key={row.id}>{row.idempotency_key}</div>
								<div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-slate-500 dark:text-slate-400">
									<span data-automation-global-inbox-row-source={row.id}>{inboxRowSource(row)}</span>
									<span data-automation-global-inbox-row-attempts={row.id}>{$t('forms.automation.attempts')}: {row.attempts}</span>
									<span>{formatDate(row.updated_at)}</span>
								</div>
							</div>
							<span data-automation-global-inbox-replay={row.id}>
								<Button size="sm" variant="secondary" loading={Boolean(inboxReplayingIds[row.id])} onclick={() => replayFormInbox(row.id)}>
									<RotateCcw class="mr-1 h-4 w-4" aria-hidden="true" />
									{$t('forms.automation.replayInbox')}
								</Button>
							</span>
						</div>
						{#if row.last_error}
							<div class="mt-2 rounded border border-red-200 bg-red-50 px-2 py-1 text-red-700 dark:border-red-800 dark:bg-red-950/30 dark:text-red-300" data-automation-global-inbox-row-error={row.id}>
								{row.last_error}
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	</section>

	<div class="grid gap-4 border-t border-slate-200 p-4 dark:border-slate-700 xl:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]">
		<section class="min-w-0 rounded-md border border-slate-200 dark:border-slate-700">
			<div class="border-b border-slate-200 px-3 py-2 dark:border-slate-700">
				<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">{$t('forms.automation.bindings')}</h4>
			</div>
			{#if loading}
				<div class="p-3 text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
			{:else if formConnectors.length === 0}
				<div class="p-3 text-sm text-slate-500 dark:text-slate-400">{$t('forms.automation.noBindings')}</div>
			{:else}
				<div class="divide-y divide-slate-200 dark:divide-slate-700">
					{#each formConnectors as connector}
						<div class="p-3" data-automation-connector={connector.id}>
							<div class="flex flex-wrap items-center justify-between gap-2">
								<div class="min-w-0">
									<div class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{connector.name}</div>
									<div class="mt-0.5 truncate font-mono text-xs text-slate-500 dark:text-slate-400">{connector.kind} · {connector.endpoint || connector.id}</div>
								</div>
								<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {connector.is_active ? statusClass('completed') : statusClass('cancelled')}">
									{connector.is_active ? $t('forms.automation.active') : $t('forms.automation.inactive')}
								</span>
							</div>
							<div class="mt-3 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900" data-automation-connector-diagnostics={connector.id}>
								<div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
									<Activity class="h-3.5 w-3.5" aria-hidden="true" />
									{$t('forms.automation.diagnostics')}
								</div>
								<div class="mt-2 grid gap-2 text-xs text-slate-600 dark:text-slate-300 sm:grid-cols-2">
									<div>
										<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.endpoint')}</span>
										<div class="mt-0.5 truncate font-mono" data-automation-diagnostic-endpoint={connector.id}>{connectorEndpoint(connector)}</div>
									</div>
									<div>
										<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.auth')}</span>
										<div class="mt-0.5 font-mono" data-automation-diagnostic-auth={connector.id}>{connectorAuthMode(connector)}</div>
									</div>
									<div>
										<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.subscribedEvents')}</span>
										<div class="mt-0.5 truncate font-mono" data-automation-diagnostic-events={connector.id}>{connectorSubscribedEventsLabel(connector)}</div>
									</div>
									<div>
										<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.routingKey')}</span>
										<div class="mt-0.5 truncate font-mono" data-automation-diagnostic-routing-key={connector.id}>{connectorRoutingKey(connector) || '-'}</div>
									</div>
								</div>
								<div class="mt-3 rounded border border-slate-200 bg-slate-50 px-2 py-1.5 text-xs dark:border-slate-700 dark:bg-slate-950" data-automation-diagnostic-last={connector.id}>
									{#if connectorLastInvocation(connector)}
										<div class="flex flex-wrap items-center gap-2">
											<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.lastDelivery')}</span>
											<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(connectorLastStatus(connector))}">
												{connectorLastStatus(connector)}
											</span>
											<span class="font-mono text-slate-500 dark:text-slate-400">{formatDate(connectorLastUpdatedAt(connector))}</span>
										</div>
										{#if connectorLastError(connector)}
											<div class="mt-1 truncate text-red-600 dark:text-red-300" data-automation-diagnostic-error={connector.id}>{connectorLastError(connector)}</div>
										{/if}
										{#if connectorLastDelivery(connector)}
											{@const delivery = connectorLastDelivery(connector)}
											{#if delivery}
												<div class="mt-2 grid gap-1 text-slate-600 dark:text-slate-300 sm:grid-cols-2" data-automation-delivery-diagnostics={connector.id}>
													<div>
														<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.deliveryStatus')}</span>
														<div class="mt-0.5 font-mono" data-automation-delivery-status={connector.id}>{deliveryText(delivery, 'status')}</div>
													</div>
													<div>
														<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.statusCode')}</span>
														<div class="mt-0.5 font-mono" data-automation-delivery-code={connector.id}>{deliveryText(delivery, 'status_code')}</div>
													</div>
													{#if deliveryText(delivery, 'response_body') !== '-'}
														<div class="sm:col-span-2">
															<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.responseBody')}</span>
															<div class="mt-0.5 truncate font-mono" data-automation-delivery-body={connector.id}>{deliveryText(delivery, 'response_body')}</div>
														</div>
													{/if}
													{#if deliveryText(delivery, 'error_message') !== '-'}
														<div class="sm:col-span-2 text-red-600 dark:text-red-300" data-automation-delivery-error={connector.id}>{deliveryText(delivery, 'error_message')}</div>
													{/if}
												</div>
											{/if}
										{/if}
									{:else}
										<span class="text-slate-500 dark:text-slate-400">{$t('forms.automation.noRecentDelivery')}</span>
									{/if}
								</div>
							</div>
							<div class="mt-3 rounded-md border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-950" data-automation-routing={connector.id}>
								<div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
									<Route class="h-3.5 w-3.5" aria-hidden="true" />
									{$t('forms.automation.routingRules')}
								</div>
								<div class="mt-2 grid gap-2 sm:grid-cols-2">
									{#each formEventOptions as eventName}
										<label class="flex items-center gap-2 rounded border border-slate-200 bg-white px-2 py-1.5 text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200">
											<input
												type="checkbox"
												class="h-3.5 w-3.5 rounded border-slate-300 text-blue-600 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800"
												checked={subscriptionDraft(connector).events.includes(eventName)}
												onchange={(event) => setConnectorEvent(connector.id, eventName, (event.currentTarget as HTMLInputElement).checked)}
												data-automation-event={`${connector.id}:${eventName}`}
											/>
											<span class="truncate font-mono">{eventName}</span>
										</label>
									{/each}
								</div>
								<div class="mt-3 flex flex-col gap-2 sm:flex-row sm:items-end">
									<label class="min-w-0 flex-1">
										<span class="text-xs font-medium text-slate-600 dark:text-slate-300">{$t('forms.automation.routingKey')}</span>
										<input
											class="mt-1 w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 font-mono text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											value={subscriptionDraft(connector).routingKey}
											placeholder={`${form.key}.${connector.kind}`}
											oninput={(event) => setConnectorRoutingKey(connector.id, (event.currentTarget as HTMLInputElement).value)}
											data-automation-routing-key={connector.id}
										/>
									</label>
									<span data-automation-save-routing={connector.id}>
										<Button
											size="sm"
											variant="secondary"
											loading={routingSavingConnectorId === connector.id}
											onclick={() => saveConnectorRouting(connector)}
										>
											{$t('forms.automation.saveRouting')}
										</Button>
									</span>
								</div>
								<div class="mt-3 rounded border border-slate-200 bg-white p-2 dark:border-slate-700 dark:bg-slate-900" data-automation-routing-conditions={connector.id}>
									<div class="flex flex-wrap items-center justify-between gap-2">
										<div class="text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
											{$t('forms.automation.routingConditions')}
										</div>
										<Button size="sm" variant="secondary" onclick={() => addRoutingCondition(connector.id)}>
											{$t('forms.automation.addCondition')}
										</Button>
									</div>
									{#if subscriptionDraft(connector).conditions.length === 0}
										<div class="mt-2 text-xs text-slate-500 dark:text-slate-400">{$t('forms.automation.noConditions')}</div>
									{:else}
										<div class="mt-2 space-y-2">
											{#each subscriptionDraft(connector).conditions as condition, index}
												<div class="grid gap-2 rounded border border-slate-200 bg-slate-50 p-2 dark:border-slate-700 dark:bg-slate-950 lg:grid-cols-[minmax(0,1fr)_minmax(0,0.8fr)_minmax(0,1fr)_auto]" data-automation-routing-condition={`${connector.id}:${index}`}>
													<label class="min-w-0">
														<span class="text-xs font-medium text-slate-600 dark:text-slate-300">{$t('forms.automation.conditionField')}</span>
														<select
															class="mt-1 w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															value={condition.field}
															onchange={(event) => updateRoutingCondition(connector.id, index, { field: (event.currentTarget as HTMLSelectElement).value })}
															data-automation-routing-condition-field={`${connector.id}:${index}`}
														>
															{#each formFields as field}
																<option value={field.key}>{field.label || field.key}</option>
															{/each}
														</select>
													</label>
													<label class="min-w-0">
														<span class="text-xs font-medium text-slate-600 dark:text-slate-300">{$t('forms.automation.conditionOperator')}</span>
														<select
															class="mt-1 w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 font-mono text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															value={condition.operator}
															onchange={(event) => updateRoutingCondition(connector.id, index, { operator: (event.currentTarget as HTMLSelectElement).value })}
															data-automation-routing-condition-operator={`${connector.id}:${index}`}
														>
															{#each routingOperators as operator}
																<option value={operator}>{operator}</option>
															{/each}
														</select>
													</label>
													<label class="min-w-0">
														<span class="text-xs font-medium text-slate-600 dark:text-slate-300">{$t('forms.automation.conditionValue')}</span>
														<input
															class="mt-1 w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															value={condition.value}
															oninput={(event) => updateRoutingCondition(connector.id, index, { value: (event.currentTarget as HTMLInputElement).value })}
															data-automation-routing-condition-value={`${connector.id}:${index}`}
														/>
													</label>
													<div class="flex items-end" data-automation-routing-condition-remove={`${connector.id}:${index}`}>
														<Button size="sm" variant="secondary" onclick={() => removeRoutingCondition(connector.id, index)}>
															{$t('common.delete')}
														</Button>
													</div>
												</div>
											{/each}
										</div>
									{/if}
								</div>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section class="min-w-0 rounded-md border border-slate-200 dark:border-slate-700">
			<div class="flex items-center justify-between gap-3 border-b border-slate-200 px-3 py-2 dark:border-slate-700">
				<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">{$t('forms.automation.receipts')}</h4>
				{#if printJobCount > 0}
					<span class="text-xs text-slate-500 dark:text-slate-400">{$t('forms.printJobs')}: {printJobCount}</span>
				{/if}
			</div>
			{#if loading}
				<div class="p-3 text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
			{:else if formInvocations.length === 0}
				<div class="p-3 text-sm text-slate-500 dark:text-slate-400">{$t('forms.automation.noReceipts')}</div>
			{:else}
				<div class="divide-y divide-slate-200 dark:divide-slate-700">
					{#each formInvocations.slice(0, 5) as invocation}
						<div class="grid gap-3 p-3 lg:grid-cols-[minmax(0,1fr)_auto]" data-automation-invocation={invocation.id}>
							<div class="min-w-0">
								<div class="flex flex-wrap items-center gap-2">
									<span class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{invocation.trigger_kind}</span>
									<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(invocation.status)}" data-automation-invocation-status={invocation.id}>
										{invocation.status}
									</span>
									{#if invocation.connector_kind}
										<span class="text-xs text-slate-500 dark:text-slate-400">{invocation.connector_kind}</span>
									{/if}
								</div>
								<div class="mt-1 truncate font-mono text-xs text-slate-500 dark:text-slate-400">
									{invocation.trigger_ref_type || 'form'}:{invocation.trigger_ref_id || form.id} · {formatDate(invocation.created_at)}
								</div>
								{#if invocation.error_message}
									<div class="mt-1 truncate text-xs text-red-600 dark:text-red-300">{invocation.error_message}</div>
								{/if}
							</div>
							<div class="flex flex-wrap items-center gap-2 lg:justify-end">
								<span data-automation-invocation-details-toggle={invocation.id}>
									<Button size="sm" variant="secondary" onclick={() => toggleInvocationDetails(invocation.id)}>
										{#if expandedInvocationId === invocation.id}
											<ChevronUp class="mr-1 h-4 w-4" aria-hidden="true" />
											{$t('forms.automation.hideDetails')}
										{:else}
											<ChevronDown class="mr-1 h-4 w-4" aria-hidden="true" />
											{$t('forms.automation.details')}
										{/if}
									</Button>
								</span>
								{#if ['pending', 'dispatched', 'running'].includes(invocation.status)}
									<span data-automation-receipt-complete={invocation.id}>
										<Button size="sm" variant="secondary" loading={actionLoading} onclick={() => reportReceipt(invocation, 'completed')}>
											<CheckCircle2 class="mr-1 h-4 w-4" aria-hidden="true" />
											{$t('forms.automation.markCompleted')}
										</Button>
									</span>
									<span data-automation-receipt-fail={invocation.id}>
										<Button size="sm" variant="danger" loading={actionLoading} onclick={() => reportReceipt(invocation, 'failed')}>
											<XCircle class="mr-1 h-4 w-4" aria-hidden="true" />
											{$t('forms.automation.markFailed')}
										</Button>
									</span>
								{:else if invocation.status === 'failed'}
									<span data-automation-retry={invocation.id}>
										<Button size="sm" variant="secondary" loading={actionLoading} onclick={() => createFormInvocation(invocation)}>
											<RotateCcw class="mr-1 h-4 w-4" aria-hidden="true" />
											{$t('common.retry')}
										</Button>
									</span>
								{/if}
							</div>
							{#if expandedInvocationId === invocation.id}
								{@const receipt = invocationReceipt(invocation)}
								{@const delivery = invocationDelivery(invocation)}
								{@const inboxRows = invocationInbox(invocation.id)}
								{@const toolCalls = invocationToolCalls(invocation.id)}
								<div class="lg:col-span-2 rounded-md border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-950" data-automation-invocation-details={invocation.id}>
									<div class="grid gap-2 text-xs text-slate-600 dark:text-slate-300 sm:grid-cols-2">
										<div>
											<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.connector')}</span>
											<div class="mt-0.5 break-words font-mono" data-automation-detail-connector={invocation.id}>{connectorLabel(invocation)}</div>
										</div>
										<div>
											<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.trigger')}</span>
											<div class="mt-0.5 break-words font-mono" data-automation-detail-trigger={invocation.id}>
												{invocation.trigger_kind} · {invocation.trigger_ref_type || 'form'}:{invocation.trigger_ref_id || form.id}
											</div>
										</div>
										<div>
											<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.auditChain')}</span>
											<div class="mt-0.5 break-words font-mono" data-automation-detail-audit={invocation.id}>{invocation.audit_chain_id || invocation.id}</div>
										</div>
										<div>
											<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.updated')}</span>
											<div class="mt-0.5 font-mono">{formatDate(invocation.updated_at)}</div>
										</div>
									</div>
									<div class="mt-3 grid gap-3 lg:grid-cols-2">
										<div>
											<div class="text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">{$t('forms.automation.payload')}</div>
											<pre class="mt-1 max-h-40 overflow-auto rounded border border-slate-200 bg-white p-2 text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200" data-automation-detail-payload={invocation.id}>{formatJson(invocation.payload)}</pre>
										</div>
										<div>
											<div class="text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">{$t('forms.automation.result')}</div>
											<pre class="mt-1 max-h-40 overflow-auto rounded border border-slate-200 bg-white p-2 text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200" data-automation-detail-result={invocation.id}>{formatJson(invocation.result)}</pre>
										</div>
									</div>
									{#if invocation.error_message}
										<div class="mt-3 rounded border border-red-200 bg-red-50 px-2 py-1.5 text-xs text-red-700 dark:border-red-800 dark:bg-red-950/30 dark:text-red-300" data-automation-detail-error={invocation.id}>
											<span class="font-semibold">{$t('forms.automation.error')}:</span> {invocation.error_message}
										</div>
									{/if}
									<div class="mt-3 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900" data-automation-correlation={invocation.id}>
										<div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
											<Activity class="h-3.5 w-3.5" aria-hidden="true" />
											{$t('forms.automation.correlation')}
										</div>
										<div class="mt-2 grid gap-2 text-xs text-slate-600 dark:text-slate-300 sm:grid-cols-2 lg:grid-cols-3">
											<div>
												<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.businessEvent')}</span>
												<div class="mt-0.5 break-words font-mono" data-automation-correlation-business-event={invocation.id}>{invocationPayloadText(invocation, 'business_event_id')}</div>
											</div>
											<div>
												<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.outbox')}</span>
												<div class="mt-0.5 break-words font-mono" data-automation-correlation-outbox={invocation.id}>{invocationPayloadText(invocation, 'outbox_id')}</div>
											</div>
											<div>
												<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.deliveryAt')}</span>
												<div class="mt-0.5 break-words font-mono" data-automation-correlation-delivery-at={invocation.id}>{delivery ? deliveryText(delivery, 'dispatched_at') : '-'}</div>
											</div>
											<div>
												<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.idempotencyKey')}</span>
												<div class="mt-0.5 break-words font-mono" data-automation-correlation-receipt-key={invocation.id}>{receipt ? receiptText(receipt, 'idempotency_key') : '-'}</div>
											</div>
											<div>
												<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.inboxRows')}</span>
												<div class="mt-0.5 break-words font-mono" data-automation-correlation-inbox={invocation.id}>{inboxRowIds(inboxRows)}</div>
											</div>
											<div>
												<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.relatedToolCalls')}</span>
												<div class="mt-0.5 font-mono" data-automation-correlation-tool-calls={invocation.id}>{toolCalls.length}</div>
											</div>
										</div>
									</div>
									{#if delivery}
										<div class="mt-3 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900" data-automation-detail-delivery={invocation.id}>
											<div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
												<Cable class="h-3.5 w-3.5" aria-hidden="true" />
												{$t('forms.automation.externalDelivery')}
											</div>
											<div class="mt-2 grid gap-2 text-xs text-slate-600 dark:text-slate-300 sm:grid-cols-2">
												<div>
													<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.deliveryStatus')}</span>
													<div class="mt-0.5 font-mono" data-automation-detail-delivery-status={invocation.id}>{deliveryText(delivery, 'status')}</div>
												</div>
												<div>
													<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.statusCode')}</span>
													<div class="mt-0.5 font-mono" data-automation-detail-delivery-code={invocation.id}>{deliveryText(delivery, 'status_code')}</div>
												</div>
												<div class="sm:col-span-2">
													<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.endpoint')}</span>
													<div class="mt-0.5 break-words font-mono" data-automation-detail-delivery-endpoint={invocation.id}>{deliveryText(delivery, 'endpoint')}</div>
												</div>
											</div>
											{#if deliveryText(delivery, 'response_body') !== '-'}
												<pre class="mt-2 max-h-28 overflow-auto rounded border border-slate-200 bg-slate-50 p-2 text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200" data-automation-detail-delivery-body={invocation.id}>{deliveryText(delivery, 'response_body')}</pre>
											{/if}
											{#if deliveryText(delivery, 'error_message') !== '-'}
												<div class="mt-2 rounded border border-red-200 bg-red-50 px-2 py-1 text-xs text-red-700 dark:border-red-800 dark:bg-red-950/30 dark:text-red-300" data-automation-detail-delivery-error={invocation.id}>
													{deliveryText(delivery, 'error_message')}
												</div>
											{/if}
										</div>
									{/if}
									{#if receipt}
										<div class="mt-3 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900" data-automation-inbox={invocation.id}>
											<div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
												<Activity class="h-3.5 w-3.5" aria-hidden="true" />
												{$t('forms.automation.inboxReceipt')}
											</div>
											<div class="mt-2 grid gap-2 text-xs text-slate-600 dark:text-slate-300 sm:grid-cols-2">
												<div>
													<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.receiptStatus')}</span>
													<div class="mt-0.5 flex flex-wrap items-center gap-1.5 font-mono" data-automation-inbox-status={invocation.id}>
														<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(receiptStatusText(invocation, receipt))}">
															{receiptStatusText(invocation, receipt)}
														</span>
														{#if receiptText(receipt, 'inbox_status') !== '-'}
															<span>{receiptText(receipt, 'inbox_status')}</span>
														{/if}
													</div>
												</div>
												<div>
													<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.idempotencyKey')}</span>
													<div class="mt-0.5 break-words font-mono" data-automation-inbox-key={invocation.id}>{receiptText(receipt, 'idempotency_key')}</div>
												</div>
												<div>
													<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.eventType')}</span>
													<div class="mt-0.5 break-words font-mono" data-automation-inbox-event={invocation.id}>{receiptText(receipt, 'event_type')}</div>
												</div>
												<div>
													<span class="font-medium text-slate-900 dark:text-slate-100">{$t('forms.automation.source')}</span>
													<div class="mt-0.5 break-words font-mono" data-automation-inbox-source={invocation.id}>{receiptSourceText(receipt)}</div>
												</div>
											</div>
											<div class="mt-3">
												<div class="text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">{$t('forms.automation.receiptPayload')}</div>
												<pre class="mt-1 max-h-32 overflow-auto rounded border border-slate-200 bg-slate-50 p-2 text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200" data-automation-inbox-payload={invocation.id}>{formatJson(receipt.payload)}</pre>
											</div>
										</div>
									{/if}
									<div class="mt-3 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900" data-automation-inbox-list={invocation.id}>
										<div class="flex flex-wrap items-center justify-between gap-2">
											<div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
												<RotateCcw class="h-3.5 w-3.5" aria-hidden="true" />
												{$t('forms.automation.inboxRows')}
											</div>
											<Button size="sm" variant="secondary" loading={Boolean(inboxLoadingInvocationIds[invocation.id])} onclick={() => loadInvocationInbox(invocation.id)}>
												<RefreshCw class="mr-1 h-4 w-4" aria-hidden="true" />
												{$t('forms.automation.refresh')}
											</Button>
										</div>
										{#if inboxLoadingInvocationIds[invocation.id] && inboxRows.length === 0}
											<div class="mt-2 text-xs text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
										{:else if inboxRows.length === 0}
											<div class="mt-2 text-xs text-slate-500 dark:text-slate-400">{$t('forms.automation.noInboxRows')}</div>
										{:else}
											<div class="mt-2 space-y-2">
												{#each inboxRows as row}
													<div class="rounded border border-slate-200 bg-slate-50 p-2 text-xs dark:border-slate-700 dark:bg-slate-950" data-automation-inbox-row={row.id}>
														<div class="flex flex-wrap items-start justify-between gap-2">
															<div class="min-w-0">
																<div class="flex flex-wrap items-center gap-2">
																	<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(row.status)}" data-automation-inbox-row-status={row.id}>
																		{row.status}
																	</span>
																	<span class="break-all font-mono text-slate-700 dark:text-slate-200">{row.event_type}</span>
																</div>
																<div class="mt-1 break-all font-mono text-slate-500 dark:text-slate-400" data-automation-inbox-row-key={row.id}>{row.idempotency_key}</div>
																<div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-slate-500 dark:text-slate-400">
																	<span data-automation-inbox-row-source={row.id}>{inboxRowSource(row)}</span>
																	<span data-automation-inbox-row-attempts={row.id}>{$t('forms.automation.attempts')}: {row.attempts}</span>
																	<span>{formatDate(row.updated_at)}</span>
																</div>
															</div>
															<span data-automation-inbox-replay={row.id}>
																<Button size="sm" variant="secondary" loading={Boolean(inboxReplayingIds[row.id])} onclick={() => replayInbox(invocation.id, row.id)}>
																	<RotateCcw class="mr-1 h-4 w-4" aria-hidden="true" />
																	{$t('forms.automation.replayInbox')}
																</Button>
															</span>
														</div>
														{#if row.last_error}
															<div class="mt-2 rounded border border-red-200 bg-red-50 px-2 py-1 text-red-700 dark:border-red-800 dark:bg-red-950/30 dark:text-red-300" data-automation-inbox-row-error={row.id}>
																{row.last_error}
															</div>
														{/if}
														<pre class="mt-2 max-h-28 overflow-auto rounded border border-slate-200 bg-white p-2 text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200" data-automation-inbox-row-payload={row.id}>{formatJson(row.payload)}</pre>
													</div>
												{/each}
											</div>
										{/if}
									</div>
									<div class="mt-3 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900" data-automation-tool-calls={invocation.id}>
										<div class="flex flex-wrap items-center justify-between gap-2">
											<div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
												<Bot class="h-3.5 w-3.5" aria-hidden="true" />
												{$t('forms.automation.toolCalls')}
											</div>
											<Button size="sm" variant="secondary" loading={Boolean(toolCallsLoadingInvocationIds[invocation.id])} onclick={() => loadInvocationToolCalls(invocation.id)}>
												<RefreshCw class="mr-1 h-4 w-4" aria-hidden="true" />
												{$t('forms.automation.refresh')}
											</Button>
										</div>
										{#if toolCallsLoadingInvocationIds[invocation.id] && toolCalls.length === 0}
											<div class="mt-2 text-xs text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
										{:else if toolCalls.length === 0}
											<div class="mt-2 text-xs text-slate-500 dark:text-slate-400">{$t('forms.automation.noToolCalls')}</div>
										{:else}
											<div class="mt-2 space-y-2">
												{#each toolCalls as call}
													<div class="rounded border border-slate-200 bg-slate-50 p-2 text-xs dark:border-slate-700 dark:bg-slate-950" data-automation-tool-call={call.id}>
														<div class="flex flex-wrap items-center gap-2">
															<span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {statusClass(call.status)}" data-automation-tool-call-status={call.id}>{call.status}</span>
															<span class="break-all font-mono text-slate-800 dark:text-slate-100" data-automation-tool-call-name={call.id}>{call.tool_name}</span>
															<span class="font-mono text-slate-500 dark:text-slate-400" data-automation-tool-call-transport={call.id}>{call.transport}</span>
															<span class="font-mono text-slate-500 dark:text-slate-400" data-automation-tool-call-duration={call.id}>{call.duration_ms}ms</span>
														</div>
														{#if call.result_summary}
															<div class="mt-2 rounded border border-emerald-200 bg-emerald-50 px-2 py-1 text-emerald-700 dark:border-emerald-800 dark:bg-emerald-950/30 dark:text-emerald-300" data-automation-tool-call-result={call.id}>
																{call.result_summary}
															</div>
														{/if}
														{#if call.error_message}
															<div class="mt-2 rounded border border-red-200 bg-red-50 px-2 py-1 text-red-700 dark:border-red-800 dark:bg-red-950/30 dark:text-red-300" data-automation-tool-call-error={call.id}>
																{call.error_message}
															</div>
														{/if}
														<pre class="mt-2 max-h-28 overflow-auto rounded border border-slate-200 bg-white p-2 text-xs text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200" data-automation-tool-call-arguments={call.id}>{formatJson(call.arguments)}</pre>
													</div>
												{/each}
											</div>
										{/if}
									</div>
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</section>
	</div>

	{#if latestInvocation}
		<div class="border-t border-slate-200 px-4 py-3 text-xs text-slate-500 dark:border-slate-700 dark:text-slate-400" data-automation-latest={latestInvocation.id}>
			{$t('forms.automation.latest')}: {latestInvocation.status} · {formatDate(latestInvocation.updated_at)}
			{#if activeInvocation}
				<span class="ml-2">{$t('forms.automation.awaitingReceipt')}</span>
			{/if}
			{#if failedInvocation}
				<span class="ml-2 text-red-600 dark:text-red-300">{$t('forms.automation.failedNeedsRetry')}</span>
			{/if}
		</div>
	{/if}
</section>

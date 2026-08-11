<script lang="ts">
	import {
		Activity,
		ArrowLeft,
		FileDown,
		FileText,
		Link,
		MessageSquare,
		Shield,
		TableProperties
	} from '@lucide/svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import {
		formsApi,
		type BusinessEvent,
		type FormAttachment,
		type FormChildRecord,
		type FormField,
		type FormPermissions,
		type FormRecord,
		type FormRecordComment,
		type FormRecordLink,
		type UniversalForm
	} from '$lib/api/forms';
	import Button from '$lib/components/Button.svelte';
	import OptionValueBadges from '$lib/components/forms/OptionValueBadges.svelte';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import { safeImageUrl, safeLinkUrl } from '$lib/utils/safe-url';
	import { locale, t } from 'svelte-i18n';

	type DetailLayoutSectionDensity = 'comfortable' | 'compact';
	type DetailLayoutSection = {
		key: string;
		title: string;
		description: string;
		columns: 1 | 2 | 3;
		density: DetailLayoutSectionDensity;
		hideEmptyFields: boolean;
		collapsible: boolean;
		defaultCollapsed: boolean;
		fields: FormField[];
	};

	type DetailPageWidgetKey = 'children' | 'links' | 'comments' | 'attachments' | 'events';
	type DetailPageWidgetRegion = 'sidebar' | 'below';
	type DetailPageWidgetDensity = 'comfortable' | 'compact';

	const detailPageWidgetKeys = new Set<DetailPageWidgetKey>([
		'children',
		'links',
		'comments',
		'attachments',
		'events'
	]);

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam($page.params.projectId, 'projectId');
	const recordId = requireRouteParam($page.params.recordId, 'recordId');

	let loading = $state(true);
	let errorMessage = $state('');
	let record = $state<FormRecord | null>(null);
	let form = $state<UniversalForm | null>(null);
	let permissions = $state<FormPermissions | null>(null);
	let links = $state<FormRecordLink[]>([]);
	let children = $state<FormChildRecord[]>([]);
	let comments = $state<FormRecordComment[]>([]);
	let attachments = $state<FormAttachment[]>([]);
	let attachmentSignedUrls = $state<Record<string, string>>({});
	let events = $state<BusinessEvent[]>([]);
	let childForms = $state<Record<string, UniversalForm>>({});
	let commentDraft = $state('');
	let commentSubmitting = $state(false);

	const fields = $derived(form ? parseFields(form.schema) : []);
	const recordEvents = $derived(
		events.filter(
			(event) =>
				event.aggregate_type === 'form_record' &&
				event.aggregate_id === record?.id &&
				event.project_id === projectId
		)
	);
	const activeAttachments = $derived(attachments.filter((attachment) => !attachment.archived_at));
	const attachmentFields = $derived(
		fields.filter((field) => ['attachment', 'image'].includes(fieldType(field)) && field.visibility?.detail !== false)
	);
	const pageWidgetKeys = $derived(detailPageWidgetKeysFromLayout(form?.detail_layout));
	const pageWidgetRegion = $derived(detailPageWidgetRegionFromLayout(form?.detail_layout));
	const pageFieldColumns = $derived(detailPageFieldColumnsFromLayout(form?.detail_layout));
	const detailHeaderSubtitleField = $derived(detailHeaderField('subtitle_field'));
	const detailHeaderBadgeField = $derived(detailHeaderField('badge_field'));
	const detailHighlightFields = $derived(detailHighlightFieldsFromLayout(form?.detail_layout));
	const memberRecordScope = $derived(permissions?.effective.record_scope === 'owned' ? 'owned' : 'all');
	const memberHiddenFieldLabels = $derived(fields.filter(memberFieldHidden).map(fieldLabel));
	const memberLockedFieldLabels = $derived(fields.filter(memberFieldLocked).map(fieldLabel));

	onMount(() => {
		void loadDetail();
	});

	function isRecord(input: unknown): input is Record<string, unknown> {
		return typeof input === 'object' && input !== null && !Array.isArray(input);
	}

	function detailPageWidgetKey(value: unknown): DetailPageWidgetKey | null {
		return typeof value === 'string' && detailPageWidgetKeys.has(value as DetailPageWidgetKey)
			? (value as DetailPageWidgetKey)
			: null;
	}

	function defaultDetailPageWidgetKeys(): DetailPageWidgetKey[] {
		return ['children', 'links', 'comments', 'attachments', 'events'];
	}

	function detailPageWidgetKeysFromLayout(
		layout: Record<string, unknown> | null | undefined
	): DetailPageWidgetKey[] {
		const rawWidgets = isRecord(layout) && Array.isArray(layout.widgets) ? layout.widgets : [];
		if (rawWidgets.length === 0) return defaultDetailPageWidgetKeys();
		const seen = new Set<DetailPageWidgetKey>();
		const keys: DetailPageWidgetKey[] = [];
		for (const rawWidget of rawWidgets) {
			const key = isRecord(rawWidget) ? detailPageWidgetKey(rawWidget.key) : detailPageWidgetKey(rawWidget);
			if (!key || seen.has(key)) continue;
			seen.add(key);
			if (!isRecord(rawWidget) || rawWidget.enabled !== false) keys.push(key);
		}
		return keys;
	}

	function detailPageWidgetRegionFromLayout(
		layout: Record<string, unknown> | null | undefined
	): DetailPageWidgetRegion {
		const page = isRecord(layout?.page) ? layout.page : {};
		return page.widget_region === 'below' ? 'below' : 'sidebar';
	}

	function detailPageFieldColumnsFromLayout(layout: Record<string, unknown> | null | undefined): 1 | 2 {
		const page = isRecord(layout?.page) ? layout.page : {};
		const rawColumns = page.field_columns;
		const columns =
			typeof rawColumns === 'number' ? rawColumns : typeof rawColumns === 'string' ? Number(rawColumns) : NaN;
		return columns === 1 ? 1 : 2;
	}

	function detailLayoutSectionColumns(value: unknown, fallback: 1 | 2 | 3 = 2): 1 | 2 | 3 {
		const numericValue = typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : NaN;
		return numericValue === 1 || numericValue === 2 || numericValue === 3 ? numericValue : fallback;
	}

	function detailLayoutSectionDescription(value: unknown): string {
		return typeof value === 'string' ? value.trim() : '';
	}

	function detailLayoutSectionDensity(value: unknown): DetailLayoutSectionDensity {
		return value === 'compact' ? 'compact' : 'comfortable';
	}

	function keepDetailSectionOpen(event: Event, collapsible: boolean) {
		if (collapsible) return;
		const details = event.currentTarget;
		if (details instanceof HTMLDetailsElement && !details.open) details.open = true;
	}

	function detailLayoutSectionContainerClass(density: DetailLayoutSectionDensity): string {
		const padding = density === 'compact' ? 'p-3' : 'p-4';
		return `rounded-md border border-slate-200 bg-white ${padding} dark:border-slate-800 dark:bg-slate-900`;
	}

	function detailLayoutSectionGridClass(columns: 1 | 2 | 3, density: DetailLayoutSectionDensity): string {
		const gap = density === 'compact' ? 'gap-3' : 'gap-4';
		if (columns === 1) return `mt-4 grid ${gap}`;
		if (columns === 3) return `mt-4 grid ${gap} md:grid-cols-2 xl:grid-cols-3`;
		return `mt-4 grid ${gap} md:grid-cols-2`;
	}

	function detailLayoutSectionFieldClass(field: FormField, columns: 1 | 2 | 3): string {
		if (fieldType(field) !== 'child_table' || columns === 1) return 'min-w-0';
		return columns === 3 ? 'min-w-0 md:col-span-2 xl:col-span-3' : 'min-w-0 md:col-span-2';
	}

	function recordValueIsEmpty(value: unknown): boolean {
		if (value === null || value === undefined) return true;
		if (typeof value === 'string') return value.trim().length === 0;
		if (Array.isArray(value)) return value.length === 0 || value.every(recordValueIsEmpty);
		if (isRecord(value)) {
			if (typeof value.display === 'string') return value.display.trim().length === 0;
			if (typeof value.name === 'string') return value.name.trim().length === 0;
			if (typeof value.email === 'string') return value.email.trim().length === 0;
			if (typeof value.title === 'string') return value.title.trim().length === 0;
			if (typeof value.decimal === 'string') return value.decimal.trim().length === 0;
			if ('value' in value) return recordValueIsEmpty(value.value);
			return Object.keys(value).length === 0;
		}
		return false;
	}

	function detailLayoutSectionFieldsForRecord(section: DetailLayoutSection, sourceRecord: FormRecord): FormField[] {
		if (!section.hideEmptyFields) return section.fields;
		return section.fields.filter((field) => !recordValueIsEmpty(sourceRecord.values[field.key]));
	}

	function detailPageWidgetConfig(key: DetailPageWidgetKey): Record<string, unknown> | null {
		const rawWidgets = isRecord(form?.detail_layout) && Array.isArray(form.detail_layout.widgets)
			? form.detail_layout.widgets
			: [];
		const rawWidget = rawWidgets.find((widget) => isRecord(widget) && widget.key === key);
		return isRecord(rawWidget) ? rawWidget : null;
	}

	function detailPageWidgetTitle(key: DetailPageWidgetKey, fallback: string): string {
		const rawWidget = detailPageWidgetConfig(key);
		const title = isRecord(rawWidget) && typeof rawWidget.title === 'string' ? rawWidget.title.trim() : '';
		return title || fallback;
	}

	function detailPageWidgetDescription(key: DetailPageWidgetKey): string {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && typeof rawWidget.description === 'string'
			? rawWidget.description.trim()
			: '';
	}

	function detailPageWidgetEmptyText(key: DetailPageWidgetKey, fallback: string): string {
		const rawWidget = detailPageWidgetConfig(key);
		const emptyText =
			isRecord(rawWidget) && typeof rawWidget.empty_text === 'string' ? rawWidget.empty_text.trim() : '';
		return emptyText || fallback;
	}

	function detailPageWidgetShowCount(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.show_count === true;
	}

	function detailPageWidgetHideEmpty(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.hide_empty === true;
	}

	function detailPageWidgetCollapsible(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.collapsible === true;
	}

	function detailPageWidgetDefaultCollapsed(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.collapsible === true && rawWidget.default_collapsed === true;
	}

	function detailPageWidgetOpen(key: DetailPageWidgetKey): boolean {
		return !detailPageWidgetCollapsible(key) || !detailPageWidgetDefaultCollapsed(key);
	}

	function detailPageWidgetDensity(key: DetailPageWidgetKey): DetailPageWidgetDensity {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.density === 'compact' ? 'compact' : 'comfortable';
	}

	function detailPageWidgetContainerClass(key: DetailPageWidgetKey): string {
		const padding = detailPageWidgetDensity(key) === 'compact' ? 'p-3' : 'p-4';
		return `rounded-md border border-slate-200 bg-white ${padding} dark:border-slate-800 dark:bg-slate-900`;
	}

	function detailPageWidgetSummaryClass(key: DetailPageWidgetKey): string {
		const margin = detailPageWidgetDensity(key) === 'compact' ? 'mb-2' : 'mb-3';
		return detailPageWidgetCollapsible(key)
			? `${margin} flex cursor-pointer list-none items-start gap-2`
			: `${margin} flex cursor-default list-none items-start gap-2`;
	}

	function detailPageWidgetBodyClass(key: DetailPageWidgetKey): string {
		return detailPageWidgetDensity(key) === 'compact' ? 'space-y-1.5' : 'space-y-2';
	}

	function keepDetailWidgetOpen(event: Event, collapsible: boolean) {
		if (collapsible) return;
		const details = event.currentTarget;
		if (details instanceof HTMLDetailsElement && !details.open) details.open = true;
	}

	function detailPageWidgetLimit(key: DetailPageWidgetKey, fallback: number): number {
		const rawWidget = detailPageWidgetConfig(key);
		const rawLimit = isRecord(rawWidget) ? rawWidget.limit : null;
		const numericLimit =
			typeof rawLimit === 'number' ? rawLimit : typeof rawLimit === 'string' ? Number(rawLimit) : NaN;
		if (!Number.isFinite(numericLimit)) return fallback;
		const limit = Math.trunc(numericLimit);
		return limit >= 1 && limit <= 50 ? limit : fallback;
	}

	function attachmentWidgetItemCount(): number {
		if (attachmentFields.length === 0) return activeAttachments.length;
		return attachmentFields.reduce(
			(count, field) => count + fieldAttachments(field.key).length + recordAttachmentValueUrls(field).length,
			0
		);
	}

	function detailHeaderField(key: 'subtitle_field' | 'badge_field'): FormField | null {
		const header = isRecord(form?.detail_layout?.header) ? form.detail_layout.header : null;
		const fieldKey = header && typeof header[key] === 'string' ? header[key] : '';
		return fields.find((field) => field.key === fieldKey && field.visibility?.detail !== false) ?? null;
	}

	function detailHighlightFieldsFromLayout(layout: Record<string, unknown> | null | undefined): FormField[] {
		const rawHighlights = isRecord(layout) && Array.isArray(layout.highlights) ? layout.highlights : [];
		const highlights: FormField[] = [];
		const seen = new Set<string>();
		for (const rawFieldKey of rawHighlights) {
			if (typeof rawFieldKey !== 'string' || seen.has(rawFieldKey)) continue;
			const field = fields.find((item) => item.key === rawFieldKey && item.visibility?.detail !== false);
			if (!field) continue;
			highlights.push(field);
			seen.add(rawFieldKey);
			if (highlights.length >= 4) break;
		}
		return highlights;
	}

	function parseFields(schema: Record<string, unknown>): FormField[] {
		const rawFields = Array.isArray(schema.fields) ? schema.fields : [];
		return rawFields
			.filter(isRecord)
			.map((field) => {
				const rawType = typeof field.type === 'string' ? field.type : 'text';
				return {
					field_id: typeof field.field_id === 'string' ? field.field_id : undefined,
					key: typeof field.key === 'string' ? field.key : '',
					label: typeof field.label === 'string' ? field.label : undefined,
					type: (rawType === 'select' ? 'single_select' : rawType) as FormField['type'],
					required: Boolean(field.required),
					options: Array.isArray(field.options)
						? field.options.filter((item): item is string => typeof item === 'string')
						: [],
					option_colors: isRecord(field.option_colors)
						? Object.fromEntries(
								Object.entries(field.option_colors).filter(
									(entry): entry is [string, string] => typeof entry[1] === 'string'
								)
							)
						: undefined,
					option_disabled: Array.isArray(field.option_disabled)
						? field.option_disabled.filter((item): item is string => typeof item === 'string')
						: undefined,
					option_archived: Array.isArray(field.option_archived)
						? field.option_archived.filter((item): item is string => typeof item === 'string')
						: undefined,
					option_descriptions: isRecord(field.option_descriptions)
						? Object.fromEntries(
								Object.entries(field.option_descriptions).filter(
									(entry): entry is [string, string] => typeof entry[1] === 'string'
								)
							)
						: undefined,
					option_groups: isRecord(field.option_groups)
						? Object.fromEntries(
								Object.entries(field.option_groups).filter(
									(entry): entry is [string, string] => typeof entry[1] === 'string'
								)
							)
						: undefined,
					amount: isRecord(field.amount) ? (field.amount as FormField['amount']) : undefined,
					visibility: isRecord(field.visibility) ? (field.visibility as FormField['visibility']) : undefined,
					relation: isRecord(field.relation) ? (field.relation as FormField['relation']) : undefined,
					attachment: isRecord(field.attachment)
						? (field.attachment as FormField['attachment'])
						: undefined
				};
			})
			.filter((field) => field.key);
	}

	function fieldType(field: FormField): string {
		return field.type ?? 'text';
	}

	function fieldLabel(field: FormField): string {
		return field.label?.trim() || field.key.replaceAll('_', ' ');
	}

	function permissionFieldList(labels: string[]): string {
		return labels.length > 0 ? labels.join(', ') : $t('forms.noFieldPolicyEffects');
	}

	function formatDate(value: string): string {
		return new Date(value).toLocaleString($locale === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function memberFieldHidden(field: FormField): boolean {
		return permissions?.effective.fields?.[field.key]?.read === false;
	}

	function memberFieldLocked(field: FormField): boolean {
		return permissions?.effective.fields?.[field.key]?.write === false;
	}

	function detailLayoutFieldKeys(section: Record<string, unknown>): string[] {
		const rawFields = Array.isArray(section.fields) ? section.fields : [];
		return rawFields
			.map((field) => {
				if (typeof field === 'string') return field;
				if (isRecord(field) && typeof field.key === 'string') return field.key;
				return '';
			})
			.filter(Boolean);
	}

	function detailLayoutSections(): DetailLayoutSection[] {
		const detailFields = fields.filter((field) => field.visibility?.detail !== false);
		const byKey = new Map(detailFields.map((field) => [field.key, field]));
		const usedFieldKeys = new Set<string>();
		const rawSections = Array.isArray(form?.detail_layout?.sections) ? form.detail_layout.sections : [];
		const sections = rawSections
			.filter(isRecord)
			.map((section, index) => {
				const layoutFields = detailLayoutFieldKeys(section)
					.map((key) => byKey.get(key))
					.filter((field): field is FormField => Boolean(field));
				if (layoutFields.length === 0) return null;
				for (const field of layoutFields) usedFieldKeys.add(field.key);
				const key =
					typeof section.key === 'string' && section.key.trim() ? section.key.trim() : `section_${index + 1}`;
				const title =
					typeof section.title === 'string' && section.title.trim()
						? section.title.trim()
						: typeof section.name === 'string' && section.name.trim()
							? section.name.trim()
							: $t('forms.detailSectionMain');
				return {
					key,
					title,
					description: detailLayoutSectionDescription(section.description),
					columns: detailLayoutSectionColumns(section.columns, pageFieldColumns),
					density: detailLayoutSectionDensity(section.density),
					hideEmptyFields: section.hide_empty_fields === true,
					collapsible: section.collapsible === true,
					defaultCollapsed: section.collapsible === true && section.default_collapsed === true,
					fields: layoutFields
				};
			})
			.filter((section): section is DetailLayoutSection => Boolean(section));
		const unplacedFields = detailFields.filter((field) => !usedFieldKeys.has(field.key));
		if (sections.length > 0 && unplacedFields.length > 0) {
			sections.push({
				key: 'additional',
				title: $t('forms.detailSectionAdditional'),
				description: '',
				columns: pageFieldColumns,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: unplacedFields
			});
		}
		return sections.length > 0
			? sections
			: [
					{
						key: 'main',
						title: $t('forms.detailSectionMain'),
						description: '',
						columns: pageFieldColumns,
						density: 'comfortable',
						hideEmptyFields: false,
						collapsible: false,
						defaultCollapsed: false,
						fields: detailFields
					}
				];
	}

	function attachmentManifestUrls(value: unknown): string[] {
		if (!value) return [];
		if (typeof value === 'string') return value.trim() ? [value.trim()] : [];
		if (Array.isArray(value)) return value.flatMap((item) => attachmentManifestUrls(item));
		if (isRecord(value)) {
			if (typeof value.url === 'string') return attachmentManifestUrls(value.url);
			if (typeof value.href === 'string') return attachmentManifestUrls(value.href);
		}
		return [];
	}

	function fieldAttachments(fieldKey: string): FormAttachment[] {
		return activeAttachments.filter((attachment) => attachment.field_key === fieldKey);
	}

	function attachmentSignedUrl(attachment: FormAttachment): string {
		return safeLinkUrl(attachmentSignedUrls[attachment.id] || attachment.url);
	}

	function attachmentThumbnailUrl(attachment: FormAttachment): string {
		return safeImageUrl(attachment.thumbnail_url || attachment.url);
	}

	async function loadAttachmentSignedUrls(items: FormAttachment[]) {
		const active = items.filter((attachment) => !attachment.archived_at && attachment.url);
		if (active.length === 0) {
			attachmentSignedUrls = {};
			return;
		}
		const entries = await Promise.all(
			active.map(async (attachment) => {
				const response = await formsApi.getAttachmentSignedDownload(attachment.id);
				return response.code === 0 && response.data?.url ? [attachment.id, response.data.url] : null;
			})
		);
		attachmentSignedUrls = Object.fromEntries(
			entries.filter((entry): entry is [string, string] => Array.isArray(entry))
		);
	}

	function recordAttachmentValueUrls(field: FormField): string[] {
		if (!record) return [];
		const metadataUrls = new Set(fieldAttachments(field.key).map((attachment) => attachment.url));
		return attachmentManifestUrls(record.values[field.key]).filter((url) => !metadataUrls.has(url));
	}

	function attachmentIsPreviewableImage(attachment: FormAttachment): boolean {
		return attachment.content_type.startsWith('image/') || /\.(png|jpe?g|webp|gif)$/i.test(attachment.file_name);
	}

	function childFormName(child: FormChildRecord): string {
		return childForms[child.record.form_id]?.name ?? child.record.form_id;
	}

	function childDisplayFields(child: FormChildRecord): FormField[] {
		const childForm = childForms[child.record.form_id];
		return childForm
			? parseFields(childForm.schema)
					.filter((field) => field.visibility?.list !== false && field.type !== 'child_table')
					.slice(0, 4)
			: [];
	}

	function eventTitle(event: BusinessEvent): string {
		if (typeof event.payload?.title === 'string') return event.payload.title;
		return event.event_type;
	}

	function eventJson(value: unknown): string {
		return JSON.stringify(value ?? {}, null, 2);
	}

	async function loadDetail() {
		loading = true;
		errorMessage = '';
		const recordResponse = await formsApi.getRecord(recordId);
		if (recordResponse.code !== 0 || !recordResponse.data) {
			errorMessage = recordResponse.message;
			loading = false;
			return;
		}
		record = recordResponse.data;

		const formResponse = await formsApi.get(recordResponse.data.form_id);
		if (formResponse.code !== 0 || !formResponse.data) {
			errorMessage = formResponse.message;
			loading = false;
			return;
		}
		form = formResponse.data;

		const [
			permissionResponse,
			linkResponse,
			childResponse,
			commentResponse,
			attachmentResponse,
			eventResponse
		] = await Promise.all([
			formsApi.getPermissions(formResponse.data.id),
			formsApi.listRecordLinks(recordResponse.data.id),
			formsApi.listRecordChildren(recordResponse.data.id, { per_page: 100 }),
			formsApi.listRecordComments(recordResponse.data.id, { per_page: 20 }),
			formsApi.listAttachments(formResponse.data.id, { record_id: recordResponse.data.id, per_page: 100 }),
			formsApi.listEvents(formResponse.data.id)
		]);

		if (permissionResponse.code === 0 && permissionResponse.data) permissions = permissionResponse.data;
		if (linkResponse.code === 0 && linkResponse.data) links = linkResponse.data;
		if (childResponse.code === 0 && childResponse.data) children = childResponse.data.items;
		if (commentResponse.code === 0 && commentResponse.data) comments = commentResponse.data.items;
		if (attachmentResponse.code === 0 && attachmentResponse.data) {
			attachments = attachmentResponse.data.items;
			await loadAttachmentSignedUrls(attachments);
		}
		if (eventResponse.code === 0 && eventResponse.data) events = eventResponse.data.items;

		const childFormIds = [...new Set(children.map((child) => child.record.form_id))].filter(
			(formId) => formId !== formResponse.data?.id
		);
		const childFormResponses = await Promise.all(childFormIds.map((formId) => formsApi.get(formId)));
		childForms = Object.fromEntries(
			childFormResponses
				.map((response) => response.data)
				.filter((item): item is UniversalForm => Boolean(item))
				.map((item) => [item.id, item])
		);

		loading = false;
	}

	async function handleCreateComment() {
		if (!record || !form) return;
		const body = commentDraft.trim();
		if (!body) return;
		commentSubmitting = true;
		const response = await formsApi.createRecordComment(record.id, {
			body,
			metadata: { source: 'web', route: 'record_detail', form_id: form.id, form_key: form.key }
		});
		commentSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		comments = [response.data, ...comments.filter((comment) => comment.id !== response.data?.id)];
		commentDraft = '';
		toast.success($t('forms.recordCommentAdded'));
	}
</script>

<svelte:head>
	<title>{record?.title ?? $t('forms.recordTitle')} · OpenPR</title>
</svelte:head>

<div class="min-h-screen bg-slate-50 px-4 py-6 dark:bg-slate-950 sm:px-6 lg:px-8">
	<div class="mx-auto max-w-7xl space-y-5">
		<a
			href={`/workspace/${workspaceId}/projects/${projectId}/forms${form && record ? `?form=${form.id}&record=${record.id}` : ''}`}
			class="inline-flex items-center gap-2 text-sm font-medium text-slate-600 hover:text-slate-950 dark:text-slate-300 dark:hover:text-white"
			data-durable-record-detail-back
		>
			<ArrowLeft class="h-4 w-4" aria-hidden="true" />
			{$t('common.back')}
		</a>

		{#if loading}
			<div class="rounded-md border border-slate-200 bg-white p-6 text-sm text-slate-500 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-400">
				{$t('common.loading')}
			</div>
		{:else if errorMessage}
			<div
				class="rounded-md border border-red-200 bg-red-50 p-6 text-sm text-red-700 dark:border-red-900/50 dark:bg-red-950/30 dark:text-red-200"
				data-durable-record-detail-error
			>
				{errorMessage}
			</div>
		{:else if record && form}
			<header
				class="border-b border-slate-200 pb-5 dark:border-slate-800"
				data-durable-record-detail-route={record.id}
				data-durable-record-detail-form={form.id}
			>
				<div class="flex flex-wrap items-start justify-between gap-4">
					<div class="min-w-0">
						<div class="flex flex-wrap items-center gap-2 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
							<span>{form.name}</span>
							<span class="font-mono">{form.key}</span>
						</div>
						<h1 class="mt-2 truncate text-2xl font-semibold text-slate-950 dark:text-slate-100">
							{record.title}
						</h1>
						{#if detailHeaderSubtitleField}
							<p
								class="mt-1 flex flex-wrap items-center gap-2 text-sm text-slate-600 dark:text-slate-300"
								data-durable-record-detail-header-subtitle={`${detailHeaderSubtitleField.key}:${record.id}`}
							>
								<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
									{fieldLabel(detailHeaderSubtitleField)}
								</span>
								<OptionValueBadges field={detailHeaderSubtitleField} value={record.values[detailHeaderSubtitleField.key]} />
							</p>
						{/if}
						<p class="mt-1 font-mono text-xs text-slate-500 dark:text-slate-400">{record.id}</p>
					</div>
					<div class="grid gap-2 text-right text-xs text-slate-500 dark:text-slate-400">
						{#if detailHeaderBadgeField}
							<div
								class="flex justify-end"
								data-durable-record-detail-header-badge={`${detailHeaderBadgeField.key}:${record.id}`}
								>
									<OptionValueBadges field={detailHeaderBadgeField} value={record.values[detailHeaderBadgeField.key]} />
								</div>
						{/if}
						<span>{$t('forms.updatedAt', { values: { date: formatDate(record.updated_at) } })}</span>
						<span>v{record.schema_version ?? form.schema_version}</span>
					</div>
				</div>
			</header>

			{#if permissions}
				<section
					class="rounded-lg border border-slate-200 bg-slate-50 px-4 py-3 text-sm dark:border-slate-700 dark:bg-slate-950/60"
					data-permission-state-summary
					data-durable-record-detail-permission-state={record.id}
					data-permission-state-mode="durable-detail"
					data-permission-state-scope={memberRecordScope}
					data-permission-state-hidden-fields={memberHiddenFieldLabels.join('|')}
					data-permission-state-locked-fields={memberLockedFieldLabels.join('|')}
				>
					<div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
						<div class="flex min-w-0 items-center gap-2">
							<Shield class="h-4 w-4 shrink-0 text-blue-600 dark:text-blue-300" aria-hidden="true" />
							<div class="min-w-0">
								<p class="font-medium text-slate-900 dark:text-slate-100">
									{$t('forms.permissionState')}
								</p>
								<p class="truncate text-xs text-slate-500 dark:text-slate-400">
									{$t('forms.permissionStateDescription')}
								</p>
								</div>
						</div>
						<div class="grid gap-2 text-xs sm:grid-cols-3 lg:min-w-[520px]">
							<div class="rounded-md border border-slate-200 bg-white px-3 py-2 dark:border-slate-700 dark:bg-slate-900">
								<span class="block font-medium uppercase text-slate-500 dark:text-slate-400">
									{$t('forms.recordScope')}
								</span>
								<span class="mt-1 block text-slate-900 dark:text-slate-100">
									{$t(`forms.recordScopes.${memberRecordScope}`)}
								</span>
								</div>
							<div class="rounded-md border border-amber-200 bg-white px-3 py-2 dark:border-amber-900/60 dark:bg-slate-900">
								<span class="block font-medium uppercase text-amber-700 dark:text-amber-200">
									{$t('forms.hiddenFieldEffect')}
								</span>
								<span class="mt-1 block truncate text-slate-900 dark:text-slate-100">
									{permissionFieldList(memberHiddenFieldLabels)}
								</span>
							</div>
							<div class="rounded-md border border-slate-200 bg-white px-3 py-2 dark:border-slate-700 dark:bg-slate-900">
								<span class="block font-medium uppercase text-slate-500 dark:text-slate-400">
									{$t('forms.lockedFieldEffect')}
								</span>
								<span class="mt-1 block truncate text-slate-900 dark:text-slate-100">
									{permissionFieldList(memberLockedFieldLabels)}
								</span>
								</div>
							</div>
						</div>
					</section>
			{/if}

			{#if detailHighlightFields.length > 0}
				<section
					class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4"
					data-durable-record-detail-highlights={record.id}
					data-durable-record-detail-highlight-order={detailHighlightFields.map((field) => field.key).join('|')}
				>
					{#each detailHighlightFields as field}
						<div
							class="min-w-0 rounded-md border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900"
							data-durable-record-detail-highlight={`${record.id}:${field.key}`}
						>
							<div class="truncate text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
								{fieldLabel(field)}
							</div>
							<div class="mt-2 break-words text-sm font-semibold text-slate-950 dark:text-slate-100">
								<OptionValueBadges {field} value={record.values[field.key]} />
							</div>
						</div>
					{/each}
				</section>
			{/if}

			<div
				class={pageWidgetRegion === 'below'
					? 'space-y-5'
					: 'grid gap-5 lg:grid-cols-[minmax(0,1fr)_360px]'}
				data-durable-record-detail-page-composition={`${record.id}:${pageWidgetRegion}`}
			>
					<main class="space-y-4" data-durable-record-detail-layout={record.id}>
						{#each detailLayoutSections() as section}
							<details
								class={detailLayoutSectionContainerClass(section.density)}
								open={!section.collapsible || !section.defaultCollapsed}
								ontoggle={(event) => keepDetailSectionOpen(event, section.collapsible)}
								data-durable-record-detail-section={`${record.id}:${section.key}`}
								data-durable-record-detail-section-density={`${record.id}:${section.key}:${section.density}`}
								data-durable-record-detail-section-collapsible={`${record.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
								data-durable-record-detail-section-open={`${record.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
							>
							<summary class={section.collapsible ? 'cursor-pointer list-none' : 'cursor-default list-none'}>
								<h2 class="inline text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
									{section.title}
								</h2>
							</summary>
							{#if section.description}
								<p
									class="mt-2 text-sm text-slate-500 dark:text-slate-400"
									data-durable-record-detail-section-description={`${record.id}:${section.key}`}
								>
									{section.description}
								</p>
							{/if}
							<dl
								class={detailLayoutSectionGridClass(section.columns, section.density)}
								data-durable-record-detail-field-columns={`${record.id}:${section.columns}`}
								data-durable-record-detail-section-columns={`${record.id}:${section.key}:${section.columns}`}
							>
								{#each detailLayoutSectionFieldsForRecord(section, record) as field}
									<div
										class={detailLayoutSectionFieldClass(field, section.columns)}
										data-durable-record-detail-field={`${record.id}:${field.key}`}
									>
										<dt class="flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
											<span>{fieldLabel(field)}</span>
											{#if memberFieldHidden(field)}
												<span
													class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
													data-durable-record-detail-permission-hidden={field.key}
												>
													{$t('forms.fieldHiddenBadge')}
												</span>
											{/if}
											{#if memberFieldLocked(field)}
												<span
													class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-300"
													data-durable-record-detail-permission-locked={field.key}
												>
													{$t('forms.fieldLockedBadge')}
												</span>
											{/if}
										</dt>
											<dd class="mt-1 break-words text-sm text-slate-900 dark:text-slate-100">
												{#if fieldType(field) === 'child_table'}
													<span class="text-slate-500 dark:text-slate-400">{$t('forms.childRecords')}</span>
												{:else}
													<OptionValueBadges {field} value={record.values[field.key]} />
												{/if}
											</dd>
										</div>
								{/each}
							</dl>
						</details>
					{/each}
				</main>

				<aside
					class={pageWidgetRegion === 'below' ? 'grid gap-4 lg:grid-cols-2' : 'space-y-4'}
					data-durable-record-detail-widget-order={pageWidgetKeys.join('|')}
					data-durable-record-detail-widget-region={pageWidgetRegion}
				>
					{#each pageWidgetKeys as widgetKey}
						{#if widgetKey === 'children'}
							{@const childrenWidgetDescription = detailPageWidgetDescription('children')}
							{#if !detailPageWidgetHideEmpty('children') || children.length > 0}
								<details
									class={detailPageWidgetContainerClass('children')}
								open={detailPageWidgetOpen('children')}
								ontoggle={(event) => keepDetailWidgetOpen(event, detailPageWidgetCollapsible('children'))}
								data-durable-record-detail-children={record.id}
								data-durable-record-detail-child-count={children.length}
									data-durable-record-detail-widget-limit={`children:${detailPageWidgetLimit('children', 50)}`}
									data-durable-record-detail-widget-collapsible={`children:${record.id}:${detailPageWidgetCollapsible('children')}:${detailPageWidgetDefaultCollapsed('children')}`}
									data-durable-record-detail-widget-density={`children:${record.id}:${detailPageWidgetDensity('children')}`}
									data-durable-record-detail-widget-open={`children:${record.id}:${detailPageWidgetOpen('children')}`}
							>
								<summary class={detailPageWidgetSummaryClass('children')}>
									<TableProperties class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
									<div class="min-w-0">
										<div class="flex min-w-0 flex-wrap items-center gap-2">
											<h2
												class="text-sm font-semibold text-slate-950 dark:text-slate-100"
												data-durable-record-detail-widget-title={`children:${record.id}`}
											>
												{detailPageWidgetTitle('children', $t('forms.childRecords'))}
											</h2>
											{#if detailPageWidgetShowCount('children')}
												<span
													class="rounded-sm bg-slate-100 px-1.5 py-0.5 font-mono text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300"
													data-durable-record-detail-widget-count-badge={`children:${record.id}`}
												>
													{children.length}
												</span>
											{/if}
										</div>
										{#if childrenWidgetDescription}
											<p
												class="mt-1 text-xs leading-5 text-slate-500 dark:text-slate-400"
												data-durable-record-detail-widget-description={`children:${record.id}`}
											>
												{childrenWidgetDescription}
											</p>
											{/if}
										</div>
								</summary>
									{#if children.length > 0}
										<div class="overflow-x-auto">
											<table class="min-w-full divide-y divide-slate-200 text-xs dark:divide-slate-800">
											<thead>
												<tr>
													<th class="px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
														{$t('forms.recordTitle')}
													</th>
													<th class="px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
														{$t('forms.recordLinks')}
													</th>
												</tr>
											</thead>
											<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
												{#each children.slice(0, detailPageWidgetLimit('children', 50)) as child}
													<tr data-durable-record-detail-child={child.record.id}>
														<td class="px-3 py-2 font-medium text-slate-900 dark:text-slate-100">
															{child.record.title}
														</td>
														<td class="px-3 py-2 text-slate-500 dark:text-slate-400">
															{childFormName(child)} · {child.relation_key}
														</td>
													</tr>
												{/each}
											</tbody>
										</table>
									</div>
								{:else}
									<p
										class="text-sm text-slate-500 dark:text-slate-400"
										data-durable-record-detail-widget-empty={`children:${record.id}`}
									>
											{detailPageWidgetEmptyText('children', $t('forms.noChildRecords'))}
										</p>
										{/if}
									</details>
								{/if}
						{:else if widgetKey === 'links'}
					{@const linksWidgetDescription = detailPageWidgetDescription('links')}
					{#if !detailPageWidgetHideEmpty('links') || links.length > 0}
							<details
								class={detailPageWidgetContainerClass('links')}
							open={detailPageWidgetOpen('links')}
							ontoggle={(event) => keepDetailWidgetOpen(event, detailPageWidgetCollapsible('links'))}
							data-durable-record-detail-links={record.id}
							data-durable-record-detail-link-count={links.length}
								data-durable-record-detail-widget-limit={`links:${detailPageWidgetLimit('links', 50)}`}
								data-durable-record-detail-widget-collapsible={`links:${record.id}:${detailPageWidgetCollapsible('links')}:${detailPageWidgetDefaultCollapsed('links')}`}
								data-durable-record-detail-widget-density={`links:${record.id}:${detailPageWidgetDensity('links')}`}
								data-durable-record-detail-widget-open={`links:${record.id}:${detailPageWidgetOpen('links')}`}
						>
							<summary class={detailPageWidgetSummaryClass('links')}>
								<Link class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
								<div class="min-w-0">
									<div class="flex min-w-0 flex-wrap items-center gap-2">
									<h2
										class="text-sm font-semibold text-slate-950 dark:text-slate-100"
										data-durable-record-detail-widget-title={`links:${record.id}`}
									>
										{detailPageWidgetTitle('links', $t('forms.recordLinks'))}
									</h2>
									{#if detailPageWidgetShowCount('links')}
										<span
											class="rounded-sm bg-slate-100 px-1.5 py-0.5 font-mono text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300"
											data-durable-record-detail-widget-count-badge={`links:${record.id}`}
										>
											{links.length}
										</span>
									{/if}
								</div>
										{#if linksWidgetDescription}
											<p
												class="mt-1 text-xs leading-5 text-slate-500 dark:text-slate-400"
												data-durable-record-detail-widget-description={`links:${record.id}`}
											>
												{linksWidgetDescription}
											</p>
											{/if}
										</div>
								</summary>
									<div class={detailPageWidgetBodyClass('links')}>
								{#each links.slice(0, detailPageWidgetLimit('links', 50)) as item}
									<div class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700">
									<div class="font-medium text-slate-800 dark:text-slate-200">
										{item.relation_key} / {item.relation_type}
									</div>
									<div class="mt-1 truncate font-mono text-slate-500 dark:text-slate-400">
										{item.target_type}:{item.target_id}
									</div>
								</div>
							{/each}
							{#if links.length === 0}
								<p
									class="text-sm text-slate-500 dark:text-slate-400"
									data-durable-record-detail-widget-empty={`links:${record.id}`}
								>
									{detailPageWidgetEmptyText('links', $t('forms.noLinkedData'))}
								</p>
									{/if}
									</div>
						</details>
						{/if}

						{:else if widgetKey === 'comments'}
					{@const commentsWidgetDescription = detailPageWidgetDescription('comments')}
					{#if !detailPageWidgetHideEmpty('comments') || comments.length > 0}
						<details
							class={detailPageWidgetContainerClass('comments')}
						open={detailPageWidgetOpen('comments')}
						ontoggle={(event) => keepDetailWidgetOpen(event, detailPageWidgetCollapsible('comments'))}
						data-durable-record-detail-comments={record.id}
							data-durable-record-detail-comment-count={comments.length}
							data-durable-record-detail-widget-limit={`comments:${detailPageWidgetLimit('comments', 5)}`}
							data-durable-record-detail-widget-collapsible={`comments:${record.id}:${detailPageWidgetCollapsible('comments')}:${detailPageWidgetDefaultCollapsed('comments')}`}
							data-durable-record-detail-widget-density={`comments:${record.id}:${detailPageWidgetDensity('comments')}`}
							data-durable-record-detail-widget-open={`comments:${record.id}:${detailPageWidgetOpen('comments')}`}
						>
							<summary class={detailPageWidgetSummaryClass('comments')}>
							<MessageSquare class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
							<div class="min-w-0">
								<div class="flex min-w-0 flex-wrap items-center gap-2">
									<h2
										class="text-sm font-semibold text-slate-950 dark:text-slate-100"
										data-durable-record-detail-widget-title={`comments:${record.id}`}
									>
										{detailPageWidgetTitle('comments', $t('forms.recordComments'))}
									</h2>
									{#if detailPageWidgetShowCount('comments')}
										<span
											class="rounded-sm bg-slate-100 px-1.5 py-0.5 font-mono text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300"
											data-durable-record-detail-widget-count-badge={`comments:${record.id}`}
										>
											{comments.length}
										</span>
									{/if}
								</div>
									{#if commentsWidgetDescription}
										<p
											class="mt-1 text-xs leading-5 text-slate-500 dark:text-slate-400"
											data-durable-record-detail-widget-description={`comments:${record.id}`}
										>
											{commentsWidgetDescription}
										</p>
									{/if}
								</div>
							</summary>
							<div class={detailPageWidgetBodyClass('comments')}>
							{#each comments.slice(0, detailPageWidgetLimit('comments', 5)) as comment}
								<div
									class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
									data-durable-record-detail-comment={comment.id}
								>
									<div class="flex flex-wrap items-center justify-between gap-2">
										<span class="font-medium text-slate-800 dark:text-slate-200">
											{comment.author_name || comment.author_email || comment.author_id?.slice(0, 8) || 'system'}
										</span>
										<span class="font-mono text-[11px] text-slate-500 dark:text-slate-400">
											{formatDate(comment.created_at)}
										</span>
									</div>
									<p class="mt-2 whitespace-pre-wrap text-sm leading-relaxed text-slate-700 dark:text-slate-300">
										{comment.body}
									</p>
								</div>
							{/each}
							{#if comments.length === 0}
								<p
									class="text-sm text-slate-500 dark:text-slate-400"
									data-durable-record-detail-widget-empty={`comments:${record.id}`}
								>
									{detailPageWidgetEmptyText('comments', $t('forms.noRecordComments'))}
								</p>
							{/if}
							<textarea
								class="min-h-20 w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
								bind:value={commentDraft}
								placeholder={$t('forms.recordCommentPlaceholder')}
								data-durable-record-detail-comment-input={record.id}
							></textarea>
							<span data-durable-record-detail-comment-submit={record.id}>
								<Button
									size="sm"
									variant="secondary"
									loading={commentSubmitting}
									disabled={!commentDraft.trim()}
									onclick={handleCreateComment}
								>
									{$t('forms.addComment')}
								</Button>
							</span>
							</div>
						</details>
					{/if}

						{:else if widgetKey === 'attachments'}
					{@const attachmentsWidgetDescription = detailPageWidgetDescription('attachments')}
					{#if !detailPageWidgetHideEmpty('attachments') || attachmentWidgetItemCount() > 0}
							<details
								class={detailPageWidgetContainerClass('attachments')}
							open={detailPageWidgetOpen('attachments')}
							ontoggle={(event) => keepDetailWidgetOpen(event, detailPageWidgetCollapsible('attachments'))}
								data-durable-record-detail-attachments={record.id}
								data-durable-record-detail-attachment-count={attachmentWidgetItemCount()}
								data-durable-record-detail-widget-collapsible={`attachments:${record.id}:${detailPageWidgetCollapsible('attachments')}:${detailPageWidgetDefaultCollapsed('attachments')}`}
								data-durable-record-detail-widget-density={`attachments:${record.id}:${detailPageWidgetDensity('attachments')}`}
								data-durable-record-detail-widget-open={`attachments:${record.id}:${detailPageWidgetOpen('attachments')}`}
						>
							<summary class={detailPageWidgetSummaryClass('attachments')}>
								<FileDown class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
								<div class="min-w-0">
									<div class="flex min-w-0 flex-wrap items-center gap-2">
									<h2
										class="text-sm font-semibold text-slate-950 dark:text-slate-100"
										data-durable-record-detail-widget-title={`attachments:${record.id}`}
									>
										{detailPageWidgetTitle('attachments', $t('forms.attachmentConfig'))}
									</h2>
									{#if detailPageWidgetShowCount('attachments')}
										<span
											class="rounded-sm bg-slate-100 px-1.5 py-0.5 font-mono text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300"
											data-durable-record-detail-widget-count-badge={`attachments:${record.id}`}
										>
											{attachmentWidgetItemCount()}
										</span>
									{/if}
								</div>
								{#if attachmentsWidgetDescription}
									<p
										class="mt-1 text-xs leading-5 text-slate-500 dark:text-slate-400"
										data-durable-record-detail-widget-description={`attachments:${record.id}`}
									>
										{attachmentsWidgetDescription}
									</p>
									{/if}
								</div>
							</summary>
								<div class={detailPageWidgetDensity('attachments') === 'compact' ? 'space-y-2' : 'space-y-3'}>
							{#each attachmentFields as field}
								{@const metadataAttachments = fieldAttachments(field.key)}
								{@const valueUrls = recordAttachmentValueUrls(field)}
								<div
									class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
									data-durable-record-detail-attachment-field={field.key}
									data-durable-record-detail-attachment-field-count={`${field.key}:${metadataAttachments.length + valueUrls.length}`}
								>
									<div class="mb-2 font-medium text-slate-800 dark:text-slate-200">{fieldLabel(field)}</div>
										<div class={detailPageWidgetBodyClass('attachments')}>
										{#each metadataAttachments as attachment}
											<div
												class="flex flex-wrap items-center gap-2 rounded bg-slate-50 px-2 py-1.5 dark:bg-slate-950"
												data-durable-record-detail-attachment={attachment.id}
											>
												{#if attachmentIsPreviewableImage(attachment)}
													<img
														src={attachmentThumbnailUrl(attachment)}
														alt={attachment.file_name}
														class="h-10 w-10 rounded object-cover ring-1 ring-slate-200 dark:ring-slate-700"
													/>
												{/if}
												<div class="min-w-0 flex-1">
													<div class="truncate font-medium text-slate-700 dark:text-slate-200">
														{attachment.file_name}
													</div>
													<div class="truncate font-mono text-[11px] text-slate-500 dark:text-slate-400">
														{attachment.url}
													</div>
													</div>
													<a
														href={attachmentSignedUrl(attachment) || undefined}
														target="_blank"
														rel="noreferrer"
														class="rounded px-2 py-1 font-medium text-blue-700 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
														data-durable-record-detail-attachment-signed-url={attachment.id}
													>
														{$t('forms.attachmentOpen')}
												</a>
											</div>
										{/each}
										{#each valueUrls as url}
											{#if safeLinkUrl(url)}
												<a
													href={safeLinkUrl(url)}
													target="_blank"
													rel="noreferrer"
													class="block truncate rounded bg-slate-50 px-2 py-1.5 font-mono text-[11px] text-blue-700 dark:bg-slate-950 dark:text-blue-300"
													data-durable-record-detail-attachment-value={`${field.key}:${url}`}
												>
													{url}
												</a>
											{:else}
												<span
													class="block truncate rounded bg-slate-50 px-2 py-1.5 font-mono text-[11px] text-amber-700 dark:bg-slate-950 dark:text-amber-300"
													data-durable-record-detail-attachment-value={`${field.key}:${url}`}
													data-durable-record-detail-attachment-value-blocked={field.key}
												>
													{url}
												</span>
											{/if}
										{/each}
										{#if metadataAttachments.length === 0 && valueUrls.length === 0}
											<p class="text-sm text-slate-500 dark:text-slate-400">
												{detailPageWidgetEmptyText('attachments', $t('forms.noRecordAttachments'))}
											</p>
										{/if}
									</div>
								</div>
							{/each}
							{#if attachmentFields.length === 0 && activeAttachments.length > 0}
								{#each activeAttachments as attachment}
									<div
										class="flex flex-wrap items-center gap-2 rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
										data-durable-record-detail-attachment={attachment.id}
									>
										{#if attachmentIsPreviewableImage(attachment)}
											<img
														src={attachmentThumbnailUrl(attachment)}
												alt={attachment.file_name}
												class="h-10 w-10 rounded object-cover ring-1 ring-slate-200 dark:ring-slate-700"
											/>
										{/if}
										<div class="min-w-0 flex-1">
											<div class="truncate font-medium text-slate-700 dark:text-slate-200">
												{attachment.file_name}
											</div>
											<div class="truncate font-mono text-[11px] text-slate-500 dark:text-slate-400">
												{attachment.field_key} · {attachment.url}
											</div>
											</div>
											<a
												href={attachmentSignedUrl(attachment) || undefined}
												target="_blank"
												rel="noreferrer"
												class="rounded px-2 py-1 font-medium text-blue-700 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
												data-durable-record-detail-attachment-signed-url={attachment.id}
											>
												{$t('forms.attachmentOpen')}
										</a>
									</div>
								{/each}
							{/if}
							{#if attachmentFields.length === 0}
								{#if activeAttachments.length === 0}
									<p
										class="text-sm text-slate-500 dark:text-slate-400"
										data-durable-record-detail-widget-empty={`attachments:${record.id}`}
									>
										{detailPageWidgetEmptyText('attachments', $t('forms.noRecordAttachments'))}
									</p>
								{/if}
								{/if}
							</div>
						</details>
						{/if}

						{:else if widgetKey === 'events'}
					{@const eventsWidgetDescription = detailPageWidgetDescription('events')}
					{#if !detailPageWidgetHideEmpty('events') || recordEvents.length > 0}
							<details
								class={detailPageWidgetContainerClass('events')}
							open={detailPageWidgetOpen('events')}
							ontoggle={(event) => keepDetailWidgetOpen(event, detailPageWidgetCollapsible('events'))}
							data-durable-record-detail-events={record.id}
								data-durable-record-detail-event-count={recordEvents.length}
								data-durable-record-detail-widget-limit={`events:${detailPageWidgetLimit('events', 5)}`}
								data-durable-record-detail-widget-collapsible={`events:${record.id}:${detailPageWidgetCollapsible('events')}:${detailPageWidgetDefaultCollapsed('events')}`}
								data-durable-record-detail-widget-density={`events:${record.id}:${detailPageWidgetDensity('events')}`}
								data-durable-record-detail-widget-open={`events:${record.id}:${detailPageWidgetOpen('events')}`}
						>
							<summary class={detailPageWidgetSummaryClass('events')}>
								<Activity class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
								<div class="min-w-0">
									<div class="flex min-w-0 flex-wrap items-center gap-2">
									<h2
										class="text-sm font-semibold text-slate-950 dark:text-slate-100"
										data-durable-record-detail-widget-title={`events:${record.id}`}
									>
										{detailPageWidgetTitle('events', $t('forms.timelineSourceEvents'))}
									</h2>
									{#if detailPageWidgetShowCount('events')}
										<span
											class="rounded-sm bg-slate-100 px-1.5 py-0.5 font-mono text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300"
											data-durable-record-detail-widget-count-badge={`events:${record.id}`}
										>
											{recordEvents.length}
										</span>
									{/if}
								</div>
								{#if eventsWidgetDescription}
									<p
										class="mt-1 text-xs leading-5 text-slate-500 dark:text-slate-400"
										data-durable-record-detail-widget-description={`events:${record.id}`}
									>
										{eventsWidgetDescription}
									</p>
									{/if}
								</div>
							</summary>
							<div class={detailPageWidgetBodyClass('events')}>
							{#each recordEvents.slice(0, detailPageWidgetLimit('events', 5)) as event}
								<details
									class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
									data-durable-record-detail-event={event.id}
								>
									<summary class="cursor-pointer list-none">
										<div class="flex flex-wrap items-start justify-between gap-2">
											<div class="min-w-0">
												<div class="truncate font-medium text-slate-800 dark:text-slate-200">
													{eventTitle(event)}
												</div>
												<div class="mt-1 font-mono text-[11px] text-slate-500 dark:text-slate-400">
													{event.event_type}
												</div>
											</div>
											<span class="font-mono text-[11px] text-slate-500 dark:text-slate-400">
												{formatDate(event.created_at)}
											</span>
										</div>
									</summary>
									<pre class="mt-3 max-h-40 overflow-auto rounded bg-slate-50 p-2 font-mono text-[11px] text-slate-600 dark:bg-slate-950 dark:text-slate-300">{eventJson(event.payload)}</pre>
								</details>
							{/each}
							{#if recordEvents.length === 0}
								<p
									class="text-sm text-slate-500 dark:text-slate-400"
									data-durable-record-detail-widget-empty={`events:${record.id}`}
								>
									{detailPageWidgetEmptyText('events', $t('forms.noRecordEvents'))}
								</p>
								{/if}
							</div>
						</details>
						{/if}
						{/if}
					{/each}
				</aside>
			</div>
		{/if}
	</div>
</div>

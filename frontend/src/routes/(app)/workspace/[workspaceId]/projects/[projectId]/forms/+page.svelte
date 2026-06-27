<script lang="ts">
	import {
		Activity,
		AlertTriangle,
		ArrowDown,
		ArrowLeftRight,
		ArrowUp,
		Calculator,
		ChevronLeft,
		ChevronRight,
		Copy,
		Eye,
		EyeOff,
		FileDown,
		FileText,
		Filter,
		FileUp,
		Link,
		MoreVertical,
		Pencil,
		Plus,
		RefreshCw,
		Shield,
		TableProperties,
		Trash2,
		X
	} from '@lucide/svelte';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { apiClient } from '$lib/api/client';
	import { connectorsApi, type Invocation } from '$lib/api/connectors';
	import {
		formsApi,
		type BusinessEvent,
		type FormAggregate,
		type FormAttachment,
		type FormFieldDependency,
		type FormFieldUsage,
		type FormField,
			type FormFieldType,
		type FormChildRecord,
		type FormPermissionAction,
		type FormPermissions,
		type FormRecordComment,
		type FormRecordImportPreview,
			type FormRecordImportRow,
			type FormRecord,
		type FormRecordLink,
		type FormRelationTarget,
		type FormView,
		type UniversalForm
	} from '$lib/api/forms';
	import { membersApi, type WorkspaceMember } from '$lib/api/members';
	import { projectsApi, type Project, type ScenarioTemplate } from '$lib/api/projects';
	import Button from '$lib/components/Button.svelte';
	import FormAutomationPanel from '$lib/components/forms/FormAutomationPanel.svelte';
	import FormDesignerWorkspace from '$lib/components/forms/FormDesignerWorkspace.svelte';
	import OptionValueBadges from '$lib/components/forms/OptionValueBadges.svelte';
	import FormWorkflowTabs, {
		type FormWorkflowMode
	} from '$lib/components/forms/FormWorkflowTabs.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { toast } from '$lib/stores/toast';
	import {
		scenarioTemplateDescription,
		scenarioTemplateIndustry,
		scenarioTemplateName
	} from '$lib/utils/project-type-i18n';
	import { requireRouteParam } from '$lib/utils/route-params';
	import { locale, t } from 'svelte-i18n';

	type ImportMappingTransform =
		| 'none'
		| 'trim'
		| 'uppercase'
		| 'lowercase'
		| 'capitalize'
		| 'titlecase'
		| 'normalize_spaces'
		| 'strip_non_digits'
		| 'slugify';
	type ImportWizardStep = 'source' | 'mapping' | 'preview' | 'commit';
	type AttachmentBundleEntry = {
		record_id: string;
		record_title: string;
		field_key: string;
		field_label: string;
		field_type: string;
		url: string;
		download_url: string;
		thumbnail_url: string | null;
		media_metadata?: Record<string, unknown> | null;
		file_name: string;
		content_type: string;
		byte_size: number;
		storage_key: string | null;
		attachment_id: string | null;
		source: string;
	};
	type AttachmentBundleExport = {
		form_id: string;
		form_key: string;
		view_id: string | null;
		view_name: string | null;
		exported_at: string;
		record_count: number;
		attachment_count: number;
		bundle_format: string;
		attachments: AttachmentBundleEntry[];
	};
	type ZipFileInput = {
		path: string;
		data: string | Uint8Array;
	};

	const workspaceId = $derived(requireRouteParam($page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam($page.params.projectId, 'projectId'));
	const fieldTypeOptions: FormFieldType[] = [
		'text',
		'phone',
		'email',
		'address',
		'location',
		'scan',
		'signature',
		'autonumber',
		'member',
		'textarea',
		'rich_text',
		'number',
		'integer',
		'amount',
		'rating',
		'progress',
		'date',
		'datetime',
		'single_select',
		'multi_select',
		'boolean',
		'relation',
		'child_table',
		'attachment',
		'image',
		'formula',
		'ai_summary'
	];
	const formPermissionActions: FormPermissionAction[] = [
		'form.view',
		'form.design',
		'record.create',
		'record.update',
		'record.delete',
		'record.export'
	];
	const detailPageWidgetOptions: {
		key: DetailPageWidgetKey;
		labelKey: string;
		descriptionKey: string;
	}[] = [
		{
			key: 'children',
			labelKey: 'forms.detailPageWidgetChildren',
			descriptionKey: 'forms.detailPageWidgetChildrenDescription'
		},
		{
			key: 'links',
			labelKey: 'forms.detailPageWidgetLinks',
			descriptionKey: 'forms.detailPageWidgetLinksDescription'
		},
		{
			key: 'comments',
			labelKey: 'forms.detailPageWidgetComments',
			descriptionKey: 'forms.detailPageWidgetCommentsDescription'
		},
		{
			key: 'attachments',
			labelKey: 'forms.detailPageWidgetAttachments',
			descriptionKey: 'forms.detailPageWidgetAttachmentsDescription'
		},
		{
			key: 'events',
			labelKey: 'forms.detailPageWidgetEvents',
			descriptionKey: 'forms.detailPageWidgetEventsDescription'
		}
	];
	const detailPageWidgetKeys = new Set(detailPageWidgetOptions.map((option) => option.key));

	type FieldDraft = {
		field_id?: string;
		key: string;
		label: string;
		type: FormFieldType;
		required: boolean;
		optionsText: string;
		optionColorsText: string;
		optionDisabledText: string;
		optionArchivedText: string;
		optionDescriptionsText: string;
		optionGroupsText: string;
		currency: string;
		scale: string;
		placeholder: string;
		helpText: string;
		defaultValue: string;
		validationMin: string;
		validationMax: string;
		validationPattern: string;
		validationMaxLength: string;
		listVisible: boolean;
		detailVisible: boolean;
		conditionalHiddenWhenText: string;
		conditionalReadOnlyWhenText: string;
		readOnly: boolean;
		formulaOp: string;
		formulaArgsText: string;
		formulaScale: string;
		formulaRelationKey: string;
		formulaChildField: string;
		relationTargetType: string;
		relationFormKey: string;
		relationKey: string;
		relationType: string;
		relationDisplayField: string;
		attachmentAcceptText: string;
		attachmentMaxSizeMb: string;
		attachmentMultiple: boolean;
		attachmentPreview: boolean;
		attachmentThumbnailFormat: string;
		attachmentPreviewFormat: string;
		attachmentVariantsText: string;
		attachmentStoragePolicy: string;
		attachmentRetentionDays: string;
		attachmentSignedUrlTtlMinutes: string;
	};

	type DesignerSchemaChange = {
		kind: 'delete' | 'rename' | 'type_change';
		field_id?: string;
		fieldKey: string;
		fieldLabel: string;
		from?: string;
		to?: string;
		valueCount: number;
		indexedCount: number;
		dependencyCount: number;
		blockingDependencyCount: number;
	};

	type DetailPageWidgetKey = 'children' | 'links' | 'comments' | 'attachments' | 'events';
	type DetailPageWidgetRegion = 'sidebar' | 'below';
	type DetailPageWidgetDensity = 'comfortable' | 'compact';

	type DetailPageWidgetEntry = {
		key: DetailPageWidgetKey;
		enabled: boolean;
		title: string;
		description: string;
		emptyText: string;
		showCount: boolean;
		hideEmpty: boolean;
		collapsible: boolean;
		defaultCollapsed: boolean;
		density: DetailPageWidgetDensity;
		limit: string;
	};
	type DetailPageCompositionDraft = {
		widgetRegion: DetailPageWidgetRegion;
		fieldColumns: 1 | 2;
	};

	type ChildEditorState = {
		relationFieldKey: string;
		childFormId: string;
		recordId?: string;
		title: string;
		draft: Record<string, unknown>;
	};

	type ViewFilterOperator = 'contains' | 'equals' | 'not_equals' | 'not_empty';
	type ViewSortDirection = 'asc' | 'desc';
	type ViewVisibility = 'shared' | 'private';
	type ViewFilterLogic = 'all' | 'any';
	type ViewCalendarRange = 'week' | 'month';
	type ViewCalendarRecurrence = 'none' | 'daily' | 'weekly' | 'monthly';
	type ViewCalendarDayScope = 'all' | 'workweek';
	type ViewCalendarLayout = 'grid' | 'agenda';
	type CardLayoutMode = 'standard' | 'gallery' | 'compact' | 'list';
	type CardFieldLimit = 3 | 6 | 9 | 99;
	type CardCoverAspect = 'auto' | 'wide' | 'square' | 'portrait';
	type CardCoverFit = 'cover' | 'contain';
	type CardCoverPosition = 'top' | 'left' | 'hidden';
	type PivotAggregate = 'sum' | 'count' | 'avg';
	type PivotTotalsMode = 'all' | 'rows' | 'columns' | 'none';
	type PivotValueDisplay = 'value' | 'percent_row' | 'percent_column' | 'percent_total';
	type PivotSortMode =
		| 'label'
		| 'row_total_desc'
		| 'row_total_asc'
		| 'column_total_desc'
		| 'column_total_asc';
	type PivotRowLimit = 0 | 2 | 5 | 10;
	type PivotColumnLimit = 0 | 2 | 5 | 10;
	type CalendarExceptionDateMap = Record<string, string[]>;
	type FormRecordScope = 'all' | 'owned';
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
	type DetailLayoutSectionDraft = {
		key: string;
		title: string;
		description: string;
		columns: 1 | 2 | 3;
		density: DetailLayoutSectionDensity;
		hideEmptyFields: boolean;
		collapsible: boolean;
		defaultCollapsed: boolean;
		fields: string[];
	};
	type DetailHeaderDraft = {
		subtitleField: string;
		badgeField: string;
	};
	type DetailHighlightDraft = {
		fields: string[];
	};
	type ViewFilterDraft = {
		field: string;
		operator: ViewFilterOperator;
		value: string;
		disabled?: boolean;
	};
	type ViewFilterGroupDraft = {
		logic: ViewFilterLogic;
		filters: ViewFilterDraft[];
		disabled?: boolean;
	};
	type ParsedViewFilterExpression =
		| { kind: 'condition'; filter: ParsedViewFilter; disabled?: boolean }
		| {
				kind: 'group';
				logic: ViewFilterLogic;
				children: ParsedViewFilterExpression[];
				disabled?: boolean;
		  };
	type CalendarRecordOccurrence = {
		key: string;
		record: FormRecord;
		date: string;
		startDate: string;
		endDate: string;
		recurring: boolean;
		spanStart: boolean;
		spanEnd: boolean;
		spanDays: number;
	};
	type CalendarGridGroup = {
		key: string;
		label: string;
		date: string;
		records: CalendarRecordOccurrence[];
	};
	type CalendarResourceRow = {
		key: string;
		label: string;
		records: FormRecord[];
		groups: CalendarGridGroup[];
	};
	type GanttTask = {
		record: FormRecord;
		start: string;
		end: string;
		dependencyValue: string;
		startOffset: number;
		span: number;
	};
	type GanttScale = 'day' | 'week' | 'month';
	type GanttTimeline = {
		startField: FormField | null;
		endField: FormField | null;
		groupField: FormField | null;
		dependencyField: FormField | null;
		scale: GanttScale;
		dates: string[];
		lanes: Array<{ key: string; label: string; tasks: GanttTask[] }>;
	};
	type PivotTable = {
		rowField: FormField | null;
		columnField: FormField | null;
		valueField: FormField | null;
		valueCurrency: string;
		aggregate: PivotAggregate;
		totalsMode: PivotTotalsMode;
		valueDisplay: PivotValueDisplay;
		sortMode: PivotSortMode;
		emptyBuckets: boolean;
		hideZeroBuckets: boolean;
		showCounts: boolean;
		heatmap: boolean;
		rowLimit: PivotRowLimit;
		columnLimit: PivotColumnLimit;
		maxCellValue: number;
		columns: Array<{ key: string; label: string; total: number }>;
		rows: Array<{
			key: string;
			label: string;
			total: number;
			cells: Record<string, number>;
			cellCounts: Record<string, number>;
		}>;
		grandTotal: number;
	};
	type PivotDrilldownSelection = {
		rowKey: string | null;
		rowLabel: string;
		columnKey: string | null;
		columnLabel: string;
	};
	type ViewTimelineSource = 'records' | 'events';
	type TimelineEventLayout = 'summary' | 'detailed';

	let project = $state<Project | null>(null);
	let scenarioTemplates = $state<ScenarioTemplate[]>([]);
	let forms = $state<UniversalForm[]>([]);
	let formViews = $state<FormView[]>([]);
	let records = $state<FormRecord[]>([]);
	let filterPreviewRecords = $state<FormRecord[]>([]);
	let formEvents = $state<BusinessEvent[]>([]);
	let automationInvocations = $state<Invocation[]>([]);
	let printJobs = $state<FormRecord[]>([]);
	let recordAttachments = $state<FormAttachment[]>([]);
	let recordAttachmentSignedUrls = $state<Record<string, string>>({});
	let aggregates = $state<Record<string, FormAggregate>>({});
	let formPermissions = $state<FormPermissions | null>(null);
	let selectedFormId = $state('');
	let selectedViewId = $state('');
	let selectedRecord = $state<FormRecord | null>(null);
	let focusedRecordIds = $state<string[]>([]);
	let selectedRecordLinks = $state<FormRecordLink[]>([]);
	let selectedRecordChildren = $state<FormChildRecord[]>([]);
	let selectedRecordComments = $state<FormRecordComment[]>([]);
	let workspaceMembers = $state<WorkspaceMember[]>([]);
	let workspaceMembersLoading = $state(false);
	let relationTargets = $state<Record<string, FormRelationTarget[]>>({});
	let relationTargetsLoading = $state(false);
	let loading = $state(true);
	let templatesLoading = $state(false);
	let viewsLoading = $state(false);
	let recordsLoading = $state(false);
	let eventsLoading = $state(false);
	let automationInvocationsLoading = $state(false);
	let recordCommentsLoading = $state(false);
	let formSubmitting = $state(false);
	let duplicatingForm = $state(false);
	let installingTemplateKey = $state<string | null>(null);
	let viewSubmitting = $state(false);
	let deletingViewId = $state<string | null>(null);
	let exportingRecords = $state(false);
	let exportedCurrentViewJsonRecordCount = $state<number | null>(null);
	let exportedAttachmentManifestCount = $state<number | null>(null);
	let exportedAttachmentBundleCount = $state<number | null>(null);
	let exportedAttachmentPackageCount = $state<number | null>(null);
	let exportedAttachmentPackageFileCount = $state<number | null>(null);
	let permissionsLoading = $state(false);
	let permissionsSaving = $state(false);
	let importPreviewing = $state(false);
	let importingRecords = $state(false);
	let recordSubmitting = $state(false);
	let attachmentSubmittingField = $state<string | null>(null);
	let deletingAttachmentId = $state<string | null>(null);
	let linkSubmitting = $state(false);
	let recordCommentSubmitting = $state(false);
	let deletingRecordId = $state<string | null>(null);
	let updatingKanbanRecordId = $state<string | null>(null);
	let draggingKanbanRecordId = $state<string | null>(null);
	let selectedKanbanRecordIds = $state<string[]>([]);
	let bulkKanbanTarget = $state('');
	let bulkMovingKanban = $state(false);
	let updatingCalendarRecordId = $state<string | null>(null);
	let draggingCalendarRecordId = $state<string | null>(null);
	let appliedCalendarFilterKey = $state<string | null>(null);
	let copiedCalendarScheduleKey = $state<string | null>(null);
	let shiftedCalendarScheduleKey = $state<string | null>(null);
	let appliedCardGroupFilterKey = $state<string | null>(null);
	let updatingGanttRecordId = $state<string | null>(null);
	let draggingGanttRecordId = $state<string | null>(null);
	let updatingCardRecordId = $state<string | null>(null);
	let collapsedCardGroupKeys = $state<string[]>([]);
	let updatingTimelineRecordId = $state<string | null>(null);
	let copiedCardRecordId = $state<string | null>(null);
	let copiedCardGroupRecordIdsKey = $state<string | null>(null);
	let exportedCardGroupRecordsKey = $state<string | null>(null);
	let exportedCardRecordId = $state<string | null>(null);
	let copiedTimelineEventAction = $state<string | null>(null);
	let exportedTimelineEventAction = $state<string | null>(null);
	let exportedTimelineEventsCsvCount = $state<number | null>(null);
	let exportedTimelineEventsJsonCount = $state<number | null>(null);
	let copiedTimelineEventIdsCount = $state<number | null>(null);
	let copiedTimelineAggregateIdsCount = $state<number | null>(null);
	let pinnedTimelineEventIds = $state<string[]>([]);
	let copiedPivotDrilldownKey = $state<string | null>(null);
	let exportedPivotDrilldownKey = $state<string | null>(null);
	let appliedPivotDrilldownFilterKey = $state<string | null>(null);
	let copiedPivotCellKey = $state<string | null>(null);
	let copiedPivotTableMatrix = $state(false);
	let exportedPivotTableMatrix = $state(false);
	let copiedFilterExpressionRecordIds = $state<string | null>(null);
	let copiedFilterExpressionJson = $state<string | null>(null);
	let copiedFilterExpressionSummary = $state<string | null>(null);
	let copiedFilterExpressionCsvRecordIds = $state<string | null>(null);
	let copiedFilterExpressionJsonRecordIds = $state<string | null>(null);
	let exportedFilterExpressionJson = $state<string | null>(null);
	let exportedFilterExpressionSummary = $state<string | null>(null);
	let exportedFilterExpressionRecordIds = $state<string | null>(null);
	let exportedFilterExpressionJsonRecordIds = $state<string | null>(null);
	let pivotDrilldown = $state<PivotDrilldownSelection | null>(null);
	let showCreateForm = $state(false);
	let showImportRecords = $state(false);
	let editingRecordId = $state<string | null>(null);
	let recordDraft = $state<Record<string, unknown>>({});
	let recordTitle = $state('');
	let linkDraft = $state({
		target_type: 'form_record',
		target_id: '',
		relation_key: 'contains',
		relation_type: 'parent_child'
	});
	let recordCommentDraft = $state('');
	let childEditor = $state<ChildEditorState | null>(null);
	let childSubmitting = $state(false);
	let deletingChildRecordId = $state<string | null>(null);
	let formDraft = $state({
		key: '',
		name: '',
		description: '',
		title_template: '{id}',
		schemaText: JSON.stringify({ version: 'openpr.form.schema.v1', fields: [] }, null, 2)
	});
	let viewDraft = $state({
		key: '',
		name: '',
		view_type: 'grid' as FormView['view_type'],
		columns: [] as string[],
		filters: [] as ViewFilterDraft[],
		filterRootLogic: 'all' as ViewFilterLogic,
		filterGroups: [] as ViewFilterGroupDraft[],
		filterField: '',
		filterOperator: 'contains' as ViewFilterOperator,
		filterValue: '',
			sortField: '',
			sortDirection: 'asc' as ViewSortDirection,
			groupBy: '',
			pivotRowField: '',
			pivotColumnField: '',
			pivotValueField: '',
			pivotAggregate: 'sum' as PivotAggregate,
			pivotTotals: 'all' as PivotTotalsMode,
			pivotValueDisplay: 'value' as PivotValueDisplay,
			pivotSort: 'label' as PivotSortMode,
			pivotEmptyBuckets: false,
			pivotHideZeroBuckets: false,
			pivotShowCounts: false,
			pivotHeatmap: false,
			pivotRowLimit: 0 as PivotRowLimit,
			pivotColumnLimit: 0 as PivotColumnLimit,
			ganttStartField: '',
			ganttEndField: '',
			ganttGroupBy: '',
			ganttDependencyField: '',
			ganttScale: 'day' as GanttScale,
			swimlaneBy: '',
		wipLimits: {} as Record<string, string>,
		dateField: '',
		calendarRange: 'week' as ViewCalendarRange,
		calendarFocusDate: '',
		calendarRecurrence: 'none' as ViewCalendarRecurrence,
			calendarDayScope: 'all' as ViewCalendarDayScope,
			calendarLayout: 'grid' as ViewCalendarLayout,
			calendarHideEmptyDays: false,
				calendarEndField: '',
			calendarResourceBy: '',
			calendarExceptionRecordId: '',
			calendarExceptionDate: '',
			calendarExceptionDates: {} as CalendarExceptionDateMap,
				cardTitleField: '',
				cardSubtitleField: '',
				cardBadgeField: '',
				cardCoverField: '',
				cardLayout: 'standard' as CardLayoutMode,
				cardCoverAspect: 'auto' as CardCoverAspect,
				cardCoverFit: 'cover' as CardCoverFit,
					cardCoverPosition: 'top' as CardCoverPosition,
					cardFieldLimit: 6 as CardFieldLimit,
					cardHideEmptyFields: false,
					cardShowTimestamps: false,
					cardShowRecordId: false,
					cardFieldKeys: [] as string[],
			timelineSource: 'records' as ViewTimelineSource,
			timelineEventLayout: 'summary' as TimelineEventLayout,
			timelineEventType: '',
			timelineEventActor: '',
			timelineEventAggregate: '',
			timelineEventQuery: '',
			timelineEventFrom: '',
			timelineEventTo: '',
			timelineTitleField: '',
			timelineSubtitleField: '',
			visibility: 'shared' as ViewVisibility,
			isDefault: false
		});
	let fieldDrafts = $state<FieldDraft[]>([blankFieldDraft()]);
	let activeMode = $state<FormWorkflowMode>('data');
	type DetailLayoutMode = 'header' | 'highlights' | 'composition' | 'sections' | 'widgets';
	const detailLayoutModes: Array<{ key: DetailLayoutMode; labelKey: string }> = [
		{ key: 'header', labelKey: 'forms.detailHeader' },
		{ key: 'highlights', labelKey: 'forms.detailHighlights' },
		{ key: 'composition', labelKey: 'forms.detailPageComposition' },
		{ key: 'sections', labelKey: 'forms.detailLayoutSections' },
		{ key: 'widgets', labelKey: 'forms.detailPageWidgets' }
	];
	let detailLayoutMode = $state<DetailLayoutMode>('header');
	let viewToolsExpanded = $state(false);
	let recordEditorOpen = $state(false);
	let recordPage = $state(1);
	const recordPageSize = 20;
	let designerFieldDrafts = $state<FieldDraft[]>([]);
	let designerFieldUsage = $state<FormFieldUsage[]>([]);
	let designerFieldDependencies = $state<FormFieldDependency[]>([]);
	let designerUsageLoading = $state(false);
	let selectedDesignerFieldIndex = $state(0);
	let designerSaving = $state(false);
	let detailPageCompositionDraft = $state<DetailPageCompositionDraft>({
		widgetRegion: 'sidebar',
		fieldColumns: 2
	});
	let detailPageCompositionSaving = $state(false);
	let detailPageCompositionSyncedFingerprint = $state('');
	let detailPageWidgetSaving = $state(false);
	let detailLayoutSectionDrafts = $state<DetailLayoutSectionDraft[]>([]);
	let detailLayoutSectionSaving = $state(false);
	let detailLayoutSectionSyncedFingerprint = $state('');
	let detailHeaderDraft = $state<DetailHeaderDraft>({ subtitleField: '', badgeField: '' });
	let detailHeaderSaving = $state(false);
	let detailHeaderSyncedFingerprint = $state('');
	let detailHighlightDraft = $state<DetailHighlightDraft>({ fields: [] });
	let detailHighlightSaving = $state(false);
	let detailHighlightSyncedFingerprint = $state('');
	let designerSyncedFormId = $state('');
	let designerConfirmOpen = $state(false);
	let pendingDesignerSchema = $state<Record<string, unknown> | null>(null);
	let pendingDesignerChanges = $state<DesignerSchemaChange[]>([]);
	let showAdvancedSchema = $state(false);
	let importText = $state('');
	let importRows = $state<FormRecordImportRow[]>([]);
	let importPreview = $state<FormRecordImportPreview | null>(null);
	let exportedImportTemplate = $state(false);
	let importedImportFileName = $state('');
	let importedImportFileLoaded = $state(false);
	let importMappingHeaders = $state<string[]>([]);
	let importMappingSelections = $state<string[]>([]);
	let importMappingTransforms = $state<ImportMappingTransform[]>([]);
	let importMappingApplied = $state(false);
	let importMappingTemplateAvailable = $state(false);
	let importMappingTemplateSaved = $state(false);
	let importMappingTemplateRestored = $state(false);
	let importWizardStep = $state<ImportWizardStep>('source');
	let memberPermissionDraft = $state<Record<FormPermissionAction, boolean>>(
		defaultPermissionActions()
	);
	let memberRecordScopeDraft = $state<FormRecordScope>('all');
	let memberFieldReadDraft = $state<Record<string, boolean>>({});
	let memberFieldWriteDraft = $state<Record<string, boolean>>({});

	const selectedForm = $derived.by(() => forms.find((form) => form.id === selectedFormId) ?? null);
	const printJobForm = $derived.by(() => forms.find((form) => form.key === 'print_job') ?? null);
	const fields = $derived(selectedForm ? parseFields(selectedForm.schema) : []);
	const detailVisibleFields = $derived(fields.filter((field) => field.visibility?.detail !== false));
	const selectedView = $derived.by(
		() => formViews.find((view) => view.id === selectedViewId) ?? formViews[0] ?? null
	);
	const selectedViewColumns = $derived.by(() => viewColumns(selectedView));
	const selectedViewType = $derived((selectedView?.view_type ?? 'grid') as FormView['view_type']);
	const detailUrlRecordId = $derived(
		$page.url.searchParams.get('record') ?? $page.url.searchParams.get('record_id') ?? ''
	);
	const focusedRecords = $derived.by(() => {
		if (focusedRecordIds.length === 0) return [];
		const focusedIds = new Set(focusedRecordIds);
		return records.filter((record) => focusedIds.has(record.id));
	});
		const memberHiddenFieldLabels = $derived(
			fields
				.filter((field) => memberFieldReadDraft[field.key] === false)
				.map((field) => fieldLabel(field))
		);
		const memberHiddenFieldKeys = $derived(
			fields.filter((field) => memberFieldReadDraft[field.key] === false).map((field) => field.key)
		);
		const memberLockedFieldLabels = $derived(
			fields
				.filter((field) => memberFieldWriteDraft[field.key] === false)
				.map((field) => fieldLabel(field))
		);
		const memberLockedFieldKeys = $derived(
			fields.filter((field) => memberFieldWriteDraft[field.key] === false).map((field) => field.key)
		);
	const listableFields = $derived(fields.filter((field) => field.visibility?.list !== false));
	const visibleFields = $derived.by(() => {
		if (selectedViewColumns.length === 0) return listableFields;
		const byKey = new Map(listableFields.map((field) => [field.key, field]));
		const ordered = selectedViewColumns
			.map((key) => byKey.get(key))
			.filter((field): field is FormField => Boolean(field));
		return ordered.length > 0 ? ordered : listableFields;
	});
		const cardFields = $derived.by(() =>
			orderedCardFieldsForView(selectedView).slice(
				0,
				viewCardFieldLimit(selectedView?.config?.card_field_limit)
			)
		);
		const selectedCardLayout = $derived(viewCardLayout(selectedView?.config?.card_layout));
		const selectedCardCoverAspect = $derived(
			viewCardCoverAspect(selectedView?.config?.card_cover_aspect)
		);
		const selectedCardCoverFit = $derived(viewCardCoverFit(selectedView?.config?.card_cover_fit));
		const selectedCardCoverPosition = $derived(
			viewCardCoverPosition(selectedView?.config?.card_cover_position)
		);
		const selectedCardHideEmptyFields = $derived(
			viewConfigBoolean(selectedView, 'card_hide_empty_fields')
		);
		const selectedCardShowTimestamps = $derived(
			viewConfigBoolean(selectedView, 'card_show_timestamps')
		);
		const selectedCardShowRecordId = $derived(
			viewConfigBoolean(selectedView, 'card_show_record_id')
		);
		const selectedCalendarLayout = $derived(
			viewCalendarLayout(selectedView?.config?.calendar_layout)
		);
		const selectedCalendarHideEmptyDays = $derived(
			viewConfigBoolean(selectedView, 'calendar_hide_empty_days')
		);
		const cardGroupField = $derived.by(() =>
			selectedViewType === 'card' ? fieldByKey(viewConfigString(selectedView, 'group_by')) : null
		);
		const cardGroups = $derived.by(() =>
			cardGroupField ? groupRecordsByField(viewRecords, cardGroupField) : []
		);
		const cardEditableSelectFields = $derived(
			cardFields.filter((field) => field.type === 'single_select' && kanbanMoveOptions(field).length > 0)
		);
	const timelineEditableSelectFields = $derived(cardEditableSelectFields);
	const viewRecords = $derived.by(() => applyViewRecordConfig(records));
	const recordPageCount = $derived(Math.max(1, Math.ceil(viewRecords.length / recordPageSize)));
	const normalizedRecordPage = $derived(Math.min(Math.max(recordPage, 1), recordPageCount));
	const paginatedViewRecords = $derived.by(() => {
		const start = (normalizedRecordPage - 1) * recordPageSize;
		return viewRecords.slice(start, start + recordPageSize);
	});
	const kanbanGroupField = $derived.by(() => viewGroupField());
	const kanbanGroups = $derived.by(() => groupRecordsByField(viewRecords, kanbanGroupField));
	const kanbanSwimlaneField = $derived.by(() => viewSwimlaneField());
	const kanbanSwimlanes = $derived.by(() => groupKanbanSwimlanes(viewRecords));
	const pivotTable = $derived.by(() => buildPivotTable(viewRecords));
	const pivotDrilldownRecords = $derived.by(() => buildPivotDrilldownRecords(viewRecords));
	const ganttTimeline = $derived.by(() => buildGanttTimeline(viewRecords));
	const calendarDateField = $derived.by(() => viewDateField());
	const calendarEndField = $derived.by(() => viewCalendarEndField());
	const calendarGroups = $derived.by(() => groupRecordsByDate(viewRecords, calendarDateField));
	const calendarGridGroups = $derived.by(() => groupRecordsByCalendarGrid(viewRecords, calendarDateField));
	const calendarResourceField = $derived.by(() => viewCalendarResourceField());
	const calendarResourceLanes = $derived.by(() => groupCalendarResourceLanes(viewRecords));
	const timelineRecords = $derived.by(() => recordsByUpdatedAt(viewRecords));
	const timelineEventTypes = $derived.by(() => eventTypes(formEvents));
	const timelineEventActors = $derived.by(() => eventActors(formEvents));
	const timelineEventAggregates = $derived.by(() => eventAggregates(formEvents));
	const timelineEvents = $derived.by(() => eventsByCreatedAt(filteredTimelineEvents(formEvents)));
	const timelineEventRows = $derived.by(() => orderedTimelineEvents(timelineEvents));
	const selectedRecordEvents = $derived.by(() =>
		selectedRecord
			? eventsByCreatedAt(
					formEvents.filter(
						(event) =>
							event.aggregate_type === 'form_record' && event.aggregate_id === selectedRecord?.id
					)
				)
			: []
	);
	const selectedRecordAutomationInvocations = $derived.by(() =>
		selectedRecord
			? automationInvocations.filter((invocation) => invocationBelongsToSelectedRecord(invocation))
			: []
	);
	const timelineTitlePermissionField = $derived.by(() => viewTimelineTitleField());
	const timelineSubtitlePermissionField = $derived.by(() => viewTimelineSubtitleField());
	const aggregateFields = $derived(
		fields.filter((field) => ['amount', 'number', 'integer'].includes(field.type ?? 'text'))
	);
	const selectedDesignerField = $derived(designerFieldDrafts[selectedDesignerFieldIndex] ?? null);
	const selectedDesignerFieldUsage = $derived(
		selectedDesignerField
			? (designerFieldUsage.find(
					(usage) =>
						(selectedDesignerField.field_id && usage.field_id === selectedDesignerField.field_id) ||
						usage.field_key === selectedDesignerField.key
				) ?? null)
			: null
	);
	const selectedDesignerFieldDependencies = $derived(
		selectedDesignerField
			? designerFieldDependencies.filter(
					(dependency) =>
						(selectedDesignerField.field_id &&
							dependency.field_id === selectedDesignerField.field_id) ||
						dependency.field_key === selectedDesignerField.key
				)
			: []
	);
	const selectedDetailPageWidgetEntries = $derived.by(() =>
		detailPageWidgetEntriesFromLayout(selectedForm?.detail_layout)
	);

	onMount(async () => {
		await Promise.all([loadProject(), loadForms(), loadScenarioTemplates(), loadWorkspaceMembers()]);
		await restoreRecordDetailFromUrl();
		loading = false;
	});

	async function loadProject() {
		const response = await projectsApi.get(projectId);
		if (response.code === 0 && response.data) {
			project = response.data;
		}
	}

	async function loadScenarioTemplates() {
		templatesLoading = true;
		const response = await projectsApi.listScenarioTemplates({ project_type_key: 'custom_form' });
		templatesLoading = false;
		if (response.code !== 0) {
			toast.error(response.message);
			scenarioTemplates = [];
			return;
		}
		scenarioTemplates = response.data?.items ?? [];
	}

	async function loadWorkspaceMembers() {
		workspaceMembersLoading = true;
		const response = await membersApi.list(workspaceId);
		workspaceMembersLoading = false;
		if (response.code !== 0) {
			workspaceMembers = [];
			toast.error(response.message);
			return;
		}
		workspaceMembers = response.data?.items ?? [];
	}

	async function loadForms() {
		const response = await formsApi.list(projectId, { page: 1, per_page: 100 });
		if (response.code !== 0) {
			toast.error(response.message);
			forms = [];
			return;
		}
		forms = response.data?.items ?? [];
		const urlFormId = $page.url.searchParams.get('form') ?? $page.url.searchParams.get('form_id');
		if (urlFormId && forms.some((form) => form.id === urlFormId)) {
			await openForm(urlFormId, { syncUrl: false });
		} else {
			clearSelectedFormState();
		}
		await loadPrintJobs();
	}

	function clearSelectedFormState() {
		selectedFormId = '';
		records = [];
		filterPreviewRecords = [];
		formViews = [];
		formEvents = [];
		automationInvocations = [];
		formPermissions = null;
		memberPermissionDraft = defaultPermissionActions();
		memberFieldReadDraft = {};
		memberFieldWriteDraft = {};
		selectedViewId = '';
		selectedRecord = null;
		selectedRecordLinks = [];
		selectedRecordChildren = [];
		selectedRecordComments = [];
		recordCommentDraft = '';
		recordAttachments = [];
		relationTargets = {};
		designerFieldDrafts = [];
		designerSyncedFormId = '';
		activeMode = 'data';
		editingRecordId = null;
		recordEditorOpen = false;
		recordPage = 1;
	}

	async function syncFormUrl(formId: string) {
		const url = new URL($page.url);
		url.searchParams.set('form', formId);
		url.searchParams.delete('form_id');
		url.searchParams.delete('record');
		url.searchParams.delete('record_id');
		const next = `${url.pathname}${url.search}${url.hash}`;
		const current = `${$page.url.pathname}${$page.url.search}${$page.url.hash}`;
		if (next !== current) {
			await goto(next, { replaceState: true, noScroll: true, keepFocus: true });
		}
	}

	async function openForm(formId: string, options: { syncUrl?: boolean } = {}) {
		const form = forms.find((item) => item.id === formId);
		if (!form) return;
		selectedFormId = form.id;
		selectedViewId = '';
		editingRecordId = null;
		recordEditorOpen = false;
		recordPage = 1;
		selectedRecord = null;
		selectedRecordLinks = [];
		selectedRecordChildren = [];
		selectedRecordComments = [];
		recordCommentDraft = '';
		recordAttachments = [];
		relationTargets = {};
		formViews = [];
		formPermissions = null;
		formEvents = [];
		automationInvocations = [];
		activeMode = 'data';
		syncDesignerDrafts(form);
		syncDetailHeaderDraft(form);
		syncDetailHighlightDraft(form);
		syncDetailPageCompositionDraft(form);
		syncDetailLayoutSectionDrafts(form);
		if (options.syncUrl !== false) await syncFormUrl(form.id);
		await loadFormPermissions();
		await loadFormViews();
		await loadRecords();
	}

	async function loadFormViews() {
		if (!selectedFormId) {
			formViews = [];
			selectedViewId = '';
			return;
		}
		viewsLoading = true;
		const response = await formsApi.listViews(selectedFormId);
		viewsLoading = false;
		if (response.code !== 0) {
			toast.error(response.message);
			formViews = [];
			selectedViewId = '';
			return;
		}
		formViews = sortViewsByDefault(response.data ?? []);
		if (!selectedViewId || !formViews.some((view) => view.id === selectedViewId)) {
			selectedViewId = formViews[0]?.id ?? '';
		}
		resetViewDraft();
	}

	async function loadFormPermissions() {
		if (!selectedFormId) {
			formPermissions = null;
			memberPermissionDraft = defaultPermissionActions();
			memberRecordScopeDraft = 'all';
			memberFieldReadDraft = {};
			memberFieldWriteDraft = {};
			return;
		}
		permissionsLoading = true;
		const response = await formsApi.getPermissions(selectedFormId);
		permissionsLoading = false;
		if (response.code !== 0 || !response.data) {
			formPermissions = null;
			memberPermissionDraft = defaultPermissionActions();
			memberRecordScopeDraft = 'all';
			memberFieldReadDraft = defaultFieldReadDraft();
			memberFieldWriteDraft = defaultFieldWriteDraft();
			toast.error(response.message);
			return;
		}
		formPermissions = response.data;
		memberPermissionDraft = rolePermissionActions(response.data, 'member');
		memberRecordScopeDraft = roleRecordScope(response.data, 'member');
		memberFieldReadDraft = roleFieldReadPermissions(response.data, 'member');
		memberFieldWriteDraft = roleFieldWritePermissions(response.data, 'member');
	}

	async function saveMemberPermissions() {
		if (!selectedForm) return;
		permissionsSaving = true;
		const response = await formsApi.updatePermissions(selectedForm.id, [
			{
				subject_type: 'role',
				subject_id: 'member',
				policy: {
					actions: memberPermissionDraft,
					record_scope: memberRecordScopeDraft,
					fields: memberFieldPolicy()
				}
			}
		]);
		permissionsSaving = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		formPermissions = response.data;
		memberPermissionDraft = rolePermissionActions(response.data, 'member');
		memberRecordScopeDraft = roleRecordScope(response.data, 'member');
		memberFieldReadDraft = roleFieldReadPermissions(response.data, 'member');
		memberFieldWriteDraft = roleFieldWritePermissions(response.data, 'member');
		toast.success($t('forms.permissionsSaved'));
	}

	async function loadRecords() {
		if (!selectedFormId) return;
		recordsLoading = true;
		const [response, previewResponse] = await Promise.all([
			formsApi.listRecords(selectedFormId, {
				page: 1,
				per_page: 100,
				view_id: selectedViewId || undefined
			}),
			formsApi.listRecords(selectedFormId, {
				page: 1,
				per_page: 100
			})
		]);
		if (response.code !== 0) {
			toast.error(response.message);
			records = [];
		} else {
			records = response.data?.items ?? [];
			if (selectedRecord && !records.some((record) => record.id === selectedRecord?.id)) {
				selectedRecord = null;
				selectedRecordLinks = [];
				selectedRecordChildren = [];
				selectedRecordComments = [];
				void syncRecordDetailUrl(null);
			}
		}
		if (previewResponse.code === 0) {
			filterPreviewRecords = previewResponse.data?.items ?? [];
		} else {
			filterPreviewRecords = records;
		}
		await loadAggregates();
		await loadRelationTargets();
		await loadFormEvents();
		await loadAutomationInvocations();
		resetRecordDraft();
		recordsLoading = false;
	}

	async function loadFormEvents() {
		if (!selectedFormId) {
			formEvents = [];
			return;
		}
		eventsLoading = true;
		const response = await formsApi.listEvents(selectedFormId);
		eventsLoading = false;
		if (response.code !== 0) {
			formEvents = [];
			toast.error(response.message);
			return;
		}
		formEvents = response.data?.items ?? [];
	}

	async function loadAutomationInvocations() {
		if (!selectedForm) {
			automationInvocations = [];
			return;
		}
		automationInvocationsLoading = true;
		const response = await connectorsApi.listInvocations(selectedForm.project_id, { page: 1, per_page: 50 });
		automationInvocationsLoading = false;
		if (response.code !== 0) {
			automationInvocations = [];
			toast.error(response.message);
			return;
		}
		automationInvocations = (response.data?.items ?? []).filter((invocation) =>
			invocationBelongsToSelectedForm(invocation)
		);
	}

	async function loadPrintJobs() {
		if (!printJobForm) {
			printJobs = [];
			return;
		}
		const response = await formsApi.listRecords(printJobForm.id, { page: 1, per_page: 8 });
		if (response.code === 0 && response.data) {
			printJobs = response.data.items ?? [];
		}
	}

	async function loadRelationTargets() {
		if (!selectedForm) {
			relationTargets = {};
			return;
		}
		const relationFields = fields.filter(
			(field) => field.type === 'relation' && field.relation?.target_type !== 'external'
		);
		if (relationFields.length === 0) {
			relationTargets = {};
			return;
		}
		relationTargetsLoading = true;
		const entries = await Promise.all(
			relationFields.map(async (field) => {
				const response = await formsApi.listRelationTargets(selectedForm.id, {
					field_key: field.key,
					per_page: 100
				});
				return [field.key, response.code === 0 && response.data ? response.data.items : []] as const;
			})
		);
		relationTargets = Object.fromEntries(entries);
		relationTargetsLoading = false;
	}

	async function loadAggregates() {
		if (!selectedFormId || !selectedForm) {
			aggregates = {};
			return;
		}
		const fieldsToAggregate = parseFields(selectedForm.schema).filter((field) =>
			['amount', 'number', 'integer'].includes(field.type ?? 'text')
		);
		const results = await Promise.all(
			fieldsToAggregate.map(async (field) => {
				const response = await formsApi.aggregate(selectedFormId, field.key, 'sum');
				return [field.key, response.code === 0 && response.data ? response.data : null] as const;
			})
		);
		aggregates = Object.fromEntries(
			results.filter((entry): entry is readonly [string, FormAggregate] => Boolean(entry[1]))
		);
	}

	async function handleCreateForm() {
		let schema: Record<string, unknown>;
		try {
			const parsed = showAdvancedSchema
				? (JSON.parse(formDraft.schemaText) as unknown)
				: buildSchemaFromDrafts();
			if (!isRecord(parsed)) throw new Error($t('forms.schemaMustBeObject'));
			schema = parsed;
		} catch (error) {
			toast.error(error instanceof Error ? error.message : $t('forms.invalidSchema'));
			return;
		}

		formSubmitting = true;
		const response = await formsApi.create(projectId, {
			key: formDraft.key.trim(),
			name: formDraft.name.trim(),
			description: formDraft.description.trim(),
			title_template: formDraft.title_template.trim() || '{id}',
			schema
		});
		formSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = [response.data, ...forms];
		showCreateForm = false;
		resetFormDraft();
		toast.success($t('forms.formCreated'));
		await openForm(response.data.id);
		activeMode = 'design';
		void loadDesignerSafety(response.data);
	}

	async function duplicateSelectedForm() {
		if (!selectedForm) return;
		duplicatingForm = true;
		const response = await formsApi.duplicate(selectedForm.id);
		duplicatingForm = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = [response.data, ...forms];
		toast.success($t('forms.formDuplicated'));
		await openForm(response.data.id);
		activeMode = 'design';
		void loadDesignerSafety(response.data);
	}

	async function handleSaveRecord() {
		if (!selectedForm) return;
		let values: Record<string, unknown>;
		try {
			values = buildRecordPayload(fields);
		} catch (error) {
			toast.error(error instanceof Error ? error.message : $t('forms.invalidRecordValues'));
			return;
		}
		recordSubmitting = true;
		const wasEditing = Boolean(editingRecordId);
		const response = editingRecordId
			? await formsApi.updateRecord(editingRecordId, {
					title: recordTitle.trim() || undefined,
					values,
					source: { type: 'web' }
				})
			: await formsApi.createRecord(selectedForm.id, {
					title: recordTitle.trim() || undefined,
					values,
					source: { type: 'web' }
				});
		recordSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		if (!wasEditing) {
			await createParentChildLinksFromRelationFields(response.data, values);
		}
		records = editingRecordId
			? records.map((record) => (record.id === response.data?.id ? response.data : record))
			: [response.data, ...records];
		filterPreviewRecords = editingRecordId
			? filterPreviewRecords.map((record) => (record.id === response.data?.id ? response.data : record))
			: [response.data, ...filterPreviewRecords];
		selectedRecord = response.data;
		editingRecordId = null;
		recordEditorOpen = false;
		recordAttachments = [];
		resetRecordDraft();
		toast.success(wasEditing ? $t('forms.recordUpdated') : $t('forms.recordSaved'));
		await Promise.all([
			reloadSelectedRecord(),
			loadAggregates(),
			loadSelectedRecordLinks(),
			loadSelectedRecordChildren(),
			loadSelectedRecordComments(),
			loadPrintJobs()
		]);
	}

	async function createParentChildLinksFromRelationFields(
		record: FormRecord,
		values: Record<string, unknown>
	) {
		const relationFields = fields.filter(
			(field) =>
				(field.type === 'relation' || field.type === 'child_table') &&
				field.relation?.relation_type === 'parent_child'
		);
		for (const field of relationFields) {
			const value = values[field.key];
			if (!isRecord(value) || typeof value.record_id !== 'string' || !value.record_id) continue;
			const response = await formsApi.createRecordLink(record.id, {
				target_type: 'form_record',
				target_id: value.record_id,
				relation_key: field.relation?.relation_key || field.key,
				relation_type: 'parent_child',
				metadata: { source: 'web', relation_field: field.key }
			});
			if (response.code !== 0) {
				toast.error(response.message);
			}
		}
	}

	async function handleDeleteRecord(record: FormRecord) {
		deletingRecordId = record.id;
		const response = await formsApi.deleteRecord(record.id);
		deletingRecordId = null;
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		records = records.filter((item) => item.id !== record.id);
		filterPreviewRecords = filterPreviewRecords.filter((item) => item.id !== record.id);
		printJobs = printJobs.filter((item) => item.id !== record.id);
		if (selectedRecord?.id === record.id) {
			selectedRecord = null;
			selectedRecordLinks = [];
			selectedRecordChildren = [];
			selectedRecordComments = [];
			recordCommentDraft = '';
		}
		if (editingRecordId === record.id) {
			cancelEditRecord();
		}
		toast.success($t('forms.recordDeleted'));
		await loadAggregates();
	}

	async function copyRecordId(record: FormRecord) {
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(record.id);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep the record action non-blocking.
		}
		copiedCardRecordId = record.id;
		toast.success($t('forms.recordIdCopied'));
	}

	function calendarScheduleKey(occurrence: CalendarRecordOccurrence): string {
		return `${occurrence.record.id}:${occurrence.startDate}`;
	}

	async function copyCalendarSchedule(occurrence: CalendarRecordOccurrence) {
		const value = JSON.stringify(
			{
				record_id: occurrence.record.id,
				title: occurrence.record.title,
				start: occurrence.startDate,
				end: occurrence.endDate,
				span_days: occurrence.spanDays,
				recurring: occurrence.recurring
			},
			null,
			2
		);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(value);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep calendar operations non-blocking.
		}
		copiedCalendarScheduleKey = calendarScheduleKey(occurrence);
		toast.success($t('forms.calendarScheduleCopied'));
	}

	function canShiftCalendarOccurrence(occurrence: CalendarRecordOccurrence): boolean {
		if (!calendarDateField || !['date', 'datetime'].includes(calendarDateField.type ?? 'text')) return false;
		return (
			occurrence.spanStart &&
			!occurrence.recurring &&
			occurrence.startDate === calendarRecordIsoDate(occurrence.record, calendarDateField)
		);
	}

	async function shiftCalendarOccurrence(occurrence: CalendarRecordOccurrence, days: number) {
		if (!canShiftCalendarOccurrence(occurrence) || !calendarDateField) return;
		const currentStart = calendarRecordIsoDate(occurrence.record, calendarDateField);
		if (!currentStart) return;
		const nextStart = addIsoDays(currentStart, days);
		const endField = viewCalendarEndField();
		const currentEnd = endField ? calendarRecordIsoDate(occurrence.record, endField) : null;
		const duration =
			currentEnd && currentEnd >= currentStart ? isoDayDiff(currentEnd, currentStart) : 0;
		const nextValues = {
			...occurrence.record.values,
			[calendarDateField.key]: calendarDatePayloadValue(calendarDateField, nextStart)
		};
		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(endField, addIsoDays(nextStart, duration));
		}
		updatingCalendarRecordId = occurrence.record.id;
		const response = await formsApi.updateRecord(occurrence.record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'calendar_shift' }
		});
		updatingCalendarRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		shiftedCalendarScheduleKey = `${occurrence.record.id}:${nextStart}`;
		toast.success($t('forms.calendarShifted'));
		await loadAggregates();
	}

	function cardRecordDetailsPayload(record: FormRecord) {
		return {
			id: record.id,
			form_id: record.form_id,
			form_key: selectedForm?.key ?? null,
			title: record.title,
			values: record.values,
			source: record.source,
			schema_version: record.schema_version ?? null,
			created_by: record.created_by ?? null,
			updated_by: record.updated_by ?? null,
			archived_at: record.archived_at ?? null,
			created_at: record.created_at,
			updated_at: record.updated_at
		};
	}

	function cardRecordDetailsJson(record: FormRecord): string {
		return JSON.stringify(cardRecordDetailsPayload(record), null, 2);
	}

	function exportCardRecordDetails(record: FormRecord) {
		if (!selectedForm) return;
		const blob = new Blob([cardRecordDetailsJson(record)], { type: 'application/json;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-${record.id}-record.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedCardRecordId = record.id;
		toast.success($t('forms.recordExported'));
	}

	function cardGroupRecordsDetailsJson(group: { key: string; label: string; records: FormRecord[] }): string {
		return JSON.stringify(
			{
				form_id: selectedForm?.id ?? null,
				form_key: selectedForm?.key ?? null,
				group_by: cardGroupField?.key ?? null,
				group_key: group.key,
				group_label: group.label,
				record_count: group.records.length,
				records: group.records.map((record) => cardRecordDetailsPayload(record))
			},
			null,
			2
		);
	}

	function safeExportSlug(value: string): string {
		return value.trim().replace(/[^a-zA-Z0-9._-]+/g, '-').replace(/^-+|-+$/g, '') || 'group';
	}

	function exportCardGroupRecords(group: { key: string; label: string; records: FormRecord[] }) {
		if (!selectedForm || group.records.length === 0) return;
		const blob = new Blob([cardGroupRecordsDetailsJson(group)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-${cardGroupField?.key ?? 'group'}-${safeExportSlug(group.key)}-records.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedCardGroupRecordsKey = cardGroupFilterKey(group);
		toast.success($t('forms.cardGroupRecordsExported', { values: { count: group.records.length } }));
	}

	async function copyTimelineEventValue(
		event: BusinessEvent,
		value: string,
		action: 'event-id' | 'aggregate' | 'details'
	) {
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(value);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep timeline operations non-blocking.
		}
		copiedTimelineEventAction = `${event.id}:${action}`;
		toast.success($t('forms.timelineEventCopied'));
	}

	function exportTimelineEventDetails(event: BusinessEvent) {
		if (!selectedForm) return;
		const blob = new Blob([timelineEventDetailsJson(event)], { type: 'application/json;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-${event.id}-details.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedTimelineEventAction = `${event.id}:details`;
		toast.success($t('forms.timelineEventExported'));
	}

	function exportTimelineEventsCsv(events: BusinessEvent[]) {
		if (!selectedForm || events.length === 0) return;
		const blob = new Blob([timelineEventsCsv(events)], { type: 'text/csv;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-timeline-events.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedTimelineEventsCsvCount = events.length;
		toast.success($t('forms.timelineEventsExported', { values: { count: events.length } }));
	}

	function exportTimelineEventsJson(events: BusinessEvent[]) {
		if (!selectedForm || events.length === 0) return;
		const blob = new Blob([timelineEventsDetailsJson(events)], { type: 'application/json;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-timeline-events.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedTimelineEventsJsonCount = events.length;
		toast.success($t('forms.timelineEventsJsonExported', { values: { count: events.length } }));
	}

	async function copyTimelineEventIds(events: BusinessEvent[]) {
		if (events.length === 0) return;
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(events.map((event) => event.id).join('\n'));
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep event-set operations non-blocking.
		}
		copiedTimelineEventIdsCount = events.length;
		toast.success($t('forms.timelineEventsCopied', { values: { count: events.length } }));
	}

	function timelineEventAggregateKeys(events: BusinessEvent[]): string[] {
		return Array.from(new Set(events.map((event) => timelineEventAggregateKey(event)).filter(Boolean)));
	}

	async function copyTimelineAggregateIds(events: BusinessEvent[]) {
		const aggregateKeys = timelineEventAggregateKeys(events);
		if (aggregateKeys.length === 0) return;
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(aggregateKeys.join('\n'));
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep event-set operations non-blocking.
		}
		copiedTimelineAggregateIdsCount = aggregateKeys.length;
		toast.success($t('forms.timelineAggregatesCopied', { values: { count: aggregateKeys.length } }));
	}

	function timelineEventPinned(event: BusinessEvent): boolean {
		return pinnedTimelineEventIds.includes(event.id);
	}

	function orderedTimelineEvents(events: BusinessEvent[]): BusinessEvent[] {
		if (pinnedTimelineEventIds.length === 0) return events;
		const pinnedOrder = new Map(pinnedTimelineEventIds.map((id, index) => [id, index]));
		return [
			...events
				.filter((event) => pinnedOrder.has(event.id))
				.sort((a, b) => (pinnedOrder.get(a.id) ?? 0) - (pinnedOrder.get(b.id) ?? 0)),
			...events.filter((event) => !pinnedOrder.has(event.id))
		];
	}

	function toggleTimelineEventPinned(event: BusinessEvent) {
		if (timelineEventPinned(event)) {
			pinnedTimelineEventIds = pinnedTimelineEventIds.filter((id) => id !== event.id);
			toast.success($t('forms.timelineEventUnpinned'));
			return;
		}
		pinnedTimelineEventIds = [event.id, ...pinnedTimelineEventIds.filter((id) => id !== event.id)];
		toast.success($t('forms.timelineEventPinned'));
	}

	function focusTimelineEventRecord(event: BusinessEvent) {
		const record = timelineEventRecord(event);
		if (!record) return;
		focusedRecordIds = [record.id];
		const gridView = formViews.find((view) => view.view_type === 'grid');
		if (gridView) {
			selectedViewId = gridView.id;
			resetViewDraft(gridView);
		}
		toast.success($t('forms.timelineEventRecordFocused'));
	}

	function timelineEventLinkedRecordIds(events: BusinessEvent[]): string[] {
		return Array.from(
			new Set(
				events
					.map((event) => timelineEventRecord(event)?.id)
					.filter((id): id is string => Boolean(id))
			)
		);
	}

	function focusTimelineEventRecords(events: BusinessEvent[]) {
		const recordIds = timelineEventLinkedRecordIds(events);
		if (recordIds.length === 0) return;
		focusedRecordIds = recordIds;
		const gridView = formViews.find((view) => view.view_type === 'grid');
		if (gridView) {
			selectedViewId = gridView.id;
			resetViewDraft(gridView);
		}
		toast.success($t('forms.focusedRecords', { values: { count: recordIds.length } }));
	}

	function applyTimelineEventQuickFilter(event: BusinessEvent, target: 'type' | 'actor' | 'aggregate') {
		if (target === 'actor' && !event.actor_id) return;
		viewDraft = {
			...viewDraft,
			view_type: 'timeline',
			timelineSource: 'events',
			timelineEventLayout: selectedTimelineEventLayout(),
			timelineEventType: target === 'type' ? event.event_type : viewDraft.timelineEventType,
			timelineEventActor: target === 'actor' ? event.actor_id ?? '' : viewDraft.timelineEventActor,
			timelineEventAggregate:
				target === 'aggregate' ? timelineEventAggregateKey(event) : viewDraft.timelineEventAggregate,
			timelineTitleField: '',
			timelineSubtitleField: ''
		};
		toast.success($t('forms.timelineEventFilterApplied'));
	}

	function timelineEventDateTimeLocal(date: Date): string {
		const localDate = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
		return localDate.toISOString().slice(0, 16);
	}

	function applyTimelineEventDayWindow(event: BusinessEvent) {
		const eventDate = new Date(event.created_at);
		if (!Number.isFinite(eventDate.getTime())) return;
		const dayStart = new Date(eventDate);
		dayStart.setHours(0, 0, 0, 0);
		const dayEnd = new Date(eventDate);
		dayEnd.setHours(23, 59, 0, 0);
		viewDraft = {
			...viewDraft,
			view_type: 'timeline',
			timelineSource: 'events',
			timelineEventLayout: selectedTimelineEventLayout(),
			timelineEventFrom: timelineEventDateTimeLocal(dayStart),
			timelineEventTo: timelineEventDateTimeLocal(dayEnd),
			timelineTitleField: '',
			timelineSubtitleField: ''
		};
		toast.success($t('forms.timelineEventDateWindowApplied'));
	}

	function clearFocusedRecord() {
		focusedRecordIds = [];
	}

	function replaceLocalRecord(record: FormRecord) {
		records = records.map((item) => (item.id === record.id ? record : item));
		filterPreviewRecords = filterPreviewRecords.map((item) => (item.id === record.id ? record : item));
		if (selectedRecord?.id === record.id) {
			selectedRecord = record;
		}
	}

	async function updateKanbanRecordField(record: FormRecord, field: FormField, value: string) {
		if (!value || record.values[field.key] === value) return;
		if (!kanbanMoveAllowed(record, value)) {
			toast.error($t('forms.kanbanWipLimitReached'));
			return;
		}
		updatingKanbanRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: { ...record.values, [field.key]: value },
			source: { type: 'web', origin: 'kanban' }
		});
		updatingKanbanRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.kanbanStatusUpdated'));
		await loadAggregates();
	}

	async function updateCardRecordSelectField(record: FormRecord, field: FormField, value: string) {
		if (!value || record.values[field.key] === value) return;
		updatingCardRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: { ...record.values, [field.key]: value },
			source: { type: 'web', origin: 'card' }
		});
		updatingCardRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.cardFieldUpdated'));
		await loadAggregates();
	}

	function cardGroupCollapseKey(group: { key: string }): string {
		return `${selectedViewId || selectedView?.id || 'card'}:${group.key}`;
	}

	function cardGroupCollapsed(group: { key: string }): boolean {
		return collapsedCardGroupKeys.includes(cardGroupCollapseKey(group));
	}

	function toggleCardGroup(group: { key: string }) {
		const key = cardGroupCollapseKey(group);
		collapsedCardGroupKeys = collapsedCardGroupKeys.includes(key)
			? collapsedCardGroupKeys.filter((item) => item !== key)
			: [...collapsedCardGroupKeys, key];
	}

	function cardGroupFilterKey(group: { key: string; label: string }): string {
		return `${cardGroupField?.key ?? '__none__'}:${group.key}:${group.label}`;
	}

	async function copyCardGroupRecordIds(group: { key: string; label: string; records: FormRecord[] }) {
		if (group.records.length === 0) return;
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(group.records.map((record) => record.id).join('\n'));
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep group operations non-blocking.
		}
		copiedCardGroupRecordIdsKey = cardGroupFilterKey(group);
		toast.success($t('forms.cardGroupRecordIdsCopied', { values: { count: group.records.length } }));
	}

	function focusCardGroupRecords(group: { records: FormRecord[] }) {
		if (group.records.length === 0) return;
		focusedRecordIds = group.records.map((record) => record.id);
		const gridView = formViews.find((view) => view.view_type === 'grid');
		if (gridView) {
			selectedViewId = gridView.id;
			resetViewDraft(gridView);
		}
		toast.success($t('forms.focusedRecords', { values: { count: group.records.length } }));
	}

	function cardGroupFilterDraft(group: { label: string }): ViewFilterDraft | null {
		if (!cardGroupField) return null;
		return {
			field: cardGroupField.key,
			operator: 'equals',
			value: group.label
		};
	}

	function canApplyCardGroupFilter(group: { label: string }): boolean {
		const filter = cardGroupFilterDraft(group);
		if (!filter) return false;
		const currentGroups = visibleViewDraftFilterGroups().filter((item) => item.filters.length > 0);
		const currentFilterCount = currentGroups.reduce((count, item) => count + item.filters.length, 0);
		if (currentFilterCount >= 10) return false;
		if (currentGroups.length < 5) return true;
		const lastGroup = currentGroups[currentGroups.length - 1];
		return Boolean(lastGroup && lastGroup.filters.length < 5);
	}

	function applyCardGroupFilter(group: { key: string; label: string }) {
		if (!canApplyCardGroupFilter(group)) return;
		const filter = cardGroupFilterDraft(group);
		if (!filter) return;
		const currentGroups = visibleViewDraftFilterGroups()
			.filter((item) => item.filters.length > 0)
			.map((item) => ({
				logic: item.logic,
				disabled: item.disabled || undefined,
				filters: item.filters.map((currentFilter) => ({ ...currentFilter }))
			}));
		const nextGroups =
			currentGroups.length === 0
				? [{ logic: 'all' as ViewFilterLogic, filters: [filter] }]
				: currentGroups.length < 5
					? [...currentGroups, { logic: 'all' as ViewFilterLogic, filters: [filter] }]
					: currentGroups.map((item, index) =>
							index === currentGroups.length - 1
								? { ...item, filters: [...item.filters, filter] }
								: item
						);
		setViewDraftFilterGroups(nextGroups);
		updateViewDraftFilterRootLogic('all');
		appliedCardGroupFilterKey = cardGroupFilterKey(group);
		toast.success($t('forms.cardGroupFilterApplied'));
	}

	async function updateTimelineRecordSelectField(record: FormRecord, field: FormField, value: string) {
		if (!value || record.values[field.key] === value) return;
		updatingTimelineRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: { ...record.values, [field.key]: value },
			source: { type: 'web', origin: 'timeline' }
		});
		updatingTimelineRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.timelineFieldUpdated'));
		await loadAggregates();
	}

	function selectedKanbanRecords(): FormRecord[] {
		const selectedIds = new Set(selectedKanbanRecordIds);
		return records.filter((record) => selectedIds.has(record.id));
	}

	function isKanbanRecordSelected(recordId: string): boolean {
		return selectedKanbanRecordIds.includes(recordId);
	}

	function setKanbanRecordSelected(recordId: string, selected: boolean) {
		selectedKanbanRecordIds = selected
			? Array.from(new Set([...selectedKanbanRecordIds, recordId]))
			: selectedKanbanRecordIds.filter((id) => id !== recordId);
	}

	function clearKanbanSelection() {
		selectedKanbanRecordIds = [];
	}

	function kanbanBulkMoveAllowed(selectedRecords: FormRecord[], targetValue: string): boolean {
		if (!kanbanGroupField || selectedRecords.length === 0) return false;
		const limit = kanbanWipLimit(targetValue);
		if (limit === null) return true;
		const selectedIds = new Set(selectedRecords.map((record) => record.id));
		const currentTargetCount = records.filter(
			(record) =>
				!selectedIds.has(record.id) &&
				String(record.values[kanbanGroupField.key] ?? '') === targetValue
		).length;
		return currentTargetCount + selectedRecords.length <= limit;
	}

	async function bulkMoveKanbanRecords() {
		if (!kanbanGroupField || !bulkKanbanTarget) return;
		const selectedRecords = selectedKanbanRecords();
		if (selectedRecords.length === 0) {
			toast.error($t('forms.kanbanBulkSelectFirst'));
			return;
		}
		if (!kanbanBulkMoveAllowed(selectedRecords, bulkKanbanTarget)) {
			toast.error($t('forms.kanbanWipLimitReached'));
			return;
		}
		bulkMovingKanban = true;
		const updatedRecords: FormRecord[] = [];
		for (const record of selectedRecords) {
			if (String(record.values[kanbanGroupField.key] ?? '') === bulkKanbanTarget) continue;
			const response = await formsApi.updateRecord(record.id, {
				values: { ...record.values, [kanbanGroupField.key]: bulkKanbanTarget },
				source: { type: 'web', origin: 'kanban_bulk' }
			});
			if (response.code !== 0 || !response.data) {
				bulkMovingKanban = false;
				toast.error(response.message);
				return;
			}
			updatedRecords.push(response.data);
		}
		bulkMovingKanban = false;
		if (updatedRecords.length > 0) {
			const updatedById = new Map(updatedRecords.map((record) => [record.id, record]));
			records = records.map((record) => updatedById.get(record.id) ?? record);
			filterPreviewRecords = filterPreviewRecords.map((record) => updatedById.get(record.id) ?? record);
			if (selectedRecord && updatedById.has(selectedRecord.id)) {
				selectedRecord = updatedById.get(selectedRecord.id) ?? selectedRecord;
			}
		}
		clearKanbanSelection();
		toast.success($t('forms.kanbanBulkMoved', { values: { count: selectedRecords.length } }));
		await loadAggregates();
	}

	function handleKanbanDragStart(event: DragEvent, record: FormRecord) {
		draggingKanbanRecordId = record.id;
		event.dataTransfer?.setData('text/plain', record.id);
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
		}
	}

	function handleKanbanDragEnd() {
		draggingKanbanRecordId = null;
	}

	function handleKanbanDragOver(event: DragEvent, groupLabel: string) {
		if (!draggingKanbanRecordId || !kanbanGroupField) return;
		const record = records.find((item) => item.id === draggingKanbanRecordId);
		if (!record || !kanbanMoveAllowed(record, groupLabel)) {
			if (event.dataTransfer) event.dataTransfer.dropEffect = 'none';
			return;
		}
		event.preventDefault();
		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
	}

	async function handleKanbanDrop(event: DragEvent, groupLabel: string) {
		event.preventDefault();
		const recordId = event.dataTransfer?.getData('text/plain') || draggingKanbanRecordId;
		draggingKanbanRecordId = null;
		if (!recordId || !kanbanGroupField) return;
		const record = records.find((item) => item.id === recordId);
		if (!record) return;
		await updateKanbanRecordField(record, kanbanGroupField, groupLabel);
	}

	async function updateCalendarRecordDate(record: FormRecord, field: FormField, value: string) {
		if (!value) return;
		const endField = viewCalendarEndField();
		const currentStart = calendarRecordIsoDate(record, field);
		const currentEnd = endField ? calendarRecordIsoDate(record, endField) : null;
		const duration =
			currentStart && currentEnd && currentEnd >= currentStart ? isoDayDiff(currentEnd, currentStart) : 0;
		const nextValue = calendarDatePayloadValue(field, value);
		const nextValues = { ...record.values, [field.key]: nextValue };
		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(endField, addIsoDays(value, duration));
		}
		if (
			record.values[field.key] === nextValues[field.key] &&
			(!endField || record.values[endField.key] === nextValues[endField.key])
		) {
			return;
		}
		updatingCalendarRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'calendar' }
		});
		updatingCalendarRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.calendarDateUpdated'));
		await loadAggregates();
	}

	async function updateCalendarSeriesDate(record: FormRecord, field: FormField, value: string) {
		if (!value) return;
		const endField = viewCalendarEndField();
		const currentStart = calendarRecordIsoDate(record, field);
		const currentEnd = endField ? calendarRecordIsoDate(record, endField) : null;
		const duration =
			currentStart && currentEnd && currentEnd >= currentStart ? isoDayDiff(currentEnd, currentStart) : 0;
		const nextValue = calendarDatePayloadValue(field, value);
		const nextValues = { ...record.values, [field.key]: nextValue };
		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(endField, addIsoDays(value, duration));
		}
		if (
			calendarRecordIsoDate(record, field) === value &&
			(!endField || record.values[endField.key] === nextValues[endField.key])
		) {
			return;
		}
		updatingCalendarRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'calendar_series' }
		});
		updatingCalendarRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.calendarSeriesUpdated'));
		await loadAggregates();
	}

	async function updateCalendarRecordEndDate(record: FormRecord, endField: FormField, value: string) {
		if (!value) return;
		const startField = viewDateField();
		const currentStart = startField ? calendarRecordIsoDate(record, startField) : null;
		const nextEnd = currentStart && value < currentStart ? currentStart : value;
		if (calendarRecordIsoDate(record, endField) === nextEnd) return;
		updatingCalendarRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: {
				...record.values,
				[endField.key]: calendarDatePayloadValue(endField, nextEnd)
			},
			source: { type: 'web', origin: 'calendar_end_resize' }
		});
		updatingCalendarRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.calendarEndDateUpdated'));
		await loadAggregates();
	}

	function handleCalendarDragStart(event: DragEvent, record: FormRecord) {
		draggingCalendarRecordId = record.id;
		event.dataTransfer?.setData('text/plain', record.id);
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
		}
	}

	function handleCalendarDragEnd() {
		draggingCalendarRecordId = null;
	}

	function handleCalendarDragOver(event: DragEvent) {
		if (!draggingCalendarRecordId || !calendarDateField) return;
		event.preventDefault();
		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
	}

	async function handleCalendarDrop(event: DragEvent, isoDate: string) {
		event.preventDefault();
		const recordId = event.dataTransfer?.getData('text/plain') || draggingCalendarRecordId;
		draggingCalendarRecordId = null;
		if (!recordId || !calendarDateField) return;
		const record = records.find((item) => item.id === recordId);
		if (!record) return;
		await updateCalendarRecordDate(record, calendarDateField, isoDate);
	}

	async function updateGanttRecordDate(record: FormRecord, startField: FormField, value: string) {
		if (!value) return;
		const endField = viewGanttEndField();
		const currentStart = calendarRecordIsoDate(record, startField);
		const currentEnd = endField ? calendarRecordIsoDate(record, endField) : null;
		const duration =
			currentStart && currentEnd && currentEnd >= currentStart ? isoDayDiff(currentEnd, currentStart) : 0;
		const nextValues = {
			...record.values,
			[startField.key]: calendarDatePayloadValue(startField, value)
		};
		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(endField, addIsoDays(value, duration));
		}
		if (
			record.values[startField.key] === nextValues[startField.key] &&
			(!endField || record.values[endField.key] === nextValues[endField.key])
		) {
			return;
		}
		updatingGanttRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'gantt' }
		});
		updatingGanttRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.ganttDateUpdated'));
		await loadAggregates();
	}

	async function updateGanttRecordEndDate(record: FormRecord, endField: FormField, value: string) {
		if (!value) return;
		const startField = viewGanttStartField();
		const currentStart = startField ? calendarRecordIsoDate(record, startField) : null;
		const nextEnd = currentStart && value < currentStart ? currentStart : value;
		if (calendarRecordIsoDate(record, endField) === nextEnd) return;
		updatingGanttRecordId = record.id;
		const response = await formsApi.updateRecord(record.id, {
			values: {
				...record.values,
				[endField.key]: calendarDatePayloadValue(endField, nextEnd)
			},
			source: { type: 'web', origin: 'gantt_resize' }
		});
		updatingGanttRecordId = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		toast.success($t('forms.ganttEndDateUpdated'));
		await loadAggregates();
	}

	async function extendGanttTaskEnd(task: GanttTask) {
		if (!ganttTimeline.endField) return;
		const nextEnd = addGanttBucket(task.end, ganttTimeline.scale, 1);
		await updateGanttRecordEndDate(task.record, ganttTimeline.endField, nextEnd);
	}

	function handleGanttDragStart(event: DragEvent, record: FormRecord) {
		draggingGanttRecordId = record.id;
		event.dataTransfer?.setData('text/plain', record.id);
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
		}
	}

	function handleGanttDragEnd() {
		draggingGanttRecordId = null;
	}

	function handleGanttDragOver(event: DragEvent) {
		if (!draggingGanttRecordId || !ganttTimeline.startField) return;
		event.preventDefault();
		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
	}

	async function handleGanttDrop(event: DragEvent, isoDate: string) {
		event.preventDefault();
		const recordId = event.dataTransfer?.getData('text/plain') || draggingGanttRecordId;
		draggingGanttRecordId = null;
		if (!recordId || !ganttTimeline.startField) return;
		const record = records.find((item) => item.id === recordId);
		if (!record) return;
		await updateGanttRecordDate(record, ganttTimeline.startField, isoDate);
	}

	async function syncRecordDetailUrl(record: FormRecord | null) {
		const url = new URL($page.url);
		if (record) {
			url.searchParams.set('form', selectedFormId);
			url.searchParams.set('record', record.id);
		} else {
			url.searchParams.delete('record');
			url.searchParams.delete('record_id');
			if (selectedFormId) {
				url.searchParams.set('form', selectedFormId);
			} else {
				url.searchParams.delete('form');
				url.searchParams.delete('form_id');
			}
		}
		const next = `${url.pathname}${url.search}${url.hash}`;
		const current = `${$page.url.pathname}${$page.url.search}${$page.url.hash}`;
		if (next !== current) {
			await goto(next, { replaceState: true, noScroll: true, keepFocus: true });
		}
	}

	async function restoreRecordDetailFromUrl() {
		const recordId = detailUrlRecordId;
		if (!recordId || selectedRecord?.id === recordId) return;
		const localRecord =
			records.find((record) => record.id === recordId) ??
			filterPreviewRecords.find((record) => record.id === recordId);
		if (localRecord) {
			await selectRecord(localRecord, { syncUrl: false });
			return;
		}
		const response = await formsApi.getRecord(recordId);
		if (response.code !== 0 || !response.data) {
			await syncRecordDetailUrl(null);
			return;
		}
		selectedRecord = response.data;
		activeMode = 'detail';
		await Promise.all([
			loadSelectedRecordLinks(),
			loadSelectedRecordChildren(),
			loadSelectedRecordComments(),
			loadRecordAttachments(response.data.id)
		]);
	}

	async function selectRecord(record: FormRecord, options: { syncUrl?: boolean } = {}) {
		selectedRecord = record;
		activeMode = 'detail';
		if (options.syncUrl !== false) await syncRecordDetailUrl(record);
		await Promise.all([
			loadSelectedRecordLinks(),
			loadSelectedRecordChildren(),
			loadSelectedRecordComments(),
			loadRecordAttachments(record.id)
		]);
	}

	function closeRecordDetail() {
		selectedRecord = null;
		selectedRecordLinks = [];
		selectedRecordChildren = [];
		selectedRecordComments = [];
		recordCommentDraft = '';
		recordAttachments = [];
		activeMode = 'data';
		void syncRecordDetailUrl(null);
	}

	function startEditRecord(record: FormRecord) {
		selectedRecord = record;
		editingRecordId = record.id;
		activeMode = 'data';
		recordTitle = record.title;
		recordDraft = Object.fromEntries(
			fields.map((field) => [field.key, draftValueForField(field, record)])
		);
		void loadRecordAttachments(record.id);
	}

	function openRecordCreateDrawer() {
		editingRecordId = null;
		selectedRecord = null;
		recordAttachments = [];
		resetRecordDraft();
		activeMode = 'data';
		recordEditorOpen = true;
	}

	function openRecordEditDrawer(record: FormRecord) {
		startEditRecord(record);
		recordEditorOpen = true;
	}

	function closeRecordEditorDrawer() {
		recordEditorOpen = false;
		cancelEditRecord();
	}

	function goToRecordPage(page: number) {
		recordPage = Math.min(Math.max(page, 1), recordPageCount);
	}

	function cancelEditRecord() {
		editingRecordId = null;
		recordTitle = '';
		resetRecordDraft();
		if (selectedRecord) {
			void loadRecordAttachments(selectedRecord.id);
		} else {
			recordAttachments = [];
		}
	}

	async function openPrintJob(record: FormRecord) {
		if (printJobForm && selectedFormId !== printJobForm.id) {
			selectedFormId = printJobForm.id;
			records = printJobs;
		}
		editingRecordId = null;
		selectedRecord = record;
		await Promise.all([loadSelectedRecordLinks(), loadSelectedRecordChildren()]);
	}

	function defaultPermissionActions(): Record<FormPermissionAction, boolean> {
		return Object.fromEntries(formPermissionActions.map((action) => [action, true])) as Record<
			FormPermissionAction,
			boolean
		>;
	}

	function rolePermissionActions(
		permissions: FormPermissions,
		role: 'owner' | 'admin' | 'member'
	): Record<FormPermissionAction, boolean> {
		const row = permissions.policies.find(
			(policy) => policy.subject_type === 'role' && policy.subject_id === role
		);
		const actions = row?.policy?.actions ?? {};
		return Object.fromEntries(
			formPermissionActions.map((action) => [action, actions[action] ?? true])
		) as Record<FormPermissionAction, boolean>;
	}

	function roleRecordScope(permissions: FormPermissions, role: 'owner' | 'admin' | 'member'): FormRecordScope {
		const row = permissions.policies.find(
			(policy) => policy.subject_type === 'role' && policy.subject_id === role
		);
		return row?.policy?.record_scope === 'owned' ? 'owned' : 'all';
	}

	function defaultFieldWriteDraft(): Record<string, boolean> {
		return Object.fromEntries(fields.map((field) => [field.key, true]));
	}

	function defaultFieldReadDraft(): Record<string, boolean> {
		return Object.fromEntries(fields.map((field) => [field.key, true]));
	}

	function roleFieldReadPermissions(
		permissions: FormPermissions,
		role: 'owner' | 'admin' | 'member'
	): Record<string, boolean> {
		const row = permissions.policies.find(
			(policy) => policy.subject_type === 'role' && policy.subject_id === role
		);
		const fieldPolicies = row?.policy?.fields ?? {};
		return Object.fromEntries(
			fields.map((field) => [field.key, fieldPolicies[field.key]?.read ?? true])
		);
	}

	function roleFieldWritePermissions(
		permissions: FormPermissions,
		role: 'owner' | 'admin' | 'member'
	): Record<string, boolean> {
		const row = permissions.policies.find(
			(policy) => policy.subject_type === 'role' && policy.subject_id === role
		);
		const fieldPolicies = row?.policy?.fields ?? {};
		return Object.fromEntries(
			fields.map((field) => [field.key, fieldPolicies[field.key]?.write ?? true])
		);
	}

	function memberFieldPolicy(): Record<string, { read?: boolean; write?: boolean }> {
		const policy: Record<string, { read?: boolean; write?: boolean }> = {};
		for (const field of fields) {
			const fieldPolicy: { read?: boolean; write?: boolean } = {};
			if (memberFieldReadDraft[field.key] === false) fieldPolicy.read = false;
			if (memberFieldWriteDraft[field.key] === false) fieldPolicy.write = false;
			if (Object.keys(fieldPolicy).length > 0) policy[field.key] = fieldPolicy;
		}
		return policy;
	}

	function setMemberPermission(action: FormPermissionAction, enabled: boolean) {
		memberPermissionDraft = { ...memberPermissionDraft, [action]: enabled };
	}

	function setMemberRecordScope(scope: FormRecordScope) {
		memberRecordScopeDraft = scope;
	}

	function setMemberFieldRead(fieldKey: string, enabled: boolean) {
		memberFieldReadDraft = { ...memberFieldReadDraft, [fieldKey]: enabled };
		if (!enabled) {
			memberFieldWriteDraft = { ...memberFieldWriteDraft, [fieldKey]: false };
		}
	}

	function setMemberFieldWrite(fieldKey: string, enabled: boolean) {
		memberFieldWriteDraft = { ...memberFieldWriteDraft, [fieldKey]: enabled };
		if (enabled) {
			memberFieldReadDraft = { ...memberFieldReadDraft, [fieldKey]: true };
		}
	}

	function permissionActionLabel(action: FormPermissionAction): string {
		return $t(`forms.permissionActions.${action.replace('.', '_')}`);
	}

	async function loadSelectedRecordLinks() {
		if (!selectedRecord) {
			selectedRecordLinks = [];
			selectedRecordChildren = [];
			selectedRecordComments = [];
			return;
		}
		const response = await formsApi.listRecordLinks(selectedRecord.id);
		if (response.code === 0 && response.data) {
			selectedRecordLinks = response.data;
		}
	}

	async function loadSelectedRecordChildren() {
		if (!selectedRecord) {
			selectedRecordChildren = [];
			return;
		}
		const response = await formsApi.listRecordChildren(selectedRecord.id, { per_page: 100 });
		if (response.code === 0 && response.data) {
			selectedRecordChildren = response.data.items;
		}
	}

	async function loadSelectedRecordComments() {
		if (!selectedRecord) {
			selectedRecordComments = [];
			return;
		}
		recordCommentsLoading = true;
		const response = await formsApi.listRecordComments(selectedRecord.id, { per_page: 20 });
		recordCommentsLoading = false;
		if (response.code !== 0) {
			selectedRecordComments = [];
			toast.error(response.message);
			return;
		}
		selectedRecordComments = response.data?.items ?? [];
	}

	async function reloadSelectedRecord() {
		if (!selectedRecord) return;
		const response = await formsApi.getRecord(selectedRecord.id);
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		selectedRecord = response.data;
		replaceLocalRecord(response.data);
	}

	async function handleCreateLink() {
		if (!selectedRecord) return;
		linkSubmitting = true;
		const response = await formsApi.createRecordLink(selectedRecord.id, {
			target_type: linkDraft.target_type.trim() || 'form_record',
			target_id: linkDraft.target_id.trim(),
			relation_key: linkDraft.relation_key.trim() || 'related',
			relation_type: linkDraft.relation_type.trim() || 'reference',
			metadata: { source: 'web' }
		});
		linkSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		selectedRecordLinks = [response.data, ...selectedRecordLinks];
		linkDraft = { ...linkDraft, target_id: '' };
		toast.success($t('forms.linkCreated'));
		await loadSelectedRecordChildren();
	}

	async function handleCreateRecordComment() {
		if (!selectedRecord) return;
		const body = recordCommentDraft.trim();
		if (!body) return;
		recordCommentSubmitting = true;
		const response = await formsApi.createRecordComment(selectedRecord.id, {
			body,
			metadata: { source: 'web', form_id: selectedForm?.id, form_key: selectedForm?.key }
		});
		recordCommentSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		selectedRecordComments = [
			response.data,
			...selectedRecordComments.filter((item) => item.id !== response.data?.id)
		];
		recordCommentDraft = '';
		await loadFormEvents();
		toast.success($t('forms.recordCommentAdded'));
	}

	function childFormForRelationField(field: FormField): UniversalForm | null {
		const formKey = field.relation?.form_key;
		return formKey ? (forms.find((form) => form.key === formKey) ?? null) : null;
	}

	function parentChildRelationFields(): FormField[] {
		return fields.filter(
			(field) =>
				(field.type === 'relation' || field.type === 'child_table') &&
				field.relation?.relation_type === 'parent_child' &&
				Boolean(childFormForRelationField(field))
		);
	}

	function childFieldsForForm(formId: string): FormField[] {
		const childForm = forms.find((form) => form.id === formId);
		return childForm ? parseFields(childForm.schema) : [];
	}

	function defaultDraftForFields(formFields: FormField[]): Record<string, unknown> {
		return Object.fromEntries(formFields.map((field) => [field.key, defaultDraftValueForField(field)]));
	}

	function startAddChildRecord(field: FormField) {
		const childForm = childFormForRelationField(field);
		if (!childForm) return;
		childEditor = {
			relationFieldKey: field.key,
			childFormId: childForm.id,
			title: '',
			draft: defaultDraftForFields(parseFields(childForm.schema))
		};
	}

	function startEditChildRecord(child: FormChildRecord) {
		const childForm = forms.find((form) => form.id === child.record.form_id);
		if (!childForm) return;
		const relationField = fields.find(
			(field) =>
				(field.type === 'relation' || field.type === 'child_table') &&
				field.relation?.relation_type === 'parent_child' &&
				field.relation?.relation_key === child.relation_key &&
				field.relation?.form_key === childForm.key
		);
		if (!relationField) return;
		const childFields = parseFields(childForm.schema);
		childEditor = {
			relationFieldKey: relationField.key,
			childFormId: childForm.id,
			recordId: child.record.id,
			title: child.record.title,
			draft: Object.fromEntries(
				childFields.map((field) => [field.key, draftValueForField(field, child.record)])
			)
		};
	}

	function cancelChildEditor() {
		childEditor = null;
	}

	async function saveChildRecord() {
		if (!selectedRecord || !childEditor) return;
		const relationField = fields.find((field) => field.key === childEditor?.relationFieldKey);
		const childForm = forms.find((form) => form.id === childEditor?.childFormId);
		if (!relationField || !childForm) return;
		let values: Record<string, unknown>;
		try {
			values = buildValuesFromDraft(parseFields(childForm.schema), childEditor.draft);
		} catch (error) {
			toast.error(error instanceof Error ? error.message : $t('forms.invalidRecordValues'));
			return;
		}

		childSubmitting = true;
		const response = childEditor.recordId
			? await formsApi.updateRecord(childEditor.recordId, {
					title: childEditor.title.trim() || undefined,
					values,
					source: { type: 'web', parent_record_id: selectedRecord.id }
				})
			: await formsApi.createRecord(childForm.id, {
					title: childEditor.title.trim() || undefined,
					values,
					source: { type: 'web', parent_record_id: selectedRecord.id }
				});
		childSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		if (!childEditor.recordId) {
			const linkResponse = await formsApi.createRecordLink(selectedRecord.id, {
				target_type: 'form_record',
				target_id: response.data.id,
				relation_key: relationField.relation?.relation_key || relationField.key,
				relation_type: 'parent_child',
				metadata: { source: 'web', inline_child: true, relation_field: relationField.key }
			});
			if (linkResponse.code !== 0) {
				toast.error(linkResponse.message);
			}
		}
		childEditor = null;
		await Promise.all([
			reloadSelectedRecord(),
			loadSelectedRecordChildren(),
			loadSelectedRecordLinks(),
			loadAggregates()
		]);
		toast.success($t('forms.childRecordSaved'));
	}

	async function deleteChildRecord(child: FormChildRecord) {
		deletingChildRecordId = child.record.id;
		const response = await formsApi.deleteRecord(child.record.id);
		deletingChildRecordId = null;
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		if (childEditor?.recordId === child.record.id) childEditor = null;
		await Promise.all([
			reloadSelectedRecord(),
			loadSelectedRecordChildren(),
			loadSelectedRecordLinks(),
			loadAggregates()
		]);
		toast.success($t('forms.childRecordDeleted'));
	}

	function handleFormChange(event: Event) {
		void openForm(selectValue(event));
	}

	function handleModeChange(mode: FormWorkflowMode) {
		activeMode = mode;
		if (mode !== 'data') {
			recordEditorOpen = false;
			editingRecordId = null;
		}
		if (mode === 'design') {
			syncDesignerDrafts();
			syncDetailHeaderDraft();
			syncDetailHighlightDraft();
			syncDetailPageCompositionDraft();
			syncDetailLayoutSectionDrafts();
			void loadDesignerSafety();
		} else if (mode === 'automation') {
			void loadFormPermissions();
		}
	}

	function resetRecordDraft() {
		const next: Record<string, unknown> = {};
		for (const field of fields) {
			next[field.key] = defaultDraftValueForField(field);
		}
		recordDraft = next;
		recordTitle = '';
	}

	function defaultDraftValueForField(field: FormField): unknown {
		if (field.default_value !== undefined && field.default_value !== null) {
			if (Array.isArray(field.default_value)) return field.default_value;
			if (isRecord(field.default_value)) return JSON.stringify(field.default_value);
			return field.default_value;
		}
		if (field.type === 'boolean') return false;
		return '';
	}

	function buildRecordPayload(formFields: FormField[]): Record<string, unknown> {
		return buildValuesFromDraft(formFields, recordDraft);
	}

	function buildValuesFromDraft(
		formFields: FormField[],
		draft: Record<string, unknown>
	): Record<string, unknown> {
		const values: Record<string, unknown> = {};
		for (const field of formFields) {
			if (fieldHiddenByCondition(field, draft)) continue;
			const raw = draft[field.key];
			const type = field.type ?? 'text';
			if (type === 'autonumber' || type === 'child_table') continue;
			if (type === 'boolean') {
				values[field.key] = Boolean(raw);
				continue;
			}
			const text = typeof raw === 'string' ? raw.trim() : '';
			if (!text && !field.required) continue;
			if (!text && field.required) {
				throw new Error($t('forms.fieldRequired', { values: { field: fieldLabel(field) } }));
			}
			if (type === 'multi_select') {
				const selected = Array.isArray(raw)
					? raw.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
					: text
							.split(',')
							.map((item) => item.trim())
							.filter(Boolean);
				if (selected.length === 0 && field.required) {
					throw new Error($t('forms.fieldRequired', { values: { field: fieldLabel(field) } }));
				}
				if (selected.length > 0) values[field.key] = selected;
			} else if (type === 'relation') {
				if (text) {
					const target = relationTargets[field.key]?.find((item) => item.record_id === text);
					values[field.key] = {
						record_id: text,
						title: target?.display || target?.title || text,
						form_key: target?.form_key ?? field.relation?.form_key
					};
				}
			} else if (type === 'member') {
				if (text) {
					const member = workspaceMembers.find((item) => item.user_id === text);
					values[field.key] = {
						user_id: text,
						...(member?.name ? { name: member.name } : {}),
						...(member?.email ? { email: member.email } : {})
					};
				}
			} else if (['attachment', 'image', 'relation', 'formula'].includes(type)) {
				values[field.key] = parseAdvancedRecordField(text);
			} else {
				values[field.key] = text;
			}
		}
		return values;
	}

	function parseAdvancedRecordField(value: string): unknown {
		const trimmed = value.trim();
		if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return trimmed;
		try {
			return JSON.parse(trimmed) as unknown;
		} catch {
			return trimmed;
		}
	}

	function draftValueForField(field: FormField, record: FormRecord): unknown {
		const value = record.values[field.key];
		const type = field.type ?? 'text';
		if (type === 'boolean') return Boolean(value);
		if (Array.isArray(value)) return type === 'multi_select' ? value : value.join(', ');
		if (isRecord(value)) {
			if (type === 'relation' && typeof value.record_id === 'string') return value.record_id;
			if (type === 'member' && typeof value.user_id === 'string') return value.user_id;
			if (type === 'amount' && typeof value.decimal === 'string') return value.decimal;
			if (['integer', 'number'].includes(type) && typeof value.value === 'number') {
				return String(value.value);
			}
			return JSON.stringify(value);
		}
		if (value === null || value === undefined) return '';
		return String(value);
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
									(entry): entry is [string, string] =>
										typeof entry[0] === 'string' && typeof entry[1] === 'string'
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
									(entry): entry is [string, string] =>
										typeof entry[0] === 'string' && typeof entry[1] === 'string'
								)
							)
						: undefined,
					option_groups: isRecord(field.option_groups)
						? Object.fromEntries(
								Object.entries(field.option_groups).filter(
									(entry): entry is [string, string] =>
										typeof entry[0] === 'string' && typeof entry[1] === 'string'
								)
							)
						: undefined,
					amount: isRecord(field.amount) ? (field.amount as FormField['amount']) : undefined,
					placeholder: typeof field.placeholder === 'string' ? field.placeholder : undefined,
					help_text: typeof field.help_text === 'string' ? field.help_text : undefined,
					default_value: field.default_value,
					validation: isRecord(field.validation) ? (field.validation as FormField['validation']) : undefined,
					visibility: isRecord(field.visibility) ? (field.visibility as FormField['visibility']) : undefined,
					conditional: isRecord(field.conditional) ? (field.conditional as FormField['conditional']) : undefined,
					policy: isRecord(field.policy) ? (field.policy as FormField['policy']) : undefined,
					formula: isRecord(field.formula) ? (field.formula as FormField['formula']) : undefined,
					relation: isRecord(field.relation) ? (field.relation as FormField['relation']) : undefined,
					attachment: isRecord(field.attachment)
						? (field.attachment as FormField['attachment'])
						: undefined
				};
			})
			.filter((field) => field.key);
	}

	function isRecord(value: unknown): value is Record<string, unknown> {
		return typeof value === 'object' && value !== null && !Array.isArray(value);
	}

	function detailPageWidgetKey(value: unknown): DetailPageWidgetKey | null {
		return typeof value === 'string' && detailPageWidgetKeys.has(value as DetailPageWidgetKey)
			? (value as DetailPageWidgetKey)
			: null;
	}

	function detailPageWidgetEntriesFromLayout(
		layout: Record<string, unknown> | null | undefined
	): DetailPageWidgetEntry[] {
		const entries: DetailPageWidgetEntry[] = [];
		const seen = new Set<DetailPageWidgetKey>();
		const rawWidgets = isRecord(layout) && Array.isArray(layout.widgets) ? layout.widgets : [];
		for (const rawWidget of rawWidgets) {
			const key = isRecord(rawWidget) ? detailPageWidgetKey(rawWidget.key) : detailPageWidgetKey(rawWidget);
			if (!key || seen.has(key)) continue;
			entries.push({
				key,
				enabled: isRecord(rawWidget) ? rawWidget.enabled !== false : true,
				title: isRecord(rawWidget) && typeof rawWidget.title === 'string' ? rawWidget.title : '',
				description:
					isRecord(rawWidget) && typeof rawWidget.description === 'string' ? rawWidget.description : '',
				emptyText: isRecord(rawWidget) && typeof rawWidget.empty_text === 'string' ? rawWidget.empty_text : '',
				showCount: isRecord(rawWidget) ? rawWidget.show_count === true : false,
					hideEmpty: isRecord(rawWidget) ? rawWidget.hide_empty === true : false,
					collapsible: isRecord(rawWidget) ? rawWidget.collapsible === true : false,
					defaultCollapsed:
						isRecord(rawWidget) && rawWidget.collapsible === true ? rawWidget.default_collapsed === true : false,
					density:
						isRecord(rawWidget) && rawWidget.density === 'compact' ? 'compact' : 'comfortable',
					limit: isRecord(rawWidget) ? detailPageWidgetLimitText(rawWidget.limit) : ''
				});
			seen.add(key);
		}
		for (const option of detailPageWidgetOptions) {
			if (!seen.has(option.key)) {
				entries.push({
					key: option.key,
					enabled: true,
					title: '',
					description: '',
					emptyText: '',
					showCount: false,
						hideEmpty: false,
						collapsible: false,
						defaultCollapsed: false,
						density: 'comfortable',
						limit: ''
					});
			}
		}
		return entries;
	}

	function detailPageWidgetOption(key: DetailPageWidgetKey) {
		return detailPageWidgetOptions.find((option) => option.key === key) ?? detailPageWidgetOptions[0];
	}

	function detailPageWidgetPayload(entries: DetailPageWidgetEntry[]) {
		return entries.map((entry) => ({
			key: entry.key,
			enabled: entry.enabled,
			...(entry.title.trim() ? { title: entry.title.trim() } : {}),
			...(entry.description.trim() ? { description: entry.description.trim() } : {}),
			...(entry.emptyText.trim() ? { empty_text: entry.emptyText.trim() } : {}),
			...(entry.showCount ? { show_count: true } : {}),
				...(entry.hideEmpty ? { hide_empty: true } : {}),
				...(entry.collapsible ? { collapsible: true } : {}),
				...(entry.collapsible && entry.defaultCollapsed ? { default_collapsed: true } : {}),
				...(entry.density === 'compact' ? { density: 'compact' } : {}),
				...(detailPageWidgetLimitNumber(entry.limit) ? { limit: detailPageWidgetLimitNumber(entry.limit) } : {})
			}));
	}

	function detailPageWidgetLimitNumber(value: unknown): number | null {
		const numericValue = typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : NaN;
		if (!Number.isFinite(numericValue)) return null;
		const limit = Math.trunc(numericValue);
		if (limit < 1 || limit > 50) return null;
		return limit;
	}

	function detailPageWidgetLimitText(value: unknown): string {
		const limit = detailPageWidgetLimitNumber(value);
		return limit ? String(limit) : '';
	}

	function detailLayoutFingerprint(form: UniversalForm | null = selectedForm): string {
		if (!form) return '';
		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.sections ?? [])}`;
	}

	function detailHeaderFingerprint(form: UniversalForm | null = selectedForm): string {
		if (!form) return '';
		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.header ?? {})}`;
	}

	function detailHighlightFingerprint(form: UniversalForm | null = selectedForm): string {
		if (!form) return '';
		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.highlights ?? [])}`;
	}

	function detailPageCompositionFingerprint(form: UniversalForm | null = selectedForm): string {
		if (!form) return '';
		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.page ?? {})}`;
	}

	function detailPageWidgetRegion(value: unknown): DetailPageWidgetRegion {
		return value === 'below' ? 'below' : 'sidebar';
	}

	function detailPageFieldColumns(value: unknown): 1 | 2 {
		const numericValue = typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : NaN;
		return numericValue === 1 ? 1 : 2;
	}

	function detailPageCompositionDraftFromForm(
		form: UniversalForm | null = selectedForm
	): DetailPageCompositionDraft {
		if (!form) return { widgetRegion: 'sidebar', fieldColumns: 2 };
		const page = isRecord(form.detail_layout?.page) ? form.detail_layout.page : {};
		return {
			widgetRegion: detailPageWidgetRegion(page.widget_region),
			fieldColumns: detailPageFieldColumns(page.field_columns)
		};
	}

	function syncDetailPageCompositionDraft(form: UniversalForm | null = selectedForm) {
		const fingerprint = detailPageCompositionFingerprint(form);
		if (!form) {
			detailPageCompositionDraft = { widgetRegion: 'sidebar', fieldColumns: 2 };
			detailPageCompositionSyncedFingerprint = '';
			return;
		}
		if (detailPageCompositionSyncedFingerprint === fingerprint) return;
		detailPageCompositionDraft = detailPageCompositionDraftFromForm(form);
		detailPageCompositionSyncedFingerprint = fingerprint;
	}

	function detailHeaderDraftFromForm(form: UniversalForm | null = selectedForm): DetailHeaderDraft {
		if (!form) return { subtitleField: '', badgeField: '' };
		const fieldKeys = new Set(parseFields(form.schema).map((field) => field.key));
		const header = isRecord(form.detail_layout?.header) ? form.detail_layout.header : {};
		const subtitleField =
			typeof header.subtitle_field === 'string' && fieldKeys.has(header.subtitle_field)
				? header.subtitle_field
				: '';
		const badgeField =
			typeof header.badge_field === 'string' && fieldKeys.has(header.badge_field)
				? header.badge_field
				: '';
		return { subtitleField, badgeField };
	}

	function syncDetailHeaderDraft(form: UniversalForm | null = selectedForm) {
		const fingerprint = detailHeaderFingerprint(form);
		if (!form) {
			detailHeaderDraft = { subtitleField: '', badgeField: '' };
			detailHeaderSyncedFingerprint = '';
			return;
		}
		if (detailHeaderSyncedFingerprint === fingerprint) return;
		detailHeaderDraft = detailHeaderDraftFromForm(form);
		detailHeaderSyncedFingerprint = fingerprint;
	}

	function detailHighlightDraftFromForm(form: UniversalForm | null = selectedForm): DetailHighlightDraft {
		if (!form) return { fields: [] };
		const fields = parseFields(form.schema).filter((field) => field.visibility?.detail !== false);
		const validFieldKeys = new Set(fields.map((field) => field.key));
		const rawHighlights = Array.isArray(form.detail_layout?.highlights) ? form.detail_layout.highlights : [];
		const highlightFields: string[] = [];
		for (const fieldKey of rawHighlights) {
			if (
				typeof fieldKey === 'string' &&
				validFieldKeys.has(fieldKey) &&
				!highlightFields.includes(fieldKey)
			) {
				highlightFields.push(fieldKey);
			}
		}
		return { fields: highlightFields.slice(0, 4) };
	}

	function syncDetailHighlightDraft(form: UniversalForm | null = selectedForm) {
		const fingerprint = detailHighlightFingerprint(form);
		if (!form) {
			detailHighlightDraft = { fields: [] };
			detailHighlightSyncedFingerprint = '';
			return;
		}
		if (detailHighlightSyncedFingerprint === fingerprint) return;
		detailHighlightDraft = detailHighlightDraftFromForm(form);
		detailHighlightSyncedFingerprint = fingerprint;
	}

	function detailLayoutSectionDraftsFromForm(
		form: UniversalForm | null = selectedForm
	): DetailLayoutSectionDraft[] {
		if (!form) return [];
		const detailFields = parseFields(form.schema).filter((field) => field.visibility?.detail !== false);
		const detailFieldKeys = detailFields.map((field) => field.key);
		const validFieldKeys = new Set(detailFieldKeys);
		const usedFieldKeys = new Set<string>();
		const rawSections = Array.isArray(form.detail_layout?.sections) ? form.detail_layout.sections : [];
		const drafts = rawSections
			.filter(isRecord)
			.map((section, index) => {
				const key =
					typeof section.key === 'string' && section.key.trim().length > 0
						? section.key.trim()
						: `section_${index + 1}`;
				const title =
					typeof section.title === 'string' && section.title.trim().length > 0
						? section.title.trim()
						: typeof section.name === 'string' && section.name.trim().length > 0
							? section.name.trim()
							: `${$t('forms.detailSectionMain')} ${index + 1}`;
				const description = detailLayoutSectionDescription(section.description);
				const columns = detailLayoutSectionColumns(section.columns);
				const density: DetailLayoutSectionDensity = section.density === 'compact' ? 'compact' : 'comfortable';
				const hideEmptyFields = section.hide_empty_fields === true;
				const collapsible = section.collapsible === true;
				const defaultCollapsed = collapsible && section.default_collapsed === true;
				const sectionFields = detailLayoutFieldKeys(section).filter((fieldKey) => {
					if (!validFieldKeys.has(fieldKey) || usedFieldKeys.has(fieldKey)) return false;
					usedFieldKeys.add(fieldKey);
					return true;
				});
				return {
					key,
					title,
					description,
					columns,
					density,
					hideEmptyFields,
					collapsible,
					defaultCollapsed,
					fields: sectionFields
				};
			})
			.filter(
				(section) =>
					section.fields.length > 0 ||
					section.title.trim().length > 0 ||
					section.description.trim().length > 0
			);
		const unplacedFields = detailFieldKeys.filter((fieldKey) => !usedFieldKeys.has(fieldKey));
		if (drafts.length === 0) {
			return [
				{
					key: 'main',
					title: $t('forms.detailSectionMain'),
					description: '',
					columns: 2,
					density: 'comfortable',
					hideEmptyFields: false,
					collapsible: false,
					defaultCollapsed: false,
					fields: detailFieldKeys
				}
			];
		}
		if (unplacedFields.length > 0) {
			drafts.push({
				key: nextDetailLayoutSectionKey(drafts, 'additional'),
				title: $t('forms.detailSectionAdditional'),
				description: '',
				columns: 2,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: unplacedFields
			});
		}
		return drafts;
	}

	function syncDetailLayoutSectionDrafts(form: UniversalForm | null = selectedForm) {
		const fingerprint = detailLayoutFingerprint(form);
		if (!form) {
			detailLayoutSectionDrafts = [];
			detailLayoutSectionSyncedFingerprint = '';
			return;
		}
		if (detailLayoutSectionSyncedFingerprint === fingerprint) return;
		detailLayoutSectionDrafts = detailLayoutSectionDraftsFromForm(form);
		detailLayoutSectionSyncedFingerprint = fingerprint;
	}

	function nextDetailLayoutSectionKey(
		sections: DetailLayoutSectionDraft[] = detailLayoutSectionDrafts,
		prefix = 'section'
	): string {
		const used = new Set(sections.map((section) => section.key));
		let index = sections.length + 1;
		let key = `${prefix}_${index}`;
		while (used.has(key)) {
			index += 1;
			key = `${prefix}_${index}`;
		}
		return key;
	}

	function detailLayoutSectionPayload(sections: DetailLayoutSectionDraft[]) {
		const validFieldKeys = new Set(fields.filter((field) => field.visibility?.detail !== false).map((field) => field.key));
		const usedFieldKeys = new Set<string>();
		return sections
			.map((section, index) => {
				const sectionFields = section.fields.filter((fieldKey) => {
					if (!validFieldKeys.has(fieldKey) || usedFieldKeys.has(fieldKey)) return false;
					usedFieldKeys.add(fieldKey);
					return true;
				});
				return {
					key: section.key.trim() || `section_${index + 1}`,
					title: section.title.trim() || `${$t('forms.detailSectionMain')} ${index + 1}`,
					...(section.description.trim() ? { description: section.description.trim() } : {}),
					...(section.columns !== 2 ? { columns: section.columns } : {}),
					...(section.density === 'compact' ? { density: 'compact' } : {}),
					...(section.hideEmptyFields ? { hide_empty_fields: true } : {}),
					...(section.collapsible ? { collapsible: true } : {}),
					...(section.collapsible && section.defaultCollapsed ? { default_collapsed: true } : {}),
					fields: sectionFields
				};
			})
			.filter((section) => section.fields.length > 0);
	}

	function detailLayoutFieldAssignedTo(fieldKey: string): string {
		return detailLayoutSectionDrafts.find((section) => section.fields.includes(fieldKey))?.key ?? '';
	}

	function updateDetailLayoutSectionTitle(index: number, title: string) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) =>
			sectionIndex === index ? { ...section, title } : section
		);
	}

	function updateDetailLayoutSectionDescription(index: number, description: string) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) =>
			sectionIndex === index ? { ...section, description } : section
		);
	}

	function updateDetailLayoutSectionColumns(index: number, columns: 1 | 2 | 3) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) =>
			sectionIndex === index ? { ...section, columns } : section
		);
	}

	function updateDetailLayoutSectionDensity(index: number, density: DetailLayoutSectionDensity) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) =>
			sectionIndex === index ? { ...section, density } : section
		);
	}

	function updateDetailLayoutSectionHideEmptyFields(index: number, hideEmptyFields: boolean) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) =>
			sectionIndex === index ? { ...section, hideEmptyFields } : section
		);
	}

	function updateDetailLayoutSectionCollapsible(index: number, collapsible: boolean) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) =>
			sectionIndex === index
				? {
						...section,
						collapsible,
						defaultCollapsed: collapsible ? section.defaultCollapsed : false
					}
				: section
		);
	}

	function updateDetailLayoutSectionDefaultCollapsed(index: number, defaultCollapsed: boolean) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) =>
			sectionIndex === index
				? { ...section, defaultCollapsed: section.collapsible && defaultCollapsed }
				: section
		);
	}

	function addDetailLayoutSection() {
		const key = nextDetailLayoutSectionKey();
		detailLayoutSectionDrafts = [
			...detailLayoutSectionDrafts,
			{
				key,
				title: `${$t('forms.detailSectionMain')} ${detailLayoutSectionDrafts.length + 1}`,
			description: '',
			columns: 2,
			density: 'comfortable',
			hideEmptyFields: false,
			collapsible: false,
			defaultCollapsed: false,
				fields: []
			}
		];
	}

	function removeDetailLayoutSection(index: number) {
		if (detailLayoutSectionDrafts.length <= 1) return;
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.filter((_, sectionIndex) => sectionIndex !== index);
	}

	function moveDetailLayoutSection(index: number, direction: -1 | 1) {
		const nextIndex = index + direction;
		if (nextIndex < 0 || nextIndex >= detailLayoutSectionDrafts.length) return;
		const next = [...detailLayoutSectionDrafts];
		[next[index], next[nextIndex]] = [next[nextIndex], next[index]];
		detailLayoutSectionDrafts = next;
	}

	function setDetailLayoutSectionField(index: number, fieldKey: string, checked: boolean) {
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((section, sectionIndex) => {
			const withoutField = section.fields.filter((key) => key !== fieldKey);
			if (sectionIndex !== index) return { ...section, fields: withoutField };
			return {
				...section,
				fields: checked ? [...withoutField, fieldKey] : withoutField
			};
		});
	}

	function moveDetailLayoutSectionField(index: number, fieldKey: string, direction: -1 | 1) {
		const section = detailLayoutSectionDrafts[index];
		if (!section) return;
		const fieldIndex = section.fields.indexOf(fieldKey);
		const nextFieldIndex = fieldIndex + direction;
		if (fieldIndex < 0 || nextFieldIndex < 0 || nextFieldIndex >= section.fields.length) return;
		const nextFields = [...section.fields];
		[nextFields[fieldIndex], nextFields[nextFieldIndex]] = [nextFields[nextFieldIndex], nextFields[fieldIndex]];
		detailLayoutSectionDrafts = detailLayoutSectionDrafts.map((item, sectionIndex) =>
			sectionIndex === index ? { ...item, fields: nextFields } : item
		);
	}

	function setDetailHeaderDraft(patch: Partial<DetailHeaderDraft>) {
		detailHeaderDraft = { ...detailHeaderDraft, ...patch };
	}

	function setDetailHighlightField(fieldKey: string, checked: boolean) {
		const nextFields = detailHighlightDraft.fields.filter((key) => key !== fieldKey);
		if (checked && nextFields.length < 4) nextFields.push(fieldKey);
		detailHighlightDraft = { fields: nextFields };
	}

	function moveDetailHighlightField(fieldKey: string, direction: -1 | 1) {
		const fields = [...detailHighlightDraft.fields];
		const index = fields.indexOf(fieldKey);
		const nextIndex = index + direction;
		if (index < 0 || nextIndex < 0 || nextIndex >= fields.length) return;
		[fields[index], fields[nextIndex]] = [fields[nextIndex], fields[index]];
		detailHighlightDraft = { fields };
	}

	function fieldLabel(field: FormField): string {
		return field.label?.trim() || field.key.replaceAll('_', ' ');
	}

	function permissionFieldList(labels: string[]): string {
		return labels.length > 0 ? labels.join(', ') : $t('forms.noFieldPolicyEffects');
	}

	function memberFieldHidden(field: FormField): boolean {
		return memberFieldReadDraft[field.key] === false;
	}

	function memberFieldLocked(field: FormField): boolean {
		return memberFieldWriteDraft[field.key] === false;
	}

	function fieldHasConditionalHidden(field: FormField): boolean {
		return Boolean(field.conditional?.hidden_when);
	}

	function fieldHasConditionalReadOnly(field: FormField): boolean {
		return Boolean(field.conditional?.read_only_when);
	}

	function recordFieldHiddenByCondition(field: FormField, record: FormRecord): boolean {
		return fieldHasConditionalHidden(field) && fieldHiddenByCondition(field, record.values);
	}

	function recordFieldReadOnlyByCondition(field: FormField, record: FormRecord): boolean {
		return (
			fieldHasConditionalReadOnly(field) &&
			fieldConditionMatches(field.conditional?.read_only_when, record.values)
		);
	}

	function recordsFieldHiddenByCondition(field: FormField | null, sourceRecords: FormRecord[]): boolean {
		return Boolean(
			field &&
				fieldHasConditionalHidden(field) &&
				sourceRecords.some((record) => recordFieldHiddenByCondition(field, record))
		);
	}

	function recordsFieldReadOnlyByCondition(field: FormField | null, sourceRecords: FormRecord[]): boolean {
		return Boolean(
			field &&
				fieldHasConditionalReadOnly(field) &&
				sourceRecords.some((record) => recordFieldReadOnlyByCondition(field, record))
		);
	}

	function fieldType(field: FormField): string {
		return field.type ?? 'text';
	}

	function fieldOptions(field: FormField): string[] {
		return field.options ?? [];
	}

	function fieldDisabledOptions(field: FormField): string[] {
		return field.option_disabled ?? [];
	}

	function fieldArchivedOptions(field: FormField): string[] {
		return field.option_archived ?? [];
	}

	function optionDisabled(field: FormField, option: string): boolean {
		return fieldDisabledOptions(field).includes(option);
	}

	function optionArchived(field: FormField, option: string): boolean {
		return fieldArchivedOptions(field).includes(option);
	}

	function recordSelectOptions(field: FormField, draft: Record<string, unknown>): string[] {
		const selected = new Set<string>();
		const value = draft[field.key];
		if (typeof value === 'string') selected.add(value);
		if (Array.isArray(value)) {
			for (const item of value) {
				if (typeof item === 'string') selected.add(item);
			}
		}
		return fieldOptions(field).filter((option) => !optionArchived(field, option) || selected.has(option));
	}

	function optionDescription(field: FormField, option: string): string {
		return field.option_descriptions?.[option]?.trim() ?? field.option_descriptions?.[option.trim()]?.trim() ?? '';
	}

	function optionGroup(field: FormField, option: string): string {
		return field.option_groups?.[option]?.trim() ?? field.option_groups?.[option.trim()]?.trim() ?? '';
	}

	function optionDisplayLabel(field: FormField, option: string): string {
		const group = optionGroup(field, option);
		return group ? `${group} / ${option}` : option;
	}

	function sanitizeOptionColor(value: unknown): string {
		if (typeof value !== 'string') return '';
		const trimmed = value.trim();
		if (/^#[0-9a-f]{6}$/i.test(trimmed) || /^#[0-9a-f]{3}$/i.test(trimmed)) return trimmed;
		return '';
	}

	function fieldPlaceholder(field: FormField): string {
		if (field.placeholder) return field.placeholder;
		if (fieldType(field) === 'amount') return field.amount?.currency ?? 'CNY';
		if (fieldType(field) === 'multi_select') return fieldOptions(field).join(', ');
		return '';
	}

	function viewColumns(view: FormView | null): string[] {
		const columns = view?.config?.columns;
		return Array.isArray(columns)
			? columns.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
			: [];
	}

	function viewConfigStringArray(view: FormView | null, key: string): string[] {
		const value = view?.config?.[key];
		return Array.isArray(value)
			? value.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
			: [];
	}

	function orderedFieldsByKeys(baseFields: FormField[], fieldKeys: string[]): FormField[] {
		if (fieldKeys.length === 0) return baseFields;
		const byKey = new Map(baseFields.map((field) => [field.key, field]));
		const ordered = fieldKeys
			.map((key) => byKey.get(key))
			.filter((field): field is FormField => Boolean(field));
		const orderedKeys = new Set(ordered.map((field) => field.key));
		return [...ordered, ...baseFields.filter((field) => !orderedKeys.has(field.key))];
	}

	function viewCardFieldKeys(view: FormView | null): string[] {
		return viewConfigStringArray(view, 'card_fields');
	}

	function orderedCardFieldsForView(view: FormView | null): FormField[] {
		return orderedFieldsByKeys(visibleFields, viewCardFieldKeys(view));
	}

	function viewConfigString(view: FormView | null, key: string): string {
		const value = view?.config?.[key];
		return typeof value === 'string' ? value.trim() : '';
	}

	function viewConfigBoolean(view: FormView | null, key: string): boolean {
		return view?.config?.[key] === true;
	}

	function viewConfigRecord(view: FormView | null, key: string): Record<string, unknown> | null {
		const value = view?.config?.[key];
		return isRecord(value) ? value : null;
	}

	function isIsoDate(value: string): boolean {
		return /^\d{4}-\d{2}-\d{2}$/.test(value);
	}

	function calendarExceptionDateMap(value: unknown): CalendarExceptionDateMap {
		if (!isRecord(value)) return {};
		const next: CalendarExceptionDateMap = {};
		for (const [recordId, dates] of Object.entries(value)) {
			if (!recordId.trim() || !Array.isArray(dates)) continue;
			const uniqueDates = Array.from(
				new Set(
					dates
						.filter((date): date is string => typeof date === 'string' && isIsoDate(date.trim()))
						.map((date) => date.trim())
				)
			).sort();
			if (uniqueDates.length > 0) next[recordId] = uniqueDates;
		}
		return next;
	}

	function viewIsDefault(view: FormView | null): boolean {
		return view?.config?.is_default === true;
	}

	function viewVisibility(view: FormView | null): ViewVisibility {
		return view?.config?.visibility === 'private' ? 'private' : 'shared';
	}

	function viewOptionLabel(view: FormView): string {
		const badges = [];
		if (viewIsDefault(view)) badges.push($t('forms.defaultViewBadge'));
		if (viewVisibility(view) === 'private') badges.push($t('forms.privateViewBadge'));
		return badges.length > 0 ? `${view.name} · ${badges.join(' · ')}` : view.name;
	}

	function sortViewsByDefault(views: FormView[]): FormView[] {
		return [...views].sort((left, right) => {
			if (viewIsDefault(left) === viewIsDefault(right)) {
				return new Date(left.created_at).getTime() - new Date(right.created_at).getTime();
			}
			return viewIsDefault(left) ? -1 : 1;
		});
	}

	function normalizeLocalDefaultViews(nextViews: FormView[], defaultViewId: string): FormView[] {
		return sortViewsByDefault(
			nextViews.map((view) =>
				view.id === defaultViewId
					? view
					: { ...view, config: { ...view.config, is_default: false } }
			)
		);
	}

	function viewFilterOperator(value: unknown): ViewFilterOperator {
		return value === 'equals' || value === 'not_equals' || value === 'not_empty'
			? value
			: 'contains';
	}

	function viewFilterLogic(value: unknown): ViewFilterLogic {
		return value === 'any' || value === 'or' ? 'any' : 'all';
	}

	function viewSortDirection(value: unknown): ViewSortDirection {
		return value === 'desc' ? 'desc' : 'asc';
	}

	function viewCalendarRange(value: unknown): ViewCalendarRange {
		return value === 'month' ? 'month' : 'week';
	}

	function viewCalendarDayScope(value: unknown): ViewCalendarDayScope {
		return value === 'workweek' ? 'workweek' : 'all';
	}

	function viewCalendarLayout(value: unknown): ViewCalendarLayout {
		return value === 'agenda' ? 'agenda' : 'grid';
	}

	function viewGanttScale(value: unknown): GanttScale {
		return value === 'week' || value === 'month' ? value : 'day';
	}

	function viewPivotAggregate(value: unknown): PivotAggregate {
		if (value === 'count' || value === 'avg') return value;
		return 'sum';
	}

	function viewPivotTotalsMode(value: unknown): PivotTotalsMode {
		if (value === 'rows' || value === 'columns' || value === 'none') return value;
		return 'all';
	}

	function viewPivotValueDisplay(value: unknown): PivotValueDisplay {
		if (value === 'percent_row' || value === 'percent_column' || value === 'percent_total') {
			return value;
		}
		return 'value';
	}

	function viewPivotSortMode(value: unknown): PivotSortMode {
		if (
			value === 'row_total_desc' ||
			value === 'row_total_asc' ||
			value === 'column_total_desc' ||
			value === 'column_total_asc'
		) {
			return value;
		}
		return 'label';
	}

	function viewPivotRowLimit(value: unknown): PivotRowLimit {
		const numeric = typeof value === 'number' ? value : Number(value);
		if (numeric === 2 || numeric === 5 || numeric === 10) return numeric;
		return 0;
	}

	function viewPivotColumnLimit(value: unknown): PivotColumnLimit {
		const numeric = typeof value === 'number' ? value : Number(value);
		if (numeric === 2 || numeric === 5 || numeric === 10) return numeric;
		return 0;
	}

	function pivotShowRowTotals(mode: PivotTotalsMode): boolean {
		return mode === 'all' || mode === 'rows';
	}

	function pivotShowColumnTotals(mode: PivotTotalsMode): boolean {
		return mode === 'all' || mode === 'columns';
	}

	function pivotCellIntensity(value: number, table: PivotTable): number {
		if (!table.heatmap || table.maxCellValue <= 0) return 0;
		const ratio = Math.min(Math.abs(value) / table.maxCellValue, 1);
		return Math.round(ratio * 100);
	}

	function pivotCellHeatmapStyle(value: number, table: PivotTable): string {
		const intensity = pivotCellIntensity(value, table);
		if (intensity <= 0) return '';
		const opacity = (0.06 + (intensity / 100) * 0.18).toFixed(3);
		return `background-color: rgba(37, 99, 235, ${opacity});`;
	}

	function viewCalendarRecurrence(value: unknown): ViewCalendarRecurrence {
		if (value === 'daily' || value === 'weekly' || value === 'monthly') return value;
		return 'none';
	}

	function viewCardLayout(value: unknown): CardLayoutMode {
		if (value === 'list') return 'list';
		if (value === 'compact') return 'compact';
		return value === 'gallery' ? 'gallery' : 'standard';
	}

	function viewCardCoverAspect(value: unknown): CardCoverAspect {
		if (value === 'wide' || value === 'square' || value === 'portrait') return value;
		return 'auto';
	}

	function viewCardCoverFit(value: unknown): CardCoverFit {
		return value === 'contain' ? 'contain' : 'cover';
	}

	function viewCardCoverPosition(value: unknown): CardCoverPosition {
		if (value === 'left' || value === 'hidden') return value;
		return 'top';
	}

	function cardCoverObjectClass(fit: CardCoverFit): string {
		return fit === 'contain' ? 'bg-slate-100 object-contain dark:bg-slate-800' : 'object-cover';
	}

	function cardCoverImageClass(layout: CardLayoutMode, aspect: CardCoverAspect, fit: CardCoverFit): string {
		const resolvedAspect =
			aspect === 'auto'
				? layout === 'gallery'
					? 'aspect-[4/3]'
					: 'aspect-[16/9]'
				: aspect === 'square'
					? 'aspect-square'
					: aspect === 'portrait'
						? 'aspect-[3/4]'
						: 'aspect-[16/9]';
		return `${resolvedAspect} w-full ${cardCoverObjectClass(fit)}`;
	}

	function viewCardFieldLimit(value: unknown): CardFieldLimit {
		const numeric = typeof value === 'number' ? value : Number(value);
		if (numeric === 3 || numeric === 9 || numeric === 99) return numeric;
		return 6;
	}

	function viewTimelineSource(value: unknown): ViewTimelineSource {
		return value === 'events' ? 'events' : 'records';
	}

	function timelineEventLayout(value: unknown): TimelineEventLayout {
		return value === 'detailed' ? 'detailed' : 'summary';
	}

	function viewFilterConfigFromRecord(filter: Record<string, unknown> | null): {
		field: FormField;
		operator: ViewFilterOperator;
		value: string;
	} | null {
		if (!filter || typeof filter.field !== 'string') return null;
		const field = fieldByKey(filter.field);
		if (!field) return null;
		return {
			field,
			operator: viewFilterOperator(filter.operator),
			value: typeof filter.value === 'string' ? filter.value : ''
		};
	}

	function viewFilterNodeDisabled(value: Record<string, unknown> | null): boolean {
		return value?.disabled === true || value?.enabled === false;
	}

	type ParsedViewFilter = {
		field: FormField;
		operator: ViewFilterOperator;
		value: string;
	};

	type ParsedViewFilterGroup = {
		logic: ViewFilterLogic;
		filters: ParsedViewFilter[];
	};

	function parsedViewFilterExpressionFromValue(value: unknown): ParsedViewFilterExpression | null {
		if (!isRecord(value)) return null;
		const filterRecord = isRecord(value.filter) ? value.filter : null;
		const filter = viewFilterConfigFromRecord(filterRecord);
		if (filter) {
			return {
				kind: 'condition',
				filter,
				disabled: viewFilterNodeDisabled(value) || viewFilterNodeDisabled(filterRecord)
			};
		}
		if (typeof value.field === 'string') {
			const inlineFilter = viewFilterConfigFromRecord(value);
			return inlineFilter
				? { kind: 'condition', filter: inlineFilter, disabled: viewFilterNodeDisabled(value) }
				: null;
		}
		const rawChildren = Array.isArray(value.children)
			? value.children
			: Array.isArray(value.expressions)
				? value.expressions
				: [];
		const children = rawChildren
			.map(parsedViewFilterExpressionFromValue)
			.filter((child): child is ParsedViewFilterExpression => Boolean(child));
		return children.length > 0
			? {
					kind: 'group',
					logic: viewFilterLogic(value.logic),
					children,
					disabled: viewFilterNodeDisabled(value)
				}
			: null;
	}

	function viewFilterGroups(): ParsedViewFilterGroup[] {
		const config = selectedView?.config ?? {};
		const groups = Array.isArray(config.filter_groups) ? config.filter_groups : [];
		const parsedGroups = groups
			.map((group) => {
				if (!isRecord(group) || !Array.isArray(group.filters)) return null;
				const filters = group.filters
					.map((filter) => (isRecord(filter) ? viewFilterConfigFromRecord(filter) : null))
					.filter((filter): filter is ParsedViewFilter => Boolean(filter));
				return filters.length > 0 ? { logic: viewFilterLogic(group.logic), filters } : null;
			})
			.filter((group): group is ParsedViewFilterGroup => Boolean(group));
		if (parsedGroups.length > 0) return parsedGroups;
		const filters = Array.isArray(config.filters) ? config.filters : [];
		const parsed = filters
			.map((filter) => (isRecord(filter) ? viewFilterConfigFromRecord(filter) : null))
			.filter((filter): filter is ParsedViewFilter => Boolean(filter));
		if (parsed.length > 0) return [{ logic: 'all', filters: parsed }];
		const legacyFilter = viewFilterConfigFromRecord(viewConfigRecord(selectedView, 'filter'));
		return legacyFilter ? [{ logic: 'all', filters: [legacyFilter] }] : [];
	}

	function viewFilterExpression(): ParsedViewFilterExpression | null {
		const config = selectedView?.config ?? {};
		const expression = parsedViewFilterExpressionFromValue(config.filter_expression);
		if (expression) return expression;
		const groups = viewFilterGroups();
		if (groups.length === 0) return null;
		return {
			kind: 'group',
			logic: 'all',
			children: groups.map((group) => ({
				kind: 'group',
				logic: group.logic,
				children: group.filters.map((filter) => ({ kind: 'condition', filter }))
			}))
		};
	}

	function viewSortConfig(): { field: FormField; direction: ViewSortDirection } | null {
		const sort = viewConfigRecord(selectedView, 'sort');
		if (!sort || typeof sort.field !== 'string') return null;
		const field = fieldByKey(sort.field);
		if (!field) return null;
		return { field, direction: viewSortDirection(sort.direction) };
	}

	function fieldByKey(fieldKey: string): FormField | null {
		return fields.find((field) => field.key === fieldKey) ?? null;
	}

	function detailLayoutFieldKeys(section: Record<string, unknown>): string[] {
		const rawFields = Array.isArray(section.fields) ? section.fields : [];
		return rawFields
			.map((field) => {
				if (typeof field === 'string') return field;
				if (isRecord(field) && typeof field.key === 'string') return field.key;
				return '';
			})
			.filter((key) => key.trim().length > 0);
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

	function detailLayoutSectionGridClass(columns: 1 | 2 | 3, density: DetailLayoutSectionDensity = 'comfortable'): string {
		const gap = density === 'compact' ? 'gap-2' : 'gap-3';
		if (columns === 1) return `grid ${gap}`;
		if (columns === 3) return `grid ${gap} sm:grid-cols-2 xl:grid-cols-3`;
		return `grid ${gap} sm:grid-cols-2`;
	}

	function detailLayoutSectionFieldClass(field: FormField, columns: 1 | 2 | 3): string {
		if (fieldType(field) !== 'child_table' || columns === 1) return 'min-w-0';
		return columns === 3 ? 'min-w-0 sm:col-span-2 xl:col-span-3' : 'min-w-0 sm:col-span-2';
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

	function detailLayoutSectionFieldsForRecord(
		section: DetailLayoutSection,
		record: FormRecord
	): FormField[] {
		if (!section.hideEmptyFields) return section.fields;
		return section.fields.filter((field) => !recordValueIsEmpty(record.values[field.key]));
	}

	function detailLayoutSections(): DetailLayoutSection[] {
		const detailFields = fields.filter((field) => field.visibility?.detail !== false);
		const byKey = new Map(detailFields.map((field) => [field.key, field]));
		const usedFieldKeys = new Set<string>();
		const rawSections = Array.isArray(selectedForm?.detail_layout?.sections)
			? selectedForm.detail_layout.sections
			: [];
		const sections = rawSections
			.filter(isRecord)
			.map((section, index) => {
				const layoutFields = detailLayoutFieldKeys(section)
					.map((key) => byKey.get(key))
					.filter((field): field is FormField => Boolean(field));
				if (layoutFields.length === 0) return null;
				for (const field of layoutFields) usedFieldKeys.add(field.key);
				const key =
					typeof section.key === 'string' && section.key.trim().length > 0
						? section.key.trim()
						: `section_${index + 1}`;
				const title =
					typeof section.title === 'string' && section.title.trim().length > 0
						? section.title.trim()
						: typeof section.name === 'string' && section.name.trim().length > 0
							? section.name.trim()
							: $t('forms.detailSectionMain');
				return {
					key,
					title,
					description: detailLayoutSectionDescription(section.description),
					columns: detailLayoutSectionColumns(section.columns),
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
				columns: 2,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: unplacedFields
			});
		}
		if (sections.length > 0) return sections;
		return [
			{
				key: 'main',
				title: $t('forms.detailSectionMain'),
				description: '',
				columns: 2,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: detailFields
			}
		];
	}

	function applyViewRecordConfig(sourceRecords: FormRecord[]): FormRecord[] {
		let nextRecords = [...sourceRecords];
		const filterExpression = viewFilterExpression();
		if (filterExpression) {
			nextRecords = nextRecords.filter((record) => {
				const matches = recordMatchesViewFilterExpression(record, filterExpression);
				return matches !== false;
			});
		}
		if (focusedRecordIds.length > 0) {
			const focusedIds = new Set(focusedRecordIds);
			nextRecords = nextRecords.filter((record) => focusedIds.has(record.id));
		}

		const sort = viewSortConfig();
		if (sort) {
			nextRecords.sort((left, right) => compareRecordField(left, right, sort.field, sort.direction));
		}
		return nextRecords;
	}

	function recordMatchesViewFilterExpression(
		record: FormRecord,
		expression: ParsedViewFilterExpression
	): boolean | null {
		if (expression.disabled) return null;
		if (expression.kind === 'condition') {
			return recordMatchesViewFilter(record, expression.filter);
		}
		const matches = expression.children
			.map((child) => recordMatchesViewFilterExpression(record, child))
			.filter((match): match is boolean => match !== null);
		if (matches.length === 0) return null;
		return expression.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
	}

	function recordMatchesViewFilter(record: FormRecord, filter: ParsedViewFilter): boolean {
		const expected = filter.value.trim().toLowerCase();
		const actual = recordComparableText(record, filter.field);
		if (filter.operator === 'not_empty') return actual.length > 0;
		if (!expected) return true;
		if (filter.operator === 'equals') return actual.toLowerCase() === expected;
		if (filter.operator === 'not_equals') return actual.toLowerCase() !== expected;
		return actual.toLowerCase().includes(expected);
	}

	function parsedViewDraftFilter(filter: ViewFilterDraft): ParsedViewFilter | null {
		if (!filter.field) return null;
		if (filter.operator !== 'not_empty' && !filter.value.trim()) return null;
		const field = fieldByKey(filter.field);
		if (!field) return null;
		return {
			field,
			operator: filter.operator,
			value: filter.operator === 'not_empty' ? '' : filter.value.trim()
		};
	}

	function viewDraftFilterMatchCount(filter: ViewFilterDraft): number | null {
		const parsed = parsedViewDraftFilter(filter);
		if (!parsed) return null;
		return filterPreviewRecords.filter((record) => recordMatchesViewFilter(record, parsed)).length;
	}

	function viewDraftFilterGroupMatchCount(group: ViewFilterGroupDraft): number | null {
		const parsedFilters = group.filters
			.filter((filter) => !filter.disabled)
			.map(parsedViewDraftFilter)
			.filter((filter): filter is ParsedViewFilter => Boolean(filter));
		if (parsedFilters.length === 0) return null;
		return filterPreviewRecords.filter((record) => {
			const matches = parsedFilters.map((filter) => recordMatchesViewFilter(record, filter));
			return group.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
		}).length;
	}

	function viewDraftFilterExpressionRecords(): FormRecord[] | null {
		const groups = visibleViewDraftFilterGroups()
			.filter((group) => !group.disabled)
			.map((group) => ({
				logic: group.logic,
				filters: group.filters
					.filter((filter) => !filter.disabled)
					.map(parsedViewDraftFilter)
					.filter((filter): filter is ParsedViewFilter => Boolean(filter))
			}))
			.filter((group) => group.filters.length > 0);
		if (groups.length === 0) return null;
		return filterPreviewRecords.filter((record) => {
			const groupMatches = groups.map((group) => {
				const matches = group.filters.map((filter) => recordMatchesViewFilter(record, filter));
				return group.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
			});
			return viewDraft.filterRootLogic === 'any'
				? groupMatches.some(Boolean)
				: groupMatches.every(Boolean);
		});
	}

	function viewDraftFilterExpressionMatchCount(): number | null {
		return viewDraftFilterExpressionRecords()?.length ?? null;
	}

	function viewDraftFilterExpressionRecordIdsText(records: FormRecord[] | null): string {
		return records?.map((record) => record.id).join('\n') ?? '';
	}

	function csvCell(value: string): string {
		return `"${value.replaceAll('"', '""')}"`;
	}

	function recordsCsv(records: FormRecord[]): string {
		const exportFields = visibleFields.length > 0 ? visibleFields : fields.slice(0, 8);
		const header = ['Record ID', 'Title', ...exportFields.map(fieldLabel)].map(csvCell).join(',');
		const rows = records.map((record) =>
			[
				record.id,
				record.title,
				...exportFields.map((field) => {
					const value = displayValue(record.values[field.key]);
					return value === '-' ? '' : value;
				})
			]
				.map(csvCell)
				.join(',')
		);
		return [header, ...rows].join('\n');
	}

	function timelineEventsCsv(events: BusinessEvent[]): string {
		const header = [
			'Event ID',
			'Type',
			'Title',
			'Subtitle',
			'Aggregate Type',
			'Aggregate ID',
			'Actor',
			'Created At',
			'Payload',
			'Metadata',
			'Source'
		]
			.map(csvCell)
			.join(',');
		const rows = events.map((event) =>
			[
				event.id,
				event.event_type,
				timelineEventTitle(event),
				timelineEventSubtitle(event),
				event.aggregate_type,
				event.aggregate_id,
				event.actor_id ?? '',
				event.created_at,
				timelineEventJson(event.payload),
				timelineEventJson(event.metadata),
				timelineEventJson(event.source)
			]
				.map(csvCell)
				.join(',')
		);
		return [header, ...rows].join('\n');
	}

	function viewDraftFilterExpressionCsv(records: FormRecord[]): string {
		return recordsCsv(records);
	}

	function viewDraftFilterExpressionConfig(): Record<string, unknown> | null {
		return viewFilterExpressionConfigFromGroups(validViewFilterGroups());
	}

	function viewDraftFilterExpressionJsonText(): string {
		const expression = viewDraftFilterExpressionConfig();
		return expression ? JSON.stringify(expression, null, 2) : '';
	}

	function filterOperatorSummary(operator: ViewFilterOperator): string {
		if (operator === 'equals') return 'equals';
		if (operator === 'not_equals') return 'does not equal';
		if (operator === 'not_empty') return 'is not empty';
		return 'contains';
	}

	function viewDraftFilterExpressionSummaryText(): string {
		const groups = validViewFilterGroups();
		if (groups.length === 0) return '';
		const rootLogic = viewDraft.filterRootLogic === 'any' ? 'ANY group' : 'ALL groups';
		const lines = [`Filter expression: ${rootLogic}`];
		for (const [groupIndex, group] of groups.entries()) {
			const groupLogic = group.logic === 'any' ? 'ANY condition' : 'ALL conditions';
			lines.push(`Group ${groupIndex + 1}${group.disabled ? ' [disabled]' : ''}: ${groupLogic}`);
			for (const [filterIndex, filter] of group.filters.entries()) {
				const field = fieldByKey(filter.field);
				const label = field ? fieldLabel(field) : filter.field;
				const value = filter.operator === 'not_empty' ? '' : ` "${filter.value}"`;
				lines.push(
					`  ${groupIndex + 1}.${filterIndex + 1}${filter.disabled ? ' [disabled]' : ''}: ${label} ${filterOperatorSummary(filter.operator)}${value}`
				);
			}
		}
		return lines.join('\n');
	}

	function viewDraftFilterExpressionJson(records: FormRecord[]): string {
		return JSON.stringify(
			{
				form_id: selectedForm?.id ?? null,
				form_key: selectedForm?.key ?? null,
				record_count: records.length,
				record_ids: records.map((record) => record.id),
				filter_expression: viewDraftFilterExpressionConfig(),
				records: records.map((record) => cardRecordDetailsPayload(record))
			},
			null,
			2
		);
	}

	async function copyViewDraftFilterExpressionRecordIds(records: FormRecord[] | null) {
		const recordIds = viewDraftFilterExpressionRecordIdsText(records);
		if (!recordIds) return;
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(recordIds);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}
		copiedFilterExpressionRecordIds = recordIds;
		toast.success($t('forms.filterExpressionRecordIdsCopied'));
	}

	async function copyViewDraftFilterExpressionJson(expressionJson: string) {
		if (!expressionJson) return;
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(expressionJson);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}
		copiedFilterExpressionJson = expressionJson;
		toast.success($t('forms.filterExpressionJsonCopied'));
	}

	async function copyViewDraftFilterExpressionSummary(expressionSummary: string) {
		if (!expressionSummary) return;
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(expressionSummary);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}
		copiedFilterExpressionSummary = expressionSummary;
		toast.success($t('forms.filterExpressionSummaryCopied'));
	}

	async function copyViewDraftFilterExpressionJsonRecords(records: FormRecord[] | null) {
		if (!records || records.length === 0) return;
		const json = viewDraftFilterExpressionJson(records);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(json);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}
		copiedFilterExpressionJsonRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success($t('forms.filterExpressionRecordsJsonCopied'));
	}

	async function copyViewDraftFilterExpressionCsvRecords(records: FormRecord[] | null) {
		if (!records || records.length === 0) return;
		const csv = viewDraftFilterExpressionCsv(records);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(csv);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}
		copiedFilterExpressionCsvRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success($t('forms.filterExpressionRecordsCsvCopied'));
	}

	function exportViewDraftFilterExpressionConfigJson(expressionJson: string) {
		if (!selectedForm || !expressionJson) return;
		const blob = new Blob([expressionJson], { type: 'application/json;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-filter-expression.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedFilterExpressionJson = expressionJson;
		toast.success($t('forms.filterExpressionJsonExported'));
	}

	function exportViewDraftFilterExpressionSummary(expressionSummary: string) {
		if (!selectedForm || !expressionSummary) return;
		const blob = new Blob([expressionSummary], { type: 'text/plain;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-filter-expression-summary.txt`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedFilterExpressionSummary = expressionSummary;
		toast.success($t('forms.filterExpressionSummaryExported'));
	}

	function focusViewDraftFilterExpressionRecords(records: FormRecord[] | null) {
		const recordIds = records?.map((record) => record.id) ?? [];
		if (recordIds.length === 0) return;
		focusedRecordIds = recordIds;
		toast.success($t('forms.focusedRecords', { values: { count: recordIds.length } }));
	}

	function exportViewDraftFilterExpressionCsv(records: FormRecord[] | null) {
		if (!selectedForm || !records || records.length === 0) return;
		const csv = viewDraftFilterExpressionCsv(records);
		const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-filter-expression-matches.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedFilterExpressionRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success($t('forms.filterExpressionRecordsExported'));
	}

	function exportViewDraftFilterExpressionJson(records: FormRecord[] | null) {
		if (!selectedForm || !records || records.length === 0) return;
		const json = viewDraftFilterExpressionJson(records);
		const blob = new Blob([json], { type: 'application/json;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-filter-expression-matches.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedFilterExpressionJsonRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success($t('forms.filterExpressionRecordsJsonExported'));
	}

	function recordComparableText(record: FormRecord, field: FormField): string {
		const value = displayValue(record.values[field.key]);
		return value === '-' ? '' : value;
	}

	function cardFieldHasDisplayValue(record: FormRecord, field: FormField): boolean {
		const value = displayValue(record.values[field.key]).trim();
		return value.length > 0 && value !== '-';
	}

	function cardDisplayFields(record: FormRecord): FormField[] {
		if (!selectedCardHideEmptyFields) return cardFields;
		return cardFields.filter((field) => cardFieldHasDisplayValue(record, field));
	}

	function recordSortValue(record: FormRecord, field: FormField): string | number {
		const raw = record.values[field.key];
		const type = field.type ?? 'text';
		if (['amount', 'number', 'integer'].includes(type)) {
			if (typeof raw === 'number') return raw;
			if (typeof raw === 'string') {
				const parsed = Number(raw);
				return Number.isNaN(parsed) ? raw : parsed;
			}
			if (isRecord(raw)) {
				const value = typeof raw.decimal === 'string' ? raw.decimal : raw.value;
				if (typeof value === 'number') return value;
				if (typeof value === 'string') {
					const parsed = Number(value);
					return Number.isNaN(parsed) ? value : parsed;
				}
			}
		}
		if (['date', 'datetime'].includes(type)) {
			const time = new Date(recordDateSource(record, field)).getTime();
			return Number.isNaN(time) ? 0 : time;
		}
		return recordComparableText(record, field).toLowerCase();
	}

	function compareRecordField(
		left: FormRecord,
		right: FormRecord,
		field: FormField,
		direction: ViewSortDirection
	): number {
		const leftValue = recordSortValue(left, field);
		const rightValue = recordSortValue(right, field);
		const multiplier = direction === 'desc' ? -1 : 1;
		if (typeof leftValue === 'number' && typeof rightValue === 'number') {
			return (leftValue - rightValue) * multiplier;
		}
		return String(leftValue).localeCompare(String(rightValue)) * multiplier;
	}

	function recordNumericValue(record: FormRecord, field: FormField | null): number {
		if (!field) return 1;
		const raw = record.values[field.key];
		if (typeof raw === 'number') return Number.isFinite(raw) ? raw : 0;
		if (typeof raw === 'string') {
			const parsed = Number(raw);
			return Number.isFinite(parsed) ? parsed : 0;
		}
		if (isRecord(raw)) {
			const value = typeof raw.decimal === 'string' ? raw.decimal : raw.value;
			if (typeof value === 'number') return Number.isFinite(value) ? value : 0;
			if (typeof value === 'string') {
				const parsed = Number(value);
				return Number.isFinite(parsed) ? parsed : 0;
			}
		}
		return 0;
	}

	function recordAmountCurrency(record: FormRecord, field: FormField | null): string {
		if ((field?.type ?? 'text') !== 'amount') return '';
		const configured = field?.amount?.currency;
		if (configured) return configured;
		const raw = field ? record.values[field.key] : null;
		if (isRecord(raw) && typeof raw.currency === 'string') return raw.currency;
		return '';
	}

	function pivotFieldBucket(record: FormRecord, field: FormField | null): { key: string; label: string } {
		const label = pivotFieldLabel(record, field);
		if (!field) return { key: '__unspecified__', label };
		const raw = record.values[field.key];
		if (typeof raw === 'string' || typeof raw === 'number' || typeof raw === 'boolean') {
			return { key: String(raw).toLowerCase(), label };
		}
		if (isRecord(raw)) {
			if (typeof raw.record_id === 'string') return { key: raw.record_id, label };
			if (typeof raw.value === 'string' || typeof raw.value === 'number') {
				return { key: String(raw.value).toLowerCase(), label };
			}
		}
		return { key: label.toLowerCase(), label };
	}

	function pivotFieldLabel(record: FormRecord, field: FormField | null): string {
		if (!field) return $t('forms.unspecified');
		const value = displayValue(record.values[field.key]);
		return value && value !== '-' ? value : $t('forms.unspecified');
	}

	function formatPivotValue(
		value: number,
		field: FormField | null,
		currency = '',
		aggregate: PivotAggregate = 'sum'
	): string {
		if (aggregate !== 'count' && (field?.type ?? 'number') === 'amount') {
			const scale = typeof field?.amount?.scale === 'number' ? field.amount.scale : 2;
			const formatted = value.toFixed(scale);
			return currency ? `${formatted} ${currency}` : formatted;
		}
		const formatted = Number.isInteger(value) ? String(value) : value.toFixed(2);
		return formatted;
	}

	function formatPivotPercent(value: number, denominator: number): string {
		if (Math.abs(denominator) < Number.EPSILON) return '0.00%';
		return `${((value / denominator) * 100).toFixed(2)}%`;
	}

	function formatPivotDisplayValue(
		value: number,
		rowTotal = 0,
		columnTotal = 0,
		table: PivotTable
	): string {
		if (table.valueDisplay === 'percent_row') return formatPivotPercent(value, rowTotal);
		if (table.valueDisplay === 'percent_column') return formatPivotPercent(value, columnTotal);
		if (table.valueDisplay === 'percent_total') return formatPivotPercent(value, table.grandTotal);
		return formatPivotValue(value, table.valueField, table.valueCurrency, table.aggregate);
	}

	function formatPivotTotalDisplayValue(
		value: number,
		totalKind: 'row' | 'column' | 'grand',
		table: PivotTable
	): string {
		if (table.valueDisplay === 'percent_row' && totalKind === 'row') {
			return formatPivotPercent(value, value);
		}
		if (table.valueDisplay === 'percent_column' && totalKind === 'column') {
			return formatPivotPercent(value, value);
		}
		if (table.valueDisplay !== 'value') return formatPivotPercent(value, table.grandTotal);
		return formatPivotValue(value, table.valueField, table.valueCurrency, table.aggregate);
	}

	function pivotAggregateValue(sum: number, count: number, aggregate: PivotAggregate): number {
		if (aggregate === 'count') return count;
		if (aggregate === 'avg') return count > 0 ? sum / count : 0;
		return sum;
	}

	function sortPivotBuckets<T extends { label: string; total: number }>(
		items: T[],
		sortMode: PivotSortMode,
		axis: 'row' | 'column'
	): T[] {
		const totalSort =
			(axis === 'row' && (sortMode === 'row_total_desc' || sortMode === 'row_total_asc')) ||
			(axis === 'column' &&
				(sortMode === 'column_total_desc' || sortMode === 'column_total_asc'));
		return [...items].sort((left, right) => {
			if (totalSort) {
				const totalDelta =
					sortMode.endsWith('_desc') ? right.total - left.total : left.total - right.total;
				if (totalDelta !== 0) return totalDelta;
			}
			return left.label.localeCompare(right.label);
		});
	}

	function pivotConfiguredBuckets(field: FormField | null): Array<{ key: string; label: string }> {
		if (field?.type !== 'single_select' || !Array.isArray(field.options)) return [];
		const buckets = new Map<string, { key: string; label: string }>();
		for (const option of field.options) {
			const label = option.trim();
			if (!label) continue;
			const key = label.toLowerCase();
			if (!buckets.has(key)) buckets.set(key, { key, label });
		}
		return Array.from(buckets.values());
	}

	function buildPivotTable(sourceRecords: FormRecord[]): PivotTable {
		const rowField = viewPivotRowField();
		const columnField = viewPivotColumnField();
		const valueField = viewPivotValueField();
		const aggregate = viewPivotAggregate(selectedView?.config?.pivot_aggregate);
		const totalsMode = viewPivotTotalsMode(selectedView?.config?.pivot_totals);
		const valueDisplay = viewPivotValueDisplay(selectedView?.config?.pivot_value_display);
		const sortMode = viewPivotSortMode(selectedView?.config?.pivot_sort);
		const emptyBuckets = viewConfigBoolean(selectedView, 'pivot_empty_buckets');
		const hideZeroBuckets = viewConfigBoolean(selectedView, 'pivot_hide_zero_buckets');
		const showCounts = viewConfigBoolean(selectedView, 'pivot_show_counts');
		const heatmap = viewConfigBoolean(selectedView, 'pivot_heatmap');
		const rowLimit = viewPivotRowLimit(selectedView?.config?.pivot_row_limit);
		const columnLimit = viewPivotColumnLimit(selectedView?.config?.pivot_column_limit);
		if (!rowField || !columnField || !valueField) {
			return {
				rowField,
				columnField,
				valueField,
				valueCurrency: '',
				aggregate,
				totalsMode,
				valueDisplay,
				sortMode,
					emptyBuckets,
					hideZeroBuckets,
					showCounts,
				heatmap,
				rowLimit,
				columnLimit,
				maxCellValue: 0,
				columns: [],
				rows: [],
				grandTotal: 0
			};
		}
		type PivotAccumulator = { sum: number; count: number };
		const rowMap = new Map<
			string,
			{ key: string; label: string; total: PivotAccumulator; cells: Record<string, PivotAccumulator> }
		>();
		const columnMap = new Map<string, { key: string; label: string; total: PivotAccumulator }>();
		const grandTotal: PivotAccumulator = { sum: 0, count: 0 };
		let valueCurrency = '';
		const ensureRow = (bucket: { key: string; label: string }) => {
			if (!rowMap.has(bucket.key)) {
				rowMap.set(bucket.key, {
					key: bucket.key,
					label: bucket.label,
					total: { sum: 0, count: 0 },
					cells: {}
				});
			}
			return rowMap.get(bucket.key);
		};
		const ensureColumn = (bucket: { key: string; label: string }) => {
			if (!columnMap.has(bucket.key)) {
				columnMap.set(bucket.key, {
					key: bucket.key,
					label: bucket.label,
					total: { sum: 0, count: 0 }
				});
			}
			return columnMap.get(bucket.key);
		};
		if (emptyBuckets) {
			for (const bucket of pivotConfiguredBuckets(rowField)) ensureRow(bucket);
			for (const bucket of pivotConfiguredBuckets(columnField)) ensureColumn(bucket);
		}
		for (const record of sourceRecords) {
			const rowBucket = pivotFieldBucket(record, rowField);
			const columnBucket = pivotFieldBucket(record, columnField);
			const value = recordNumericValue(record, valueField);
			if (!valueCurrency) valueCurrency = recordAmountCurrency(record, valueField);
			const row = ensureRow(rowBucket);
			const column = ensureColumn(columnBucket);
			if (!row || !column) continue;
			if (!row.cells[columnBucket.key]) row.cells[columnBucket.key] = { sum: 0, count: 0 };
			row.cells[columnBucket.key].sum += value;
			row.cells[columnBucket.key].count += 1;
			row.total.sum += value;
			row.total.count += 1;
			column.total.sum += value;
			column.total.count += 1;
			grandTotal.sum += value;
			grandTotal.count += 1;
		}
		const rows = sortPivotBuckets(
			Array.from(rowMap.values()).map((row) => ({
				key: row.key,
				label: row.label,
				total: pivotAggregateValue(row.total.sum, row.total.count, aggregate),
				cells: Object.fromEntries(
					Object.entries(row.cells).map(([key, cell]) => [
						key,
						pivotAggregateValue(cell.sum, cell.count, aggregate)
					])
				),
				cellCounts: Object.fromEntries(
					Object.entries(row.cells).map(([key, cell]) => [key, cell.count])
				)
			})),
			sortMode,
			'row'
		);
		const columns = sortPivotBuckets(
			Array.from(columnMap.values()).map((column) => ({
				key: column.key,
				label: column.label,
				total: pivotAggregateValue(column.total.sum, column.total.count, aggregate)
			})),
			sortMode,
			'column'
		);
		const filteredRows = hideZeroBuckets ? rows.filter((row) => Math.abs(row.total) > Number.EPSILON) : rows;
		const filteredColumns = hideZeroBuckets
			? columns.filter((column) => Math.abs(column.total) > Number.EPSILON)
			: columns;
		const visibleColumns = columnLimit > 0 ? filteredColumns.slice(0, columnLimit) : filteredColumns;
		const visibleRows = rowLimit > 0 ? filteredRows.slice(0, rowLimit) : filteredRows;
		const maxCellValue = Math.max(
			0,
			...visibleRows.flatMap((row) =>
				visibleColumns.map((column) => Math.abs(row.cells[column.key] ?? 0))
			)
		);
		return {
			rowField,
			columnField,
			valueField,
			valueCurrency,
			aggregate,
			totalsMode,
			valueDisplay,
				sortMode,
				emptyBuckets,
				hideZeroBuckets,
				showCounts,
			heatmap,
			rowLimit,
			columnLimit,
			maxCellValue,
			columns: visibleColumns,
			rows: visibleRows,
			grandTotal: pivotAggregateValue(grandTotal.sum, grandTotal.count, aggregate)
		};
	}

	function buildPivotDrilldownRecords(sourceRecords: FormRecord[]): FormRecord[] {
		const drilldown = pivotDrilldown;
		if (!drilldown || !pivotTable.rowField || !pivotTable.columnField) return [];
		return sourceRecords.filter((record) => {
			const rowBucket = pivotFieldBucket(record, pivotTable.rowField);
			const columnBucket = pivotFieldBucket(record, pivotTable.columnField);
			const rowMatches = drilldown.rowKey === null || rowBucket.key === drilldown.rowKey;
			const columnMatches = drilldown.columnKey === null || columnBucket.key === drilldown.columnKey;
			return rowMatches && columnMatches;
		});
	}

	function pivotRowRecords(row: { key: string }): FormRecord[] {
		if (!pivotTable.rowField) return [];
		return viewRecords.filter((record) => pivotFieldBucket(record, pivotTable.rowField).key === row.key);
	}

	function pivotColumnRecords(column: { key: string }): FormRecord[] {
		if (!pivotTable.columnField) return [];
		return viewRecords.filter((record) => pivotFieldBucket(record, pivotTable.columnField).key === column.key);
	}

	function pivotCellRecords(row: { key: string }, column: { key: string }): FormRecord[] {
		if (!pivotTable.rowField || !pivotTable.columnField) return [];
		return viewRecords.filter(
			(record) =>
				pivotFieldBucket(record, pivotTable.rowField).key === row.key &&
				pivotFieldBucket(record, pivotTable.columnField).key === column.key
		);
	}

	function selectPivotDrilldown(
		row: { key: string; label: string } | null,
		column: { key: string; label: string } | null
	) {
		pivotDrilldown = {
			rowKey: row?.key ?? null,
			rowLabel: row?.label ?? $t('forms.pivotTotal'),
			columnKey: column?.key ?? null,
			columnLabel: column?.label ?? $t('forms.pivotTotal')
		};
		copiedPivotDrilldownKey = null;
		exportedPivotDrilldownKey = null;
		appliedPivotDrilldownFilterKey = null;
	}

	function pivotDrilldownKey(): string | null {
		if (!pivotDrilldown) return null;
		return `${pivotDrilldown.rowKey ?? '__all__'}:${pivotDrilldown.columnKey ?? '__all__'}`;
	}

	function pivotDrilldownFilterDrafts(): ViewFilterDraft[] {
		if (!pivotDrilldown) return [];
		const filters: ViewFilterDraft[] = [];
		if (
			pivotDrilldown.rowKey !== null &&
			pivotTable.rowField &&
			pivotDrilldown.rowLabel !== $t('forms.unspecified')
		) {
			filters.push({
				field: pivotTable.rowField.key,
				operator: 'equals',
				value: pivotDrilldown.rowLabel
			});
		}
		if (
			pivotDrilldown.columnKey !== null &&
			pivotTable.columnField &&
			pivotDrilldown.columnLabel !== $t('forms.unspecified')
		) {
			filters.push({
				field: pivotTable.columnField.key,
				operator: 'equals',
				value: pivotDrilldown.columnLabel
			});
		}
		return filters;
	}

	function canApplyPivotDrilldownFilters(): boolean {
		const filters = pivotDrilldownFilterDrafts();
		if (filters.length === 0) return false;
		const currentGroups = visibleViewDraftFilterGroups().filter((group) => group.filters.length > 0);
		const currentFilterCount = currentGroups.reduce((count, group) => count + group.filters.length, 0);
		if (currentFilterCount + filters.length > 10) return false;
		if (currentGroups.length < 5) return true;
		const lastGroup = currentGroups[currentGroups.length - 1];
		return Boolean(lastGroup && lastGroup.filters.length + filters.length <= 5);
	}

	function applyPivotDrilldownFilters() {
		if (!canApplyPivotDrilldownFilters()) return;
		const filters = pivotDrilldownFilterDrafts();
		const currentGroups = visibleViewDraftFilterGroups()
			.filter((group) => group.filters.length > 0)
			.map((group) => ({
				logic: group.logic,
				disabled: group.disabled || undefined,
				filters: group.filters.map((filter) => ({ ...filter }))
			}));
		const nextGroups =
			currentGroups.length === 0
				? [{ logic: 'all' as ViewFilterLogic, filters }]
				: currentGroups.length < 5
					? [...currentGroups, { logic: 'all' as ViewFilterLogic, filters }]
					: currentGroups.map((group, index) =>
							index === currentGroups.length - 1
								? { ...group, filters: [...group.filters, ...filters] }
								: group
						);
		setViewDraftFilterGroups(nextGroups);
		updateViewDraftFilterRootLogic('all');
		appliedPivotDrilldownFilterKey = pivotDrilldownKey();
		toast.success($t('forms.pivotDrilldownFiltersApplied'));
	}

	async function copyPivotCellValue(
		row: { key: string; label: string; total: number },
		column: { key: string; label: string; total: number },
		value: number
	) {
		const text = formatPivotDisplayValue(value, row.total, column.total, pivotTable);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(text);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep pivot operations non-blocking.
		}
		copiedPivotCellKey = `${row.label}:${column.label}`;
		toast.success($t('forms.pivotCellCopied'));
	}

	async function copyPivotDrilldownRecordIds() {
		if (!pivotDrilldown || pivotDrilldownRecords.length === 0) return;
		const recordIds = pivotDrilldownRecords.map((record) => record.id);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(recordIds.join('\n'));
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep pivot operations non-blocking.
		}
		copiedPivotDrilldownKey = pivotDrilldownKey();
		toast.success($t('forms.pivotDrilldownIdsCopied'));
	}

	function exportPivotDrilldownRecordsCsv() {
		if (!selectedForm || !pivotDrilldown || pivotDrilldownRecords.length === 0) return;
		const csv = recordsCsv(pivotDrilldownRecords);
		const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-pivot-drilldown-records.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedPivotDrilldownKey = pivotDrilldownKey();
		toast.success($t('forms.pivotDrilldownExported'));
	}

	function pivotTableMatrixRows(table: PivotTable): string[][] {
		if (!table.rowField || !table.columnField || !table.valueField) return [];
		const header = [
			fieldLabel(table.rowField),
			...table.columns.map((column) => column.label),
			...(pivotShowRowTotals(table.totalsMode) ? [$t('forms.pivotTotal')] : [])
		];
		const rows = table.rows.map((row) => [
			row.label,
			...table.columns.map((column) =>
				formatPivotDisplayValue(row.cells[column.key] ?? 0, row.total, column.total, table)
			),
			...(pivotShowRowTotals(table.totalsMode)
				? [formatPivotTotalDisplayValue(row.total, 'row', table)]
				: [])
		]);
		if (pivotShowColumnTotals(table.totalsMode)) {
			rows.push([
				$t('forms.pivotTotal'),
				...table.columns.map((column) => formatPivotTotalDisplayValue(column.total, 'column', table)),
				...(pivotShowRowTotals(table.totalsMode)
					? [formatPivotTotalDisplayValue(table.grandTotal, 'grand', table)]
				: [])
			]);
		}
		return [header, ...rows];
	}

	function pivotTableMatrixText(table: PivotTable): string {
		return pivotTableMatrixRows(table)
			.map((row) => row.join('\t'))
			.join('\n');
	}

	function pivotTableMatrixCsv(table: PivotTable): string {
		return pivotTableMatrixRows(table)
			.map((row) => row.map(csvCell).join(','))
			.join('\n');
	}

	async function copyPivotTableMatrix() {
		const text = pivotTableMatrixText(pivotTable);
		if (!text) return;
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;
		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(text);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep pivot operations non-blocking.
		}
		copiedPivotTableMatrix = true;
		toast.success($t('forms.pivotTableCopied'));
	}

	function exportPivotTableMatrixCsv() {
		const csv = pivotTableMatrixCsv(pivotTable);
		if (!selectedForm || !csv) return;
		const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${selectedForm.key}-pivot-matrix.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedPivotTableMatrix = true;
		toast.success($t('forms.pivotTableExported'));
	}

	function focusPivotDrilldownRecords() {
		const recordIds = pivotDrilldownRecords.map((record) => record.id);
		if (recordIds.length === 0) return;
		focusedRecordIds = recordIds;
		pivotDrilldown = null;
		const gridView = formViews.find((view) => view.view_type === 'grid');
		if (gridView) {
			selectedViewId = gridView.id;
			resetViewDraft(gridView);
		}
		toast.success($t('forms.pivotDrilldownFocused'));
	}

	function viewGroupField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'group_by'));
		if (configured) return configured;
		const statusLike = visibleFields.find((field) =>
			['single_select', 'boolean'].includes(field.type ?? 'text')
		);
		if (statusLike) return statusLike;
		return visibleFields[0] ?? null;
	}

	function viewSwimlaneField(): FormField | null {
		const configuredKey = viewConfigString(selectedView, 'swimlane_by');
		if (!configuredKey || configuredKey === kanbanGroupField?.key) return null;
		return fieldByKey(configuredKey);
	}

	function kanbanMoveOptions(field: FormField | null): string[] {
		if (!field || field.type !== 'single_select') return [];
		return Array.isArray(field.options)
			? field.options.filter((option) => option.trim() && !optionArchived(field, option))
			: [];
	}

	function kanbanWipLimits(): Record<string, number> {
		const raw = selectedView?.config?.wip_limits;
		if (!isRecord(raw)) return {};
		const limits: Record<string, number> = {};
		for (const [key, value] of Object.entries(raw)) {
			const limit = typeof value === 'number' ? value : Number(value);
			if (Number.isFinite(limit) && limit >= 0) {
				limits[key] = Math.floor(limit);
			}
		}
		return limits;
	}

	function kanbanWipLimit(label: string): number | null {
		const limit = kanbanWipLimits()[label];
		return typeof limit === 'number' ? limit : null;
	}

	function kanbanMoveAllowed(record: FormRecord, targetValue: string): boolean {
		if (!kanbanGroupField) return false;
		if (String(record.values[kanbanGroupField.key] ?? '') === targetValue) return true;
		const limit = kanbanWipLimit(targetValue);
		if (limit === null) return true;
		const count = kanbanColumnRecordCount(targetValue);
		return count < limit;
	}

	function kanbanColumnRecordCount(groupLabel: string): number {
		if (!kanbanGroupField) return 0;
		return records.filter((item) => String(item.values[kanbanGroupField.key] ?? '') === groupLabel)
			.length;
	}

	function kanbanWipLabel(groupLabel: string, count: number): string {
		const limit = kanbanWipLimit(groupLabel);
		return limit === null
			? $t('forms.groupRecordCount', { values: { count } })
			: $t('forms.kanbanWipCount', { values: { count, limit } });
	}

	function kanbanWipExceeded(groupLabel: string, count: number): boolean {
		const limit = kanbanWipLimit(groupLabel);
		return limit !== null && count >= limit;
	}

	function kanbanColumnWipExceeded(groupLabel: string): boolean {
		return kanbanWipExceeded(groupLabel, kanbanColumnRecordCount(groupLabel));
	}

	function viewDateField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'date_field'));
		if (configured) return configured;
		return visibleFields.find((field) => ['date', 'datetime'].includes(field.type ?? 'text')) ?? null;
	}

	function viewCardTitleField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'card_title_field'));
		if (configured) return configured;
		return null;
	}

	function viewCardSubtitleField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'card_subtitle_field'));
		if (configured && configured.key !== viewCardTitleField()?.key) return configured;
		return null;
	}

	function viewCardBadgeField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'card_badge_field'));
		const titleField = viewCardTitleField();
		const subtitleField = viewCardSubtitleField();
		if (configured && configured.key !== titleField?.key && configured.key !== subtitleField?.key) {
			return configured;
		}
		return null;
	}

	function viewCardCoverField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'card_cover_field'));
		if (configured && ['image', 'attachment'].includes(configured.type ?? 'text')) return configured;
		return null;
	}

	function viewPivotRowField(): FormField | null {
		return fieldByKey(viewConfigString(selectedView, 'pivot_row_field'));
	}

	function viewPivotColumnField(): FormField | null {
		const rowField = viewPivotRowField();
		const configured = fieldByKey(viewConfigString(selectedView, 'pivot_column_field'));
		if (configured && configured.key !== rowField?.key) return configured;
		return null;
	}

	function viewPivotValueField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'pivot_value_field'));
		if (configured && ['amount', 'number', 'integer'].includes(configured.type ?? 'text')) return configured;
		return null;
	}

	function viewGanttStartField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'gantt_start_field'));
		if (configured && ['date', 'datetime'].includes(configured.type ?? 'text')) return configured;
		return visibleFields.find((field) => ['date', 'datetime'].includes(field.type ?? 'text')) ?? null;
	}

	function viewGanttEndField(): FormField | null {
		const startField = viewGanttStartField();
		const configured = fieldByKey(viewConfigString(selectedView, 'gantt_end_field'));
		if (
			configured &&
			configured.key !== startField?.key &&
			['date', 'datetime'].includes(configured.type ?? 'text')
		) {
			return configured;
		}
		return null;
	}

	function viewGanttGroupField(): FormField | null {
		const startField = viewGanttStartField();
		const endField = viewGanttEndField();
		const configured = fieldByKey(viewConfigString(selectedView, 'gantt_group_by'));
		if (configured && configured.key !== startField?.key && configured.key !== endField?.key) return configured;
		return null;
	}

	function viewGanttDependencyField(): FormField | null {
		const startField = viewGanttStartField();
		const endField = viewGanttEndField();
		const groupField = viewGanttGroupField();
		const configured = fieldByKey(viewConfigString(selectedView, 'gantt_dependency_field'));
		if (
			configured &&
			configured.key !== startField?.key &&
			configured.key !== endField?.key &&
			configured.key !== groupField?.key
		) {
			return configured;
		}
		return null;
	}

	function viewTimelineTitleField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'timeline_title_field'));
		if (configured) return configured;
		return null;
	}

	function viewTimelineSubtitleField(): FormField | null {
		const configured = fieldByKey(viewConfigString(selectedView, 'timeline_subtitle_field'));
		if (configured && configured.key !== viewTimelineTitleField()?.key) return configured;
		return null;
	}

	function selectedTimelineSource(): ViewTimelineSource {
		return viewTimelineSource(selectedView?.config?.timeline_source);
	}

	function selectedTimelineEventLayout(): TimelineEventLayout {
		return timelineEventLayout(selectedView?.config?.timeline_event_layout);
	}

	function selectedTimelineEventType(): string {
		return viewConfigString(selectedView, 'timeline_event_type');
	}

	function selectedTimelineEventActor(): string {
		return viewConfigString(selectedView, 'timeline_event_actor');
	}

	function selectedTimelineEventAggregate(): string {
		return viewConfigString(selectedView, 'timeline_event_aggregate');
	}

	function selectedTimelineEventQuery(): string {
		return viewConfigString(selectedView, 'timeline_event_query').trim();
	}

	function selectedTimelineEventFrom(): string {
		return viewConfigString(selectedView, 'timeline_event_from').trim();
	}

	function selectedTimelineEventTo(): string {
		return viewConfigString(selectedView, 'timeline_event_to').trim();
	}

	function viewCalendarResourceField(): FormField | null {
		const configuredKey = viewConfigString(selectedView, 'calendar_resource_by');
		if (!configuredKey || configuredKey === calendarDateField?.key) return null;
		return fieldByKey(configuredKey);
	}

	function viewCalendarEndField(): FormField | null {
		const configuredKey = viewConfigString(selectedView, 'calendar_end_field');
		if (!configuredKey || configuredKey === calendarDateField?.key) return null;
		const configured = fieldByKey(configuredKey);
		if (!configured || !['date', 'datetime'].includes(configured.type ?? 'text')) return null;
		return configured;
	}

	function viewCalendarFocusDate(view: FormView | null = selectedView): string {
		const focusDate = viewConfigString(view, 'calendar_focus_date');
		return isIsoDate(focusDate) ? focusDate : '';
	}

	function viewDraftCalendarFocusBaseDate(): string {
		if (isIsoDate(viewDraft.calendarFocusDate)) return viewDraft.calendarFocusDate;
		const selectedFocusDate = viewCalendarFocusDate();
		if (selectedFocusDate) return selectedFocusDate;
		const field = fieldByKey(viewDraft.dateField) ?? calendarDateField;
		const sourceDates = viewRecords
			.map((record) => calendarRecordIsoDate(record, field))
			.filter((date): date is string => Boolean(date))
			.sort((left, right) => left.localeCompare(right));
		return sourceDates[0] ?? new Date().toISOString().slice(0, 10);
	}

	function shiftViewDraftCalendarFocus(amount: number) {
		if (viewDraft.view_type !== 'calendar') return;
		const range = viewDraft.calendarRange;
		const baseDate = viewDraftCalendarFocusBaseDate();
		const nextDate =
			range === 'month' ? addIsoMonths(monthStartIso(baseDate), amount) : addIsoDays(weekStartIso(baseDate), amount * 7);
		viewDraft = { ...viewDraft, calendarFocusDate: nextDate };
	}

	function selectedCalendarExceptionDates(): CalendarExceptionDateMap {
		return calendarExceptionDateMap(selectedView?.config?.calendar_exception_dates);
	}

	function calendarExceptionDateSet(recordId: string): Set<string> {
		return new Set(selectedCalendarExceptionDates()[recordId] ?? []);
	}

	function recordDisplay(record: FormRecord, field: FormField | null): string {
		return field ? displayValue(record.values[field.key]) : '-';
	}

	function cardTitle(record: FormRecord): string {
		const value = recordDisplay(record, viewCardTitleField());
		return value && value !== '-' ? value : record.title;
	}

	function cardSubtitle(record: FormRecord): string {
		const value = recordDisplay(record, viewCardSubtitleField());
		return value && value !== '-' ? value : record.id.slice(0, 8);
	}

	function cardBadge(record: FormRecord): string {
		const value = recordDisplay(record, viewCardBadgeField());
		return value && value !== '-' ? value : '';
	}

	function recordMediaUrl(record: FormRecord, field: FormField | null): string {
		if (!field) return '';
		const raw = record.values[field.key];
		if (typeof raw === 'string') return raw;
		if (isRecord(raw)) {
			if (typeof raw.url === 'string') return raw.url;
			if (typeof raw.href === 'string') return raw.href;
			if (typeof raw.src === 'string') return raw.src;
		}
		if (Array.isArray(raw)) {
			const first = raw.find((item) => typeof item === 'string' || isRecord(item));
			if (typeof first === 'string') return first;
			if (isRecord(first)) {
				if (typeof first.url === 'string') return first.url;
				if (typeof first.href === 'string') return first.href;
				if (typeof first.src === 'string') return first.src;
			}
		}
		return '';
	}

	function cardCoverUrl(record: FormRecord): string {
		return recordMediaUrl(record, viewCardCoverField());
	}

	function timelineTitle(record: FormRecord): string {
		const value = recordDisplay(record, viewTimelineTitleField());
		return value && value !== '-' ? value : record.title;
	}

	function timelineSubtitle(record: FormRecord): string {
		const value = recordDisplay(record, viewTimelineSubtitleField());
		return value && value !== '-' ? value : cardFields[0] ? recordDisplay(record, cardFields[0]) : record.id.slice(0, 8);
	}

	function eventValue(event: BusinessEvent, keys: string[]): string {
		for (const source of [event.payload, event.metadata, event.source]) {
			for (const key of keys) {
				const value = source[key];
				if (typeof value === 'string' && value.trim()) return value.trim();
				if (typeof value === 'number' || typeof value === 'boolean') return String(value);
			}
		}
		return '';
	}

	function objectContainsString(value: unknown, expected: string, depth = 0): boolean {
		if (!expected || depth > 5) return false;
		if (typeof value === 'string') return value === expected;
		if (typeof value === 'number' || typeof value === 'boolean') return String(value) === expected;
		if (Array.isArray(value)) return value.some((item) => objectContainsString(item, expected, depth + 1));
		if (!isRecord(value)) return false;
		return Object.values(value).some((item) => objectContainsString(item, expected, depth + 1));
	}

	function invocationBelongsToSelectedForm(invocation: Invocation): boolean {
		if (!selectedForm) return false;
		return (
			(invocation.trigger_ref_type === 'form' && invocation.trigger_ref_id === selectedForm.id) ||
			objectContainsString(invocation.payload, selectedForm.id) ||
			objectContainsString(invocation.payload, selectedForm.key) ||
			objectContainsString(invocation.result, selectedForm.id) ||
			objectContainsString(invocation.result, selectedForm.key)
		);
	}

	function invocationBelongsToSelectedRecord(invocation: Invocation): boolean {
		if (!selectedRecord) return false;
		return (
			(invocation.trigger_ref_type === 'form_record' && invocation.trigger_ref_id === selectedRecord.id) ||
			objectContainsString(invocation.payload, selectedRecord.id) ||
			objectContainsString(invocation.result, selectedRecord.id)
		);
	}

	function invocationConnectorLabel(invocation: Invocation): string {
		const delivery = isRecord(invocation.result?.connector_delivery)
			? invocation.result.connector_delivery
			: null;
		if (delivery && typeof delivery.connector_name === 'string' && delivery.connector_name.trim()) {
			return delivery.connector_name.trim();
		}
		return invocation.connector_kind || invocation.connector_id || $t('forms.automation.noConnector');
	}

	function invocationDeliveryStatus(invocation: Invocation): string {
		const delivery = isRecord(invocation.result?.connector_delivery)
			? invocation.result.connector_delivery
			: null;
		if (delivery && typeof delivery.status === 'string' && delivery.status.trim()) return delivery.status.trim();
		return invocation.status;
	}

	function timelineEventAggregateKey(event: BusinessEvent): string {
		return `${event.aggregate_type}:${event.aggregate_id}`;
	}

	function timelineEventTitle(event: BusinessEvent): string {
		return eventValue(event, ['title', 'record_title', 'form_name']) || event.event_type;
	}

	function timelineEventSubtitle(event: BusinessEvent): string {
		const actor = event.actor_id ? `${$t('forms.eventActor')} ${event.actor_id}` : '';
		const aggregate = timelineEventAggregateKey(event);
		return actor ? `${aggregate} · ${actor}` : aggregate;
	}

	function timelineEventRecord(event: BusinessEvent): FormRecord | null {
		if (event.aggregate_type !== 'form_record') return null;
		return records.find((record) => record.id === event.aggregate_id) ?? null;
	}

	function timelineEventJson(value: unknown): string {
		if (!value || (isRecord(value) && Object.keys(value).length === 0)) return '{}';
		return JSON.stringify(value, null, 2);
	}

	function timelineEventDetailsPayload(event: BusinessEvent) {
		return {
			id: event.id,
			event_type: event.event_type,
			created_at: event.created_at,
			aggregate_type: event.aggregate_type,
			aggregate_id: event.aggregate_id,
			actor_id: event.actor_id,
			title: timelineEventTitle(event),
			subtitle: timelineEventSubtitle(event),
			payload: event.payload,
			metadata: event.metadata,
			source: event.source
		};
	}

	function timelineEventDetailsJson(event: BusinessEvent): string {
		return JSON.stringify(timelineEventDetailsPayload(event), null, 2);
	}

	function timelineEventsDetailsJson(events: BusinessEvent[]): string {
		return JSON.stringify(events.map((event) => timelineEventDetailsPayload(event)), null, 2);
	}

	function timelineEventSearchText(event: BusinessEvent): string {
		return [
			event.id,
			event.event_type,
			event.aggregate_type,
			event.aggregate_id,
			event.actor_id ?? '',
			timelineEventTitle(event),
			timelineEventSubtitle(event),
			timelineEventJson(event.payload),
			timelineEventJson(event.metadata),
			timelineEventJson(event.source)
		]
			.join('\n')
			.toLowerCase();
	}

	function recordGroupLabel(record: FormRecord, field: FormField | null): string {
		const value = recordDisplay(record, field);
		return value && value !== '-' ? value : $t('forms.unspecified');
	}

	function recordDateSource(record: FormRecord, field: FormField | null): string {
		const value = field ? record.values[field.key] : null;
		if (typeof value === 'string' && value.trim()) return value;
		if (isRecord(value)) {
			if (typeof value.value === 'string' && value.value.trim()) return value.value;
			if (typeof value.date === 'string' && value.date.trim()) return value.date;
			if (typeof value.datetime === 'string' && value.datetime.trim()) return value.datetime;
		}
		return record.updated_at || record.created_at;
	}

	function calendarDateInputValue(record: FormRecord, field: FormField | null): string {
		if (!field || !['date', 'datetime'].includes(field.type ?? 'text')) return '';
		const source = recordDateSource(record, field);
		const match = source.match(/^\d{4}-\d{2}-\d{2}/);
		return match?.[0] ?? '';
	}

	function calendarRecordIsoDate(record: FormRecord, field: FormField | null): string | null {
		const source = recordDateSource(record, field);
		const match = source.match(/^\d{4}-\d{2}-\d{2}/);
		return match?.[0] ?? null;
	}

	function addIsoDays(isoDate: string, days: number): string {
		const date = new Date(`${isoDate}T00:00:00.000Z`);
		date.setUTCDate(date.getUTCDate() + days);
		return date.toISOString().slice(0, 10);
	}

	function isoDayDiff(left: string, right: string): number {
		const leftTime = new Date(`${left}T00:00:00.000Z`).getTime();
		const rightTime = new Date(`${right}T00:00:00.000Z`).getTime();
		if (!Number.isFinite(leftTime) || !Number.isFinite(rightTime)) return 0;
		return Math.round((leftTime - rightTime) / 86_400_000);
	}

	function addIsoMonths(isoDate: string, months: number): string {
		const date = new Date(`${isoDate}T00:00:00.000Z`);
		const day = date.getUTCDate();
		date.setUTCMonth(date.getUTCMonth() + months, 1);
		const maxDay = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 0)).getUTCDate();
		date.setUTCDate(Math.min(day, maxDay));
		return date.toISOString().slice(0, 10);
	}

	function monthStartIso(isoDate: string): string {
		const date = new Date(`${isoDate}T00:00:00.000Z`);
		return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1))
			.toISOString()
			.slice(0, 10);
	}

	function weekStartIso(isoDate: string): string {
		const date = new Date(`${isoDate}T00:00:00.000Z`);
		const day = date.getUTCDay();
		const offset = day === 0 ? -6 : 1 - day;
		date.setUTCDate(date.getUTCDate() + offset);
		return date.toISOString().slice(0, 10);
	}

	function monthDayCount(isoDate: string): number {
		const date = new Date(`${isoDate}T00:00:00.000Z`);
		return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 0)).getUTCDate();
	}

	function ganttBucketStartIso(isoDate: string, scale: GanttScale): string {
		if (scale === 'month') return monthStartIso(isoDate);
		if (scale === 'week') return weekStartIso(isoDate);
		return isoDate;
	}

	function addGanttBucket(isoDate: string, scale: GanttScale, amount: number): string {
		if (scale === 'month') return addIsoMonths(isoDate, amount);
		if (scale === 'week') return addIsoDays(isoDate, amount * 7);
		return addIsoDays(isoDate, amount);
	}

	function ganttBucketLimit(scale: GanttScale): number {
		if (scale === 'day') return 45;
		return 18;
	}

	function calendarGridLabel(isoDate: string): string {
		const date = new Date(`${isoDate}T00:00:00.000Z`);
		return date.toLocaleDateString($locale === 'en' ? 'en-US' : 'zh-CN', {
			month: '2-digit',
			day: '2-digit',
			weekday: 'short',
			timeZone: 'UTC'
		});
	}

	function isIsoWorkday(isoDate: string): boolean {
		const day = new Date(`${isoDate}T00:00:00.000Z`).getUTCDay();
		return day >= 1 && day <= 5;
	}

	function calendarDateInScope(isoDate: string): boolean {
		const dayScope = viewCalendarDayScope(selectedView?.config?.calendar_day_scope);
		return dayScope === 'all' || isIsoWorkday(isoDate);
	}

	function ganttDateLabel(isoDate: string, scale: GanttScale = 'day'): string {
		const date = new Date(`${isoDate}T00:00:00.000Z`);
		if (scale === 'month') {
			return date.toLocaleDateString($locale === 'en' ? 'en-US' : 'zh-CN', {
				year: 'numeric',
				month: '2-digit',
				timeZone: 'UTC'
			});
		}
		return date.toLocaleDateString($locale === 'en' ? 'en-US' : 'zh-CN', {
			month: '2-digit',
			day: '2-digit',
			timeZone: 'UTC'
		});
	}

	function calendarOccurrenceDates(
		baseDate: string,
		gridStartDate: string,
		gridEndDate: string,
		recurrence: ViewCalendarRecurrence
	): string[] {
		if (recurrence === 'none') return [baseDate];
		if (baseDate > gridEndDate) return [baseDate];
		const dates: string[] = [];
		let cursor = baseDate;
		while (cursor <= gridEndDate) {
			if (cursor >= gridStartDate) dates.push(cursor);
			if (recurrence === 'daily') {
				cursor = addIsoDays(cursor, 1);
			} else if (recurrence === 'weekly') {
				cursor = addIsoDays(cursor, 7);
			} else {
				cursor = addIsoMonths(cursor, 1);
			}
		}
		return dates.length > 0 ? dates : [baseDate];
	}

	function calendarDatePayloadValue(field: FormField, value: string): string {
		if ((field.type ?? 'date') === 'datetime') return `${value}T00:00:00.000Z`;
		return value;
	}

	function recordDateBucket(record: FormRecord, field: FormField | null): string {
		const raw = recordDateSource(record, field);
		const date = new Date(raw);
		if (Number.isNaN(date.getTime())) return $t('forms.noDate');
		return date.toLocaleDateString($locale === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit'
		});
	}

	function groupRecordsByField(
		sourceRecords: FormRecord[],
		field: FormField | null
	): Array<{ key: string; label: string; records: FormRecord[] }> {
		const groups = new Map<string, { key: string; label: string; records: FormRecord[] }>();
		if (field?.type === 'single_select' && Array.isArray(field.options)) {
			for (const option of field.options) {
				const label = option.trim();
				if (label) groups.set(label.toLowerCase(), { key: label.toLowerCase(), label, records: [] });
			}
		}
		for (const record of sourceRecords) {
			const label = recordGroupLabel(record, field);
			const key = label.toLowerCase();
			const existing = groups.get(key);
			if (existing) {
				existing.records.push(record);
			} else {
				groups.set(key, { key, label, records: [record] });
			}
		}
		return Array.from(groups.values()).sort((left, right) => left.label.localeCompare(right.label));
	}

	function groupKanbanSwimlanes(
		sourceRecords: FormRecord[]
	): Array<{
		key: string;
		label: string;
		records: FormRecord[];
		groups: Array<{ key: string; label: string; records: FormRecord[] }>;
	}> {
		if (!kanbanSwimlaneField) return [];
		return groupRecordsByField(sourceRecords, kanbanSwimlaneField).map((swimlane) => ({
			...swimlane,
			groups: groupRecordsByField(swimlane.records, kanbanGroupField)
		}));
	}

	function kanbanSwimlaneRows(): Array<{
		key: string;
		label: string;
		records: FormRecord[];
		groups: Array<{ key: string; label: string; records: FormRecord[] }>;
	}> {
		if (kanbanSwimlaneField) return kanbanSwimlanes;
		return [{ key: '__all__', label: '', records: viewRecords, groups: kanbanGroups }];
	}

	function groupRecordsByDate(
		sourceRecords: FormRecord[],
		field: FormField | null
	): Array<{ key: string; label: string; records: FormRecord[] }> {
		const groups = new Map<string, { key: string; label: string; records: FormRecord[] }>();
		for (const record of sourceRecordsByDate(sourceRecords, field)) {
			const label = recordDateBucket(record, field);
			const existing = groups.get(label);
			if (existing) {
				existing.records.push(record);
			} else {
				groups.set(label, { key: label, label, records: [record] });
			}
		}
		return Array.from(groups.values());
	}

	function groupRecordsByCalendarGrid(
		sourceRecords: FormRecord[],
		field: FormField | null,
		scopeRecords: FormRecord[] = sourceRecords
	): CalendarGridGroup[] {
		const dated = sourceRecords
			.map((record) => ({ record, date: calendarRecordIsoDate(record, field) }))
			.filter((item): item is { record: FormRecord; date: string } => Boolean(item.date));
		const scopedDates = scopeRecords
			.map((record) => calendarRecordIsoDate(record, field))
			.filter((date): date is string => Boolean(date));
		const focusDate = viewCalendarFocusDate();
		const startDate =
			focusDate ||
			(scopedDates.sort((left, right) => left.localeCompare(right))[0] ??
				dated.map((item) => item.date).sort((left, right) => left.localeCompare(right))[0] ??
				new Date().toISOString().slice(0, 10));
		const range = viewCalendarRange(selectedView?.config?.calendar_range);
		const gridStartDate = range === 'month' ? monthStartIso(startDate) : startDate;
		const gridDayCount = range === 'month' ? monthDayCount(startDate) : 7;
		const gridEndDate = addIsoDays(gridStartDate, gridDayCount - 1);
		const recurrence = viewCalendarRecurrence(selectedView?.config?.calendar_recurrence);
		const groups = new Map<
			string,
			CalendarGridGroup
		>();
			for (let index = 0; index < gridDayCount; index += 1) {
				const date = addIsoDays(gridStartDate, index);
				if (calendarDateInScope(date)) {
					groups.set(date, { key: date, label: calendarGridLabel(date), date, records: [] });
				}
			}
			for (const { record, date } of dated) {
				const rawEnd = calendarEndField ? calendarRecordIsoDate(record, calendarEndField) : null;
				const endDate = rawEnd && rawEnd >= date ? rawEnd : date;
				const spanDays = isoDayDiff(endDate, date) + 1;
				const exceptionDates = calendarExceptionDateSet(record.id);
				for (const occurrenceDate of calendarOccurrenceDates(
					date,
					gridStartDate,
					gridEndDate,
					recurrence
				)) {
					if (exceptionDates.has(occurrenceDate)) continue;
					const occurrenceEndDate = addIsoDays(occurrenceDate, spanDays - 1);
					let bucketDate = occurrenceDate < gridStartDate ? gridStartDate : occurrenceDate;
					while (bucketDate <= occurrenceEndDate && bucketDate <= gridEndDate) {
						if (!calendarDateInScope(bucketDate)) {
							bucketDate = addIsoDays(bucketDate, 1);
							continue;
						}
						if (!groups.has(bucketDate)) {
							groups.set(bucketDate, {
								key: bucketDate,
								label: calendarGridLabel(bucketDate),
								date: bucketDate,
								records: []
							});
						}
						groups.get(bucketDate)?.records.push({
							key: `${record.id}:${occurrenceDate}:${bucketDate}`,
							record,
							date: bucketDate,
							startDate: occurrenceDate,
							endDate: occurrenceEndDate,
							recurring: occurrenceDate !== date,
							spanStart: bucketDate === occurrenceDate,
							spanEnd: bucketDate === occurrenceEndDate,
							spanDays
						});
						bucketDate = addIsoDays(bucketDate, 1);
					}
				}
			}
			const orderedGroups = Array.from(groups.values()).sort((left, right) =>
				left.date.localeCompare(right.date)
			);
			return selectedCalendarHideEmptyDays
				? orderedGroups.filter((group) => group.records.length > 0)
				: orderedGroups;
		}

	function groupCalendarResourceLanes(
		sourceRecords: FormRecord[]
	): CalendarResourceRow[] {
		if (!calendarResourceField) return [];
		return groupRecordsByField(sourceRecords, calendarResourceField).map((resource) => ({
			...resource,
			groups: groupRecordsByCalendarGrid(resource.records, calendarDateField, sourceRecords)
		}));
	}

	function calendarResourceRows(): CalendarResourceRow[] {
		if (calendarResourceField) return calendarResourceLanes;
		return [{ key: '__all__', label: '', records: viewRecords, groups: calendarGridGroups }];
	}

	function calendarBucketFilterKey(isoDate: string, resource: CalendarResourceRow): string {
		return `${isoDate}:${calendarResourceField ? resource.key : '__all__'}`;
	}

	function calendarBucketFilterDrafts(isoDate: string, resource: CalendarResourceRow): ViewFilterDraft[] {
		if (!calendarDateField) return [];
		const filters: ViewFilterDraft[] = [
			{
				field: calendarDateField.key,
				operator: (calendarDateField.type ?? 'date') === 'datetime' ? 'contains' : 'equals',
				value: isoDate
			}
		];
		if (calendarResourceField && resource.key !== '__all__') {
			filters.push({
				field: calendarResourceField.key,
				operator: 'equals',
				value: resource.label
			});
		}
		return filters;
	}

	function canApplyCalendarBucketFilters(isoDate: string, resource: CalendarResourceRow): boolean {
		const filters = calendarBucketFilterDrafts(isoDate, resource);
		if (filters.length === 0) return false;
		const currentGroups = visibleViewDraftFilterGroups().filter((group) => group.filters.length > 0);
		const currentFilterCount = currentGroups.reduce((count, group) => count + group.filters.length, 0);
		if (currentFilterCount + filters.length > 10) return false;
		if (currentGroups.length < 5) return true;
		const lastGroup = currentGroups[currentGroups.length - 1];
		return Boolean(lastGroup && lastGroup.filters.length + filters.length <= 5);
	}

	function applyCalendarBucketFilters(isoDate: string, resource: CalendarResourceRow) {
		if (!canApplyCalendarBucketFilters(isoDate, resource)) return;
		const filters = calendarBucketFilterDrafts(isoDate, resource);
		const currentGroups = visibleViewDraftFilterGroups()
			.filter((group) => group.filters.length > 0)
			.map((group) => ({
				logic: group.logic,
				disabled: group.disabled || undefined,
				filters: group.filters.map((filter) => ({ ...filter }))
			}));
		const nextGroups =
			currentGroups.length === 0
				? [{ logic: 'all' as ViewFilterLogic, filters }]
				: currentGroups.length < 5
					? [...currentGroups, { logic: 'all' as ViewFilterLogic, filters }]
					: currentGroups.map((group, index) =>
							index === currentGroups.length - 1
								? { ...group, filters: [...group.filters, ...filters] }
								: group
						);
		setViewDraftFilterGroups(nextGroups);
		updateViewDraftFilterRootLogic('all');
		appliedCalendarFilterKey = calendarBucketFilterKey(isoDate, resource);
		toast.success($t('forms.calendarBucketFiltersApplied'));
	}

	function calendarAgendaGroups(resource: CalendarResourceRow): CalendarGridGroup[] {
		return resource.groups
			.map((group) => ({
				...group,
				records: group.records.filter((occurrence) => occurrence.spanStart)
			}))
			.filter((group) => group.records.length > 0);
	}

	function calendarAgendaRecordCount(resource: CalendarResourceRow): number {
		return calendarAgendaGroups(resource).reduce((total, group) => total + group.records.length, 0);
	}

	function buildGanttTimeline(sourceRecords: FormRecord[]): GanttTimeline {
		const startField = viewGanttStartField();
		const endField = viewGanttEndField();
		const groupField = viewGanttGroupField();
		const dependencyField = viewGanttDependencyField();
		const scale = viewGanttScale(selectedView?.config?.gantt_scale);
		if (!startField) {
			return { startField, endField, groupField, dependencyField, scale, dates: [], lanes: [] };
		}
		const dated = sourceRecords
			.map((record) => {
				const start = calendarRecordIsoDate(record, startField);
				const rawEnd = endField ? calendarRecordIsoDate(record, endField) : null;
				if (!start) return null;
				const end = rawEnd && rawEnd >= start ? rawEnd : start;
				return { record, start, end };
			})
			.filter((item): item is { record: FormRecord; start: string; end: string } => Boolean(item));
		if (dated.length === 0) {
			return { startField, endField, groupField, dependencyField, scale, dates: [], lanes: [] };
		}
		const firstStart = dated.map((item) => item.start).sort((left, right) => left.localeCompare(right))[0];
		const maxEnd = dated.map((item) => item.end).sort((left, right) => right.localeCompare(left))[0];
		const axisStart = ganttBucketStartIso(firstStart, scale);
		const lastBucket = ganttBucketStartIso(maxEnd, scale);
		const bucketCount = (() => {
			if (scale === 'day') {
				return Math.min(Math.max(isoDayDiff(lastBucket, axisStart) + 1, 7), ganttBucketLimit(scale));
			}
			const candidateDates = Array.from({ length: ganttBucketLimit(scale) }, (_, index) =>
				addGanttBucket(axisStart, scale, index)
			);
			const lastIndex = candidateDates.findIndex((date) => date >= lastBucket);
			return Math.max(lastIndex >= 0 ? lastIndex + 1 : ganttBucketLimit(scale), 4);
		})();
		const axisEnd = addGanttBucket(axisStart, scale, bucketCount - 1);
		const dates = Array.from({ length: bucketCount }, (_, index) => addGanttBucket(axisStart, scale, index));
		const bucketIndex = (date: string) => {
			const bucket = ganttBucketStartIso(date, scale);
			const index = dates.indexOf(bucket);
			if (index >= 0) return index;
			return bucket < axisStart ? 0 : dates.length - 1;
		};
		const laneMap = new Map<string, { key: string; label: string; tasks: GanttTask[] }>();
		const ensureLane = (key: string, label: string) => {
			if (!laneMap.has(key)) laneMap.set(key, { key, label, tasks: [] });
			return laneMap.get(key);
		};
		if (groupField?.type === 'single_select' && Array.isArray(groupField.options)) {
			for (const option of groupField.options) {
				const label = option.trim();
				if (label) ensureLane(label.toLowerCase(), label);
			}
		}
		for (const item of dated) {
			const label = groupField ? recordGroupLabel(item.record, groupField) : $t('forms.ganttUngrouped');
			const key = groupField ? label.toLowerCase() : '__all__';
			const lane = ensureLane(key, label);
			const start = item.start < axisStart ? axisStart : item.start;
			const end = item.end > axisEnd ? axisEnd : item.end;
			const startOffset = bucketIndex(start);
			const endOffset = bucketIndex(end);
			lane?.tasks.push({
				record: item.record,
				start: item.start,
				end: item.end,
				dependencyValue: dependencyField ? recordDisplay(item.record, dependencyField) : '',
				startOffset,
				span: Math.max(1, endOffset - startOffset + 1)
			});
		}
		return {
			startField,
			endField,
			groupField,
			dependencyField,
			scale,
			dates,
			lanes: Array.from(laneMap.values())
				.map((lane) => ({
					...lane,
					tasks: lane.tasks.sort(
						(left, right) =>
							left.start.localeCompare(right.start) || left.record.title.localeCompare(right.record.title)
					)
				}))
				.sort((left, right) => left.label.localeCompare(right.label))
		};
	}

	function sourceRecordsByDate(sourceRecords: FormRecord[], field: FormField | null): FormRecord[] {
		return [...sourceRecords].sort((left, right) => {
			const leftTime = new Date(recordDateSource(left, field)).getTime();
			const rightTime = new Date(recordDateSource(right, field)).getTime();
			return (Number.isNaN(leftTime) ? 0 : leftTime) - (Number.isNaN(rightTime) ? 0 : rightTime);
		});
	}

	function recordsByUpdatedAt(sourceRecords: FormRecord[]): FormRecord[] {
		return [...sourceRecords].sort(
			(left, right) => new Date(right.updated_at).getTime() - new Date(left.updated_at).getTime()
		);
	}

	function eventsByCreatedAt(sourceEvents: BusinessEvent[]): BusinessEvent[] {
		return [...sourceEvents].sort(
			(left, right) => new Date(right.created_at).getTime() - new Date(left.created_at).getTime()
		);
	}

	function eventTypes(sourceEvents: BusinessEvent[]): string[] {
		return Array.from(new Set(sourceEvents.map((event) => event.event_type).filter(Boolean))).sort();
	}

	function eventActors(sourceEvents: BusinessEvent[]): string[] {
		return Array.from(
			new Set(
				sourceEvents
					.map((event) => event.actor_id)
					.filter((actorId): actorId is string => typeof actorId === 'string' && actorId.length > 0)
			)
		).sort();
	}

	function eventAggregates(sourceEvents: BusinessEvent[]): string[] {
		return Array.from(new Set(sourceEvents.map((event) => timelineEventAggregateKey(event)))).sort();
	}

	function filteredTimelineEvents(sourceEvents: BusinessEvent[]): BusinessEvent[] {
		const eventType = selectedTimelineEventType();
		const eventActor = selectedTimelineEventActor();
		const eventAggregate = selectedTimelineEventAggregate();
		const eventQuery = selectedTimelineEventQuery().toLowerCase();
		const eventFrom = timelineEventBoundary(selectedTimelineEventFrom());
		const eventTo = timelineEventBoundary(selectedTimelineEventTo());
		return sourceEvents.filter((event) => {
			const eventTime = new Date(event.created_at).getTime();
			if (eventType && event.event_type !== eventType) return false;
			if (eventActor && event.actor_id !== eventActor) return false;
			if (eventAggregate && timelineEventAggregateKey(event) !== eventAggregate) return false;
			if (eventQuery && !timelineEventSearchText(event).includes(eventQuery)) return false;
			if (eventFrom !== null && Number.isFinite(eventTime) && eventTime < eventFrom) return false;
			if (eventTo !== null && Number.isFinite(eventTime) && eventTime > eventTo) return false;
			return true;
		});
	}

	function timelineEventBoundary(value: string): number | null {
		const trimmed = value.trim();
		if (!trimmed) return null;
		const parsed = new Date(trimmed).getTime();
		return Number.isFinite(parsed) ? parsed : null;
	}

	function parseViewFilterDrafts(rawFilters: unknown[]): ViewFilterDraft[] {
		return rawFilters
			.map((filter): ViewFilterDraft | null => {
				if (!isRecord(filter) || typeof filter.field !== 'string') return null;
				return {
					field: filter.field,
					operator: viewFilterOperator(filter.operator),
					value: typeof filter.value === 'string' ? filter.value : '',
					disabled: viewFilterNodeDisabled(filter) ? true : undefined
				};
			})
			.filter((filter): filter is ViewFilterDraft => Boolean(filter))
			.slice(0, 5);
	}

	function viewDraftExpressionFromView(view: FormView): Record<string, unknown> | null {
		return isRecord(view.config?.filter_expression) ? view.config.filter_expression : null;
	}

	function viewDraftFilterGroupsFromExpression(expression: Record<string, unknown>): ViewFilterGroupDraft[] {
		const rawChildren = Array.isArray(expression.children)
			? expression.children
			: Array.isArray(expression.expressions)
				? expression.expressions
				: [];
		const groups = rawChildren
			.map((child): ViewFilterGroupDraft | null => {
				if (!isRecord(child)) return null;
				if (typeof child.field === 'string' || isRecord(child.filter)) {
					const filter = parseViewFilterDrafts([isRecord(child.filter) ? child.filter : child])[0];
					if (filter && viewFilterNodeDisabled(child)) filter.disabled = true;
					return filter
						? {
								logic: 'all' as ViewFilterLogic,
								filters: [filter],
								disabled: viewFilterNodeDisabled(child) ? true : undefined
							}
						: null;
				}
				const childFilters = Array.isArray(child.children)
					? child.children
							.map((item) => {
								if (!isRecord(item)) return null;
								const filter = isRecord(item.filter) ? item.filter : item;
								if (viewFilterNodeDisabled(item) && !viewFilterNodeDisabled(filter)) {
									return { ...filter, disabled: true };
								}
								return filter;
							})
							.filter(isRecord)
					: [];
				const filters = parseViewFilterDrafts(childFilters);
				return filters.length > 0
					? {
							logic: viewFilterLogic(child.logic),
							filters,
							disabled: viewFilterNodeDisabled(child) ? true : undefined
						}
					: null;
			})
			.filter((group): group is ViewFilterGroupDraft => Boolean(group))
			.slice(0, 5);
		if (groups.length > 0) return groups;
		const filter = isRecord(expression.filter) ? expression.filter : expression;
		const filters = parseViewFilterDrafts([filter]);
		return filters.length > 0 ? [{ logic: 'all', filters }] : [];
	}

	function viewDraftFilterRootLogicFromView(view: FormView): ViewFilterLogic {
		const expression = viewDraftExpressionFromView(view);
		return expression ? viewFilterLogic(expression.logic) : 'all';
	}

	function viewDraftWipLimitsFromView(view: FormView): Record<string, string> {
		const raw = isRecord(view.config?.wip_limits) ? view.config.wip_limits : {};
		const limits: Record<string, string> = {};
		for (const [key, value] of Object.entries(raw)) {
			const limit = typeof value === 'number' ? value : Number(value);
			if (Number.isFinite(limit) && limit >= 0) {
				limits[key] = String(Math.floor(limit));
			}
		}
		return limits;
	}

	function viewDraftCalendarExceptionDatesFromView(view: FormView): CalendarExceptionDateMap {
		return calendarExceptionDateMap(view.config?.calendar_exception_dates);
	}

	function viewDraftFilterGroupsFromView(view: FormView): ViewFilterGroupDraft[] {
		const expression = viewDraftExpressionFromView(view);
		if (expression) {
			const groups = viewDraftFilterGroupsFromExpression(expression);
			if (groups.length > 0) return groups;
		}
		const rawGroups = Array.isArray(view.config?.filter_groups) ? view.config.filter_groups : [];
		const groups = rawGroups
			.map((group): ViewFilterGroupDraft | null => {
				if (!isRecord(group) || !Array.isArray(group.filters)) return null;
				const filters = parseViewFilterDrafts(group.filters);
				return filters.length > 0
					? {
							logic: viewFilterLogic(group.logic),
							filters,
							disabled: viewFilterNodeDisabled(group) ? true : undefined
						}
					: null;
			})
			.filter((group): group is ViewFilterGroupDraft => Boolean(group))
			.slice(0, 5);
		if (groups.length > 0) return groups;
		const rawFilters = Array.isArray(view.config?.filters) ? view.config.filters : [];
		const filters = parseViewFilterDrafts(rawFilters);
		if (filters.length > 0) return [{ logic: 'all', filters }];
		const legacy = viewConfigRecord(view, 'filter');
		if (!legacy || typeof legacy.field !== 'string') return [];
		return [
			{
				logic: 'all',
				filters: [
					{
						field: legacy.field,
						operator: viewFilterOperator(legacy.operator),
						value: typeof legacy.value === 'string' ? legacy.value : '',
						disabled: viewFilterNodeDisabled(legacy) ? true : undefined
					}
				]
			}
		];
	}

	function syncLegacyViewFilterFields(filterGroups: ViewFilterGroupDraft[]): Pick<
		typeof viewDraft,
		'filters' | 'filterField' | 'filterOperator' | 'filterValue'
	> {
		const filters = filterGroups[0]?.filters ?? [];
		const filter = filters[0];
		return {
			filters,
			filterField: filter?.field ?? '',
			filterOperator: filter?.operator ?? 'contains',
			filterValue: filter?.value ?? ''
		};
	}

	function defaultViewColumns(): string[] {
		return fields
			.filter((field) => field.visibility?.list !== false)
			.slice(0, 8)
			.map((field) => field.key);
	}

	function resetViewDraft(view: FormView | null = selectedView) {
		if (view) {
			const filterGroups = viewDraftFilterGroupsFromView(view);
			const sort = viewConfigRecord(view, 'sort');
			viewDraft = {
				key: view.key,
				name: view.name,
				view_type: view.view_type,
				columns: viewColumns(view),
				filterGroups,
				filterRootLogic: viewDraftFilterRootLogicFromView(view),
				...syncLegacyViewFilterFields(filterGroups),
				sortField: typeof sort?.field === 'string' ? sort.field : '',
					sortDirection: viewSortDirection(sort?.direction),
					groupBy: viewConfigString(view, 'group_by'),
					pivotRowField: viewConfigString(view, 'pivot_row_field'),
					pivotColumnField: viewConfigString(view, 'pivot_column_field'),
					pivotValueField: viewConfigString(view, 'pivot_value_field'),
					pivotAggregate: viewPivotAggregate(view.config?.pivot_aggregate),
					pivotTotals: viewPivotTotalsMode(view.config?.pivot_totals),
					pivotValueDisplay: viewPivotValueDisplay(view.config?.pivot_value_display),
					pivotSort: viewPivotSortMode(view.config?.pivot_sort),
					pivotEmptyBuckets: viewConfigBoolean(view, 'pivot_empty_buckets'),
					pivotHideZeroBuckets: viewConfigBoolean(view, 'pivot_hide_zero_buckets'),
					pivotShowCounts: viewConfigBoolean(view, 'pivot_show_counts'),
					pivotHeatmap: viewConfigBoolean(view, 'pivot_heatmap'),
					pivotRowLimit: viewPivotRowLimit(view.config?.pivot_row_limit),
					pivotColumnLimit: viewPivotColumnLimit(view.config?.pivot_column_limit),
					ganttStartField: viewConfigString(view, 'gantt_start_field'),
					ganttEndField: viewConfigString(view, 'gantt_end_field'),
					ganttGroupBy: viewConfigString(view, 'gantt_group_by'),
					ganttDependencyField: viewConfigString(view, 'gantt_dependency_field'),
					ganttScale: viewGanttScale(view.config?.gantt_scale),
					swimlaneBy: viewConfigString(view, 'swimlane_by'),
				wipLimits: viewDraftWipLimitsFromView(view),
					dateField: viewConfigString(view, 'date_field'),
					calendarRange: viewCalendarRange(view.config?.calendar_range),
					calendarFocusDate: viewCalendarFocusDate(view),
					calendarRecurrence: viewCalendarRecurrence(view.config?.calendar_recurrence),
					calendarDayScope: viewCalendarDayScope(view.config?.calendar_day_scope),
					calendarLayout: viewCalendarLayout(view.config?.calendar_layout),
					calendarHideEmptyDays: viewConfigBoolean(view, 'calendar_hide_empty_days'),
					calendarEndField: viewConfigString(view, 'calendar_end_field'),
					calendarResourceBy: viewConfigString(view, 'calendar_resource_by'),
					calendarExceptionRecordId: '',
					calendarExceptionDate: '',
					calendarExceptionDates: viewDraftCalendarExceptionDatesFromView(view),
					cardTitleField: viewConfigString(view, 'card_title_field'),
						cardSubtitleField: viewConfigString(view, 'card_subtitle_field'),
						cardBadgeField: viewConfigString(view, 'card_badge_field'),
						cardCoverField: viewConfigString(view, 'card_cover_field'),
						cardLayout: viewCardLayout(view.config?.card_layout),
						cardCoverAspect: viewCardCoverAspect(view.config?.card_cover_aspect),
						cardCoverFit: viewCardCoverFit(view.config?.card_cover_fit),
						cardCoverPosition: viewCardCoverPosition(view.config?.card_cover_position),
						cardFieldLimit: viewCardFieldLimit(view.config?.card_field_limit),
						cardHideEmptyFields: viewConfigBoolean(view, 'card_hide_empty_fields'),
						cardShowTimestamps: viewConfigBoolean(view, 'card_show_timestamps'),
						cardShowRecordId: viewConfigBoolean(view, 'card_show_record_id'),
						cardFieldKeys: viewCardFieldKeys(view),
					timelineSource: viewTimelineSource(view.config?.timeline_source),
					timelineEventLayout: timelineEventLayout(view.config?.timeline_event_layout),
					timelineEventType: viewConfigString(view, 'timeline_event_type'),
					timelineEventActor: viewConfigString(view, 'timeline_event_actor'),
					timelineEventAggregate: viewConfigString(view, 'timeline_event_aggregate'),
					timelineEventQuery: viewConfigString(view, 'timeline_event_query'),
					timelineEventFrom: viewConfigString(view, 'timeline_event_from'),
					timelineEventTo: viewConfigString(view, 'timeline_event_to'),
				timelineTitleField: viewConfigString(view, 'timeline_title_field'),
				timelineSubtitleField: viewConfigString(view, 'timeline_subtitle_field'),
				visibility: viewVisibility(view),
				isDefault: viewIsDefault(view)
			};
			return;
		}
		viewDraft = {
			key: '',
			name: '',
			view_type: 'grid',
			columns: defaultViewColumns(),
			filters: [],
			filterRootLogic: 'all',
			filterGroups: [],
			filterField: '',
			filterOperator: 'contains',
			filterValue: '',
			sortField: '',
			sortDirection: 'asc',
			groupBy: '',
			pivotRowField: '',
			pivotColumnField: '',
			pivotValueField: '',
			pivotAggregate: 'sum',
			pivotTotals: 'all',
			pivotValueDisplay: 'value',
			pivotSort: 'label',
			pivotEmptyBuckets: false,
			pivotHideZeroBuckets: false,
			pivotShowCounts: false,
			pivotHeatmap: false,
			pivotRowLimit: 0,
			pivotColumnLimit: 0,
			ganttStartField: '',
			ganttEndField: '',
			ganttGroupBy: '',
			ganttDependencyField: '',
			ganttScale: 'day',
			swimlaneBy: '',
			wipLimits: {},
			dateField: '',
				calendarRange: 'week',
				calendarFocusDate: '',
				calendarRecurrence: 'none',
				calendarDayScope: 'all',
				calendarLayout: 'grid',
				calendarHideEmptyDays: false,
				calendarEndField: '',
				calendarResourceBy: '',
				calendarExceptionRecordId: '',
				calendarExceptionDate: '',
				calendarExceptionDates: {},
				cardTitleField: '',
				cardSubtitleField: '',
				cardBadgeField: '',
				cardCoverField: '',
				cardLayout: 'standard',
				cardCoverAspect: 'auto',
				cardCoverFit: 'cover',
				cardCoverPosition: 'top',
				cardFieldLimit: 6,
				cardHideEmptyFields: false,
				cardShowTimestamps: false,
				cardShowRecordId: false,
				cardFieldKeys: [],
				timelineSource: 'records',
				timelineEventLayout: 'summary',
				timelineEventType: '',
				timelineEventActor: '',
				timelineEventAggregate: '',
				timelineEventQuery: '',
				timelineEventFrom: '',
				timelineEventTo: '',
			timelineTitleField: '',
			timelineSubtitleField: '',
			visibility: 'shared',
			isDefault: false
		};
	}

	function startNewViewDraft() {
		const suffix = Date.now().toString(36);
		viewDraft = {
			key: `view_${suffix}`,
			name: $t('forms.customView'),
			view_type: 'grid',
			columns: defaultViewColumns(),
			filters: [],
			filterRootLogic: 'all',
			filterGroups: [],
			filterField: '',
			filterOperator: 'contains',
			filterValue: '',
				sortField: '',
				sortDirection: 'asc',
				groupBy: '',
				pivotRowField: '',
				pivotColumnField: '',
				pivotValueField: '',
				pivotAggregate: 'sum',
				pivotTotals: 'all',
				pivotValueDisplay: 'value',
				pivotSort: 'label',
				pivotEmptyBuckets: false,
				pivotHideZeroBuckets: false,
				pivotShowCounts: false,
				pivotHeatmap: false,
				pivotRowLimit: 0,
				pivotColumnLimit: 0,
				ganttStartField: '',
				ganttEndField: '',
				ganttGroupBy: '',
				ganttDependencyField: '',
				ganttScale: 'day',
				swimlaneBy: '',
			wipLimits: {},
			dateField: '',
				calendarRange: 'week',
				calendarFocusDate: '',
				calendarRecurrence: 'none',
				calendarDayScope: 'all',
				calendarLayout: 'grid',
				calendarHideEmptyDays: false,
				calendarEndField: '',
				calendarResourceBy: '',
				calendarExceptionRecordId: '',
				calendarExceptionDate: '',
				calendarExceptionDates: {},
				cardTitleField: '',
				cardSubtitleField: '',
				cardBadgeField: '',
				cardCoverField: '',
				cardLayout: 'standard',
				cardCoverAspect: 'auto',
				cardCoverFit: 'cover',
				cardCoverPosition: 'top',
				cardFieldLimit: 6,
				cardHideEmptyFields: false,
				cardShowTimestamps: false,
				cardShowRecordId: false,
				cardFieldKeys: [],
				timelineSource: 'records',
				timelineEventLayout: 'summary',
				timelineEventType: '',
				timelineEventActor: '',
				timelineEventAggregate: '',
				timelineEventQuery: '',
				timelineEventFrom: '',
				timelineEventTo: '',
			timelineTitleField: '',
			timelineSubtitleField: '',
			visibility: 'shared',
			isDefault: false
		};
		selectedViewId = '';
	}

	function setViewDraftColumn(fieldKey: string, checked: boolean) {
		viewDraft = {
			...viewDraft,
			columns: checked
				? Array.from(new Set([...viewDraft.columns, fieldKey]))
				: viewDraft.columns.filter((key) => key !== fieldKey)
		};
	}

	function viewDraftHasColumn(fieldKey: string): boolean {
		return viewDraft.columns.includes(fieldKey);
	}

	function viewDraftColumnFields(): FormField[] {
		const listable = fields.filter((field) => field.visibility?.list !== false);
		const draftColumns = viewDraft.columns.length > 0 ? viewDraft.columns : defaultViewColumns();
		return orderedFieldsByKeys(
			listable.filter((field) => draftColumns.includes(field.key)),
			draftColumns
		);
	}

	function viewDraftCardFields(): FormField[] {
		return orderedFieldsByKeys(viewDraftColumnFields(), viewDraft.cardFieldKeys);
	}

	function validViewDraftCardFieldKeys(): string[] {
		const draftFields = viewDraftCardFields();
		const defaultKeys = viewDraftColumnFields().map((field) => field.key);
		const orderedKeys = draftFields.map((field) => field.key);
		return orderedKeys.some((key, index) => key !== defaultKeys[index]) ? orderedKeys : [];
	}

	function moveViewDraftCardField(fieldIndex: number, direction: -1 | 1) {
		const fieldsInOrder = viewDraftCardFields();
		const targetIndex = fieldIndex + direction;
		if (targetIndex < 0 || targetIndex >= fieldsInOrder.length) return;
		[fieldsInOrder[fieldIndex], fieldsInOrder[targetIndex]] = [
			fieldsInOrder[targetIndex],
			fieldsInOrder[fieldIndex]
		];
		viewDraft = {
			...viewDraft,
			cardFieldKeys: fieldsInOrder.map((field) => field.key)
		};
	}

	function setViewDraftFilterGroups(filterGroups: ViewFilterGroupDraft[]) {
		const nextGroups = filterGroups
			.map((group) => ({
				logic: group.logic,
				filters: group.filters.slice(0, 5),
				disabled: group.disabled || undefined
			}))
			.filter((group) => group.filters.length > 0)
			.slice(0, 5);
		viewDraft = {
			...viewDraft,
			filterGroups: nextGroups,
			...syncLegacyViewFilterFields(nextGroups)
		};
	}

	function updateViewDraftFilterRootLogic(logic: ViewFilterLogic) {
		viewDraft = { ...viewDraft, filterRootLogic: logic };
	}

	function setViewDraftFilters(filters: ViewFilterDraft[], groupIndex = 0) {
		const nextFilters = filters.slice(0, 5);
		const nextGroups =
			viewDraft.filterGroups.length > 0
				? viewDraft.filterGroups.map((group) => ({ ...group, filters: [...group.filters] }))
				: [{ logic: 'all' as ViewFilterLogic, filters: [] }];
		nextGroups[groupIndex] = {
			logic: nextGroups[groupIndex]?.logic ?? 'all',
			filters: nextFilters
		};
		setViewDraftFilterGroups(nextGroups);
	}

	function updateViewDraftFilterGroupLogic(groupIndex: number, logic: ViewFilterLogic) {
		const nextGroups = viewDraft.filterGroups.map((group, itemIndex) =>
			itemIndex === groupIndex ? { ...group, logic } : group
		);
		setViewDraftFilterGroups(nextGroups);
	}

	function toggleViewDraftFilterGroupDisabled(groupIndex: number) {
		syncVisibleFilterGroups();
		const nextGroups = visibleViewDraftFilterGroups().map((group, itemIndex) => ({
			...group,
			filters: group.filters.map((filter) => ({ ...filter })),
			disabled: itemIndex === groupIndex ? !group.disabled : group.disabled || undefined
		}));
		setViewDraftFilterGroups(nextGroups);
	}

	function addViewDraftFilterGroup() {
		if (viewDraft.filterGroups.length >= 5) return;
		setViewDraftFilterGroups([
			...viewDraft.filterGroups,
			{ logic: 'any', filters: [{ field: '', operator: 'contains', value: '' }] }
		]);
	}

	function removeViewDraftFilterGroup(groupIndex: number) {
		setViewDraftFilterGroups(viewDraft.filterGroups.filter((_, itemIndex) => itemIndex !== groupIndex));
	}

	function canDuplicateViewDraftFilterGroup(groupIndex: number): boolean {
		const group = visibleViewDraftFilterGroups()[groupIndex];
		if (!group || group.filters.length === 0) return false;
		return viewDraft.filterGroups.length < 5 && !wouldExceedFilterLimit(group.filters.length);
	}

	function duplicateViewDraftFilterGroup(groupIndex: number) {
		if (!canDuplicateViewDraftFilterGroup(groupIndex)) return;
		syncVisibleFilterGroups();
		const nextGroups = viewDraft.filterGroups.map((group) => ({
			logic: group.logic,
			filters: group.filters.map((filter) => ({ ...filter }))
		}));
		const sourceGroup = nextGroups[groupIndex];
		if (!sourceGroup) return;
		nextGroups.splice(groupIndex + 1, 0, {
			logic: sourceGroup.logic,
			filters: sourceGroup.filters.map((filter) => ({ ...filter }))
		});
		setViewDraftFilterGroups(nextGroups);
	}

	function canMergeViewDraftFilterGroup(groupIndex: number): boolean {
		const groups = visibleViewDraftFilterGroups();
		const sourceGroup = groups[groupIndex];
		const previousGroup = groups[groupIndex - 1];
		if (groupIndex <= 0 || !sourceGroup || !previousGroup) return false;
		return previousGroup.filters.length + sourceGroup.filters.length <= 5;
	}

	function mergeViewDraftFilterGroupWithPrevious(groupIndex: number) {
		if (!canMergeViewDraftFilterGroup(groupIndex)) return;
		const nextGroups = visibleViewDraftFilterGroups().map((group) => ({
			logic: group.logic,
			filters: group.filters.map((filter) => ({ ...filter }))
		}));
		const sourceGroup = nextGroups[groupIndex];
		const previousGroup = nextGroups[groupIndex - 1];
		if (!sourceGroup || !previousGroup) return;
		previousGroup.filters = [...previousGroup.filters, ...sourceGroup.filters.map((filter) => ({ ...filter }))];
		nextGroups.splice(groupIndex, 1);
		setViewDraftFilterGroups(nextGroups);
	}

	function moveViewDraftFilterGroup(groupIndex: number, direction: -1 | 1) {
		const targetIndex = groupIndex + direction;
		if (targetIndex < 0 || targetIndex >= viewDraft.filterGroups.length) return;
		const nextGroups = viewDraft.filterGroups.map((group) => ({
			...group,
			filters: [...group.filters]
		}));
		[nextGroups[groupIndex], nextGroups[targetIndex]] = [nextGroups[targetIndex], nextGroups[groupIndex]];
		setViewDraftFilterGroups(nextGroups);
	}

	function visibleViewDraftFilterGroups(): ViewFilterGroupDraft[] {
		if (viewDraft.filterGroups.length > 0) return viewDraft.filterGroups;
		return [{ logic: 'all', filters: viewDraft.filters }];
	}

	function filterLabelSuffix(groupIndex: number, filterIndex: number): string {
		return groupIndex === 0 ? String(filterIndex + 1) : `${groupIndex + 1}.${filterIndex + 1}`;
	}

	function hasReachedFilterLimit(): boolean {
		return viewDraft.filterGroups.reduce((count, group) => count + group.filters.length, 0) >= 10;
	}

	function wouldExceedFilterLimit(extraCount: number): boolean {
		return (
			viewDraft.filterGroups.reduce((count, group) => count + group.filters.length, 0) + extraCount >
			10
		);
	}

	function syncVisibleFilterGroups() {
		if (viewDraft.filterGroups.length === 0) {
			setViewDraftFilterGroups([{ logic: 'all', filters: viewDraft.filters }]);
		}
	}

	function updateViewDraftFilter(index: number, patch: Partial<ViewFilterDraft>, groupIndex = 0) {
		syncVisibleFilterGroups();
		const sourceGroup = visibleViewDraftFilterGroups()[groupIndex] ?? { logic: 'all', filters: [] };
		const nextFilters = sourceGroup.filters.map((filter, itemIndex) =>
			itemIndex === index ? { ...filter, ...patch } : filter
		);
		setViewDraftFilters(nextFilters, groupIndex);
	}

	function toggleViewDraftFilterDisabled(groupIndex: number, filterIndex: number) {
		const sourceGroup = visibleViewDraftFilterGroups()[groupIndex] ?? { logic: 'all', filters: [] };
		const sourceFilter = sourceGroup.filters[filterIndex];
		if (!sourceFilter) return;
		updateViewDraftFilter(filterIndex, { disabled: !sourceFilter.disabled }, groupIndex);
	}

	function addViewDraftFilter(groupIndex = 0) {
		if (hasReachedFilterLimit()) return;
		syncVisibleFilterGroups();
		const sourceGroup = visibleViewDraftFilterGroups()[groupIndex] ?? { logic: 'all', filters: [] };
		setViewDraftFilters(
			[...sourceGroup.filters, { field: '', operator: 'contains', value: '' }],
			groupIndex
		);
	}

	function removeViewDraftFilter(index: number, groupIndex = 0) {
		const sourceGroup = visibleViewDraftFilterGroups()[groupIndex] ?? { logic: 'all', filters: [] };
		setViewDraftFilters(
			sourceGroup.filters.filter((_, itemIndex) => itemIndex !== index),
			groupIndex
		);
	}

	function duplicateViewDraftFilter(groupIndex: number, filterIndex: number) {
		if (hasReachedFilterLimit()) return;
		syncVisibleFilterGroups();
		const sourceGroup = visibleViewDraftFilterGroups()[groupIndex] ?? { logic: 'all', filters: [] };
		const sourceFilter = sourceGroup.filters[filterIndex];
		if (!sourceFilter) return;
		const nextFilters = sourceGroup.filters.map((filter) => ({ ...filter }));
		nextFilters.splice(filterIndex + 1, 0, { ...sourceFilter });
		setViewDraftFilters(nextFilters, groupIndex);
	}

	function canSplitViewDraftFilterToGroup(groupIndex: number, filterIndex: number): boolean {
		const sourceGroup = visibleViewDraftFilterGroups()[groupIndex];
		return Boolean(
			sourceGroup &&
				sourceGroup.filters[filterIndex] &&
				sourceGroup.filters.length > 1 &&
				visibleViewDraftFilterGroups().length < 5
		);
	}

	function splitViewDraftFilterToGroup(groupIndex: number, filterIndex: number) {
		if (!canSplitViewDraftFilterToGroup(groupIndex, filterIndex)) return;
		const nextGroups = visibleViewDraftFilterGroups().map((group) => ({
			logic: group.logic,
			filters: group.filters.map((filter) => ({ ...filter }))
		}));
		const sourceGroup = nextGroups[groupIndex];
		if (!sourceGroup) return;
		const [sourceFilter] = sourceGroup.filters.splice(filterIndex, 1);
		if (!sourceFilter) return;
		nextGroups.splice(groupIndex + 1, 0, {
			logic: sourceGroup.logic,
			filters: [{ ...sourceFilter }]
		});
		setViewDraftFilterGroups(nextGroups);
	}

	function moveViewDraftFilter(groupIndex: number, filterIndex: number, direction: -1 | 1) {
		const sourceGroup = visibleViewDraftFilterGroups()[groupIndex] ?? { logic: 'all', filters: [] };
		const targetIndex = filterIndex + direction;
		if (targetIndex < 0 || targetIndex >= sourceGroup.filters.length) return;
		const nextFilters = sourceGroup.filters.map((filter) => ({ ...filter }));
		[nextFilters[filterIndex], nextFilters[targetIndex]] = [
			nextFilters[targetIndex],
			nextFilters[filterIndex]
		];
		setViewDraftFilters(nextFilters, groupIndex);
	}

	function validViewFilterGroups(): ViewFilterGroupDraft[] {
		return viewDraft.filterGroups
			.map((group) => ({
				logic: group.logic,
				filters: group.filters
					.filter((filter) => filter.field && (filter.operator === 'not_empty' || filter.value.trim()))
					.map((filter) => ({
						field: filter.field,
						operator: filter.operator,
						value: filter.operator === 'not_empty' ? '' : filter.value.trim(),
						disabled: filter.disabled || undefined
					})),
				disabled: group.disabled || undefined
			}))
			.filter((group) => group.filters.length > 0);
	}

	function viewFilterExpressionConfigFromGroups(
		filterGroups: ViewFilterGroupDraft[]
	): Record<string, unknown> | null {
		if (filterGroups.length === 0) return null;
		return {
			logic: viewDraft.filterRootLogic,
			children: filterGroups.map((group) => ({
				logic: group.logic,
				disabled: group.disabled || undefined,
				children: group.filters.map((filter) => ({
					disabled: filter.disabled || undefined,
					filter: {
						field: filter.field,
						operator: filter.operator,
						value: filter.operator === 'not_empty' ? '' : filter.value.trim()
					}
				}))
			}))
		};
	}

	function viewDraftKanbanGroupField(): FormField | null {
		return fieldByKey(viewDraft.groupBy);
	}

	function viewDraftSwimlaneFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'kanban') return [];
		return fields.filter(
			(field) => field.visibility?.list !== false && field.key !== viewDraft.groupBy
		);
	}

	function validViewDraftSwimlaneBy(): string {
		if (viewDraft.view_type !== 'kanban' || !viewDraft.swimlaneBy) return '';
		if (viewDraft.swimlaneBy === viewDraft.groupBy) return '';
		return viewDraftSwimlaneFieldCandidates().some((field) => field.key === viewDraft.swimlaneBy)
			? viewDraft.swimlaneBy
			: '';
	}

	function viewDraftCalendarResourceFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'calendar') return [];
		return fields.filter(
			(field) => field.visibility?.list !== false && field.key !== viewDraft.dateField
		);
	}

	function viewDraftCalendarEndFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'calendar') return [];
		return fields.filter(
			(field) =>
				field.visibility?.list !== false &&
				field.key !== viewDraft.dateField &&
				['date', 'datetime'].includes(field.type ?? 'text')
		);
	}

	function validViewDraftCalendarEndField(): string {
		if (viewDraft.view_type !== 'calendar' || !viewDraft.calendarEndField) return '';
		if (viewDraft.calendarEndField === viewDraft.dateField) return '';
		return viewDraftCalendarEndFieldCandidates().some((field) => field.key === viewDraft.calendarEndField)
			? viewDraft.calendarEndField
			: '';
	}

	function validViewDraftCalendarResourceBy(): string {
		if (viewDraft.view_type !== 'calendar' || !viewDraft.calendarResourceBy) return '';
		if (viewDraft.calendarResourceBy === viewDraft.dateField) return '';
		return viewDraftCalendarResourceFieldCandidates().some(
			(field) => field.key === viewDraft.calendarResourceBy
		)
			? viewDraft.calendarResourceBy
			: '';
	}

	function viewDraftPivotDimensionFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'pivot') return [];
		return fields.filter((field) => field.visibility?.list !== false);
	}

	function viewDraftPivotValueFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'pivot') return [];
		return fields.filter(
			(field) => field.visibility?.list !== false && ['amount', 'number', 'integer'].includes(field.type ?? 'text')
		);
	}

	function validViewDraftPivotRowField(): string {
		if (viewDraft.view_type !== 'pivot' || !viewDraft.pivotRowField) return '';
		return viewDraftPivotDimensionFieldCandidates().some((field) => field.key === viewDraft.pivotRowField)
			? viewDraft.pivotRowField
			: '';
	}

	function validViewDraftPivotColumnField(): string {
		if (viewDraft.view_type !== 'pivot' || !viewDraft.pivotColumnField) return '';
		if (viewDraft.pivotColumnField === viewDraft.pivotRowField) return '';
		return viewDraftPivotDimensionFieldCandidates().some((field) => field.key === viewDraft.pivotColumnField)
			? viewDraft.pivotColumnField
			: '';
	}

	function validViewDraftPivotValueField(): string {
		if (viewDraft.view_type !== 'pivot' || !viewDraft.pivotValueField) return '';
		return viewDraftPivotValueFieldCandidates().some((field) => field.key === viewDraft.pivotValueField)
			? viewDraft.pivotValueField
			: '';
	}

	function swapViewDraftPivotAxes() {
		if (viewDraft.view_type !== 'pivot' || !viewDraft.pivotRowField || !viewDraft.pivotColumnField) {
			return;
		}
		viewDraft = {
			...viewDraft,
			pivotRowField: viewDraft.pivotColumnField,
			pivotColumnField: viewDraft.pivotRowField
		};
	}

	function viewDraftGanttDateFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'gantt') return [];
		return fields.filter(
			(field) => field.visibility?.list !== false && ['date', 'datetime'].includes(field.type ?? 'text')
		);
	}

	function viewDraftGanttGroupFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'gantt') return [];
		return fields.filter(
			(field) =>
				field.visibility?.list !== false &&
				field.key !== viewDraft.ganttStartField &&
				field.key !== viewDraft.ganttEndField
		);
	}

	function viewDraftGanttDependencyFieldCandidates(): FormField[] {
		if (viewDraft.view_type !== 'gantt') return [];
		return fields.filter(
			(field) =>
				field.visibility?.list !== false &&
				field.key !== viewDraft.ganttStartField &&
				field.key !== viewDraft.ganttEndField &&
				field.key !== viewDraft.ganttGroupBy
		);
	}

	function validViewDraftGanttStartField(): string {
		if (viewDraft.view_type !== 'gantt' || !viewDraft.ganttStartField) return '';
		return viewDraftGanttDateFieldCandidates().some((field) => field.key === viewDraft.ganttStartField)
			? viewDraft.ganttStartField
			: '';
	}

	function validViewDraftGanttEndField(): string {
		if (viewDraft.view_type !== 'gantt' || !viewDraft.ganttEndField) return '';
		if (viewDraft.ganttEndField === viewDraft.ganttStartField) return '';
		return viewDraftGanttDateFieldCandidates().some((field) => field.key === viewDraft.ganttEndField)
			? viewDraft.ganttEndField
			: '';
	}

	function validViewDraftGanttGroupBy(): string {
		if (viewDraft.view_type !== 'gantt' || !viewDraft.ganttGroupBy) return '';
		return viewDraftGanttGroupFieldCandidates().some((field) => field.key === viewDraft.ganttGroupBy)
			? viewDraft.ganttGroupBy
			: '';
	}

	function validViewDraftGanttDependencyField(): string {
		if (viewDraft.view_type !== 'gantt' || !viewDraft.ganttDependencyField) return '';
		return viewDraftGanttDependencyFieldCandidates().some((field) => field.key === viewDraft.ganttDependencyField)
			? viewDraft.ganttDependencyField
			: '';
	}

	function validViewDraftCalendarExceptionDates(): CalendarExceptionDateMap {
		const next = calendarExceptionDateMap(viewDraft.calendarExceptionDates);
		const recordId = viewDraft.calendarExceptionRecordId.trim();
		const date = viewDraft.calendarExceptionDate.trim();
		if (recordId && isIsoDate(date)) {
			next[recordId] = Array.from(new Set([...(next[recordId] ?? []), date])).sort();
		}
		return next;
	}

	function calendarExceptionRecordLabel(recordId: string): string {
		const record = records.find((item) => item.id === recordId);
		return record ? record.title : recordId;
	}

	function viewDraftCalendarExceptionItems(): Array<{ recordId: string; date: string; label: string }> {
		return Object.entries(validViewDraftCalendarExceptionDates())
			.flatMap(([recordId, dates]) =>
				dates.map((date) => ({
					recordId,
					date,
					label: calendarExceptionRecordLabel(recordId)
				}))
			)
			.sort((left, right) => left.date.localeCompare(right.date) || left.label.localeCompare(right.label));
	}

	function addViewDraftCalendarException() {
		const recordId = viewDraft.calendarExceptionRecordId.trim();
		const date = viewDraft.calendarExceptionDate.trim();
		if (!recordId || !isIsoDate(date)) return;
		const nextDates = new Set(viewDraft.calendarExceptionDates[recordId] ?? []);
		nextDates.add(date);
		viewDraft = {
			...viewDraft,
			calendarExceptionDate: '',
			calendarExceptionDates: {
				...viewDraft.calendarExceptionDates,
				[recordId]: Array.from(nextDates).sort()
			}
		};
	}

	function removeViewDraftCalendarException(recordId: string, date: string) {
		const next = { ...viewDraft.calendarExceptionDates };
		const dates = (next[recordId] ?? []).filter((item) => item !== date);
		if (dates.length > 0) {
			next[recordId] = dates;
		} else {
			delete next[recordId];
		}
		viewDraft = { ...viewDraft, calendarExceptionDates: next };
	}

	function viewDraftKanbanWipOptions(): string[] {
		const field = viewDraftKanbanGroupField();
		if (viewDraft.view_type !== 'kanban' || field?.type !== 'single_select') return [];
		return kanbanMoveOptions(field);
	}

	function setViewDraftWipLimit(option: string, value: string) {
		const nextLimits = { ...viewDraft.wipLimits };
		const trimmed = value.trim();
		if (!trimmed) {
			delete nextLimits[option];
		} else {
			nextLimits[option] = trimmed;
		}
		viewDraft = { ...viewDraft, wipLimits: nextLimits };
	}

	function validViewDraftWipLimits(): Record<string, number> {
		const limits: Record<string, number> = {};
		for (const option of viewDraftKanbanWipOptions()) {
			const raw = viewDraft.wipLimits[option];
			if (!raw?.trim()) continue;
			const limit = Number(raw);
			if (Number.isFinite(limit) && limit >= 0) {
				limits[option] = Math.floor(limit);
			}
		}
		return limits;
	}

	function viewConfigFromDraft(): Record<string, unknown> {
		const config: Record<string, unknown> = {
			columns: viewDraft.columns.length > 0 ? viewDraft.columns : defaultViewColumns(),
			density: 'compact',
			visibility: viewDraft.visibility
		};
		const filterGroups = validViewFilterGroups();
		const filters = filterGroups[0]?.filters ?? [];
		if (filters.length > 0) {
			const filterExpression = viewFilterExpressionConfigFromGroups(filterGroups);
			if (filterExpression) config.filter_expression = filterExpression;
			config.filter_groups = filterGroups;
			config.filters = filters;
			config.filter = filters[0];
		}
		if (viewDraft.sortField) {
			config.sort = {
				field: viewDraft.sortField,
				direction: viewDraft.sortDirection
			};
		}
		if (viewDraft.groupBy) config.group_by = viewDraft.groupBy;
		const swimlaneBy = validViewDraftSwimlaneBy();
		if (swimlaneBy) config.swimlane_by = swimlaneBy;
		const wipLimits = validViewDraftWipLimits();
		if (Object.keys(wipLimits).length > 0) config.wip_limits = wipLimits;
		if (viewDraft.dateField) config.date_field = viewDraft.dateField;
		if (viewDraft.view_type === 'pivot') {
			const pivotRowField = validViewDraftPivotRowField();
			const pivotColumnField = validViewDraftPivotColumnField();
			const pivotValueField = validViewDraftPivotValueField();
			if (pivotRowField) config.pivot_row_field = pivotRowField;
			if (pivotColumnField) config.pivot_column_field = pivotColumnField;
			if (pivotValueField) config.pivot_value_field = pivotValueField;
			config.pivot_aggregate = viewDraft.pivotAggregate;
			config.pivot_totals = viewDraft.pivotTotals;
			config.pivot_value_display = viewDraft.pivotValueDisplay;
			config.pivot_sort = viewDraft.pivotSort;
			if (viewDraft.pivotEmptyBuckets) config.pivot_empty_buckets = true;
			if (viewDraft.pivotHideZeroBuckets) config.pivot_hide_zero_buckets = true;
			if (viewDraft.pivotShowCounts) config.pivot_show_counts = true;
			if (viewDraft.pivotHeatmap) config.pivot_heatmap = true;
			if (viewDraft.pivotRowLimit > 0) config.pivot_row_limit = viewDraft.pivotRowLimit;
			if (viewDraft.pivotColumnLimit > 0) config.pivot_column_limit = viewDraft.pivotColumnLimit;
		}
		if (viewDraft.view_type === 'gantt') {
			const ganttStartField = validViewDraftGanttStartField();
			const ganttEndField = validViewDraftGanttEndField();
			const ganttGroupBy = validViewDraftGanttGroupBy();
			const ganttDependencyField = validViewDraftGanttDependencyField();
			config.gantt_scale = viewDraft.ganttScale;
			if (ganttStartField) config.gantt_start_field = ganttStartField;
			if (ganttEndField) config.gantt_end_field = ganttEndField;
			if (ganttGroupBy) config.gantt_group_by = ganttGroupBy;
			if (ganttDependencyField) config.gantt_dependency_field = ganttDependencyField;
		}
			if (viewDraft.view_type === 'calendar') {
				config.calendar_range = viewDraft.calendarRange;
				if (isIsoDate(viewDraft.calendarFocusDate)) config.calendar_focus_date = viewDraft.calendarFocusDate;
				config.calendar_day_scope = viewDraft.calendarDayScope;
				config.calendar_layout = viewDraft.calendarLayout;
				if (viewDraft.calendarHideEmptyDays) config.calendar_hide_empty_days = true;
				const calendarEndField = validViewDraftCalendarEndField();
			if (calendarEndField) config.calendar_end_field = calendarEndField;
			if (viewDraft.calendarRecurrence !== 'none') {
				config.calendar_recurrence = viewDraft.calendarRecurrence;
				}
				const calendarResourceBy = validViewDraftCalendarResourceBy();
				if (calendarResourceBy) config.calendar_resource_by = calendarResourceBy;
				const calendarExceptionDates = validViewDraftCalendarExceptionDates();
				if (Object.keys(calendarExceptionDates).length > 0) {
					config.calendar_exception_dates = calendarExceptionDates;
				}
			}
			if (viewDraft.view_type === 'card') {
				if (viewDraft.cardTitleField) config.card_title_field = viewDraft.cardTitleField;
				if (viewDraft.cardSubtitleField && viewDraft.cardSubtitleField !== viewDraft.cardTitleField) {
					config.card_subtitle_field = viewDraft.cardSubtitleField;
				}
				if (
					viewDraft.cardBadgeField &&
					viewDraft.cardBadgeField !== viewDraft.cardTitleField &&
					viewDraft.cardBadgeField !== viewDraft.cardSubtitleField
				) {
					config.card_badge_field = viewDraft.cardBadgeField;
				}
				if (viewDraft.cardCoverField) config.card_cover_field = viewDraft.cardCoverField;
				if (viewDraft.cardLayout !== 'standard') config.card_layout = viewDraft.cardLayout;
				if (viewDraft.cardCoverAspect !== 'auto') config.card_cover_aspect = viewDraft.cardCoverAspect;
				if (viewDraft.cardCoverFit !== 'cover') config.card_cover_fit = viewDraft.cardCoverFit;
					if (viewDraft.cardCoverPosition !== 'top') config.card_cover_position = viewDraft.cardCoverPosition;
					if (viewDraft.cardFieldLimit !== 6) config.card_field_limit = viewDraft.cardFieldLimit;
					if (viewDraft.cardHideEmptyFields) config.card_hide_empty_fields = true;
					if (viewDraft.cardShowTimestamps) config.card_show_timestamps = true;
					if (viewDraft.cardShowRecordId) config.card_show_record_id = true;
					const cardFieldKeys = validViewDraftCardFieldKeys();
				if (cardFieldKeys.length > 0) config.card_fields = cardFieldKeys;
			}
			if (viewDraft.view_type === 'timeline') {
				config.timeline_source = viewDraft.timelineSource;
				if (viewDraft.timelineSource === 'records' && viewDraft.timelineTitleField) config.timeline_title_field = viewDraft.timelineTitleField;
				if (
					viewDraft.timelineSource === 'records' &&
					viewDraft.timelineSubtitleField &&
					viewDraft.timelineSubtitleField !== viewDraft.timelineTitleField
				) {
					config.timeline_subtitle_field = viewDraft.timelineSubtitleField;
				}
				if (viewDraft.timelineSource === 'events') {
					config.timeline_event_layout = viewDraft.timelineEventLayout;
					if (viewDraft.timelineEventType) config.timeline_event_type = viewDraft.timelineEventType;
					if (viewDraft.timelineEventActor) config.timeline_event_actor = viewDraft.timelineEventActor;
					if (viewDraft.timelineEventAggregate) config.timeline_event_aggregate = viewDraft.timelineEventAggregate;
					if (viewDraft.timelineEventQuery.trim()) config.timeline_event_query = viewDraft.timelineEventQuery.trim();
					if (viewDraft.timelineEventFrom.trim()) config.timeline_event_from = viewDraft.timelineEventFrom.trim();
					if (viewDraft.timelineEventTo.trim()) config.timeline_event_to = viewDraft.timelineEventTo.trim();
				}
			}
			config.is_default = viewDraft.isDefault;
			return config;
		}

	async function saveViewDraft() {
		if (!selectedForm) return;
		const key = viewDraft.key.trim();
		const name = viewDraft.name.trim();
		if (!key || !name) {
			toast.error($t('forms.viewNameRequired'));
			return;
		}
		viewSubmitting = true;
		const response = selectedViewId
			? await formsApi.updateView(selectedViewId, {
					name,
					view_type: viewDraft.view_type,
					config: viewConfigFromDraft(),
					expected_updated_at: selectedView?.id === selectedViewId ? selectedView.updated_at : undefined
				})
			: await formsApi.createView(selectedForm.id, {
					key,
					name,
					view_type: viewDraft.view_type,
					config: viewConfigFromDraft()
				});
		viewSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		const nextViews = selectedViewId
			? formViews.map((view) => (view.id === response.data?.id ? response.data : view))
			: [...formViews, response.data];
		formViews = viewIsDefault(response.data)
			? normalizeLocalDefaultViews(nextViews, response.data.id)
			: sortViewsByDefault(nextViews);
		selectedViewId = response.data.id;
		resetViewDraft(response.data);
		await loadRecords();
		toast.success($t('forms.viewSaved'));
	}

	async function deleteSelectedView() {
		if (!selectedView) return;
		deletingViewId = selectedView.id;
		const response = await formsApi.deleteView(selectedView.id);
		deletingViewId = null;
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		formViews = formViews.filter((view) => view.id !== selectedView.id);
		selectedViewId = formViews[0]?.id ?? '';
		resetViewDraft();
		toast.success($t('forms.viewDeleted'));
	}

		async function exportCurrentView() {
			if (!selectedForm) return;
			exportingRecords = true;
		const response = await formsApi.exportRecords(selectedForm.id, {
			view_id: selectedViewId || undefined,
			columns: selectedViewId ? undefined : visibleFields.map((field) => field.key),
			format: 'csv'
		});
		exportingRecords = false;
		if (response.code !== 0 || !response.data || !response.data.csv) {
			toast.error(response.message || $t('forms.exportFailed'));
			return;
		}
		const blob = new Blob([response.data.csv], { type: 'text/csv;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = response.data.file_name;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
			toast.success($t('forms.recordsExported'));
		}

		async function exportCurrentViewJson() {
			if (!selectedForm) return;
			exportingRecords = true;
		const response = await formsApi.exportRecords(selectedForm.id, {
			view_id: selectedViewId || undefined,
			columns: selectedViewId ? undefined : visibleFields.map((field) => field.key),
			format: 'json'
		});
		exportingRecords = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message || $t('forms.exportFailed'));
			return;
		}
		const blob = new Blob([JSON.stringify(response.data, null, 2)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = response.data.file_name;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		exportedCurrentViewJsonRecordCount = response.data.rows.length;
			toast.success($t('forms.recordsJsonExported'));
		}

		function exportAttachmentManifest() {
			if (!selectedForm) return;
			const attachmentFields = fields.filter((field) => ['attachment', 'image'].includes(fieldType(field)));
			const attachments = viewRecords.flatMap((record) =>
				attachmentFields.flatMap((field) => {
					const value = record.values?.[field.key];
					const urls = attachmentManifestUrls(value);
					return urls.map((url) => ({
						record_id: record.id,
						record_title: record.title,
						field_key: field.key,
						field_label: fieldLabel(field),
						field_type: fieldType(field),
						url,
						file_name: attachmentFileName(url, field),
						content_type: attachmentContentType(attachmentFileName(url, field), field)
					}));
				})
			);
			const manifest = {
				form_id: selectedForm.id,
				form_key: selectedForm.key,
				view_id: selectedViewId || null,
				view_name: selectedView?.name ?? null,
				exported_at: new Date().toISOString(),
				record_count: viewRecords.length,
				attachment_count: attachments.length,
				attachments
			};
			const blob = new Blob([JSON.stringify(manifest, null, 2)], {
				type: 'application/json;charset=utf-8'
			});
			const url = URL.createObjectURL(blob);
			const anchor = document.createElement('a');
			anchor.href = url;
			anchor.download = `${selectedForm.key}-attachment-manifest.json`;
			document.body.appendChild(anchor);
			anchor.click();
			anchor.remove();
			URL.revokeObjectURL(url);
			exportedAttachmentManifestCount = attachments.length;
			toast.success($t('forms.attachmentManifestExported'));
		}

		async function collectAttachmentBundle(): Promise<AttachmentBundleExport | null> {
			if (!selectedForm) return null;
			const attachmentFields = fields.filter((field) => ['attachment', 'image'].includes(fieldType(field)));
			const recordIds = new Set(viewRecords.map((record) => record.id));
			exportingRecords = true;
			const metadataResponse = await formsApi.listAttachments(selectedForm.id, { per_page: 200 });
			exportingRecords = false;
			if (metadataResponse.code !== 0 || !metadataResponse.data) {
				toast.error(metadataResponse.message || $t('forms.exportFailed'));
				return null;
			}
			const metadataAttachments = metadataResponse.data.items.filter(
				(attachment) =>
					!attachment.archived_at &&
					attachment.record_id &&
					recordIds.has(attachment.record_id) &&
					attachmentFields.some((field) => field.key === attachment.field_key)
			);
			const signedEntries = await Promise.all(
				metadataAttachments.map(async (attachment) => {
					const response = await formsApi.getAttachmentSignedDownload(attachment.id);
					return [
						attachment.id,
						response.code === 0 && response.data?.url ? response.data.url : attachment.url
					] as const;
				})
			);
			const signedUrls = Object.fromEntries(signedEntries);
			const metadataByValue = new Map(
				metadataAttachments.map((attachment) => [
					`${attachment.record_id ?? ''}:${attachment.field_key}:${attachment.url}`,
					attachment
				])
			);
			const bundledKeys = new Set<string>();
			const attachments: AttachmentBundleEntry[] = viewRecords.flatMap((record) =>
				attachmentFields.flatMap((field) => {
					const value = record.values?.[field.key];
					const urls = attachmentManifestUrls(value);
					return urls.map((url) => {
						const metadata = metadataByValue.get(`${record.id}:${field.key}:${url}`);
						const bundleKey = `${record.id}:${field.key}:${url}`;
						bundledKeys.add(bundleKey);
						return {
							record_id: record.id,
							record_title: record.title,
							field_key: field.key,
							field_label: fieldLabel(field),
							field_type: fieldType(field),
							url,
							download_url: metadata ? signedUrls[metadata.id] || metadata.url : url,
							thumbnail_url: metadata?.thumbnail_url ?? null,
							file_name: metadata?.file_name ?? attachmentFileName(url, field),
							content_type: metadata?.content_type || attachmentContentType(metadata?.file_name ?? attachmentFileName(url, field), field),
							byte_size: metadata?.byte_size ?? 0,
							storage_key: metadata?.storage_key ?? null,
							attachment_id: metadata?.id ?? null,
							source: metadata ? 'metadata' : 'record_value'
						};
					});
				})
			);
			for (const attachment of metadataAttachments) {
				const bundleKey = `${attachment.record_id ?? ''}:${attachment.field_key}:${attachment.url}`;
				if (bundledKeys.has(bundleKey)) continue;
				const record = viewRecords.find((item) => item.id === attachment.record_id);
				const field = attachmentFields.find((item) => item.key === attachment.field_key);
				attachments.push({
					record_id: attachment.record_id ?? '',
					record_title: record?.title ?? '',
					field_key: attachment.field_key,
					field_label: field ? fieldLabel(field) : attachment.field_key,
					field_type: field ? fieldType(field) : 'attachment',
					url: attachment.url,
					download_url: signedUrls[attachment.id] || attachment.url,
					thumbnail_url: attachment.thumbnail_url ?? null,
					file_name: attachment.file_name,
					content_type: attachment.content_type,
					byte_size: attachment.byte_size,
					storage_key: attachment.storage_key,
					attachment_id: attachment.id,
					source: 'metadata'
				});
			}
			return {
				form_id: selectedForm.id,
				form_key: selectedForm.key,
				view_id: selectedViewId || null,
				view_name: selectedView?.name ?? null,
				exported_at: new Date().toISOString(),
				record_count: viewRecords.length,
				attachment_count: attachments.length,
				bundle_format: 'attachment-bundle-json-v1',
				attachments
			};
		}

		async function exportAttachmentBundle() {
			const bundle = await collectAttachmentBundle();
			if (!bundle) return;
			const blob = new Blob([JSON.stringify(bundle, null, 2)], {
				type: 'application/json;charset=utf-8'
			});
			const url = URL.createObjectURL(blob);
			const anchor = document.createElement('a');
			anchor.href = url;
			anchor.download = `${bundle.form_key}-attachment-bundle.json`;
			document.body.appendChild(anchor);
			anchor.click();
			anchor.remove();
			URL.revokeObjectURL(url);
			exportedAttachmentBundleCount = bundle.attachments.length;
			toast.success($t('forms.attachmentBundleExported'));
		}

		async function exportAttachmentPackage() {
			if (!selectedForm) return;
			const bundle = await collectAttachmentBundle();
			if (!bundle) return;
			const packagedFiles = await Promise.all(
				bundle.attachments.map(async (attachment, index) => {
					const baseName = `${String(index + 1).padStart(3, '0')}-${safeZipSegment(attachment.file_name)}`;
					const bytes = await fetchAttachmentPackageBytes(attachment);
					if (bytes) {
						return {
							file: {
								path: `files/${baseName}`,
								data: bytes
							},
							manifest: {
								...attachment,
								package_path: `files/${baseName}`,
								package_status: 'included'
							}
						};
					}
					return {
						file: {
							path: `links/${baseName}.url.txt`,
							data: `${attachment.download_url || attachment.url}\n`
						},
						manifest: {
							...attachment,
							package_path: `links/${baseName}.url.txt`,
							package_status: 'external_url'
						}
					};
				})
			);
			const packageManifest = {
				...bundle,
				bundle_format: 'attachment-zip-package-v1',
				file_count: packagedFiles.length,
				binary_file_count: packagedFiles.filter((entry) => entry.manifest.package_status === 'included')
					.length,
				attachments: packagedFiles.map((entry) => entry.manifest)
			};
			const zip = createStoredZipBlob([
				{
					path: 'manifest.json',
					data: JSON.stringify(packageManifest, null, 2)
				},
				...packagedFiles.map((entry) => entry.file)
			]);
			const url = URL.createObjectURL(zip);
			const anchor = document.createElement('a');
			anchor.href = url;
			anchor.download = `${selectedForm.key}-attachment-package.zip`;
			document.body.appendChild(anchor);
			anchor.click();
			anchor.remove();
			URL.revokeObjectURL(url);
			exportedAttachmentPackageCount = bundle.attachments.length;
			exportedAttachmentPackageFileCount = packagedFiles.length;
			toast.success($t('forms.attachmentPackageExported'));
		}

		async function fetchAttachmentPackageBytes(
			attachment: AttachmentBundleEntry
		): Promise<Uint8Array | null> {
			const directUrl = sameOriginUrl(attachment.url);
			const signedUrl = sameOriginUrl(attachment.download_url);
			for (const candidate of [directUrl, signedUrl].filter(Boolean)) {
				try {
					const response = await fetch(candidate, { redirect: 'manual' });
					if (!response.ok) continue;
					return new Uint8Array(await response.arrayBuffer());
				} catch {
					continue;
				}
			}
			return null;
		}

		function sameOriginUrl(value: string | null | undefined): string {
			if (!value) return '';
			try {
				const url = new URL(value, window.location.origin);
				return url.origin === window.location.origin ? url.toString() : '';
			} catch {
				return '';
			}
		}

		function safeZipSegment(value: string): string {
			const safe = value
				.trim()
				.replaceAll('\\', '-')
				.replaceAll('/', '-')
				.replace(/[^a-zA-Z0-9._-]+/g, '-')
				.replace(/^-+|-+$/g, '');
			return safe || 'attachment.bin';
		}

		function createStoredZipBlob(files: ZipFileInput[]): Blob {
			const encoder = new TextEncoder();
			const localParts: Uint8Array[] = [];
			const centralParts: Uint8Array[] = [];
			let offset = 0;

			for (const file of files) {
				const nameBytes = encoder.encode(file.path);
				const dataBytes = typeof file.data === 'string' ? encoder.encode(file.data) : file.data;
				const crc = crc32(dataBytes);
				const localHeader = new Uint8Array(30 + nameBytes.length);
				const localView = new DataView(localHeader.buffer);
				localView.setUint32(0, 0x04034b50, true);
				localView.setUint16(4, 20, true);
				localView.setUint16(6, 0, true);
				localView.setUint16(8, 0, true);
				localView.setUint16(10, 0, true);
				localView.setUint16(12, 0, true);
				localView.setUint32(14, crc, true);
				localView.setUint32(18, dataBytes.length, true);
				localView.setUint32(22, dataBytes.length, true);
				localView.setUint16(26, nameBytes.length, true);
				localView.setUint16(28, 0, true);
				localHeader.set(nameBytes, 30);
				localParts.push(localHeader, dataBytes);

				const centralHeader = new Uint8Array(46 + nameBytes.length);
				const centralView = new DataView(centralHeader.buffer);
				centralView.setUint32(0, 0x02014b50, true);
				centralView.setUint16(4, 20, true);
				centralView.setUint16(6, 20, true);
				centralView.setUint16(8, 0, true);
				centralView.setUint16(10, 0, true);
				centralView.setUint16(12, 0, true);
				centralView.setUint16(14, 0, true);
				centralView.setUint32(16, crc, true);
				centralView.setUint32(20, dataBytes.length, true);
				centralView.setUint32(24, dataBytes.length, true);
				centralView.setUint16(28, nameBytes.length, true);
				centralView.setUint16(30, 0, true);
				centralView.setUint16(32, 0, true);
				centralView.setUint16(34, 0, true);
				centralView.setUint16(36, 0, true);
				centralView.setUint32(38, 0, true);
				centralView.setUint32(42, offset, true);
				centralHeader.set(nameBytes, 46);
				centralParts.push(centralHeader);
				offset += localHeader.length + dataBytes.length;
			}

			const centralOffset = offset;
			const centralSize = centralParts.reduce((sum, part) => sum + part.length, 0);
			const end = new Uint8Array(22);
			const endView = new DataView(end.buffer);
			endView.setUint32(0, 0x06054b50, true);
			endView.setUint16(4, 0, true);
			endView.setUint16(6, 0, true);
			endView.setUint16(8, files.length, true);
			endView.setUint16(10, files.length, true);
			endView.setUint32(12, centralSize, true);
			endView.setUint32(16, centralOffset, true);
			endView.setUint16(20, 0, true);

			return new Blob([...localParts, ...centralParts, end].map(zipBlobPart), {
				type: 'application/zip'
			});
		}

		function zipBlobPart(bytes: Uint8Array): ArrayBuffer {
			const copy = new Uint8Array(bytes.byteLength);
			copy.set(bytes);
			return copy.buffer;
		}

		const crc32Table = Array.from({ length: 256 }, (_, index) => {
			let value = index;
			for (let bit = 0; bit < 8; bit += 1) {
				value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
			}
			return value >>> 0;
		});

		function crc32(bytes: Uint8Array): number {
			let crc = 0xffffffff;
			for (const byte of bytes) {
				crc = crc32Table[(crc ^ byte) & 0xff] ^ (crc >>> 8);
			}
			return (crc ^ 0xffffffff) >>> 0;
		}

		function attachmentManifestUrls(value: unknown): string[] {
			if (typeof value === 'string') return value.trim() ? [value.trim()] : [];
			if (Array.isArray(value)) {
				return value.flatMap((item) => attachmentManifestUrls(item));
			}
			if (isRecord(value)) {
				if (typeof value.url === 'string') return attachmentManifestUrls(value.url);
				if (typeof value.href === 'string') return attachmentManifestUrls(value.href);
			}
			return [];
		}

		function openImportRecords() {
			importText = sampleImportText();
			importRows = [];
			importPreview = null;
			importWizardStep = 'source';
			exportedImportTemplate = false;
			importedImportFileName = '';
			importedImportFileLoaded = false;
			refreshImportMappingFromText();
			showImportRecords = true;
		}

		function closeImportRecords() {
			showImportRecords = false;
			importPreview = null;
			importWizardStep = 'source';
		}

		function resetImportPreviewForSourceChange() {
			importRows = [];
			importPreview = null;
			importWizardStep = 'source';
		}

		function continueImportFromSource() {
			refreshImportMappingFromText();
			importPreview = null;
			if (importMappingHeaders.length > 0) {
				importWizardStep = 'mapping';
				return;
			}
			void previewImportRecords();
		}

		function continueImportFromMapping() {
			if (importMappingHeaders.length > 0 && !importMappingApplied) {
				applyImportMapping();
			}
			void previewImportRecords();
		}

		function sampleImportText(): string {
			if (visibleFields.length === 0) return '[]';
			return importTemplateCsv();
		}

		function importTemplateCsv(): string {
			const header = ['Title', ...visibleFields.map((field) => field.key)].map(csvCell).join(',');
			const values = ['', ...visibleFields.map((field) => sampleImportValue(field))].map(csvCell).join(',');
			return `${header}\n${values}\n`;
		}

		function sampleImportValue(field: FormField): string {
			const type = fieldType(field);
			if (type === 'boolean') return 'true';
			if (type === 'integer') return '1';
			if (type === 'number' || type === 'amount') return '1.00';
			if (type === 'single_select') return fieldOptions(field)[0] ?? '';
			if (type === 'multi_select') return (fieldOptions(field)[0] ?? '').replaceAll(',', ' ');
			return fieldLabel(field);
		}

		function exportImportTemplate() {
			if (!selectedForm) return;
			const csv = importTemplateCsv();
			const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
			const url = URL.createObjectURL(blob);
			const anchor = document.createElement('a');
			anchor.href = url;
			anchor.download = `${selectedForm.key}-import-template.csv`;
			document.body.appendChild(anchor);
			anchor.click();
			anchor.remove();
			URL.revokeObjectURL(url);
			exportedImportTemplate = true;
			toast.success($t('forms.importTemplateExported'));
		}

		async function loadImportFile(event: Event) {
			const input = event.currentTarget as HTMLInputElement;
			const file = input.files?.[0];
			if (!file) return;
			try {
				importText = await file.text();
				resetImportPreviewForSourceChange();
				importedImportFileName = file.name;
				importedImportFileLoaded = true;
				refreshImportMappingFromText();
				toast.success($t('forms.importFileLoaded', { values: { name: file.name } }));
			} catch {
				toast.error($t('forms.importFileFailed'));
			} finally {
				input.value = '';
			}
		}

		function refreshImportMappingFromText() {
			const trimmed = importText.trim();
			if (!trimmed || trimmed.startsWith('[')) {
				resetImportMappingState();
				return;
			}
			const headers = parseCsvLines(trimmed)[0]?.map((header) => header.trim()) ?? [];
			const savedTemplate = readImportMappingTemplate(headers);
			importMappingHeaders = headers;
			importMappingSelections =
				savedTemplate?.selections ?? headers.map((header) => defaultImportMappingSelection(header));
			importMappingTransforms = savedTemplate?.transforms ?? headers.map(() => 'trim');
			importMappingApplied = false;
			importMappingTemplateAvailable = Boolean(savedTemplate);
			importMappingTemplateSaved = false;
			importMappingTemplateRestored = Boolean(savedTemplate);
		}

		function resetImportMappingState() {
			importMappingHeaders = [];
			importMappingSelections = [];
			importMappingTransforms = [];
			importMappingApplied = false;
			importMappingTemplateAvailable = false;
			importMappingTemplateSaved = false;
			importMappingTemplateRestored = false;
		}

		function defaultImportMappingSelection(header: string): string {
			if (header.toLowerCase() === 'title' || header === '标题') return '__title';
			return fieldForImportHeader(header)?.key ?? '__skip';
		}

		function importMappingTemplateKey(headers = importMappingHeaders): string {
			if (!selectedForm || headers.length === 0) return '';
			return `openpr:forms:import-mapping:${selectedForm.id}:${headers.join('\u001f')}`;
		}

		function isValidImportMappingSelection(selection: string): boolean {
			return selection === '__skip' || selection === '__title' || fields.some((field) => field.key === selection);
		}

		function isValidImportMappingTransform(value: string): value is ImportMappingTransform {
			return [
				'none',
				'trim',
				'uppercase',
				'lowercase',
				'capitalize',
				'titlecase',
				'normalize_spaces',
				'strip_non_digits',
				'slugify'
			].includes(value);
		}

		function readImportMappingTemplate(
			headers = importMappingHeaders
		): { selections: string[]; transforms: ImportMappingTransform[] } | null {
			if (typeof window === 'undefined') return null;
			const key = importMappingTemplateKey(headers);
			if (!key) return null;
			try {
				const raw = window.localStorage.getItem(key);
				if (!raw) return null;
				const parsed = JSON.parse(raw) as unknown;
				if (!isRecord(parsed) || !Array.isArray(parsed.selections)) return null;
				const selections = parsed.selections.filter(
					(selection): selection is string => typeof selection === 'string'
				);
				if (selections.length !== headers.length || !selections.every(isValidImportMappingSelection)) return null;
				const transforms = Array.isArray(parsed.transforms)
					? parsed.transforms.filter(
							(transform): transform is ImportMappingTransform =>
								typeof transform === 'string' && isValidImportMappingTransform(transform)
						)
					: headers.map(() => 'trim' as ImportMappingTransform);
				if (transforms.length !== headers.length) return null;
				return { selections, transforms };
			} catch {
				return null;
			}
		}

		function saveImportMappingTemplate() {
			if (typeof window === 'undefined') return;
			const key = importMappingTemplateKey();
			if (!key || importMappingHeaders.length === 0) {
				toast.error($t('forms.importMappingTemplateUnavailable'));
				return;
			}
			const selections = importMappingSelections.map((selection) =>
				isValidImportMappingSelection(selection) ? selection : '__skip'
			);
			const transforms = importMappingTransforms.map((transform) =>
				isValidImportMappingTransform(transform) ? transform : 'trim'
			);
			window.localStorage.setItem(
				key,
				JSON.stringify({
					form_id: selectedForm?.id,
					headers: importMappingHeaders,
					selections,
					transforms,
					updated_at: new Date().toISOString()
				})
			);
			importMappingSelections = selections;
			importMappingTransforms = transforms;
			importMappingTemplateAvailable = true;
			importMappingTemplateSaved = true;
			importMappingTemplateRestored = false;
			toast.success($t('forms.importMappingTemplateSaved'));
		}

		function restoreImportMappingTemplate() {
			const savedTemplate = readImportMappingTemplate();
			if (!savedTemplate) {
				toast.error($t('forms.importMappingTemplateUnavailable'));
				return;
			}
			importMappingSelections = savedTemplate.selections;
			importMappingTransforms = savedTemplate.transforms;
			importMappingApplied = false;
			importMappingTemplateAvailable = true;
			importMappingTemplateRestored = true;
			toast.success($t('forms.importMappingTemplateRestored'));
		}

		function applyImportMapping() {
			const lines = parseCsvLines(importText.trim());
			if (lines.length < 2 || importMappingHeaders.length === 0) {
				toast.error($t('forms.importNeedsRows'));
				return;
			}
			const selectedColumns = importMappingSelections
				.map((selection, index) => ({
					selection,
					index,
					transform: importMappingTransforms[index] ?? 'trim'
				}))
				.filter((column) => column.selection !== '__skip');
			if (selectedColumns.length === 0) {
				toast.error($t('forms.importMappingNeedsField'));
				return;
			}
			const remappedRows = [
				selectedColumns.map((column) => (column.selection === '__title' ? 'Title' : column.selection)),
				...lines
					.slice(1)
					.map((row) =>
						selectedColumns.map((column) =>
							applyImportMappingTransform(row[column.index] ?? '', column.transform)
						)
					)
			];
			importText = remappedRows.map((row) => row.map(csvCell).join(',')).join('\n');
			importRows = [];
			importPreview = null;
			importMappingApplied = true;
			importWizardStep = 'mapping';
			toast.success($t('forms.importMappingApplied'));
		}

		function applyImportMappingTransform(value: string, transform: ImportMappingTransform): string {
			if (transform === 'none') return value;
			const trimmed = value.trim();
			if (transform === 'uppercase') return trimmed.toUpperCase();
			if (transform === 'lowercase') return trimmed.toLowerCase();
			if (transform === 'capitalize') return capitalizeImportValue(trimmed);
			if (transform === 'titlecase') return trimmed.split(/\s+/).filter(Boolean).map(capitalizeImportValue).join(' ');
			if (transform === 'normalize_spaces') return trimmed.split(/\s+/).filter(Boolean).join(' ');
			if (transform === 'strip_non_digits') return trimmed.replace(/\D+/g, '');
			if (transform === 'slugify') return slugifyImportValue(trimmed);
			return trimmed;
		}

		function capitalizeImportValue(value: string): string {
			if (!value) return '';
			return `${value.slice(0, 1).toUpperCase()}${value.slice(1).toLowerCase()}`;
		}

		function slugifyImportValue(value: string): string {
			return value
				.toLowerCase()
				.replace(/[^a-z0-9]+/g, '-')
				.replace(/^-+|-+$/g, '');
		}

		function parseImportRowsFromText(): FormRecordImportRow[] {
			const trimmed = importText.trim();
			if (!trimmed) throw new Error($t('forms.importEmpty'));
			if (trimmed.startsWith('[')) {
				const parsed = JSON.parse(trimmed) as unknown;
				if (!Array.isArray(parsed)) throw new Error($t('forms.importInvalidJson'));
				return parsed.map((item, index) => normalizeImportItem(item, index + 1));
			}
			return parseCsvImport(trimmed);
		}

		function normalizeImportItem(item: unknown, rowNumber: number): FormRecordImportRow {
			if (!isRecord(item)) throw new Error($t('forms.importInvalidRow', { values: { row: rowNumber } }));
			const values = isRecord(item.values) ? item.values : item;
			const title = typeof item.title === 'string' ? item.title : undefined;
			return {
				row_number: rowNumber,
				title,
				values: values as Record<string, unknown>,
				source: { type: 'web', origin: 'import' }
			};
		}

		function parseCsvImport(text: string): FormRecordImportRow[] {
			const lines = parseCsvLines(text);
			if (lines.length < 2) throw new Error($t('forms.importNeedsRows'));
			const headers = lines[0].map((header) => header.trim());
			const rows: FormRecordImportRow[] = [];
			for (let index = 1; index < lines.length; index += 1) {
				const cells = lines[index];
				if (cells.every((cell) => cell.trim() === '')) continue;
				const values: Record<string, unknown> = {};
				let title: string | undefined;
				for (let column = 0; column < headers.length; column += 1) {
					const header = headers[column] ?? '';
					const raw = cells[column] ?? '';
					if (header.toLowerCase() === 'title' || header === '标题') {
						title = raw.trim() || undefined;
						continue;
					}
					const field = fieldForImportHeader(header);
					if (!field) continue;
					const value = importCellValue(field, raw);
					if (value !== undefined) values[field.key] = value;
				}
				rows.push({
					row_number: index,
					title,
					values,
					source: { type: 'web', origin: 'import' }
				});
			}
			return rows;
		}

		function parseCsvLines(text: string): string[][] {
			const rows: string[][] = [];
			let row: string[] = [];
			let cell = '';
			let quoted = false;
			for (let index = 0; index < text.length; index += 1) {
				const char = text[index];
				const next = text[index + 1];
				if (quoted) {
					if (char === '"' && next === '"') {
						cell += '"';
						index += 1;
					} else if (char === '"') {
						quoted = false;
					} else {
						cell += char;
					}
					continue;
				}
				if (char === '"') {
					quoted = true;
				} else if (char === ',') {
					row.push(cell);
					cell = '';
				} else if (char === '\n') {
					row.push(cell);
					rows.push(row);
					row = [];
					cell = '';
				} else if (char !== '\r') {
					cell += char;
				}
			}
			row.push(cell);
			rows.push(row);
			return rows;
		}

		function fieldForImportHeader(header: string): FormField | null {
			const normalized = header.trim();
			if (!normalized) return null;
			return (
				fields.find(
					(field) =>
						field.key === normalized ||
						fieldLabel(field) === normalized ||
						field.key.toLowerCase() === normalized.toLowerCase()
				) ?? null
			);
		}

		function importCellValue(field: FormField, raw: string): unknown {
			const trimmed = raw.trim();
			if (!trimmed) return undefined;
			const type = fieldType(field);
			if (type === 'boolean') return ['true', '1', 'yes', 'y', '是'].includes(trimmed.toLowerCase());
			if (type === 'multi_select') {
				return trimmed
					.split(/[;|]/)
					.map((item) => item.trim())
					.filter(Boolean);
			}
			if (['attachment', 'image', 'relation', 'child_table', 'formula'].includes(type)) {
				return parseAdvancedRecordField(trimmed);
			}
			return trimmed;
		}

		async function previewImportRecords() {
			if (!selectedForm) return;
			let rows: FormRecordImportRow[];
			try {
				rows = parseImportRowsFromText();
			} catch (error) {
				toast.error(error instanceof Error ? error.message : $t('forms.importFailed'));
				return;
			}
			importPreviewing = true;
			const response = await formsApi.previewImportRecords(selectedForm.id, rows);
			importPreviewing = false;
			if (response.code !== 0 || !response.data) {
				toast.error(response.message || $t('forms.importFailed'));
				return;
			}
			importRows = rows;
			importPreview = response.data;
			importWizardStep = 'preview';
			toast.success($t('forms.importPreviewReady'));
		}

		async function commitImportRecords() {
			if (!selectedForm || importRows.length === 0) return;
			importingRecords = true;
			const response = await formsApi.importRecords(selectedForm.id, importRows);
			importingRecords = false;
			if (response.code !== 0 || !response.data) {
				toast.error(response.message || $t('forms.importFailed'));
				return;
			}
			importPreview = response.data;
			if (response.data.invalid_rows > 0) {
				importWizardStep = 'preview';
				toast.error($t('forms.importHasInvalidRows'));
				return;
			}
			importWizardStep = 'commit';
			showImportRecords = false;
			await loadRecords();
			toast.success($t('forms.recordsImported', { values: { count: response.data.created_count } }));
		}

	function relationTargetOptions(field: FormField): FormRelationTarget[] {
		return relationTargets[field.key] ?? [];
	}

	function relationDraftValue(field: FormField): string {
		const value = recordDraft[field.key];
		if (typeof value === 'string') return value;
		if (isRecord(value) && typeof value.record_id === 'string') return value.record_id;
		return '';
	}

	function blankFieldDraft(): FieldDraft {
		return {
			field_id: undefined,
			key: '',
			label: '',
			type: 'text',
			required: false,
			optionsText: '',
			optionColorsText: '',
			optionDisabledText: '',
			optionArchivedText: '',
			optionDescriptionsText: '',
			optionGroupsText: '',
			currency: 'CNY',
			scale: '2',
			placeholder: '',
			helpText: '',
			defaultValue: '',
			validationMin: '',
			validationMax: '',
			validationPattern: '',
			validationMaxLength: '',
			listVisible: true,
			detailVisible: true,
			conditionalHiddenWhenText: '',
			conditionalReadOnlyWhenText: '',
			readOnly: false,
			formulaOp: 'add',
			formulaArgsText: '',
			formulaScale: '2',
			formulaRelationKey: '',
			formulaChildField: '',
			relationTargetType: 'form_record',
			relationFormKey: '',
			relationKey: 'related',
			relationType: 'relation',
			relationDisplayField: '',
			attachmentAcceptText: '',
			attachmentMaxSizeMb: '',
			attachmentMultiple: false,
			attachmentPreview: true,
			attachmentThumbnailFormat: 'png',
			attachmentPreviewFormat: 'png',
			attachmentVariantsText: '',
			attachmentStoragePolicy: 'local',
			attachmentRetentionDays: '',
			attachmentSignedUrlTtlMinutes: '10'
		};
	}

	function resetFormDraft() {
		formDraft = {
			key: '',
			name: '',
			description: '',
			title_template: '{id}',
			schemaText: JSON.stringify({ version: 'openpr.form.schema.v1', fields: [] }, null, 2)
		};
		fieldDrafts = [blankFieldDraft()];
		showAdvancedSchema = false;
	}

	function normalizeTemplateSchema(schema: Record<string, unknown>): Record<string, unknown> {
		const fields = Array.isArray(schema.fields)
			? schema.fields.map((field) => {
					if (!isRecord(field)) return field;
					return {
						...field,
						type: field.type === 'select' ? 'single_select' : field.type
					};
				})
			: [];
		return {
			...schema,
			version: typeof schema.version === 'string' ? schema.version : 'openpr.form.schema.v1',
			fields
		};
	}

	function templateFormKey(template: ScenarioTemplate): string {
		return template.key.replace(/_default$/, '');
	}

	function templateTitleTemplate(schema: Record<string, unknown>): string {
		const field = parseFields(schema).find((item) => item.required) ?? parseFields(schema)[0];
		return field ? `{${field.key}}` : '{id}';
	}

	function applyScenarioTemplate(template: ScenarioTemplate) {
		const schema = normalizeTemplateSchema(template.field_schema);
		formDraft = {
			key: templateFormKey(template),
			name: scenarioTemplateName(template, $t),
			description: scenarioTemplateDescription(template, $t),
			title_template: templateTitleTemplate(schema),
			schemaText: JSON.stringify(schema, null, 2)
		};
		fieldDrafts = fieldDraftsFromSchema(schema);
		showAdvancedSchema = false;
		toast.success($t('forms.templateApplied'));
	}

	function uniqueTemplateKey(template: ScenarioTemplate): string {
		const base = templateFormKey(template);
		if (!forms.some((form) => form.key === base)) return base;
		return `${base}_${Date.now().toString(36)}`;
	}

	async function installScenarioTemplate(template: ScenarioTemplate) {
		installingTemplateKey = template.key;
		const schema = normalizeTemplateSchema(template.field_schema);
		const response = await formsApi.createFromTemplate(projectId, {
			template_key: template.key,
			key: uniqueTemplateKey(template),
			name: scenarioTemplateName(template, $t),
			description: scenarioTemplateDescription(template, $t),
			title_template: templateTitleTemplate(schema)
		});
		installingTemplateKey = null;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = [response.data, ...forms.filter((form) => form.id !== response.data?.id)];
		showCreateForm = false;
		toast.success($t('forms.templateInstalled'));
		await openForm(response.data.id);
		activeMode = 'design';
		void loadDesignerSafety(response.data);
	}

	function fieldDraftsFromSchema(schema: Record<string, unknown>): FieldDraft[] {
		const parsedFields = parseFields(schema);
		return parsedFields.length > 0
			? parsedFields.map((field) => ({
					field_id: field.field_id,
					key: field.key,
					label: field.label ?? field.key,
					type: (field.type ?? 'text') as FormFieldType,
					required: Boolean(field.required),
					optionsText: fieldOptions(field).join('\n'),
					optionColorsText: optionColorsToText(field.option_colors),
					optionDisabledText: fieldDisabledOptions(field).join('\n'),
					optionArchivedText: fieldArchivedOptions(field).join('\n'),
					optionDescriptionsText: optionMetadataToText(field.option_descriptions),
					optionGroupsText: optionMetadataToText(field.option_groups),
					currency: field.amount?.currency ?? 'CNY',
					scale: String(field.amount?.scale ?? 2),
					placeholder: field.placeholder ?? '',
					helpText: field.help_text ?? '',
					defaultValue:
						field.default_value === undefined || field.default_value === null
							? ''
							: typeof field.default_value === 'string'
								? field.default_value
								: JSON.stringify(field.default_value),
					validationMin:
						field.validation?.min === undefined || field.validation?.min === null
							? ''
							: String(field.validation.min),
					validationMax:
						field.validation?.max === undefined || field.validation?.max === null
							? ''
							: String(field.validation.max),
					validationPattern: field.validation?.pattern ?? '',
					validationMaxLength:
						field.validation?.max_length === undefined || field.validation?.max_length === null
							? ''
							: String(field.validation.max_length),
					listVisible: field.visibility?.list ?? true,
					detailVisible: field.visibility?.detail ?? true,
					conditionalHiddenWhenText: fieldConditionToText(field.conditional?.hidden_when),
					conditionalReadOnlyWhenText: fieldConditionToText(field.conditional?.read_only_when),
					readOnly: field.policy?.read_only ?? false,
					formulaOp: field.formula?.op ?? 'add',
					formulaArgsText: formulaArgsToText(field.formula?.args),
					formulaScale:
						field.formula?.scale === undefined || field.formula?.scale === null
							? '2'
							: String(field.formula.scale),
					formulaRelationKey: field.formula?.relation_key ?? '',
					formulaChildField: field.formula?.field ?? '',
					relationTargetType: field.relation?.target_type ?? 'form_record',
					relationFormKey: field.relation?.form_key ?? '',
					relationKey: field.relation?.relation_key ?? 'related',
					relationType: field.relation?.relation_type ?? 'relation',
					relationDisplayField: field.relation?.display_field ?? '',
					attachmentAcceptText: (field.attachment?.accept ?? []).join('\n'),
					attachmentMaxSizeMb:
						field.attachment?.max_size_mb === undefined || field.attachment?.max_size_mb === null
							? ''
							: String(field.attachment.max_size_mb),
					attachmentMultiple: field.attachment?.multiple ?? false,
					attachmentPreview:
						field.attachment?.preview ?? field.attachment?.thumbnail ?? field.type === 'image',
					attachmentThumbnailFormat: field.attachment?.thumbnail_format ?? 'png',
					attachmentPreviewFormat: field.attachment?.preview_format ?? 'png',
					attachmentVariantsText: attachmentVariantsToText(field.attachment?.variants),
					attachmentStoragePolicy: field.attachment?.storage_policy ?? 'local',
					attachmentRetentionDays:
						field.attachment?.retention_days === undefined || field.attachment?.retention_days === null
							? ''
							: String(field.attachment.retention_days),
					attachmentSignedUrlTtlMinutes:
						field.attachment?.signed_url_ttl_minutes === undefined ||
						field.attachment?.signed_url_ttl_minutes === null
							? '10'
							: String(field.attachment.signed_url_ttl_minutes)
				}))
			: [blankFieldDraft()];
	}

	function buildSchemaFromDrafts(drafts: FieldDraft[] = fieldDrafts, syncFormDraft = true): Record<string, unknown> {
		const fields = drafts
			.map((field) => ({
				field_id: field.field_id,
				key: field.key.trim(),
				label: field.label.trim(),
				type: field.type,
				required: field.required,
				options: parseOptionsText(field.optionsText),
				optionColors: parseOptionColorsText(field.optionColorsText),
				optionDisabled: parseOptionsText(field.optionDisabledText),
				optionArchived: parseOptionsText(field.optionArchivedText),
				optionDescriptions: parseOptionMetadataText(field.optionDescriptionsText),
				optionGroups: parseOptionMetadataText(field.optionGroupsText),
				currency: field.currency.trim().toUpperCase() || 'CNY',
				scale: Number.parseInt(field.scale, 10),
				placeholder: field.placeholder.trim(),
				helpText: field.helpText.trim(),
				defaultValue: field.defaultValue.trim(),
				validationMin: field.validationMin.trim(),
				validationMax: field.validationMax.trim(),
				validationPattern: field.validationPattern.trim(),
				validationMaxLength: field.validationMaxLength.trim(),
				listVisible: field.listVisible,
				detailVisible: field.detailVisible,
				conditionalHiddenWhen: parseFieldConditionText(field.conditionalHiddenWhenText),
				conditionalReadOnlyWhen: parseFieldConditionText(field.conditionalReadOnlyWhenText),
				readOnly: field.readOnly,
				formulaOp: field.formulaOp.trim(),
				formulaArgs: parseFormulaArgsText(field.formulaArgsText),
				formulaScale: Number.parseInt(field.formulaScale, 10),
				formulaRelationKey: field.formulaRelationKey.trim(),
				formulaChildField: field.formulaChildField.trim(),
				relationTargetType: field.relationTargetType.trim() || 'form_record',
				relationFormKey: field.relationFormKey.trim(),
				relationKey: field.relationKey.trim(),
				relationType: field.relationType.trim(),
				relationDisplayField: field.relationDisplayField.trim(),
				attachmentAccept: parseOptionsText(field.attachmentAcceptText),
				attachmentMaxSizeMb: Number.parseInt(field.attachmentMaxSizeMb, 10),
				attachmentMultiple: field.attachmentMultiple,
				attachmentPreview: field.attachmentPreview,
				attachmentThumbnailFormat: sanitizeAttachmentDerivativeFormat(field.attachmentThumbnailFormat),
				attachmentPreviewFormat: sanitizeAttachmentDerivativeFormat(field.attachmentPreviewFormat),
				attachmentVariants: parseAttachmentVariantsText(field.attachmentVariantsText),
				attachmentStoragePolicy: field.attachmentStoragePolicy.trim() || 'local',
				attachmentRetentionDays: Number.parseInt(field.attachmentRetentionDays, 10),
				attachmentSignedUrlTtlMinutes: Number.parseInt(field.attachmentSignedUrlTtlMinutes, 10)
			}))
			.filter((field) => field.key || field.label);
		if (fields.length === 0) {
			throw new Error($t('forms.fieldRequired', { values: { field: $t('forms.fields') } }));
		}
		const schemaFields = fields.map((field) => {
			if (!field.key.match(/^[a-z][a-z0-9_]*$/)) {
				throw new Error($t('forms.invalidFieldKey'));
			}
			const schemaField: Record<string, unknown> = {
				key: field.key,
				label: field.label || field.key,
				type: field.type,
				required: field.required
			};
			if (field.field_id) schemaField.field_id = field.field_id;
			if (['single_select', 'multi_select'].includes(field.type)) {
				if (field.options.length === 0) throw new Error($t('forms.optionsRequired'));
				schemaField.options = field.options;
				const optionColors = Object.fromEntries(
					Object.entries(field.optionColors).filter(
						([option, color]) => field.options.includes(option) && Boolean(color)
					)
				);
				if (Object.keys(optionColors).length > 0) schemaField.option_colors = optionColors;
				const optionDisabled = field.optionDisabled.filter((option) => field.options.includes(option));
				if (optionDisabled.length > 0) schemaField.option_disabled = optionDisabled;
				const optionArchived = field.optionArchived.filter((option) => field.options.includes(option));
				if (optionArchived.length > 0) schemaField.option_archived = optionArchived;
				const optionDescriptions = Object.fromEntries(
					Object.entries(field.optionDescriptions).filter(
						([option, description]) => field.options.includes(option) && Boolean(description)
					)
				);
				if (Object.keys(optionDescriptions).length > 0) schemaField.option_descriptions = optionDescriptions;
				const optionGroups = Object.fromEntries(
					Object.entries(field.optionGroups).filter(
						([option, group]) => field.options.includes(option) && Boolean(group)
					)
				);
				if (Object.keys(optionGroups).length > 0) schemaField.option_groups = optionGroups;
				if (field.defaultValue) {
					schemaField.default_value = parseSelectDefaultValue(
						field.defaultValue,
						field.type,
						field.options,
						optionDisabled,
						optionArchived
					);
				}
			}
			if (field.type === 'amount') {
				schemaField.amount = {
					currency: field.currency,
					scale: Number.isFinite(field.scale) ? Math.min(Math.max(field.scale, 0), 8) : 2
				};
			}
			if (field.placeholder) schemaField.placeholder = field.placeholder;
			if (field.helpText) schemaField.help_text = field.helpText;
			if (field.defaultValue && !['single_select', 'multi_select'].includes(field.type)) {
				schemaField.default_value = parseFieldDefaultValue(field.defaultValue, field.type);
			}
			const validation: Record<string, unknown> = {};
			if (field.validationMin) validation.min = parseValidationBoundary(field.validationMin);
			if (field.validationMax) validation.max = parseValidationBoundary(field.validationMax);
			if (field.validationPattern) validation.pattern = field.validationPattern;
			if (field.validationMaxLength) {
				const maxLength = Number.parseInt(field.validationMaxLength, 10);
				if (Number.isFinite(maxLength) && maxLength > 0) validation.max_length = maxLength;
			}
			if (Object.keys(validation).length > 0) schemaField.validation = validation;
			if (!field.listVisible || !field.detailVisible) {
				schemaField.visibility = { list: field.listVisible, detail: field.detailVisible };
			}
			const conditional: Record<string, unknown> = {};
			if (field.conditionalHiddenWhen) conditional.hidden_when = field.conditionalHiddenWhen;
			if (field.conditionalReadOnlyWhen) conditional.read_only_when = field.conditionalReadOnlyWhen;
			if (Object.keys(conditional).length > 0) schemaField.conditional = conditional;
			if (field.readOnly) schemaField.policy = { read_only: true };
			const isFormulaCapableField = ['formula', 'amount', 'number', 'integer'].includes(field.type);
			const isChildFormula = field.formulaOp.startsWith('child_');
			const hasChildFormulaConfig = isChildFormula && field.formulaRelationKey;
			const hasRegularFormulaConfig = !isChildFormula && field.formulaArgs.length > 0;
			if (isFormulaCapableField && field.formulaOp && (hasChildFormulaConfig || hasRegularFormulaConfig)) {
				const formula: Record<string, unknown> = { op: field.formulaOp };
				if (isChildFormula) {
					formula.relation_key = field.formulaRelationKey;
					if (field.formulaChildField) formula.field = field.formulaChildField;
				} else {
					formula.args = field.formulaArgs;
				}
				if (field.type === 'formula' && Number.isFinite(field.formulaScale)) {
					formula.scale = Math.min(Math.max(field.formulaScale, 0), 8);
				}
				schemaField.formula = formula;
			}
			if (field.type === 'relation' || field.type === 'child_table') {
				const relation: Record<string, unknown> = {
					target_type: field.type === 'child_table' ? 'form_record' : field.relationTargetType
				};
				if (field.relationFormKey) relation.form_key = field.relationFormKey;
				if (field.relationKey) relation.relation_key = field.relationKey;
				relation.relation_type = field.type === 'child_table' ? 'parent_child' : field.relationType || 'relation';
				if (field.relationDisplayField) relation.display_field = field.relationDisplayField;
				schemaField.relation = relation;
			}
			if (['attachment', 'image'].includes(field.type)) {
				const attachment: Record<string, unknown> = {
					multiple: field.attachmentMultiple,
					preview: field.attachmentPreview
				};
				if (field.attachmentThumbnailFormat !== 'png') {
					attachment.thumbnail_format = field.attachmentThumbnailFormat;
				}
				if (field.attachmentPreviewFormat !== 'png') {
					attachment.preview_format = field.attachmentPreviewFormat;
				}
				if (field.attachmentVariants.length > 0) {
					attachment.variants = field.attachmentVariants;
				}
				if (field.attachmentAccept.length > 0) attachment.accept = field.attachmentAccept;
				if (Number.isFinite(field.attachmentMaxSizeMb) && field.attachmentMaxSizeMb > 0) {
					attachment.max_size_mb = field.attachmentMaxSizeMb;
				}
				attachment.storage_policy = field.attachmentStoragePolicy;
				if (Number.isFinite(field.attachmentRetentionDays) && field.attachmentRetentionDays > 0) {
					attachment.retention_days = field.attachmentRetentionDays;
				}
				if (Number.isFinite(field.attachmentSignedUrlTtlMinutes) && field.attachmentSignedUrlTtlMinutes > 0) {
					attachment.signed_url_ttl_minutes = field.attachmentSignedUrlTtlMinutes;
				}
				if (field.type === 'image') attachment.thumbnail = field.attachmentPreview;
				schemaField.attachment = attachment;
			}
			return schemaField;
		});
		const schema = { version: 'openpr.form.schema.v1', fields: schemaFields };
		if (syncFormDraft) formDraft.schemaText = JSON.stringify(schema, null, 2);
		return schema;
	}

	function parseOptionsText(value: string): string[] {
		return value
			.split(/[\n,]/)
			.map((item) => item.trim())
			.filter(Boolean);
	}

	function parseOptionColorsText(value: string): Record<string, string> {
		const colors: Record<string, string> = {};
		for (const line of value.split('\n')) {
			const trimmed = line.trim();
			if (!trimmed) continue;
			const [rawOption, ...rawColorParts] = trimmed.split(/[:=]/);
			const option = rawOption?.trim();
			const color = sanitizeOptionColor(rawColorParts.join(':').trim());
			if (option && color) colors[option] = color;
		}
		return colors;
	}

	function parseOptionMetadataText(value: string): Record<string, string> {
		const metadata: Record<string, string> = {};
		for (const line of value.split('\n')) {
			const trimmed = line.trim();
			if (!trimmed) continue;
			const separatorIndex = trimmed.search(/[:=]/);
			if (separatorIndex <= 0) continue;
			const option = trimmed.slice(0, separatorIndex).trim();
			const text = trimmed.slice(separatorIndex + 1).trim();
			if (option && text) metadata[option] = text;
		}
		return metadata;
	}

	function optionColorsToText(value: unknown): string {
		if (!isRecord(value)) return '';
		return Object.entries(value)
			.filter((entry): entry is [string, string] => typeof entry[1] === 'string' && Boolean(sanitizeOptionColor(entry[1])))
			.map(([option, color]) => `${option}: ${sanitizeOptionColor(color)}`)
			.join('\n');
	}

	function optionMetadataToText(value: unknown): string {
		if (!isRecord(value)) return '';
		return Object.entries(value)
			.filter((entry): entry is [string, string] => typeof entry[1] === 'string' && Boolean(entry[1].trim()))
			.map(([option, text]) => `${option}: ${text.trim()}`)
			.join('\n');
	}

	function sanitizeAttachmentDerivativeFormat(value: string | undefined | null): string {
		const normalized = (value ?? '').trim().toLowerCase();
		if (normalized === 'jpg' || normalized === 'image/jpg' || normalized === 'image/jpeg') return 'jpeg';
		if (normalized === 'jpeg' || normalized === 'png' || normalized === 'webp') return normalized;
		return 'png';
	}

	function attachmentVariantsToText(value: unknown): string {
		if (!Array.isArray(value)) return '';
		return value
			.map((variant) => {
				if (!isRecord(variant) || typeof variant.kind !== 'string') return '';
				const kind = variant.kind.trim();
				if (!kind) return '';
				const maxDimension =
					typeof variant.max_dimension === 'number' && Number.isFinite(variant.max_dimension)
						? String(variant.max_dimension)
						: '';
				const format =
					typeof variant.format === 'string' ? sanitizeAttachmentDerivativeFormat(variant.format) : 'png';
				return [kind, maxDimension, format].filter(Boolean).join(':');
			})
			.filter(Boolean)
			.join('\n');
	}

	function parseAttachmentVariantsText(
		value: string
	): Array<{ kind: string; max_dimension?: number; format?: string }> {
		return value
			.split('\n')
			.map((line) => line.trim())
			.filter(Boolean)
			.map((line) => {
				const [rawKind, rawMaxDimension = '', rawFormat = ''] = line.split(':').map((part) => part.trim());
				const variant: { kind: string; max_dimension?: number; format?: string } = {
					kind: rawKind.toLowerCase().replace(/[\s.]+/g, '-')
				};
				const maxDimension = Number.parseInt(rawMaxDimension, 10);
				if (Number.isFinite(maxDimension) && maxDimension > 0) variant.max_dimension = maxDimension;
				const format = sanitizeAttachmentDerivativeFormat(rawFormat);
				if (format !== 'png') variant.format = format;
				return variant;
			})
			.filter((variant) => /^[a-z0-9_-]{1,32}$/.test(variant.kind));
	}

	type FieldConditionOperator =
		| 'equals'
		| 'not_equals'
		| 'contains'
		| 'not_contains'
		| 'is_empty'
		| 'not_empty'
		| 'gt'
		| 'gte'
		| 'lt'
		| 'lte';

	type FieldConditionItem = { field: string; equals?: string; operator?: FieldConditionOperator; value?: string };

	type FieldConditionDraft =
		| FieldConditionItem
		| {
				logic: 'all' | 'any';
				conditions: FieldConditionItem[];
			};

	function normalizeFieldConditionOperator(value: string): FieldConditionOperator | null {
		const normalized = value.trim().toLowerCase();
		if (
			normalized === 'equals' ||
			normalized === 'eq' ||
			normalized === '==' ||
			normalized === '='
		) {
			return 'equals';
		}
		if (
			normalized === 'not_equals' ||
			normalized === 'neq' ||
			normalized === '!=' ||
			normalized === '<>'
		) {
			return 'not_equals';
		}
		if (normalized === 'contains') return 'contains';
		if (normalized === 'not_contains' || normalized === 'not-contains') return 'not_contains';
		if (normalized === 'is_empty' || normalized === 'empty') return 'is_empty';
		if (normalized === 'not_empty' || normalized === 'exists') return 'not_empty';
		if (normalized === 'gt' || normalized === '>') return 'gt';
		if (normalized === 'gte' || normalized === '>=') return 'gte';
		if (normalized === 'lt' || normalized === '<') return 'lt';
		if (normalized === 'lte' || normalized === '<=') return 'lte';
		return null;
	}

	function fieldConditionOperatorNeedsValue(operator: FieldConditionOperator): boolean {
		return operator !== 'is_empty' && operator !== 'not_empty';
	}

	function parseFieldConditionLine(value: string): FieldConditionItem | null {
		const trimmed = value.trim();
		if (!trimmed) return null;
		const notEqualsIndex = trimmed.indexOf('!=');
		if (notEqualsIndex >= 0) {
			const field = trimmed.slice(0, notEqualsIndex).trim();
			const conditionValue = trimmed.slice(notEqualsIndex + 2).trim();
			return field && conditionValue
				? { field, operator: 'not_equals', value: conditionValue }
				: null;
		}
		const [operatorField, rawOperator, ...rawOperatorValueParts] = trimmed.split(/\s+/);
		const operator = normalizeFieldConditionOperator(rawOperator ?? '');
		if (operator && operatorField) {
			const conditionValue = rawOperatorValueParts.join(' ').trim();
			if (fieldConditionOperatorNeedsValue(operator) && !conditionValue) return null;
			return fieldConditionOperatorNeedsValue(operator)
				? { field: operatorField, operator, value: conditionValue }
				: { field: operatorField, operator };
		}

		const [rawField, ...rawValueParts] = trimmed.split('=');
		const field = rawField?.trim();
		const equals = rawValueParts.join('=').trim();
		return rawValueParts.length > 0 && field && equals ? { field, equals } : null;
	}

	function parseFieldConditionText(value: string): FieldConditionDraft | null {
		const lines = value
			.split('\n')
			.map((line) => line.trim())
			.filter(Boolean);
		if (lines.length === 0) return null;
		const first = lines[0]?.toLowerCase();
		const hasLogicPrefix = first === 'all:' || first === 'any:';
		const logic = hasLogicPrefix ? (first.slice(0, -1) as 'all' | 'any') : 'all';
		const conditionLines = hasLogicPrefix ? lines.slice(1) : lines;
		const conditions = conditionLines.map(parseFieldConditionLine).filter((item): item is FieldConditionItem => Boolean(item));
		if (conditions.length === 0) return null;
		if (!hasLogicPrefix && conditions.length === 1) return conditions[0];
		return { logic, conditions };
	}

	function fieldConditionToText(value: unknown): string {
		if (!isRecord(value)) return '';
		if (Array.isArray(value.conditions)) {
			const logic = value.logic === 'any' ? 'any' : 'all';
			const lines = value.conditions
				.filter(isRecord)
				.map((condition) => {
					const field = typeof condition.field === 'string' ? condition.field.trim() : '';
					const equals = typeof condition.equals === 'string' ? condition.equals.trim() : '';
					const operator =
						typeof condition.operator === 'string'
							? normalizeFieldConditionOperator(condition.operator) ?? 'equals'
							: 'equals';
					const conditionValue =
						typeof condition.value === 'string' ? condition.value.trim() : equals;
					if (!field || (fieldConditionOperatorNeedsValue(operator) && !conditionValue)) return '';
					if (operator === 'equals') return `${field}=${conditionValue}`;
					if (operator === 'not_equals') return `${field}!=${conditionValue}`;
					return fieldConditionOperatorNeedsValue(operator)
						? `${field} ${operator} ${conditionValue}`
						: `${field} ${operator}`;
				})
				.filter(Boolean);
			return lines.length > 0 ? [`${logic}:`, ...lines].join('\n') : '';
		}
		const field = typeof value.field === 'string' ? value.field.trim() : '';
		const equals = typeof value.equals === 'string' ? value.equals.trim() : '';
		const operator =
			typeof value.operator === 'string'
				? normalizeFieldConditionOperator(value.operator) ?? 'equals'
				: 'equals';
		const conditionValue = typeof value.value === 'string' ? value.value.trim() : equals;
		if (!field || (fieldConditionOperatorNeedsValue(operator) && !conditionValue)) return '';
		if (operator === 'equals') return `${field}=${conditionValue}`;
		if (operator === 'not_equals') return `${field}!=${conditionValue}`;
		return fieldConditionOperatorNeedsValue(operator) ? `${field} ${operator} ${conditionValue}` : `${field} ${operator}`;
	}

	function draftConditionValue(draft: Record<string, unknown>, fieldKey: string): string {
		const value = draft[fieldKey];
		if (Array.isArray(value)) return value.filter((item): item is string => typeof item === 'string').join(',');
		if (isRecord(value)) {
			if (typeof value.record_id === 'string') return value.record_id;
			if (typeof value.user_id === 'string') return value.user_id;
			if (typeof value.value === 'string' || typeof value.value === 'number') return String(value.value);
		}
		if (value === null || value === undefined) return '';
		return String(value);
	}

	function fieldConditionMatches(condition: unknown, draft: Record<string, unknown>): boolean {
		if (!isRecord(condition)) return false;
		if (Array.isArray(condition.conditions)) {
			const checks = condition.conditions
				.filter(isRecord)
				.map((item) => fieldConditionMatches(item, draft));
			if (checks.length === 0) return false;
			return condition.logic === 'any' ? checks.some(Boolean) : checks.every(Boolean);
		}
		const field = typeof condition.field === 'string' ? condition.field.trim() : '';
		const operator =
			typeof condition.operator === 'string'
				? normalizeFieldConditionOperator(condition.operator) ?? 'equals'
				: 'equals';
		const equals = typeof condition.equals === 'string' ? condition.equals.trim() : '';
		const conditionValue = typeof condition.value === 'string' ? condition.value.trim() : equals;
		if (!field || (fieldConditionOperatorNeedsValue(operator) && !conditionValue)) return false;
		const draftValue = draftConditionValue(draft, field);
		if (operator === 'equals') return draftValue === conditionValue;
		if (operator === 'not_equals') return draftValue !== conditionValue;
		if (operator === 'contains') return draftValue.includes(conditionValue);
		if (operator === 'not_contains') return !draftValue.includes(conditionValue);
		if (operator === 'is_empty') return !draftValue.trim();
		if (operator === 'not_empty') return Boolean(draftValue.trim());
		const actualNumber = Number(draftValue);
		const expectedNumber = Number(conditionValue);
		if (!Number.isFinite(actualNumber) || !Number.isFinite(expectedNumber)) return false;
		if (operator === 'gt') return actualNumber > expectedNumber;
		if (operator === 'gte') return actualNumber >= expectedNumber;
		if (operator === 'lt') return actualNumber < expectedNumber;
		if (operator === 'lte') return actualNumber <= expectedNumber;
		return false;
	}

	function fieldHiddenByCondition(field: FormField, draft: Record<string, unknown>): boolean {
		return fieldConditionMatches(field.conditional?.hidden_when, draft);
	}

	function fieldReadOnlyByCondition(field: FormField, draft: Record<string, unknown>): boolean {
		return field.policy?.read_only === true || fieldConditionMatches(field.conditional?.read_only_when, draft);
	}

	function formulaArgsToText(args: unknown[] | undefined): string {
		if (!Array.isArray(args)) return '';
		return args
			.map((arg) => {
				if (typeof arg === 'string') return arg;
				if (isRecord(arg) && typeof arg.field === 'string') return arg.field;
				if (isRecord(arg) && typeof arg.value === 'string') return arg.value;
				return JSON.stringify(arg);
			})
			.join('\n');
	}

	function parseFormulaArgsText(value: string): unknown[] {
		return value
			.split(/[\n,]/)
			.map((item) => item.trim())
			.filter(Boolean)
			.map((item) => {
				if (item.startsWith('{')) {
					try {
						return JSON.parse(item) as unknown;
					} catch {
						return item;
					}
				}
				return item;
			});
	}

	function parseValidationBoundary(value: string): string | number {
		const numberValue = Number(value);
		return Number.isFinite(numberValue) && value.trim() !== '' ? numberValue : value;
	}

	function parseFieldDefaultValue(value: string, type: FormFieldType): unknown {
		const trimmed = value.trim();
		if (trimmed === '') return undefined;
		if (type === 'boolean') return trimmed === 'true' || trimmed === 'yes' || trimmed === '1';
		if (type === 'integer') {
			const integerValue = Number.parseInt(trimmed, 10);
			return Number.isFinite(integerValue) ? integerValue : trimmed;
		}
		if (type === 'number') {
			const numberValue = Number(trimmed);
			return Number.isFinite(numberValue) ? numberValue : trimmed;
		}
		if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
			try {
				return JSON.parse(trimmed) as unknown;
			} catch {
				return trimmed;
			}
		}
		return trimmed;
	}

	function parseSelectDefaultValue(
		value: string,
		type: FormFieldType,
		options: string[],
		disabledOptions: string[],
		archivedOptions: string[]
	): unknown {
		const trimmed = value.trim();
		if (!trimmed) return undefined;
		const values = type === 'multi_select' ? parseDefaultMultiSelectValue(trimmed) : [trimmed];
		for (const item of values) {
			if (!options.includes(item)) {
				throw new Error($t('forms.defaultOptionInvalid', { values: { option: item } }));
			}
			if (disabledOptions.includes(item)) {
				throw new Error($t('forms.defaultOptionDisabled', { values: { option: item } }));
			}
			if (archivedOptions.includes(item)) {
				throw new Error($t('forms.defaultOptionArchived', { values: { option: item } }));
			}
		}
		return type === 'multi_select' ? values : values[0];
	}

	function parseDefaultMultiSelectValue(value: string): string[] {
		if (value.startsWith('[')) {
			try {
				const parsed = JSON.parse(value) as unknown;
				if (Array.isArray(parsed)) {
					return parsed.filter((item): item is string => typeof item === 'string' && item.trim().length > 0);
				}
			} catch {
				// Fall through to delimiter parsing so the designer can recover from partial JSON.
			}
		}
		return value
			.split(/[,\n]/)
			.map((item) => item.trim())
			.filter(Boolean);
	}

	function addFieldDraft() {
		fieldDrafts = [...fieldDrafts, blankFieldDraft()];
	}

	function removeFieldDraft(index: number) {
		fieldDrafts =
			fieldDrafts.length > 1 ? fieldDrafts.filter((_, itemIndex) => itemIndex !== index) : [blankFieldDraft()];
	}

	function syncDesignerDrafts(form: UniversalForm | null = selectedForm) {
		if (!form) {
			designerFieldDrafts = [];
			designerFieldUsage = [];
			designerFieldDependencies = [];
			designerSyncedFormId = '';
			selectedDesignerFieldIndex = 0;
			return;
		}
		if (designerSyncedFormId === form.id) return;
		designerFieldDrafts = fieldDraftsFromSchema(form.schema);
		designerSyncedFormId = form.id;
		selectedDesignerFieldIndex = 0;
	}

	async function loadDesignerSafety(form: UniversalForm | null = selectedForm) {
		if (!form) {
			designerFieldUsage = [];
			designerFieldDependencies = [];
			return;
		}
		designerUsageLoading = true;
		const [usageResponse, dependencyResponse] = await Promise.all([
			formsApi.getFieldUsage(form.id),
			formsApi.getFieldDependencies(form.id)
		]);
		designerUsageLoading = false;
		if (usageResponse.code !== 0) {
			designerFieldUsage = [];
			toast.error(usageResponse.message);
		} else {
			designerFieldUsage = usageResponse.data?.fields ?? [];
		}
		if (dependencyResponse.code !== 0) {
			designerFieldDependencies = [];
			toast.error(dependencyResponse.message);
		} else {
			designerFieldDependencies = dependencyResponse.data?.dependencies ?? [];
		}
	}

	function blockingDependenciesForField(field: FieldDraft): FormFieldDependency[] {
		return designerFieldDependencies.filter(
			(dependency) =>
				dependency.blocking &&
				((field.field_id && dependency.field_id === field.field_id) ||
					dependency.field_key === field.key)
		);
	}

	function fieldIdentity(field: { field_id?: string; key: string }): string {
		return field.field_id || field.key;
	}

	function usageForSchemaField(field: FormField | FieldDraft): FormFieldUsage | null {
		return (
			designerFieldUsage.find(
				(usage) =>
					(field.field_id && usage.field_id === field.field_id) || usage.field_key === field.key
			) ?? null
		);
	}

	function dependenciesForSchemaField(field: FormField | FieldDraft): FormFieldDependency[] {
		return designerFieldDependencies.filter(
			(dependency) =>
				(field.field_id && dependency.field_id === field.field_id) || dependency.field_key === field.key
		);
	}

	function usageCountsForSchemaField(field: FormField | FieldDraft) {
		const usage = usageForSchemaField(field);
		const dependencies = dependenciesForSchemaField(field);
		return {
			valueCount: usage?.value_count ?? 0,
			indexedCount: usage?.indexed_count ?? 0,
			dependencyCount: dependencies.length,
			blockingDependencyCount: dependencies.filter((dependency) => dependency.blocking).length
		};
	}

	function collectDesignerSchemaChanges(drafts: FieldDraft[]): DesignerSchemaChange[] {
		if (!selectedForm) return [];
		const originalFields = parseFields(selectedForm.schema);
		const draftByIdentity = new Map(
			drafts
				.filter((field) => field.key.trim())
				.map((field) => [fieldIdentity(field), field])
		);
		const changes: DesignerSchemaChange[] = [];

		for (const original of originalFields) {
			const identity = fieldIdentity(original);
			const draft = draftByIdentity.get(identity);
			const counts = usageCountsForSchemaField(original);
			if (!draft) {
				changes.push({
					kind: 'delete',
					field_id: original.field_id,
					fieldKey: original.key,
					fieldLabel: fieldLabel(original),
					from: original.key,
					...counts
				});
				continue;
			}
			if (draft.key !== original.key) {
				changes.push({
					kind: 'rename',
					field_id: original.field_id,
					fieldKey: original.key,
					fieldLabel: fieldLabel(original),
					from: original.key,
					to: draft.key,
					...counts
				});
			}
			const nextType = draft.type ?? 'text';
			const originalType = original.type ?? 'text';
			if (nextType !== originalType) {
				changes.push({
					kind: 'type_change',
					field_id: original.field_id,
					fieldKey: original.key,
					fieldLabel: fieldLabel(original),
					from: $t(`forms.fieldTypes.${originalType}`),
					to: $t(`forms.fieldTypes.${nextType}`),
					...counts
				});
			}
		}

		return changes;
	}

	function closeDesignerConfirm() {
		designerConfirmOpen = false;
		pendingDesignerSchema = null;
		pendingDesignerChanges = [];
	}

	function addDesignerField(type: FormFieldType = 'text') {
		const field = {
			...blankFieldDraft(),
			type,
			key: nextDesignerFieldKey(type),
			label: $t(`forms.fieldTypes.${type}`)
		};
		designerFieldDrafts = [...designerFieldDrafts, field];
		selectedDesignerFieldIndex = designerFieldDrafts.length - 1;
	}

	function nextDesignerFieldKey(type: FormFieldType): string {
		const base = type.replaceAll('-', '_');
		const used = new Set(designerFieldDrafts.map((field) => field.key));
		let index = designerFieldDrafts.length + 1;
		let key = `${base}_${index}`;
		while (used.has(key)) {
			index += 1;
			key = `${base}_${index}`;
		}
		return key;
	}

	function updateDesignerField(index: number, patch: Partial<FieldDraft>) {
		designerFieldDrafts = designerFieldDrafts.map((field, itemIndex) =>
			itemIndex === index ? { ...field, ...patch } : field
		);
	}

	function duplicateDesignerField(index: number) {
		const field = designerFieldDrafts[index];
		if (!field) return;
		const duplicate = {
			...field,
			field_id: undefined,
			key: nextDesignerFieldKey(field.type),
			label: `${field.label || field.key} ${$t('forms.copySuffix')}`
		};
		designerFieldDrafts = [
			...designerFieldDrafts.slice(0, index + 1),
			duplicate,
			...designerFieldDrafts.slice(index + 1)
		];
		selectedDesignerFieldIndex = index + 1;
	}

	function removeDesignerField(index: number) {
		if (designerFieldDrafts.length <= 1) return;
		const field = designerFieldDrafts[index];
		if (field && blockingDependenciesForField(field).length > 0) {
			toast.error($t('forms.deleteBlockedByDependencies'));
			return;
		}
		designerFieldDrafts = designerFieldDrafts.filter((_, itemIndex) => itemIndex !== index);
		selectedDesignerFieldIndex = Math.max(0, Math.min(index, designerFieldDrafts.length - 1));
	}

	function moveDesignerField(index: number, direction: -1 | 1) {
		const nextIndex = index + direction;
		if (nextIndex < 0 || nextIndex >= designerFieldDrafts.length) return;
		const next = [...designerFieldDrafts];
		[next[index], next[nextIndex]] = [next[nextIndex], next[index]];
		designerFieldDrafts = next;
		selectedDesignerFieldIndex = nextIndex;
	}

	async function saveDesignerSchema() {
		if (!selectedForm) return;
		let schema: Record<string, unknown>;
		try {
			schema = buildSchemaFromDrafts(designerFieldDrafts, false);
		} catch (error) {
			toast.error(error instanceof Error ? error.message : $t('forms.invalidSchema'));
			return;
		}
		const changes = collectDesignerSchemaChanges(designerFieldDrafts);
		if (changes.length > 0) {
			pendingDesignerSchema = schema;
			pendingDesignerChanges = changes;
			designerConfirmOpen = true;
			return;
		}
		await persistDesignerSchema(schema);
	}

	async function confirmDesignerSchemaSave() {
		if (!pendingDesignerSchema) return;
		const schema = pendingDesignerSchema;
		designerConfirmOpen = false;
		pendingDesignerSchema = null;
		pendingDesignerChanges = [];
		await persistDesignerSchema(schema);
	}

	async function persistDesignerSchema(schema: Record<string, unknown>) {
		if (!selectedForm) return;
		designerSaving = true;
		const response = await formsApi.update(selectedForm.id, {
			schema,
			expected_schema_version: selectedForm.schema_version
		});
		designerSaving = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = forms.map((form) => (form.id === response.data?.id ? response.data : form));
		designerSyncedFormId = '';
		syncDesignerDrafts(response.data);
		detailHeaderSyncedFingerprint = '';
		syncDetailHeaderDraft(response.data);
		detailHighlightSyncedFingerprint = '';
		syncDetailHighlightDraft(response.data);
		detailPageCompositionSyncedFingerprint = '';
		syncDetailPageCompositionDraft(response.data);
		detailLayoutSectionSyncedFingerprint = '';
		syncDetailLayoutSectionDrafts(response.data);
		await loadDesignerSafety(response.data);
		toast.success($t('forms.designSaved'));
		await loadRecords();
	}

	async function saveDetailHeader() {
		if (!selectedForm) return;
		detailHeaderSaving = true;
		const response = await formsApi.update(selectedForm.id, {
			detail_layout: {
				...(selectedForm.detail_layout ?? {}),
				header: {
					subtitle_field: detailHeaderDraft.subtitleField || undefined,
					badge_field: detailHeaderDraft.badgeField || undefined
				}
			},
			expected_schema_version: selectedForm.schema_version
		});
		detailHeaderSaving = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = forms.map((form) => (form.id === response.data?.id ? response.data : form));
		detailHeaderSyncedFingerprint = '';
		syncDetailHeaderDraft(response.data);
		toast.success($t('forms.detailHeaderSaved'));
	}

	async function saveDetailHighlights() {
		if (!selectedForm) return;
		detailHighlightSaving = true;
		const response = await formsApi.update(selectedForm.id, {
			detail_layout: {
				...(selectedForm.detail_layout ?? {}),
				highlights: detailHighlightDraft.fields
			},
			expected_schema_version: selectedForm.schema_version
		});
		detailHighlightSaving = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = forms.map((form) => (form.id === response.data?.id ? response.data : form));
		detailHighlightSyncedFingerprint = '';
		syncDetailHighlightDraft(response.data);
		toast.success($t('forms.detailHighlightsSaved'));
	}

	async function saveDetailPageComposition() {
		if (!selectedForm) return;
		detailPageCompositionSaving = true;
		const response = await formsApi.update(selectedForm.id, {
			detail_layout: {
				...(selectedForm.detail_layout ?? {}),
				page: {
					...(isRecord(selectedForm.detail_layout?.page) ? selectedForm.detail_layout.page : {}),
					widget_region: detailPageCompositionDraft.widgetRegion,
					field_columns: detailPageCompositionDraft.fieldColumns
				}
			},
			expected_schema_version: selectedForm.schema_version
		});
		detailPageCompositionSaving = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = forms.map((form) => (form.id === response.data?.id ? response.data : form));
		detailPageCompositionSyncedFingerprint = '';
		syncDetailPageCompositionDraft(response.data);
		toast.success($t('forms.detailPageCompositionSaved'));
	}

	async function saveDetailLayoutSections() {
		if (!selectedForm) return;
		const sections = detailLayoutSectionPayload(detailLayoutSectionDrafts);
		detailLayoutSectionSaving = true;
		const response = await formsApi.update(selectedForm.id, {
			detail_layout: {
				...(selectedForm.detail_layout ?? {}),
				sections
			},
			expected_schema_version: selectedForm.schema_version
		});
		detailLayoutSectionSaving = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = forms.map((form) => (form.id === response.data?.id ? response.data : form));
		detailLayoutSectionSyncedFingerprint = '';
		syncDetailLayoutSectionDrafts(response.data);
		toast.success($t('forms.detailLayoutSectionsSaved'));
	}

	async function persistDetailPageWidgets(entries: DetailPageWidgetEntry[]) {
		if (!selectedForm) return;
		detailPageWidgetSaving = true;
		const response = await formsApi.update(selectedForm.id, {
			detail_layout: {
				...(selectedForm.detail_layout ?? {}),
				widgets: detailPageWidgetPayload(entries)
			},
			expected_schema_version: selectedForm.schema_version
		});
		detailPageWidgetSaving = false;
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		forms = forms.map((form) => (form.id === response.data?.id ? response.data : form));
		toast.success($t('forms.detailPageWidgetsSaved'));
	}

	async function setDetailPageWidgetEnabled(key: DetailPageWidgetKey, enabled: boolean) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, enabled } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetTitle(key: DetailPageWidgetKey, title: string) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, title } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetDescription(key: DetailPageWidgetKey, description: string) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, description } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetEmptyText(key: DetailPageWidgetKey, emptyText: string) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, emptyText } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetShowCount(key: DetailPageWidgetKey, showCount: boolean) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, showCount } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetHideEmpty(key: DetailPageWidgetKey, hideEmpty: boolean) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, hideEmpty } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetCollapsible(key: DetailPageWidgetKey, collapsible: boolean) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key
				? {
						...entry,
						collapsible,
						defaultCollapsed: collapsible ? entry.defaultCollapsed : false
					}
				: entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetDefaultCollapsed(key: DetailPageWidgetKey, defaultCollapsed: boolean) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, defaultCollapsed: entry.collapsible && defaultCollapsed } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetDensity(key: DetailPageWidgetKey, density: DetailPageWidgetDensity) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, density } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function setDetailPageWidgetLimit(key: DetailPageWidgetKey, limit: string) {
		const entries = selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, limit: detailPageWidgetLimitText(limit) } : entry
		);
		await persistDetailPageWidgets(entries);
	}

	async function moveDetailPageWidget(key: DetailPageWidgetKey, direction: -1 | 1) {
		const entries = [...selectedDetailPageWidgetEntries];
		const index = entries.findIndex((entry) => entry.key === key);
		const nextIndex = index + direction;
		if (index < 0 || nextIndex < 0 || nextIndex >= entries.length) return;
		[entries[index], entries[nextIndex]] = [entries[nextIndex], entries[index]];
		await persistDetailPageWidgets(entries);
	}

	function setDraftValue(fieldKey: string, value: unknown) {
		recordDraft = { ...recordDraft, [fieldKey]: value };
	}

	function attachmentUrl(field: FormField): string {
		const value = recordDraft[field.key];
		if (typeof value === 'string') return value.trim();
		if (isRecord(value) && typeof value.url === 'string') return value.url.trim();
		return '';
	}

	function attachmentFileName(url: string, field: FormField): string {
		const cleaned = url.trim();
		if (!cleaned) return field.type === 'image' ? `${field.key}.png` : `${field.key}.bin`;
		try {
			const parsed = new URL(cleaned);
			const name = parsed.pathname.split('/').filter(Boolean).pop();
			if (name) return decodeURIComponent(name);
		} catch {
			const name = cleaned.split('/').filter(Boolean).pop();
			if (name) return name;
		}
		return field.type === 'image' ? `${field.key}.png` : `${field.key}.bin`;
	}

	function attachmentContentType(fileName: string, field: FormField): string {
		const lower = fileName.toLowerCase();
		if (field.type === 'image') {
			if (lower.endsWith('.jpg') || lower.endsWith('.jpeg')) return 'image/jpeg';
			if (lower.endsWith('.webp')) return 'image/webp';
			if (lower.endsWith('.gif')) return 'image/gif';
			return 'image/png';
		}
		if (lower.endsWith('.pdf')) return 'application/pdf';
		if (lower.endsWith('.png')) return 'image/png';
		if (lower.endsWith('.jpg') || lower.endsWith('.jpeg')) return 'image/jpeg';
		return 'application/octet-stream';
	}

	function attachmentAcceptAttr(field: FormField): string | undefined {
		const accept = field.attachment?.accept?.filter(Boolean) ?? [];
		if (accept.length > 0) return accept.join(',');
		return field.type === 'image' ? 'image/*' : undefined;
	}

	function attachmentStorageName(fileName: string): string {
		return fileName.replace(/[^A-Za-z0-9._-]+/g, '_').replace(/^_+|_+$/g, '') || 'file';
	}

	async function uploadFormAttachmentFile(
		file: File
	): Promise<{ url: string; filename: string; thumbnail_url?: string } | null> {
		const formData = new FormData();
		formData.append('file', file);
		const headers = new Headers();
		const token = apiClient.getToken();
		if (token) headers.set('Authorization', `Bearer ${token}`);
		try {
			const response = await fetch('/api/v1/upload', {
				method: 'POST',
				headers,
				body: formData
			});
			const result = (await response.json()) as {
				code?: number;
				message?: string;
				data?: { url?: string; filename?: string; thumbnail_url?: string };
			};
			if (result.code === 0 && result.data?.url) {
				return {
					url: result.data.url,
					filename: result.data.filename || attachmentStorageName(file.name || 'file'),
					thumbnail_url: result.data.thumbnail_url
				};
			}
			toast.error(result.message || $t('toast.uploadFail'));
		} catch (_error) {
			toast.error($t('toast.uploadFail'));
		}
		return null;
	}

	function fieldAttachments(fieldKey: string): FormAttachment[] {
		return recordAttachments.filter((attachment) => attachment.field_key === fieldKey && !attachment.archived_at);
	}

	function attachmentSignedUrl(attachment: FormAttachment): string {
		return recordAttachmentSignedUrls[attachment.id] || attachment.url;
	}

	function attachmentThumbnailUrl(attachment: FormAttachment): string {
		return attachment.thumbnail_url || attachment.url;
	}

	async function loadAttachmentSignedUrls(attachments: FormAttachment[]) {
		const active = attachments.filter((attachment) => !attachment.archived_at && attachment.url);
		if (active.length === 0) {
			recordAttachmentSignedUrls = {};
			return;
		}
		const entries = await Promise.all(
			active.map(async (attachment) => {
				const response = await formsApi.getAttachmentSignedDownload(attachment.id);
				return response.code === 0 && response.data?.url ? [attachment.id, response.data.url] : null;
			})
		);
		recordAttachmentSignedUrls = Object.fromEntries(
			entries.filter((entry): entry is [string, string] => Array.isArray(entry))
		);
	}

	function selectedRecordAttachmentFields(): FormField[] {
		return fields.filter((field) => {
			const type = fieldType(field);
			return ['attachment', 'image'].includes(type) && field.visibility?.detail !== false;
		});
	}

	function selectedRecordAttachmentUrls(field: FormField): string[] {
		if (!selectedRecord) return [];
		return attachmentManifestUrls(selectedRecord.values[field.key]);
	}

	function selectedRecordAttachmentValueUrls(field: FormField): string[] {
		const metadataUrls = new Set(fieldAttachments(field.key).map((attachment) => attachment.url));
		return selectedRecordAttachmentUrls(field).filter((url) => url && !metadataUrls.has(url));
	}

	function attachmentIsPreviewableImage(attachment: FormAttachment): boolean {
		return attachment.content_type?.startsWith('image/') || /\.(png|jpe?g|webp|gif)$/i.test(attachment.file_name);
	}

	function recordValueUrl(value: unknown): string {
		if (typeof value === 'string') return value;
		if (isRecord(value) && typeof value.url === 'string') return value.url;
		return '';
	}

	async function loadRecordAttachments(recordId: string) {
		if (!selectedForm) return;
		const response = await formsApi.listAttachments(selectedForm.id, {
			record_id: recordId,
			per_page: 100
		});
		if (response.code !== 0) {
			toast.error(response.message);
			recordAttachments = [];
			return;
		}
		recordAttachments = response.data?.items ?? [];
		await loadAttachmentSignedUrls(recordAttachments);
	}

	async function syncRecordAttachmentValue(field: FormField, value: string) {
		if (!editingRecordId) return;
		const currentRecord = records.find((record) => record.id === editingRecordId) ?? selectedRecord;
		const values = { ...(currentRecord?.values ?? recordDraft), [field.key]: value };
		const response = await formsApi.updateRecord(editingRecordId, {
			values,
			source: { type: 'web', origin: 'attachment' }
		});
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			return;
		}
		replaceLocalRecord(response.data);
		selectedRecord = response.data;
		recordDraft = { ...recordDraft, [field.key]: value };
	}

	async function createAttachmentForField(field: FormField) {
		if (!selectedForm || !editingRecordId) {
			toast.error($t('forms.attachmentSaveRecordFirst'));
			return;
		}
		const url = attachmentUrl(field);
		if (!url) {
			toast.error($t('forms.attachmentUrlRequired'));
			return;
		}
		attachmentSubmittingField = field.key;
		const fileName = attachmentFileName(url, field);
		const response = await formsApi.createAttachment(selectedForm.id, {
			field_key: field.key,
			record_id: editingRecordId,
			file_name: fileName,
			content_type: attachmentContentType(fileName, field),
			byte_size: 0,
			storage_key: `form-records/${editingRecordId}/${field.key}/${Date.now()}-${fileName}`,
			url
		});
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			attachmentSubmittingField = null;
			return;
		}
		recordAttachments = [response.data, ...recordAttachments.filter((item) => item.id !== response.data?.id)];
		await loadAttachmentSignedUrls(recordAttachments);
		await syncRecordAttachmentValue(field, response.data.url);
		attachmentSubmittingField = null;
		toast.success($t('forms.attachmentLinked'));
	}

	async function createAttachmentFromLocalFile(field: FormField, event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		if (!selectedForm || !editingRecordId) {
			toast.error($t('forms.attachmentSaveRecordFirst'));
			return;
		}
		attachmentSubmittingField = field.key;
		const storageName = attachmentStorageName(file.name || (field.type === 'image' ? `${field.key}.png` : `${field.key}.bin`));
		const uploaded = await uploadFormAttachmentFile(file);
		if (!uploaded) {
			attachmentSubmittingField = null;
			return;
		}
		const response = await formsApi.createAttachment(selectedForm.id, {
			field_key: field.key,
			record_id: editingRecordId,
			file_name: file.name || storageName,
			content_type: file.type || attachmentContentType(storageName, field),
			byte_size: file.size,
			storage_key: `form-records/${editingRecordId}/${field.key}/server-upload-${Date.now()}-${uploaded.filename}`,
			url: uploaded.url,
			thumbnail_url: uploaded.thumbnail_url
		});
		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			attachmentSubmittingField = null;
			return;
		}
		recordAttachments = [response.data, ...recordAttachments.filter((item) => item.id !== response.data?.id)];
		await loadAttachmentSignedUrls(recordAttachments);
		await syncRecordAttachmentValue(field, response.data.url);
		attachmentSubmittingField = null;
		toast.success($t('forms.attachmentLinked'));
	}

	async function removeAttachment(field: FormField, attachment: FormAttachment) {
		deletingAttachmentId = attachment.id;
		const response = await formsApi.deleteAttachment(attachment.id);
		if (response.code !== 0) {
			toast.error(response.message);
			deletingAttachmentId = null;
			return;
		}
		recordAttachments = recordAttachments.filter((item) => item.id !== attachment.id);
		const remainingSignedUrls = { ...recordAttachmentSignedUrls };
		delete remainingSignedUrls[attachment.id];
		recordAttachmentSignedUrls = remainingSignedUrls;
		if (attachment.url?.startsWith('blob:')) {
			URL.revokeObjectURL(attachment.url);
		}
		const currentValue = recordValueUrl(recordDraft[field.key]);
		if (currentValue === attachment.url) {
			await syncRecordAttachmentValue(field, '');
		}
		deletingAttachmentId = null;
		toast.success($t('forms.attachmentRemoved'));
	}

	function draftArray(fieldKey: string): string[] {
		const value = recordDraft[fieldKey];
		return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
	}

	function isDraftOptionSelected(fieldKey: string, option: string): boolean {
		return draftArray(fieldKey).includes(option);
	}

	function toggleDraftOption(fieldKey: string, option: string, checked: boolean) {
		const current = draftArray(fieldKey);
		setDraftValue(
			fieldKey,
			checked ? Array.from(new Set([...current, option])) : current.filter((item) => item !== option)
		);
	}

	function draftString(fieldKey: string): string {
		const value = recordDraft[fieldKey];
		return typeof value === 'string' ? value : '';
	}

	function updateChildDraftValue(fieldKey: string, value: unknown) {
		if (!childEditor) return;
		childEditor = { ...childEditor, draft: { ...childEditor.draft, [fieldKey]: value } };
	}

	function updateChildTitle(value: string) {
		if (!childEditor) return;
		childEditor = { ...childEditor, title: value };
	}

	function draftArrayFrom(draft: Record<string, unknown>, fieldKey: string): string[] {
		const value = draft[fieldKey];
		return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
	}

	function draftStringFrom(draft: Record<string, unknown>, fieldKey: string): string {
		const value = draft[fieldKey];
		return typeof value === 'string' ? value : '';
	}

	function isChildDraftOptionSelected(fieldKey: string, option: string): boolean {
		return childEditor ? draftArrayFrom(childEditor.draft, fieldKey).includes(option) : false;
	}

	function toggleChildDraftOption(fieldKey: string, option: string, checked: boolean) {
		if (!childEditor) return;
		const current = draftArrayFrom(childEditor.draft, fieldKey);
		updateChildDraftValue(
			fieldKey,
			checked ? Array.from(new Set([...current, option])) : current.filter((item) => item !== option)
		);
	}

	function optionDisabledForSingleDraft(field: FormField, option: string): boolean {
		return (optionDisabled(field, option) || optionArchived(field, option)) && draftString(field.key) !== option;
	}

	function optionDisabledForMultiDraft(field: FormField, option: string): boolean {
		return (optionDisabled(field, option) || optionArchived(field, option)) && !isDraftOptionSelected(field.key, option);
	}

	function optionDisabledForChildSingleDraft(field: FormField, option: string): boolean {
		return (
			(optionDisabled(field, option) || optionArchived(field, option)) &&
			(!childEditor || draftStringFrom(childEditor.draft, field.key) !== option)
		);
	}

	function optionDisabledForChildMultiDraft(field: FormField, option: string): boolean {
		return (
			(optionDisabled(field, option) || optionArchived(field, option)) &&
			!isChildDraftOptionSelected(field.key, option)
		);
	}

	type SignatureDraftScope = 'record' | 'child';

	let activeSignatureCanvas: HTMLCanvasElement | null = null;

	function signatureImageValue(draft: Record<string, unknown>, fieldKey: string): string {
		const value = draft[fieldKey];
		if (typeof value !== 'string') return '';
		return value.startsWith('data:image/') ||
			value.startsWith('/api/v1/uploads/signatures/') ||
			value.startsWith('/uploads/signatures/')
			? value
			: '';
	}

	function prepareSignatureCanvas(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
		const rect = canvas.getBoundingClientRect();
		const pixelRatio = window.devicePixelRatio || 1;
		const width = Math.max(320, Math.round(rect.width || 320));
		const height = Math.max(96, Math.round(rect.height || 112));
		if (canvas.width !== Math.round(width * pixelRatio) || canvas.height !== Math.round(height * pixelRatio)) {
			canvas.width = Math.round(width * pixelRatio);
			canvas.height = Math.round(height * pixelRatio);
			canvas.style.height = `${height}px`;
		}
		const context = canvas.getContext('2d');
		if (!context) return null;
		context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
		context.lineCap = 'round';
		context.lineJoin = 'round';
		context.lineWidth = 2;
		context.strokeStyle = '#0f172a';
		return context;
	}

	function signaturePoint(event: PointerEvent, canvas: HTMLCanvasElement): { x: number; y: number } {
		const rect = canvas.getBoundingClientRect();
		return {
			x: event.clientX - rect.left,
			y: event.clientY - rect.top
		};
	}

	function setSignatureValue(fieldKey: string, scope: SignatureDraftScope, value: string) {
		if (scope === 'child') {
			updateChildDraftValue(fieldKey, value);
		} else {
			setDraftValue(fieldKey, value);
		}
	}

	function captureSignatureCanvas(canvas: HTMLCanvasElement, fieldKey: string, scope: SignatureDraftScope) {
		setSignatureValue(fieldKey, scope, canvas.toDataURL('image/png'));
	}

	function startSignatureStroke(
		event: PointerEvent,
		fieldKey: string,
		scope: SignatureDraftScope,
		readOnly: boolean
	) {
		if (readOnly || !(event.currentTarget instanceof HTMLCanvasElement)) return;
		const canvas = event.currentTarget;
		const context = prepareSignatureCanvas(canvas);
		if (!context) return;
		const point = signaturePoint(event, canvas);
		context.beginPath();
		context.moveTo(point.x, point.y);
		activeSignatureCanvas = canvas;
		try {
			canvas.setPointerCapture(event.pointerId);
		} catch {
			// Synthetic smoke-test pointer events may not be registered for capture.
		}
		event.preventDefault();
		captureSignatureCanvas(canvas, fieldKey, scope);
	}

	function moveSignatureStroke(
		event: PointerEvent,
		fieldKey: string,
		scope: SignatureDraftScope,
		readOnly: boolean
	) {
		if (readOnly || !(event.currentTarget instanceof HTMLCanvasElement)) return;
		const canvas = event.currentTarget;
		if (activeSignatureCanvas !== canvas) return;
		const context = prepareSignatureCanvas(canvas);
		if (!context) return;
		const point = signaturePoint(event, canvas);
		context.lineTo(point.x, point.y);
		context.stroke();
		event.preventDefault();
		captureSignatureCanvas(canvas, fieldKey, scope);
	}

	function endSignatureStroke(
		event: PointerEvent,
		fieldKey: string,
		scope: SignatureDraftScope,
		readOnly: boolean
	) {
		if (!(event.currentTarget instanceof HTMLCanvasElement)) return;
		const canvas = event.currentTarget;
		if (activeSignatureCanvas !== canvas) return;
		activeSignatureCanvas = null;
		if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
		if (!readOnly) captureSignatureCanvas(canvas, fieldKey, scope);
	}

	function clearSignatureCanvas(fieldKey: string, scope: SignatureDraftScope, readOnly: boolean) {
		if (readOnly) return;
		const attribute = scope === 'child' ? 'data-child-signature-canvas' : 'data-signature-canvas';
		const canvas = document.querySelector<HTMLCanvasElement>(`canvas[${attribute}="${fieldKey}"]`);
		if (canvas) {
			const context = prepareSignatureCanvas(canvas);
			if (context) {
				context.save();
				context.setTransform(1, 0, 0, 1, 0, 0);
				context.clearRect(0, 0, canvas.width, canvas.height);
				context.restore();
			}
		}
		setSignatureValue(fieldKey, scope, '');
	}

	function displayValue(value: unknown): string {
		if (value === null || value === undefined || value === '') return '-';
		if (typeof value === 'boolean') return value ? $t('forms.yes') : $t('forms.no');
		if (typeof value === 'string' || typeof value === 'number') return String(value);
		if (Array.isArray(value)) return value.map(displayValue).join(', ');
		if (isRecord(value)) {
			if (typeof value.display === 'string') return value.display;
			if (typeof value.name === 'string') return value.name;
			if (typeof value.email === 'string') return value.email;
			if (typeof value.title === 'string') return value.title;
			if (typeof value.decimal === 'string')
				return `${value.decimal}${typeof value.currency === 'string' ? ` ${value.currency}` : ''}`;
			if (typeof value.value === 'number') return String(value.value);
			return JSON.stringify(value);
		}
		return String(value);
	}

	function displayAggregate(field: FormField): string {
		const aggregate = aggregates[field.key];
		const decimal = aggregate?.decimal ?? '0';
		if (field.type === 'amount') {
			const currency = aggregate?.currency ?? field.amount?.currency ?? '';
			return currency ? `${decimal} ${currency}` : decimal;
		}
		return decimal;
	}

	function childRecordFields(child: FormChildRecord): FormField[] {
		const childForm = forms.find((form) => form.id === child.record.form_id);
		return childForm ? parseFields(childForm.schema).filter((field) => field.visibility?.list !== false) : [];
	}

	function childRelationKeyForField(field: FormField): string {
		return field.relation?.relation_key || field.key;
	}

	function childRecordsForRelationField(field: FormField): FormChildRecord[] {
		const childForm = childFormForRelationField(field);
		const relationKey = childRelationKeyForField(field);
		return selectedRecordChildren.filter(
			(child) =>
				child.relation_key === relationKey && (!childForm || child.record.form_id === childForm.id)
		);
	}

	function childGridFieldsForRelationField(field: FormField): FormField[] {
		const childForm = childFormForRelationField(field);
		return childForm
			? parseFields(childForm.schema).filter(
					(childField) => childField.visibility?.list !== false && childField.type !== 'child_table'
				)
			: [];
	}

	function formListFields(form: UniversalForm): FormField[] {
		return parseFields(form.schema).filter((field) => field.visibility?.list !== false);
	}

	function formFieldSummary(form: UniversalForm): string {
		const labels = formListFields(form)
			.slice(0, 3)
			.map((field) => fieldLabel(field));
		return labels.length > 0 ? labels.join(' / ') : $t('forms.noFields');
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

	function inputValue(event: Event): string {
		return event.currentTarget instanceof HTMLInputElement ||
			event.currentTarget instanceof HTMLTextAreaElement
			? event.currentTarget.value
			: '';
	}

	function selectValue(event: Event): string {
		return event.currentTarget instanceof HTMLSelectElement ? event.currentTarget.value : '';
	}
</script>

<div class="mx-auto max-w-7xl space-y-5">
	<div
		class="flex flex-col gap-3 border-b border-slate-200 pb-4 dark:border-slate-700 sm:flex-row sm:items-center sm:justify-between"
	>
		<div>
			<div class="text-sm text-slate-600 dark:text-slate-400">
				<a href="/workspace" class="hover:text-blue-600">{$t('forms.workspace')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<a href={`/workspace/${workspaceId}/projects`} class="hover:text-blue-600">{$t('forms.projects')}</a>
				<span class="mx-1 text-slate-400">/</span>
				<a href={`/workspace/${workspaceId}/projects/${projectId}`} class="hover:text-blue-600"
					>{project?.name ?? $t('forms.projectFallback')}</a
				>
			</div>
			<h1 class="mt-2 text-2xl font-semibold text-slate-950 dark:text-slate-100">{$t('forms.title')}</h1>
		</div>
		<div class="flex flex-wrap gap-2">
			<Button variant="secondary" onclick={() => loadForms()}>
				<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.refresh')}
			</Button>
			<Button onclick={() => (showCreateForm = !showCreateForm)}>
				<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.newForm')}
			</Button>
		</div>
	</div>

	{#if loading}
		<div
			class="rounded-md border border-slate-200 bg-white p-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400"
		>
			{$t('forms.loadingForms')}
		</div>
	{:else}
		{#if showCreateForm || forms.length === 0}
			<section
				class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
			>
				<div class="mb-4 flex items-start justify-between gap-4">
					<div>
						<h2 class="text-base font-semibold text-slate-950 dark:text-slate-100">
							{$t('forms.defineType')}
						</h2>
						<p class="mt-1 text-sm text-slate-600 dark:text-slate-400">
							{$t('forms.defineTypeDescription')}
						</p>
					</div>
				</div>

				<div class="mb-5 rounded-md border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-950">
					<div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
						<div>
							<h3 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
								{$t('forms.templateLibrary')}
							</h3>
							<p class="text-xs text-slate-500 dark:text-slate-400">
								{$t('forms.templateLibraryDescription')}
							</p>
						</div>
						<Button size="sm" variant="secondary" loading={templatesLoading} onclick={loadScenarioTemplates}>
							<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
							{$t('forms.refreshTemplates')}
						</Button>
					</div>
					{#if scenarioTemplates.length > 0}
						<div class="mt-3 grid gap-2 lg:grid-cols-3">
							{#each scenarioTemplates as template}
								<div class="rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900">
									<div class="flex items-start justify-between gap-3">
										<div class="min-w-0">
											<div class="flex flex-wrap items-center gap-2">
												<h4 class="truncate text-sm font-semibold text-slate-950 dark:text-slate-100">
													{scenarioTemplateName(template, $t)}
												</h4>
												<span class="rounded bg-slate-100 px-1.5 py-0.5 text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300">
													{scenarioTemplateIndustry(template.industry, $t)}
												</span>
											</div>
											<p class="mt-1 line-clamp-2 text-xs text-slate-500 dark:text-slate-400">
												{scenarioTemplateDescription(template, $t)}
											</p>
											<p class="mt-2 font-mono text-[11px] text-slate-400">{template.key}</p>
										</div>
									</div>
									<div class="mt-3 flex flex-wrap justify-end gap-2">
										<Button size="sm" variant="secondary" onclick={() => applyScenarioTemplate(template)}>
											{$t('forms.applyTemplate')}
										</Button>
										<Button
											size="sm"
											loading={installingTemplateKey === template.key}
											onclick={() => installScenarioTemplate(template)}
										>
											<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
											{$t('forms.installTemplate')}
										</Button>
									</div>
								</div>
							{/each}
						</div>
					{:else}
						<p class="mt-3 text-sm text-slate-500 dark:text-slate-400">
							{templatesLoading ? $t('common.loading') : $t('forms.noTemplates')}
						</p>
					{/if}
				</div>

				<div class="grid gap-3 md:grid-cols-4">
					<label class="space-y-1">
						<span class="text-sm font-medium text-slate-700 dark:text-slate-300">{$t('forms.key')}</span>
						<input
							class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
							bind:value={formDraft.key}
						/>
					</label>
					<label class="space-y-1 md:col-span-2">
						<span class="text-sm font-medium text-slate-700 dark:text-slate-300">{$t('forms.name')}</span>
						<input
							class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
							bind:value={formDraft.name}
						/>
					</label>
					<label class="space-y-1">
						<span class="text-sm font-medium text-slate-700 dark:text-slate-300"
							>{$t('forms.titleTemplate')}</span
						>
						<input
							class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
							bind:value={formDraft.title_template}
						/>
					</label>
					<label class="space-y-1 md:col-span-4">
						<span class="text-sm font-medium text-slate-700 dark:text-slate-300">{$t('forms.description')}</span>
						<input
							class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
							bind:value={formDraft.description}
						/>
					</label>
				</div>
				<div class="mt-5 rounded-lg border border-slate-200 dark:border-slate-700">
					<div
						class="flex flex-col gap-2 border-b border-slate-200 px-3 py-3 dark:border-slate-700 sm:flex-row sm:items-center sm:justify-between"
					>
						<div>
							<h3 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
								{$t('forms.fieldDesigner')}
							</h3>
							<p class="text-xs text-slate-500 dark:text-slate-400">
								{$t('forms.fieldDesignerDescription')}
							</p>
						</div>
						<button
							type="button"
							class="rounded-md border border-slate-200 px-3 py-1.5 text-sm text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
							onclick={addFieldDraft}
						>
							{$t('forms.addField')}
						</button>
					</div>
					<div class="divide-y divide-slate-100 dark:divide-slate-800">
						{#each fieldDrafts as field, index}
							<div class="grid gap-3 p-3 md:grid-cols-[1fr_1fr_160px_90px_44px]">
								<label class="space-y-1">
									<span class="text-xs font-medium text-slate-600 dark:text-slate-400"
										>{$t('forms.fieldKey')}</span
									>
									<input
										class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
										placeholder="customer_name"
										bind:value={field.key}
									/>
								</label>
								<label class="space-y-1">
									<span class="text-xs font-medium text-slate-600 dark:text-slate-400"
										>{$t('forms.fieldLabel')}</span
									>
									<input
										class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
										placeholder={$t('forms.fieldLabelPlaceholder')}
										bind:value={field.label}
									/>
								</label>
								<label class="space-y-1">
									<span class="text-xs font-medium text-slate-600 dark:text-slate-400"
										>{$t('forms.fieldType')}</span
									>
									<select
										class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
										bind:value={field.type}
									>
										{#each fieldTypeOptions as type}
											<option value={type}>{$t(`forms.fieldTypes.${type}`)}</option>
										{/each}
									</select>
								</label>
								<label class="flex items-end gap-2 pb-2 text-sm text-slate-700 dark:text-slate-300">
									<input
										type="checkbox"
										class="h-4 w-4 rounded border-slate-300 text-blue-600"
										bind:checked={field.required}
									/>
									{$t('forms.required')}
								</label>
								<div class="flex items-end justify-end">
									<button
										type="button"
										class="inline-flex h-9 w-9 items-center justify-center rounded-md text-red-600 hover:bg-red-50 disabled:opacity-40 dark:hover:bg-red-950/30"
										disabled={fieldDrafts.length === 1}
										onclick={() => removeFieldDraft(index)}
										aria-label={$t('forms.removeField')}
									>
										<Trash2 class="h-4 w-4" aria-hidden="true" />
									</button>
								</div>
								{#if field.type === 'single_select' || field.type === 'multi_select'}
									<label class="space-y-1 md:col-span-5">
										<span class="text-xs font-medium text-slate-600 dark:text-slate-400"
											>{$t('forms.optionList')}</span
										>
										<textarea
											class="min-h-20 w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											placeholder={$t('forms.optionListPlaceholder')}
											bind:value={field.optionsText}
										></textarea>
									</label>
								{/if}
								{#if field.type === 'amount'}
									<div class="grid gap-3 md:col-span-5 md:grid-cols-[160px_120px]">
										<label class="space-y-1">
											<span class="text-xs font-medium text-slate-600 dark:text-slate-400"
												>{$t('forms.currency')}</span
											>
											<input
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm uppercase text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												maxlength="3"
												bind:value={field.currency}
											/>
										</label>
										<label class="space-y-1">
											<span class="text-xs font-medium text-slate-600 dark:text-slate-400"
												>{$t('forms.scale')}</span
											>
											<input
												type="number"
												min="0"
												max="8"
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												bind:value={field.scale}
											/>
										</label>
									</div>
								{/if}
							</div>
						{/each}
					</div>
				</div>
				<div class="mt-4 rounded-lg border border-slate-200 dark:border-slate-700">
					<button
						type="button"
						class="flex w-full items-center justify-between px-3 py-2 text-left text-sm font-medium text-slate-700 hover:bg-slate-50 dark:text-slate-300 dark:hover:bg-slate-800"
						onclick={() => {
							if (!showAdvancedSchema) buildSchemaFromDrafts();
							showAdvancedSchema = !showAdvancedSchema;
						}}
					>
						<span>{$t('forms.advancedSchema')}</span>
						<span class="text-xs text-slate-500"
							>{showAdvancedSchema ? $t('forms.hideAdvanced') : $t('forms.showAdvanced')}</span
						>
					</button>
					{#if showAdvancedSchema}
						<label class="block space-y-1 border-t border-slate-200 p-3 dark:border-slate-700">
							<span class="text-sm font-medium text-slate-700 dark:text-slate-300">{$t('forms.schemaJson')}</span>
							<textarea
								class="h-56 w-full rounded-md border border-slate-300 bg-white px-3 py-2 font-mono text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
								bind:value={formDraft.schemaText}
							></textarea>
						</label>
					{/if}
				</div>
					<div class="mt-4 flex justify-end">
						<Button loading={formSubmitting} onclick={handleCreateForm}>{$t('forms.createForm')}</Button>
					</div>
				</section>
			{/if}

			{#if forms.length > 0 && !selectedForm}
				<section
					class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
					data-form-list-home
				>
					<div class="mb-4">
						<div>
							<h2 class="text-lg font-semibold text-slate-950 dark:text-slate-100">
								{$t('forms.formListTitle')}
							</h2>
							<p class="mt-1 text-sm text-slate-600 dark:text-slate-400">
								{$t('forms.formListDescription')}
							</p>
						</div>
					</div>
					<div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
						{#each forms as form}
							<button
								type="button"
								class="min-h-40 rounded-lg border border-slate-200 bg-slate-50 p-4 text-left transition-colors hover:border-blue-200 hover:bg-blue-50 dark:border-slate-700 dark:bg-slate-950 dark:hover:border-blue-900 dark:hover:bg-blue-950/30"
								onclick={() => openForm(form.id)}
							>
								<div class="flex items-start justify-between gap-3">
									<div class="min-w-0">
										<div class="flex items-center gap-2">
											<TableProperties class="h-4 w-4 shrink-0 text-blue-600 dark:text-blue-300" aria-hidden="true" />
											<h3 class="truncate text-base font-semibold text-slate-950 dark:text-slate-100">
												{form.name}
											</h3>
										</div>
										<p class="mt-1 font-mono text-xs text-slate-500 dark:text-slate-400">
											{form.key}
										</p>
									</div>
									<span class="rounded bg-white px-2 py-0.5 text-xs font-medium text-slate-600 ring-1 ring-slate-200 dark:bg-slate-900 dark:text-slate-300 dark:ring-slate-700">
										{$t('forms.fieldCount', { values: { count: formListFields(form).length } })}
									</span>
								</div>
								<p class="mt-3 line-clamp-2 min-h-10 text-sm leading-5 text-slate-600 dark:text-slate-400">
									{form.description || formFieldSummary(form)}
								</p>
								<div class="mt-4 flex items-center justify-between gap-3 border-t border-slate-200 pt-3 text-xs dark:border-slate-800">
									<span class="truncate text-slate-500 dark:text-slate-400">
										{$t('forms.updatedAt', { values: { date: formatDate(form.updated_at) } })}
									</span>
									<span class="font-medium text-blue-700 dark:text-blue-300">{$t('forms.openForm')}</span>
								</div>
							</button>
							{/each}
						</div>
					</section>
			{/if}

				{#if forms.length > 0 && selectedForm}
					<section class="space-y-4">

									{#if activeMode === 'data'}
									<section
										class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
										data-form-data-list-toolbar
										>
											<div class="flex flex-col gap-3 xl:flex-row xl:items-start xl:justify-between">
												<div>
													<div class="flex flex-wrap items-center gap-2">
														<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
															{$t('forms.recordsList')}
														</h3>
														<span class="rounded-md bg-slate-100 px-2 py-0.5 text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300">
															{#if recordsLoading}
																{$t('common.loading')}
															{:else if viewRecords.length === records.length}
																{$t('forms.rows', { values: { count: records.length } })}
															{:else}
																{$t('forms.filteredRows', {
																	values: { visible: viewRecords.length, total: records.length }
																})}
															{/if}
														</span>
													</div>
												</div>
													<div class="flex flex-wrap items-center gap-1.5">
														<label class="inline-flex items-center gap-2 text-xs font-medium text-slate-600 dark:text-slate-300">
														<span>{$t('forms.dataTableSwitcher')}</span>
														<select
															id="form-switcher"
															value={selectedFormId}
															onchange={handleFormChange}
															class="h-8 min-w-40 rounded-md border border-slate-300 bg-white px-2 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
														>
															{#each forms as form}
																<option value={form.id}>{form.name}</option>
																{/each}
															</select>
														</label>
													<button
														type="button"
														class="inline-flex h-8 items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
														aria-expanded={viewToolsExpanded}
														data-view-tools-toggle
														onclick={() => (viewToolsExpanded = !viewToolsExpanded)}
													>
														<Filter class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
														{$t('forms.savedViews')}
														<ArrowDown
															class="ml-1.5 h-3.5 w-3.5 transition-transform {viewToolsExpanded ? 'rotate-180' : ''}"
															aria-hidden="true"
														/>
													</button>
												<Button size="sm" onclick={openRecordCreateDrawer}>
													<Plus class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
													{$t('forms.newRecordAction')}
												</Button>
												<button
													type="button"
													class="inline-flex h-8 items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
													onclick={() => handleModeChange('design')}
												>
													<TableProperties class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
													{$t('forms.modes.design')}
												</button>
												<button
													type="button"
													class="inline-flex h-8 items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
													onclick={() => handleModeChange('layout')}
												>
													<FileText class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
													{$t('forms.modes.layout')}
												</button>
												<button
													type="button"
													class="inline-flex h-8 items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
													onclick={() => handleModeChange('permissions')}
												>
													<Shield class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
													{$t('forms.modes.permissions')}
												</button>
												<button
													type="button"
													class="inline-flex h-8 items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
													onclick={() => handleModeChange('importExport')}
												>
													<FileUp class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
													{$t('forms.modes.importExport')}
												</button>
												<button
													type="button"
													class="inline-flex h-8 items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
													onclick={() => handleModeChange('automation')}
												>
													<Activity class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
													{$t('forms.modes.automation')}
													</button>
												</div>
											</div>
					</section>
								{:else}
									<section
										class="rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900"
										data-form-config-workbench
									>
										<div class="flex flex-col gap-3 border-b border-slate-200 p-4 dark:border-slate-700 sm:flex-row sm:items-center sm:justify-between">
											<div>
												<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
													{$t('forms.formConfig')}
												</h3>
												<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
													{$t(`forms.modeContext.${activeMode}`, { values: { name: selectedForm.name } })}
												</p>
											</div>
											<Button size="sm" variant="secondary" onclick={() => handleModeChange('data')}>
												<ChevronLeft class="mr-2 h-4 w-4" aria-hidden="true" />
												{$t('forms.backToRecords')}
											</Button>
										</div>
										<FormWorkflowTabs activeMode={activeMode} onModeChange={handleModeChange} />
									</section>
								{/if}
									{#if formPermissions && activeMode !== 'permissions' && activeMode !== 'data'}
									<section
										class="rounded-md border border-slate-200 bg-white px-3 py-2 text-sm dark:border-slate-700 dark:bg-slate-900"
										data-permission-state-summary
										data-permission-state-mode={activeMode}
										data-permission-state-scope={memberRecordScopeDraft}
										data-permission-state-hidden-fields={memberHiddenFieldLabels.join('|')}
										data-permission-state-locked-fields={memberLockedFieldLabels.join('|')}
									>
										<div class="flex flex-wrap items-center gap-2 text-xs">
											<span class="inline-flex items-center gap-1 font-medium text-slate-800 dark:text-slate-100">
												<Shield class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
												{$t('forms.permissionState')}
											</span>
											<span class="rounded-md border border-slate-200 bg-slate-50 px-2 py-1 text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200">
												{$t('forms.recordScope')}: {$t(`forms.recordScopes.${memberRecordScopeDraft}`)}
											</span>
											<span class="rounded-md border border-amber-200 bg-amber-50 px-2 py-1 text-amber-800 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-200">
												{$t('forms.hiddenFieldEffect')}: {permissionFieldList(memberHiddenFieldLabels)}
											</span>
											<span class="rounded-md border border-slate-200 bg-slate-50 px-2 py-1 text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200">
												{$t('forms.lockedFieldEffect')}: {permissionFieldList(memberLockedFieldLabels)}
											</span>
											</div>
										</section>
							{/if}

							{#if activeMode === 'automation' || activeMode === 'permissions'}
							{#if activeMode === 'automation'}
								<div data-form-mode="automation">
									<FormAutomationPanel form={selectedForm} printJobCount={printJobs.length} />
								</div>
							{/if}
							{#if activeMode === 'permissions'}
							<section
								class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
								data-form-mode="permissions"
							>
								<div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
									<div>
										<div class="flex items-center gap-2">
											<Shield class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
											<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.permissions')}
											</h3>
										</div>
										<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
											{$t('forms.permissionsDescription')}
										</p>
									</div>
									<Button size="sm" loading={permissionsSaving} onclick={saveMemberPermissions}>
										{$t('forms.savePermissions')}
									</Button>
								</div>
								{#if permissionsLoading}
									<p class="text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</p>
								{:else}
									<div class="grid gap-3 lg:grid-cols-[180px_minmax(0,1fr)]">
										<div class="rounded-md border border-slate-200 bg-slate-50 p-3 text-sm dark:border-slate-700 dark:bg-slate-950">
											<p class="font-medium text-slate-900 dark:text-slate-100">
												{$t('roles.member')}
											</p>
											<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
												{$t('forms.memberPermissionHint')}
											</p>
										</div>
										<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
											{#each formPermissionActions as action}
												<label
													class="flex min-h-12 items-center gap-2 rounded-md border border-slate-200 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:text-slate-300"
												>
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-slate-300 text-blue-600"
														checked={memberPermissionDraft[action]}
														onchange={(event) =>
															setMemberPermission(
																action,
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
															)}
													/>
													<span>{permissionActionLabel(action)}</span>
												</label>
											{/each}
										</div>
										<label class="rounded-md border border-slate-200 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:text-slate-300">
											<span class="block text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
												{$t('forms.recordScope')}
											</span>
											<select
												value={memberRecordScopeDraft}
												onchange={(event) => setMemberRecordScope(selectValue(event) as FormRecordScope)}
												class="mt-2 w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											>
												<option value="all">{$t('forms.recordScopeAll')}</option>
												<option value="owned">{$t('forms.recordScopeOwned')}</option>
											</select>
											<span class="mt-1 block text-xs text-slate-500 dark:text-slate-400">
												{$t('forms.recordScopeHint')}
											</span>
										</label>
										<div class="lg:col-span-2 rounded-md border border-slate-200 px-3 py-2 text-sm dark:border-slate-700">
											<div class="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
												<div>
													<p class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
														{$t('forms.fieldPermissions')}
													</p>
													<p class="text-xs text-slate-500 dark:text-slate-400">
														{$t('forms.fieldPermissionsHint')}
													</p>
												</div>
												<div class="flex gap-3 text-xs text-slate-500 dark:text-slate-400">
													<span
														data-field-read-denied-count={fields.filter((field) => memberFieldReadDraft[field.key] === false).length}
													>
														{$t('forms.fieldReadDeniedCount', {
															values: {
																count: fields.filter((field) => memberFieldReadDraft[field.key] === false).length
															}
														})}
													</span>
													<span
														data-field-write-denied-count={fields.filter((field) => memberFieldWriteDraft[field.key] === false).length}
													>
														{$t('forms.fieldWriteDeniedCount', {
															values: {
																count: fields.filter((field) => memberFieldWriteDraft[field.key] === false).length
															}
														})}
													</span>
												</div>
											</div>
											<div class="mt-3 grid gap-2 md:grid-cols-2" data-permission-effect-summary>
												<div
													class="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 dark:border-amber-900/60 dark:bg-amber-950/20"
													data-permission-effect-hidden-fields={memberHiddenFieldLabels.join('|')}
												>
													<div class="flex items-center gap-2">
														<AlertTriangle class="h-4 w-4 text-amber-600 dark:text-amber-300" aria-hidden="true" />
														<p class="text-xs font-medium uppercase text-amber-800 dark:text-amber-200">
															{$t('forms.hiddenFieldEffect')}
														</p>
													</div>
													<p class="mt-1 text-xs text-amber-900 dark:text-amber-100">
														{permissionFieldList(memberHiddenFieldLabels)}
													</p>
													<p class="mt-1 text-xs text-amber-700 dark:text-amber-300">
														{$t('forms.hiddenFieldEffectHint')}
													</p>
												</div>
												<div
													class="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 dark:border-slate-700 dark:bg-slate-950"
													data-permission-effect-locked-fields={memberLockedFieldLabels.join('|')}
												>
													<div class="flex items-center gap-2">
														<Shield class="h-4 w-4 text-slate-600 dark:text-slate-300" aria-hidden="true" />
														<p class="text-xs font-medium uppercase text-slate-600 dark:text-slate-300">
															{$t('forms.lockedFieldEffect')}
														</p>
													</div>
													<p class="mt-1 text-xs text-slate-800 dark:text-slate-100">
														{permissionFieldList(memberLockedFieldLabels)}
													</p>
													<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
														{$t('forms.lockedFieldEffectHint')}
													</p>
												</div>
											</div>
											<div class="mt-3 grid gap-2">
												{#each fields as field}
													<div
														class="grid min-h-10 grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-3 rounded-md border border-slate-200 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:text-slate-300"
													>
														<span class="min-w-0 truncate">{field.label ?? field.key}</span>
														<label class="flex items-center gap-2 text-xs">
															<input
																type="checkbox"
																class="h-4 w-4 rounded border-slate-300 text-blue-600"
																checked={memberFieldReadDraft[field.key] ?? true}
																data-field-read-permission={field.key}
																onchange={(event) =>
																	setMemberFieldRead(
																		field.key,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																	)}
															/>
															<span>{$t('forms.fieldRead')}</span>
														</label>
														<label class="flex items-center gap-2 text-xs">
															<input
																type="checkbox"
																class="h-4 w-4 rounded border-slate-300 text-blue-600"
																checked={memberFieldWriteDraft[field.key] ?? true}
																data-field-write-permission={field.key}
																onchange={(event) =>
																	setMemberFieldWrite(
																		field.key,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																	)}
															/>
															<span>{$t('forms.fieldWrite')}</span>
														</label>
													</div>
												{/each}
											</div>
										</div>
									</div>
									{#if formPermissions}
										<div class="mt-3 rounded-md bg-slate-50 px-3 py-2 text-xs text-slate-600 dark:bg-slate-950 dark:text-slate-400">
											{$t('forms.currentEffectiveRole', {
												values: { role: formPermissions.effective.role }
											})}
											·
											{formPermissionActions
												.filter((action) => formPermissions?.effective.actions[action])
												.map(permissionActionLabel)
												.join(' / ')}
											·
											{$t(`forms.recordScopes.${formPermissions.effective.record_scope}`)}
										</div>
									{/if}
								{/if}
							</section>
							{/if}
							{#if activeMode === 'automation' && printJobs.length > 0}
							<section
								class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
							>
							<div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
								<div>
									<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
										{$t('forms.printJobs')}
									</h3>
									<p class="text-sm text-slate-500 dark:text-slate-400">
										{$t('forms.printJobsDescription')}
									</p>
								</div>
								{#if printJobForm}
									<button
										type="button"
										class="rounded-md border border-slate-200 px-3 py-1.5 text-sm text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
										onclick={() => {
											selectedFormId = printJobForm.id;
											selectedRecord = null;
											selectedRecordLinks = [];
											selectedRecordChildren = [];
											selectedRecordComments = [];
											recordCommentDraft = '';
											relationTargets = {};
											void loadRecords();
										}}
									>
										{$t('forms.openPrintForm')}
									</button>
								{/if}
							</div>
							<div class="grid gap-2 md:grid-cols-3">
								{#each printJobs as job}
									<button
										type="button"
										class="rounded-md border border-slate-200 p-3 text-left hover:border-blue-200 hover:bg-blue-50 dark:border-slate-700 dark:hover:bg-slate-800"
										onclick={() => openPrintJob(job)}
									>
										<div class="flex items-center justify-between gap-2">
											<span class="truncate text-sm font-medium text-slate-900 dark:text-slate-100"
												>{displayValue(job.values.job_type) || job.title}</span
											>
											<span
												class="rounded px-2 py-0.5 text-xs font-medium {displayValue(
													job.values.status
												) === 'failed'
													? 'bg-red-50 text-red-700'
													: displayValue(job.values.status) === 'printed'
														? 'bg-emerald-50 text-emerald-700'
														: 'bg-slate-100 text-slate-600'}"
											>
												{displayValue(job.values.status)}
											</span>
										</div>
										<div class="mt-2 text-xs text-slate-500 dark:text-slate-400">
											{displayValue(job.values.printer)} · {$t('forms.retry')} {displayValue(
												job.values.retry_count
											)}
										</div>
									</button>
								{/each}
							</div>
							</section>
							{/if}
						{:else if activeMode === 'importExport'}
							<section
								class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
								data-form-mode="import-export"
							>
								<div class="mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
									<div>
										<div class="flex items-center gap-2">
											<FileUp class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
											<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.modes.importExport')}
											</h3>
										</div>
									</div>
									<Button size="sm" onclick={openImportRecords}>
										<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
										{$t('forms.importRecords')}
									</Button>
								</div>
								<div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
									<button
										type="button"
										class="flex min-h-24 flex-col items-start justify-between rounded-md border border-slate-200 bg-slate-50 p-3 text-left text-sm hover:border-blue-200 hover:bg-blue-50 dark:border-slate-700 dark:bg-slate-950 dark:hover:border-blue-900 dark:hover:bg-blue-950/30"
										onclick={exportImportTemplate}
										disabled={!selectedForm || visibleFields.length === 0}
										data-import-template-export
										data-import-template-exported={exportedImportTemplate ? 'true' : undefined}
									>
										<FileDown class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<span class="font-medium text-slate-900 dark:text-slate-100">
											{$t('forms.downloadImportTemplate')}
										</span>
									</button>
									<button
										type="button"
										class="flex min-h-24 flex-col items-start justify-between rounded-md border border-slate-200 bg-slate-50 p-3 text-left text-sm hover:border-blue-200 hover:bg-blue-50 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:hover:border-blue-900 dark:hover:bg-blue-950/30"
										onclick={exportCurrentView}
										disabled={exportingRecords}
									>
										<FileDown class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<span class="font-medium text-slate-900 dark:text-slate-100">
											{$t('forms.exportCurrentView')}
										</span>
									</button>
									<button
										type="button"
										class="flex min-h-24 flex-col items-start justify-between rounded-md border border-slate-200 bg-slate-50 p-3 text-left text-sm hover:border-blue-200 hover:bg-blue-50 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:hover:border-blue-900 dark:hover:bg-blue-950/30"
										onclick={exportCurrentViewJson}
										disabled={exportingRecords}
										data-current-view-export-json
										data-current-view-json-record-count={exportedCurrentViewJsonRecordCount ?? ''}
									>
										<FileText class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<span class="font-medium text-slate-900 dark:text-slate-100">
											{$t('forms.exportCurrentViewJson')}
										</span>
									</button>
									<button
										type="button"
										class="flex min-h-24 flex-col items-start justify-between rounded-md border border-slate-200 bg-slate-50 p-3 text-left text-sm hover:border-blue-200 hover:bg-blue-50 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:hover:border-blue-900 dark:hover:bg-blue-950/30"
										onclick={exportAttachmentManifest}
										disabled={exportingRecords}
										data-attachment-manifest-export
										data-attachment-manifest-count={exportedAttachmentManifestCount ?? ''}
									>
										<FileDown class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<span class="font-medium text-slate-900 dark:text-slate-100">
											{$t('forms.exportAttachmentManifest')}
										</span>
									</button>
									<button
										type="button"
										class="flex min-h-24 flex-col items-start justify-between rounded-md border border-slate-200 bg-slate-50 p-3 text-left text-sm hover:border-blue-200 hover:bg-blue-50 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:hover:border-blue-900 dark:hover:bg-blue-950/30"
										onclick={() => void exportAttachmentBundle()}
										disabled={exportingRecords}
										data-attachment-bundle-export
										data-attachment-bundle-count={exportedAttachmentBundleCount ?? ''}
									>
										<FileDown class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<span class="font-medium text-slate-900 dark:text-slate-100">
											{$t('forms.exportAttachmentBundle')}
										</span>
									</button>
									<button
										type="button"
										class="flex min-h-24 flex-col items-start justify-between rounded-md border border-slate-200 bg-slate-50 p-3 text-left text-sm hover:border-blue-200 hover:bg-blue-50 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:hover:border-blue-900 dark:hover:bg-blue-950/30"
										onclick={() => void exportAttachmentPackage()}
										disabled={exportingRecords}
										data-attachment-package-export
										data-attachment-package-count={exportedAttachmentPackageCount ?? ''}
										data-attachment-package-file-count={exportedAttachmentPackageFileCount ?? ''}
									>
										<FileDown class="h-4 w-4 text-slate-500 dark:text-slate-400" aria-hidden="true" />
										<span class="font-medium text-slate-900 dark:text-slate-100">
											{$t('forms.exportAttachmentPackage')}
										</span>
									</button>
								</div>
							</section>
						{:else if activeMode === 'design' || activeMode === 'layout'}
							<div
								class="grid min-w-0 gap-4"
								data-form-mode={activeMode === 'design' ? 'field-design' : 'detail-layout'}
							>
								{#if activeMode === 'design'}
								<FormDesignerWorkspace
									form={selectedForm}
									fieldDrafts={designerFieldDrafts}
									fieldTypeOptions={fieldTypeOptions}
									selectedIndex={selectedDesignerFieldIndex}
									selectedFieldUsage={selectedDesignerFieldUsage}
									selectedFieldDependencies={selectedDesignerFieldDependencies}
									hiddenFieldKeys={memberHiddenFieldKeys}
									lockedFieldKeys={memberLockedFieldKeys}
									usageLoading={designerUsageLoading}
									saving={designerSaving}
									onSelectField={(index) => (selectedDesignerFieldIndex = index)}
									onAppendField={addDesignerField}
									onUpdateField={updateDesignerField}
									onDuplicateField={duplicateDesignerField}
									onRemoveField={removeDesignerField}
									onMoveField={moveDesignerField}
									onSave={saveDesignerSchema}
								/>
								{/if}
								{#if activeMode === 'layout'}
								<div class="space-y-4">
									<div
										class="flex flex-wrap gap-1 rounded-lg border border-slate-200 bg-white p-1 dark:border-slate-700 dark:bg-slate-900"
										data-detail-layout-tabs
									>
										{#each detailLayoutModes as mode}
											<button
												type="button"
												class="inline-flex h-9 items-center rounded-md px-3 text-sm font-medium transition-colors {detailLayoutMode ===
												mode.key
													? 'bg-blue-600 text-white shadow-sm dark:bg-blue-500'
													: 'text-slate-600 hover:bg-slate-100 hover:text-slate-950 dark:text-slate-300 dark:hover:bg-slate-800 dark:hover:text-slate-100'}"
												aria-current={detailLayoutMode === mode.key ? 'page' : undefined}
												data-detail-layout-tab={mode.key}
												onclick={() => (detailLayoutMode = mode.key)}
											>
												{$t(mode.labelKey)}
											</button>
										{/each}
									</div>
										<section
											class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900 {detailLayoutMode ===
											'header'
												? ''
												: 'hidden'}"
											data-detail-header-builder={selectedForm?.id ?? ''}
										>
										<div class="mb-3">
											<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.detailHeader')}
											</h3>
											<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
												{$t('forms.detailHeaderDescription')}
											</p>
										</div>
										<div class="grid gap-3">
											<label class="space-y-1">
												<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
													{$t('forms.detailHeaderSubtitleField')}
												</span>
												<select
													value={detailHeaderDraft.subtitleField}
													disabled={detailHeaderSaving}
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
													data-detail-header-subtitle-field
													onchange={(event) =>
														setDetailHeaderDraft({
															subtitleField:
																event.currentTarget instanceof HTMLSelectElement
																	? event.currentTarget.value
																	: ''
														})}
												>
													<option value="">{$t('forms.detailHeaderNoField')}</option>
													{#each detailVisibleFields as field}
														<option value={field.key}>{fieldLabel(field)}</option>
													{/each}
												</select>
											</label>
											<label class="space-y-1">
												<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
													{$t('forms.detailHeaderBadgeField')}
												</span>
												<select
													value={detailHeaderDraft.badgeField}
													disabled={detailHeaderSaving}
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
													data-detail-header-badge-field
													onchange={(event) =>
														setDetailHeaderDraft({
															badgeField:
																event.currentTarget instanceof HTMLSelectElement
																	? event.currentTarget.value
																	: ''
														})}
												>
													<option value="">{$t('forms.detailHeaderNoField')}</option>
													{#each detailVisibleFields as field}
														<option value={field.key}>{fieldLabel(field)}</option>
													{/each}
												</select>
											</label>
										</div>
										<div class="mt-3 flex justify-end">
											<span data-detail-header-save>
												<Button size="sm" loading={detailHeaderSaving} onclick={saveDetailHeader}>
													{$t('forms.saveDetailHeader')}
												</Button>
											</span>
										</div>
									</section>
										<section
											class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900 {detailLayoutMode ===
											'highlights'
												? ''
												: 'hidden'}"
											data-detail-highlights-builder={selectedForm?.id ?? ''}
										>
										<div class="mb-3 flex items-start justify-between gap-3">
											<div>
												<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
													{$t('forms.detailHighlights')}
												</h3>
												<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
													{$t('forms.detailHighlightsDescription')}
												</p>
											</div>
											{#if detailHighlightSaving}
												<span class="text-xs font-medium text-slate-500 dark:text-slate-400">
													{$t('common.saving')}
												</span>
											{/if}
										</div>
											<div class="space-y-2">
											{#each detailVisibleFields as field}
												{@const highlightIndex = detailHighlightDraft.fields.indexOf(field.key)}
												{@const isChecked = highlightIndex >= 0}
												<div
													class="grid min-h-14 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-md border border-slate-200 px-3 py-2 dark:border-slate-700"
													data-detail-highlight-field={field.key}
													data-detail-highlight-selected={`${field.key}:${isChecked}`}
												>
													<label class="flex h-10 w-10 items-center justify-center">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-slate-300 text-blue-600"
															checked={isChecked}
															disabled={
																detailHighlightSaving ||
																(!isChecked && detailHighlightDraft.fields.length >= 4)
															}
															aria-label={fieldLabel(field)}
															data-detail-highlight-toggle={field.key}
															onchange={(event) =>
																setDetailHighlightField(
																	field.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>
													</label>
													<div class="min-w-0">
														<div class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">
															{fieldLabel(field)}
														</div>
														<p class="mt-0.5 truncate font-mono text-xs text-slate-500 dark:text-slate-400">
															{field.key}
														</p>
													</div>
													<div class="flex items-center gap-1">
														<button
															type="button"
															class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
															disabled={!isChecked || highlightIndex === 0 || detailHighlightSaving}
															aria-label={$t('forms.moveHighlightUp')}
															title={$t('forms.moveHighlightUp')}
															data-detail-highlight-move-up={field.key}
															onclick={() => moveDetailHighlightField(field.key, -1)}
														>
															<ArrowUp class="h-4 w-4" aria-hidden="true" />
														</button>
														<button
															type="button"
															class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
															disabled={
																!isChecked ||
																highlightIndex === detailHighlightDraft.fields.length - 1 ||
																detailHighlightSaving
															}
															aria-label={$t('forms.moveHighlightDown')}
															title={$t('forms.moveHighlightDown')}
															data-detail-highlight-move-down={field.key}
															onclick={() => moveDetailHighlightField(field.key, 1)}
														>
															<ArrowDown class="h-4 w-4" aria-hidden="true" />
														</button>
													</div>
												</div>
											{/each}
										</div>
										<div class="mt-3 flex items-center justify-between gap-3">
											<span class="text-xs text-slate-500 dark:text-slate-400">
												{$t('forms.detailHighlightsLimit', {
													values: { count: detailHighlightDraft.fields.length }
												})}
											</span>
											<span data-detail-highlights-save>
												<Button size="sm" loading={detailHighlightSaving} onclick={saveDetailHighlights}>
													{$t('forms.saveDetailHighlights')}
												</Button>
											</span>
										</div>
									</section>
										<section
											class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900 {detailLayoutMode ===
											'composition'
												? ''
												: 'hidden'}"
											data-detail-page-composition-builder={selectedForm?.id ?? ''}
										>
										<div class="mb-3">
											<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.detailPageComposition')}
											</h3>
											<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
												{$t('forms.detailPageCompositionDescription')}
											</p>
										</div>
										<label class="space-y-1">
											<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
												{$t('forms.detailPageWidgetRegion')}
											</span>
											<select
												value={detailPageCompositionDraft.widgetRegion}
												disabled={detailPageCompositionSaving}
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												data-detail-page-widget-region
												onchange={(event) =>
													(detailPageCompositionDraft = {
														...detailPageCompositionDraft,
														widgetRegion:
															event.currentTarget instanceof HTMLSelectElement
																? detailPageWidgetRegion(event.currentTarget.value)
																: 'sidebar'
													})}
											>
												<option value="sidebar">{$t('forms.detailPageWidgetRegionSidebar')}</option>
												<option value="below">{$t('forms.detailPageWidgetRegionBelow')}</option>
											</select>
										</label>
										<label class="mt-3 block space-y-1">
											<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
												{$t('forms.detailPageFieldColumns')}
											</span>
											<select
												value={String(detailPageCompositionDraft.fieldColumns)}
												disabled={detailPageCompositionSaving}
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												data-detail-page-field-columns
												onchange={(event) =>
													(detailPageCompositionDraft = {
														...detailPageCompositionDraft,
														fieldColumns:
															event.currentTarget instanceof HTMLSelectElement
																? detailPageFieldColumns(event.currentTarget.value)
																: 2
													})}
											>
												<option value="1">{$t('forms.detailPageFieldColumnsOne')}</option>
												<option value="2">{$t('forms.detailPageFieldColumnsTwo')}</option>
											</select>
										</label>
										<div class="mt-3 flex justify-end">
											<span data-detail-page-composition-save>
												<Button
													size="sm"
													loading={detailPageCompositionSaving}
													onclick={saveDetailPageComposition}
												>
													{$t('forms.saveDetailPageComposition')}
												</Button>
											</span>
										</div>
									</section>
										<section
											class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900 {detailLayoutMode ===
											'sections'
												? ''
												: 'hidden'}"
											data-detail-layout-section-builder={selectedForm?.id ?? ''}
										>
										<div class="mb-3 flex items-start justify-between gap-3">
											<div>
												<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
													{$t('forms.detailLayoutSections')}
												</h3>
												<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
													{$t('forms.detailLayoutSectionsDescription')}
												</p>
											</div>
											<Button size="sm" variant="secondary" onclick={addDetailLayoutSection}>
												<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
												{$t('forms.addDetailSection')}
											</Button>
										</div>
										<div class="space-y-3">
											{#each detailLayoutSectionDrafts as section, sectionIndex}
												<div
													class="rounded-md border border-slate-200 p-3 dark:border-slate-700"
													data-detail-layout-section-row={section.key}
												>
													<div class="mb-3 grid grid-cols-[minmax(0,1fr)_auto] items-end gap-2 md:grid-cols-[minmax(0,1fr)_120px_132px_auto]">
														<label class="min-w-0 space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.detailLayoutSectionTitle')}
															</span>
															<input
																value={section.title}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																data-detail-layout-section-title={section.key}
																oninput={(event) =>
																	updateDetailLayoutSectionTitle(
																		sectionIndex,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.value
																			: ''
																)}
															/>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.detailLayoutSectionColumns')}
															</span>
															<select
																value={String(section.columns)}
																disabled={detailLayoutSectionSaving}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																data-detail-layout-section-columns={section.key}
																onchange={(event) =>
																	updateDetailLayoutSectionColumns(
																		sectionIndex,
																		event.currentTarget instanceof HTMLSelectElement
																			? detailLayoutSectionColumns(event.currentTarget.value)
																			: 2
																	)}
															>
																<option value="1">{$t('forms.detailLayoutSectionColumnsOne')}</option>
																<option value="2">{$t('forms.detailLayoutSectionColumnsTwo')}</option>
																<option value="3">{$t('forms.detailLayoutSectionColumnsThree')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.detailLayoutSectionDensity')}
															</span>
															<select
																value={section.density}
																disabled={detailLayoutSectionSaving}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																data-detail-layout-section-density={section.key}
																onchange={(event) =>
																	updateDetailLayoutSectionDensity(
																		sectionIndex,
																		event.currentTarget instanceof HTMLSelectElement &&
																			event.currentTarget.value === 'compact'
																			? 'compact'
																			: 'comfortable'
																	)}
															>
																<option value="comfortable">{$t('forms.detailLayoutSectionDensityComfortable')}</option>
																<option value="compact">{$t('forms.detailLayoutSectionDensityCompact')}</option>
															</select>
														</label>
														<div class="flex items-center gap-1">
															<button
																type="button"
																class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
																disabled={sectionIndex === 0 || detailLayoutSectionSaving}
																aria-label={$t('forms.moveSectionUp')}
																title={$t('forms.moveSectionUp')}
																data-detail-layout-section-move-up={section.key}
																onclick={() => moveDetailLayoutSection(sectionIndex, -1)}
															>
																<ArrowUp class="h-4 w-4" aria-hidden="true" />
															</button>
															<button
																type="button"
																class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
																disabled={
																	sectionIndex === detailLayoutSectionDrafts.length - 1 ||
																	detailLayoutSectionSaving
																}
																aria-label={$t('forms.moveSectionDown')}
																title={$t('forms.moveSectionDown')}
																data-detail-layout-section-move-down={section.key}
																onclick={() => moveDetailLayoutSection(sectionIndex, 1)}
															>
																<ArrowDown class="h-4 w-4" aria-hidden="true" />
															</button>
															<button
																type="button"
																class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-red-600 hover:bg-red-50 disabled:opacity-40 dark:border-slate-700 dark:hover:bg-red-950/30"
																disabled={detailLayoutSectionDrafts.length <= 1 || detailLayoutSectionSaving}
																aria-label={$t('forms.removeDetailSection')}
																title={$t('forms.removeDetailSection')}
																data-detail-layout-section-remove={section.key}
																onclick={() => removeDetailLayoutSection(sectionIndex)}
															>
																<Trash2 class="h-4 w-4" aria-hidden="true" />
															</button>
														</div>
													</div>
													<label class="mb-3 block space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.detailLayoutSectionDescription')}
														</span>
														<textarea
															value={section.description}
															rows="2"
															disabled={detailLayoutSectionSaving}
															placeholder={$t('forms.detailLayoutSectionDescriptionPlaceholder')}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 placeholder:text-slate-400 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															data-detail-layout-section-description={section.key}
															oninput={(event) =>
																updateDetailLayoutSectionDescription(
																	sectionIndex,
																	event.currentTarget instanceof HTMLTextAreaElement
																		? event.currentTarget.value
																		: ''
															)}
														></textarea>
													</label>
													<label class="mb-3 flex items-center gap-2 text-sm text-slate-700 dark:text-slate-200">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-slate-300 text-blue-600"
															checked={section.hideEmptyFields}
															disabled={detailLayoutSectionSaving}
															data-detail-layout-section-hide-empty-fields={section.key}
															onchange={(event) =>
																updateDetailLayoutSectionHideEmptyFields(
																	sectionIndex,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>
														<span>{$t('forms.detailLayoutSectionHideEmptyFields')}</span>
													</label>
													<div class="mb-3 grid gap-2 sm:grid-cols-2">
														<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-200">
															<input
																type="checkbox"
																class="h-4 w-4 rounded border-slate-300 text-blue-600"
																checked={section.collapsible}
																disabled={detailLayoutSectionSaving}
																data-detail-layout-section-collapsible={section.key}
																onchange={(event) =>
																	updateDetailLayoutSectionCollapsible(
																		sectionIndex,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																	)
																}
															/>
															<span>{$t('forms.detailLayoutSectionCollapsible')}</span>
														</label>
														<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-200">
															<input
																type="checkbox"
																class="h-4 w-4 rounded border-slate-300 text-blue-600"
																checked={section.defaultCollapsed}
																disabled={detailLayoutSectionSaving || !section.collapsible}
																data-detail-layout-section-default-collapsed={section.key}
																onchange={(event) =>
																	updateDetailLayoutSectionDefaultCollapsed(
																		sectionIndex,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																	)
																}
															/>
															<span>{$t('forms.detailLayoutSectionDefaultCollapsed')}</span>
														</label>
													</div>
													<div class="space-y-1.5">
														{#each detailVisibleFields as field}
															{@const assignedSectionKey = detailLayoutFieldAssignedTo(field.key)}
															{@const fieldIndex = section.fields.indexOf(field.key)}
															<div
																class="grid min-h-9 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded-md px-2 py-1 text-sm hover:bg-slate-50 dark:hover:bg-slate-800"
																data-detail-layout-section-field={`${section.key}:${field.key}`}
																data-detail-layout-section-field-assigned={assignedSectionKey || 'unassigned'}
															>
																<input
																	type="checkbox"
																	class="h-4 w-4 rounded border-slate-300 text-blue-600"
																	checked={section.fields.includes(field.key)}
																	disabled={detailLayoutSectionSaving}
																	aria-label={`${$t('forms.detailLayoutField')}: ${fieldLabel(field)} / ${section.title}`}
																	onchange={(event) =>
																		setDetailLayoutSectionField(
																			sectionIndex,
																			field.key,
																			event.currentTarget instanceof HTMLInputElement
																				? event.currentTarget.checked
																				: false
																		)}
																/>
																<div class="min-w-0">
																	<span class="block truncate text-slate-800 dark:text-slate-200">
																		{fieldLabel(field)}
																	</span>
																	{#if assignedSectionKey && assignedSectionKey !== section.key}
																		<span class="text-xs text-slate-500 dark:text-slate-400">
																			{$t('forms.detailLayoutFieldAssigned')}
																		</span>
																	{/if}
																</div>
																<div class="flex items-center gap-1">
																	<button
																		type="button"
																		class="inline-flex h-7 w-7 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
																		disabled={fieldIndex <= 0 || detailLayoutSectionSaving}
																		aria-label={$t('forms.moveFieldUp')}
																		title={$t('forms.moveFieldUp')}
																		data-detail-layout-field-move-up={`${section.key}:${field.key}`}
																		onclick={() => moveDetailLayoutSectionField(sectionIndex, field.key, -1)}
																	>
																		<ArrowUp class="h-3.5 w-3.5" aria-hidden="true" />
																	</button>
																	<button
																		type="button"
																		class="inline-flex h-7 w-7 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
																		disabled={
																			fieldIndex < 0 ||
																			fieldIndex === section.fields.length - 1 ||
																			detailLayoutSectionSaving
																		}
																		aria-label={$t('forms.moveFieldDown')}
																		title={$t('forms.moveFieldDown')}
																		data-detail-layout-field-move-down={`${section.key}:${field.key}`}
																		onclick={() => moveDetailLayoutSectionField(sectionIndex, field.key, 1)}
																	>
																		<ArrowDown class="h-3.5 w-3.5" aria-hidden="true" />
																	</button>
																</div>
															</div>
														{/each}
													</div>
												</div>
											{/each}
										</div>
										<div class="mt-3 flex justify-end">
											<span data-detail-layout-section-save>
												<Button
													size="sm"
													loading={detailLayoutSectionSaving}
													onclick={saveDetailLayoutSections}
												>
													{$t('forms.saveDetailSections')}
												</Button>
											</span>
										</div>
									</section>
									<section
										class="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900 {detailLayoutMode ===
										'widgets'
											? ''
											: 'hidden'}"
										data-detail-page-widget-builder={selectedForm?.id ?? ''}
									>
									<div class="mb-3 flex items-start justify-between gap-3">
										<div>
											<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.detailPageWidgets')}
											</h3>
											<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
												{$t('forms.detailPageWidgetsDescription')}
											</p>
										</div>
										{#if detailPageWidgetSaving}
											<span class="text-xs font-medium text-slate-500 dark:text-slate-400">
												{$t('common.saving')}
											</span>
										{/if}
									</div>
									<div class="space-y-2">
										{#each selectedDetailPageWidgetEntries as widget, index}
											{@const option = detailPageWidgetOption(widget.key)}
											<div
												class="grid min-h-16 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-md border border-slate-200 px-3 py-2 dark:border-slate-700"
												data-detail-page-widget-row={widget.key}
												data-detail-page-widget-enabled={`${widget.key}:${widget.enabled}`}
											>
												<label class="flex h-10 w-10 items-center justify-center">
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-slate-300 text-blue-600"
														checked={widget.enabled}
														disabled={detailPageWidgetSaving}
														aria-label={$t(option.labelKey)}
														data-detail-page-widget-toggle={widget.key}
														onchange={(event) =>
															void setDetailPageWidgetEnabled(
																widget.key,
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
															)}
													/>
												</label>
												<div class="min-w-0">
													<div class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">
														{$t(option.labelKey)}
													</div>
													<p class="mt-0.5 text-xs leading-5 text-slate-500 dark:text-slate-400">
														{$t(option.descriptionKey)}
													</p>
													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.detailPageWidgetCustomTitle')}
														</span>
														<input
															value={widget.title}
															disabled={detailPageWidgetSaving}
															placeholder={$t(option.labelKey)}
															class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															data-detail-page-widget-title={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetTitle(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.value
																		: ''
															)}
														/>
													</label>
													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.detailPageWidgetDescription')}
														</span>
														<textarea
															value={widget.description}
															disabled={detailPageWidgetSaving}
															rows="2"
															placeholder={$t('forms.detailPageWidgetDescriptionPlaceholder')}
															class="w-full resize-y rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															data-detail-page-widget-description={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDescription(
																	widget.key,
																	event.currentTarget instanceof HTMLTextAreaElement
																		? event.currentTarget.value
																		: ''
																)}
														></textarea>
													</label>
														<label class="mt-2 block space-y-1">
															<span class="text-[11px] font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.detailPageWidgetLimit')}
															</span>
														<input
															type="number"
															min="1"
															max="50"
															value={widget.limit}
															disabled={detailPageWidgetSaving}
															placeholder={$t('forms.detailPageWidgetLimitDefault')}
															class="w-28 rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															data-detail-page-widget-limit={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetLimit(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.value
																		: ''
																)}
															/>
														</label>
														<label class="mt-2 block space-y-1">
															<span class="text-[11px] font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.detailPageWidgetDensity')}
															</span>
															<select
																value={widget.density}
																disabled={detailPageWidgetSaving}
																class="w-40 rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																data-detail-page-widget-density={widget.key}
																onchange={(event) =>
																	void setDetailPageWidgetDensity(
																		widget.key,
																		event.currentTarget instanceof HTMLSelectElement &&
																			event.currentTarget.value === 'compact'
																			? 'compact'
																			: 'comfortable'
																	)}
															>
																<option value="comfortable">{$t('forms.detailPageWidgetDensityComfortable')}</option>
																<option value="compact">{$t('forms.detailPageWidgetDensityCompact')}</option>
															</select>
														</label>
														<label class="mt-2 inline-flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
															<input
																type="checkbox"
															class="h-4 w-4 rounded border-slate-300 text-blue-600"
															checked={widget.showCount}
															disabled={detailPageWidgetSaving}
															data-detail-page-widget-show-count={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetShowCount(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>
														<span>{$t('forms.detailPageWidgetShowCount')}</span>
													</label>
													<label class="mt-2 inline-flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-slate-300 text-blue-600"
															checked={widget.hideEmpty}
															disabled={detailPageWidgetSaving}
															data-detail-page-widget-hide-empty={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetHideEmpty(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>
														<span>{$t('forms.detailPageWidgetHideEmpty')}</span>
													</label>
													<label class="mt-2 inline-flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-slate-300 text-blue-600"
															checked={widget.collapsible}
															disabled={detailPageWidgetSaving}
															data-detail-page-widget-collapsible={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetCollapsible(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>
														<span>{$t('forms.detailPageWidgetCollapsible')}</span>
													</label>
													<label class="mt-2 inline-flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-slate-300 text-blue-600"
															checked={widget.defaultCollapsed}
															disabled={detailPageWidgetSaving || !widget.collapsible}
															data-detail-page-widget-default-collapsed={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDefaultCollapsed(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>
														<span>{$t('forms.detailPageWidgetDefaultCollapsed')}</span>
													</label>
													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.detailPageWidgetEmptyText')}
														</span>
														<textarea
															value={widget.emptyText}
															disabled={detailPageWidgetSaving}
															rows="2"
															placeholder={$t('forms.detailPageWidgetEmptyTextPlaceholder')}
															class="w-full resize-y rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															data-detail-page-widget-empty-text={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetEmptyText(
																	widget.key,
																	event.currentTarget instanceof HTMLTextAreaElement
																		? event.currentTarget.value
																		: ''
																)}
														></textarea>
													</label>
												</div>
												<div class="flex items-center gap-1">
													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
														disabled={index === 0 || detailPageWidgetSaving}
														aria-label={$t('forms.moveWidgetUp')}
														title={$t('forms.moveWidgetUp')}
														data-detail-page-widget-move-up={widget.key}
														onclick={() => void moveDetailPageWidget(widget.key, -1)}
													>
														<ArrowUp class="h-4 w-4" aria-hidden="true" />
													</button>
													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
														disabled={index === selectedDetailPageWidgetEntries.length - 1 || detailPageWidgetSaving}
														aria-label={$t('forms.moveWidgetDown')}
														title={$t('forms.moveWidgetDown')}
														data-detail-page-widget-move-down={widget.key}
														onclick={() => void moveDetailPageWidget(widget.key, 1)}
													>
														<ArrowDown class="h-4 w-4" aria-hidden="true" />
													</button>
												</div>
											</div>
										{/each}
										</div>
									</section>
								</div>
								{/if}
							</div>
						{:else}

							<div
								class={activeMode === 'detail'
									? 'grid min-w-0 gap-4 xl:grid-cols-[minmax(0,1fr)_360px]'
									: 'grid min-w-0 gap-4'}
								>
									{#if activeMode === 'data'}
									<div class="min-w-0 space-y-4" data-record-list-stack>
										{#if viewToolsExpanded}
									<section
										class="w-full rounded-lg border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900"
										data-view-tools-expanded-panel
									>
									<div class="mb-3 flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
										<div>
											<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
											{$t('forms.savedViews')}
										</h3>
										<p class="text-sm text-slate-500 dark:text-slate-400">
											{$t('forms.savedViewsDescription')}
										</p>
									</div>
									<div class="flex flex-wrap gap-2">
										<Button size="sm" variant="secondary" onclick={startNewViewDraft}>
											<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
											{$t('forms.newView')}
										</Button>
										<Button size="sm" loading={viewSubmitting} onclick={saveViewDraft}>
											{$t('forms.saveView')}
										</Button>
										</div>
										</div>
										<div
											class="max-h-[62vh] space-y-4 overflow-auto pr-1"
										>
										<details
											class="rounded-md border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900"
											data-view-setup-disclosure
										>
											<summary
												class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 text-sm font-semibold text-slate-900 marker:hidden dark:text-slate-100"
											>
												<span>{$t('forms.viewSetup')}</span>
												<span class="text-xs font-normal text-slate-500 dark:text-slate-400">
													{$t('forms.viewSetupDescription')}
												</span>
											</summary>
											<div class="space-y-2 border-t border-slate-100 p-3 dark:border-slate-800">
										<label class="block space-y-1">
											<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
												{$t('forms.currentView')}
											</span>
											<select
												value={selectedViewId}
												disabled={viewsLoading}
												onchange={(event) => {
													selectedViewId = selectValue(event);
													resetViewDraft();
													void loadRecords();
												}}
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											>
												<option value="">
													{viewsLoading ? $t('common.loading') : $t('forms.unsavedView')}
													</option>
													{#each formViews as view}
														<option value={view.id}>
															{viewOptionLabel(view)}
														</option>
													{/each}
												</select>
										</label>
										<div class="grid grid-cols-2 gap-2">
											<label class="space-y-1">
												<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
													{$t('forms.viewKey')}
												</span>
												<input
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-50 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
													value={viewDraft.key}
													disabled={Boolean(selectedViewId)}
													oninput={(event) => (viewDraft = { ...viewDraft, key: inputValue(event) })}
												/>
											</label>
											<label class="space-y-1">
												<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
													{$t('forms.viewType')}
												</span>
												<select
													value={viewDraft.view_type}
													onchange={(event) =>
														(viewDraft = {
															...viewDraft,
																view_type: selectValue(event) as FormView['view_type'],
																pivotRowField:
																	selectValue(event) === 'pivot' ? viewDraft.pivotRowField : '',
																pivotColumnField:
																	selectValue(event) === 'pivot' ? viewDraft.pivotColumnField : '',
				pivotValueField:
					selectValue(event) === 'pivot' ? viewDraft.pivotValueField : '',
				pivotAggregate:
					selectValue(event) === 'pivot' ? viewDraft.pivotAggregate : 'sum',
				pivotTotals:
					selectValue(event) === 'pivot' ? viewDraft.pivotTotals : 'all',
				pivotValueDisplay:
					selectValue(event) === 'pivot' ? viewDraft.pivotValueDisplay : 'value',
				pivotSort:
					selectValue(event) === 'pivot' ? viewDraft.pivotSort : 'label',
					pivotEmptyBuckets:
						selectValue(event) === 'pivot' ? viewDraft.pivotEmptyBuckets : false,
					pivotHideZeroBuckets:
						selectValue(event) === 'pivot' ? viewDraft.pivotHideZeroBuckets : false,
					pivotShowCounts:
						selectValue(event) === 'pivot' ? viewDraft.pivotShowCounts : false,
				pivotHeatmap:
					selectValue(event) === 'pivot' ? viewDraft.pivotHeatmap : false,
				pivotRowLimit:
					selectValue(event) === 'pivot' ? viewDraft.pivotRowLimit : 0,
				pivotColumnLimit:
					selectValue(event) === 'pivot' ? viewDraft.pivotColumnLimit : 0,
				ganttStartField:
					selectValue(event) === 'gantt' ? viewDraft.ganttStartField : '',
																ganttEndField:
																	selectValue(event) === 'gantt' ? viewDraft.ganttEndField : '',
																ganttGroupBy:
																	selectValue(event) === 'gantt' ? viewDraft.ganttGroupBy : '',
																ganttDependencyField:
																	selectValue(event) === 'gantt' ? viewDraft.ganttDependencyField : '',
																ganttScale:
																	selectValue(event) === 'gantt' ? viewDraft.ganttScale : 'day',
																swimlaneBy:
																	selectValue(event) === 'kanban' ? viewDraft.swimlaneBy : '',
															calendarRange:
																selectValue(event) === 'calendar' ? viewDraft.calendarRange : 'week',
															calendarFocusDate:
																selectValue(event) === 'calendar' ? viewDraft.calendarFocusDate : '',
															calendarRecurrence:
																selectValue(event) === 'calendar' ? viewDraft.calendarRecurrence : 'none',
															calendarDayScope:
																selectValue(event) === 'calendar' ? viewDraft.calendarDayScope : 'all',
															calendarLayout:
																selectValue(event) === 'calendar' ? viewDraft.calendarLayout : 'grid',
															calendarHideEmptyDays:
																selectValue(event) === 'calendar' ? viewDraft.calendarHideEmptyDays : false,
															calendarEndField:
																selectValue(event) === 'calendar' ? viewDraft.calendarEndField : '',
																calendarResourceBy:
																	selectValue(event) === 'calendar' ? viewDraft.calendarResourceBy : '',
																calendarExceptionRecordId:
																	selectValue(event) === 'calendar' ? viewDraft.calendarExceptionRecordId : '',
																calendarExceptionDate:
																	selectValue(event) === 'calendar' ? viewDraft.calendarExceptionDate : '',
																calendarExceptionDates:
																	selectValue(event) === 'calendar' ? viewDraft.calendarExceptionDates : {},
																	cardTitleField:
																		selectValue(event) === 'card' ? viewDraft.cardTitleField : '',
																cardSubtitleField:
																	selectValue(event) === 'card' ? viewDraft.cardSubtitleField : '',
																cardBadgeField:
																	selectValue(event) === 'card' ? viewDraft.cardBadgeField : '',
																	cardCoverField:
																		selectValue(event) === 'card' ? viewDraft.cardCoverField : '',
																	cardLayout:
																		selectValue(event) === 'card' ? viewDraft.cardLayout : 'standard',
																	cardCoverAspect:
																		selectValue(event) === 'card' ? viewDraft.cardCoverAspect : 'auto',
																	cardCoverFit:
																		selectValue(event) === 'card' ? viewDraft.cardCoverFit : 'cover',
																	cardCoverPosition:
																		selectValue(event) === 'card' ? viewDraft.cardCoverPosition : 'top',
																	cardFieldLimit:
																		selectValue(event) === 'card' ? viewDraft.cardFieldLimit : 6,
																		cardHideEmptyFields:
																			selectValue(event) === 'card' ? viewDraft.cardHideEmptyFields : false,
																		cardShowTimestamps:
																			selectValue(event) === 'card' ? viewDraft.cardShowTimestamps : false,
																		cardShowRecordId:
																			selectValue(event) === 'card' ? viewDraft.cardShowRecordId : false,
																		timelineSource:
																			selectValue(event) === 'timeline' ? viewDraft.timelineSource : 'records',
																timelineEventLayout:
																	selectValue(event) === 'timeline' ? viewDraft.timelineEventLayout : 'summary',
																timelineEventType:
																	selectValue(event) === 'timeline' ? viewDraft.timelineEventType : '',
																timelineEventActor:
																	selectValue(event) === 'timeline' ? viewDraft.timelineEventActor : '',
																timelineEventAggregate:
																	selectValue(event) === 'timeline' ? viewDraft.timelineEventAggregate : '',
																timelineEventQuery:
																	selectValue(event) === 'timeline' ? viewDraft.timelineEventQuery : '',
																timelineEventFrom:
																	selectValue(event) === 'timeline' ? viewDraft.timelineEventFrom : '',
																timelineEventTo:
																	selectValue(event) === 'timeline' ? viewDraft.timelineEventTo : '',
																timelineTitleField:
																	selectValue(event) === 'timeline' ? viewDraft.timelineTitleField : '',
																timelineSubtitleField:
																	selectValue(event) === 'timeline' ? viewDraft.timelineSubtitleField : '',
																wipLimits:
																	selectValue(event) === 'kanban' ? viewDraft.wipLimits : {}
															})}
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												>
													<option value="grid">Grid</option>
													<option value="detail">Detail</option>
													<option value="kanban">Kanban</option>
														<option value="calendar">Calendar</option>
														<option value="card">Card</option>
														<option value="timeline">Timeline</option>
														<option value="pivot">Pivot</option>
														<option value="gantt">Gantt</option>
																</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineSource')}
															</span>
															<select
																value={viewDraft.timelineSource}
																disabled={viewDraft.view_type !== 'timeline'}
																onchange={(event) => {
																	const timelineSource = selectValue(event) as ViewTimelineSource;
																	viewDraft = {
																		...viewDraft,
																		timelineSource,
																		timelineEventLayout:
																			timelineSource === 'events' ? viewDraft.timelineEventLayout : 'summary',
																		timelineEventType:
																			timelineSource === 'events' ? viewDraft.timelineEventType : '',
																		timelineEventActor:
																			timelineSource === 'events' ? viewDraft.timelineEventActor : '',
																		timelineEventAggregate:
																			timelineSource === 'events' ? viewDraft.timelineEventAggregate : '',
																		timelineEventQuery:
																			timelineSource === 'events' ? viewDraft.timelineEventQuery : '',
																		timelineEventFrom:
																			timelineSource === 'events' ? viewDraft.timelineEventFrom : '',
																		timelineEventTo:
																			timelineSource === 'events' ? viewDraft.timelineEventTo : '',
																		timelineTitleField:
																			timelineSource === 'events' ? '' : viewDraft.timelineTitleField,
																		timelineSubtitleField:
																			timelineSource === 'events' ? '' : viewDraft.timelineSubtitleField
																	};
																}}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="records">{$t('forms.timelineSourceRecords')}</option>
																<option value="events">{$t('forms.timelineSourceEvents')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineEventLayout')}
															</span>
															<select
																value={viewDraft.timelineEventLayout}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource !== 'events'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineEventLayout: timelineEventLayout(selectValue(event))
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="summary">{$t('forms.timelineEventLayoutSummary')}</option>
																<option value="detailed">{$t('forms.timelineEventLayoutDetailed')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineEventType')}
															</span>
															<select
																value={viewDraft.timelineEventType}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource !== 'events'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineEventType: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.timelineEventTypeAll')}</option>
																{#each timelineEventTypes as eventType}
																	<option value={eventType}>{eventType}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineEventActorFilter')}
															</span>
															<select
																value={viewDraft.timelineEventActor}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource !== 'events'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineEventActor: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.timelineEventActorAll')}</option>
																{#each timelineEventActors as actorId}
																	<option value={actorId}>{actorId}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineEventAggregateFilter')}
															</span>
															<select
																value={viewDraft.timelineEventAggregate}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource !== 'events'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineEventAggregate: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.timelineEventAggregateAll')}</option>
																{#each timelineEventAggregates as aggregate}
																	<option value={aggregate}>{aggregate}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineEventSearch')}
															</span>
															<input
																value={viewDraft.timelineEventQuery}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource !== 'events'}
																oninput={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineEventQuery: inputValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
																placeholder={$t('forms.timelineEventSearchPlaceholder')}
															/>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineEventFrom')}
															</span>
															<input
																type="datetime-local"
																value={viewDraft.timelineEventFrom}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource !== 'events'}
																oninput={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineEventFrom: inputValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															/>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineEventTo')}
															</span>
															<input
																type="datetime-local"
																value={viewDraft.timelineEventTo}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource !== 'events'}
																oninput={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineEventTo: inputValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															/>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineTitleField')}
															</span>
															<select
																value={viewDraft.timelineTitleField}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource === 'events'}
																onchange={(event) => {
																	const timelineTitleField = selectValue(event);
																	viewDraft = {
																		...viewDraft,
																		timelineTitleField,
																		timelineSubtitleField:
																			viewDraft.timelineSubtitleField === timelineTitleField
																				? ''
																				: viewDraft.timelineSubtitleField
																	};
																}}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.recordTitle')}</option>
																{#each fields.filter((field) => field.visibility?.list !== false) as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.timelineSubtitleField')}
															</span>
															<select
																value={viewDraft.timelineSubtitleField}
																disabled={viewDraft.view_type !== 'timeline' || viewDraft.timelineSource === 'events'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		timelineSubtitleField: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each fields.filter((field) => field.visibility?.list !== false && field.key !== viewDraft.timelineTitleField) as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.cardCoverAspect')}
															</span>
															<select
																value={viewDraft.cardCoverAspect}
																disabled={viewDraft.view_type !== 'card'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		cardCoverAspect: viewCardCoverAspect(selectValue(event))
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="auto">{$t('forms.cardCoverAspectAuto')}</option>
																<option value="wide">{$t('forms.cardCoverAspectWide')}</option>
																<option value="square">{$t('forms.cardCoverAspectSquare')}</option>
																<option value="portrait">{$t('forms.cardCoverAspectPortrait')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.cardCoverFit')}
															</span>
															<select
																value={viewDraft.cardCoverFit}
																disabled={viewDraft.view_type !== 'card'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		cardCoverFit: viewCardCoverFit(selectValue(event))
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="cover">{$t('forms.cardCoverFitCover')}</option>
																<option value="contain">{$t('forms.cardCoverFitContain')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.cardCoverPosition')}
															</span>
															<select
																value={viewDraft.cardCoverPosition}
																disabled={viewDraft.view_type !== 'card'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		cardCoverPosition: viewCardCoverPosition(selectValue(event))
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="top">{$t('forms.cardCoverPositionTop')}</option>
																<option value="left">{$t('forms.cardCoverPositionLeft')}</option>
																<option value="hidden">{$t('forms.cardCoverPositionHidden')}</option>
															</select>
														</label>
													</div>
											<label class="block space-y-1">
												<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
													{$t('forms.viewName')}
												</span>
												<input
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												value={viewDraft.name}
													oninput={(event) => (viewDraft = { ...viewDraft, name: inputValue(event) })}
													/>
												</label>
												<label class="block space-y-1">
													<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
														{$t('forms.viewVisibility')}
													</span>
													<select
														value={viewDraft.visibility}
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																visibility: selectValue(event) as ViewVisibility
															})}
														class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
													>
														<option value="shared">{$t('forms.viewVisibilityShared')}</option>
														<option value="private">{$t('forms.viewVisibilityPrivate')}</option>
													</select>
												</label>
												<label
													class="flex items-start gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 dark:border-slate-700 dark:bg-slate-950"
												>
												<input
													type="checkbox"
													class="mt-0.5 h-4 w-4 rounded border-slate-300 text-blue-600"
													checked={viewDraft.isDefault}
													onchange={(event) =>
														(viewDraft = {
															...viewDraft,
															isDefault:
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
														})}
												/>
												<span class="min-w-0">
													<span class="block text-sm font-medium text-slate-800 dark:text-slate-200">
														{$t('forms.defaultView')}
													</span>
													<span class="mt-0.5 block text-xs text-slate-500 dark:text-slate-400">
														{$t('forms.defaultViewDescription')}
													</span>
												</span>
											</label>
											<div
												class="space-y-3 rounded-md border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-950"
												data-view-config-panel
											>
												<div>
													<h4 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
														{$t('forms.viewRules')}
													</h4>
													<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
														{$t('forms.viewRulesDescription')}
													</p>
												</div>
													<div class="space-y-2">
														<div class="flex items-center justify-between gap-3">
															<h5 class="text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.filterConditions')}
															</h5>
															<div class="flex flex-wrap gap-2">
																<Button
																	size="sm"
																	variant="secondary"
																	onclick={() => addViewDraftFilter(0)}
																	disabled={hasReachedFilterLimit()}
																>
																	<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
																	{$t('forms.addFilter')}
																</Button>
																<Button
																	size="sm"
																	variant="secondary"
																	onclick={addViewDraftFilterGroup}
																	disabled={viewDraft.filterGroups.length >= 5}
																>
																	<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
																	{$t('forms.addFilterGroup')}
																</Button>
															</div>
														</div>
														{#if visibleViewDraftFilterGroups().every((group) => group.filters.length === 0)}
															<p class="rounded-md border border-dashed border-slate-300 bg-white px-3 py-2 text-xs text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400">
																{$t('forms.noFilterConditions')}
															</p>
														{/if}
														{#if visibleViewDraftFilterGroups().some((group) => group.filters.length > 0)}
															{@const expressionMatchingRecords = viewDraftFilterExpressionRecords()}
															{@const expressionMatchCount = expressionMatchingRecords?.length ?? null}
															{@const expressionRecordIdsText =
																viewDraftFilterExpressionRecordIdsText(expressionMatchingRecords)}
															{@const expressionJsonText = viewDraftFilterExpressionJsonText()}
															{@const expressionSummaryText = viewDraftFilterExpressionSummaryText()}
															<div class="flex flex-wrap items-end gap-2">
																<label class="block max-w-xs flex-1 space-y-1">
																	<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																		{$t('forms.filterGroupLogic')}
																	</span>
																	<select
																		value={viewDraft.filterRootLogic}
																		onchange={(event) =>
																			updateViewDraftFilterRootLogic(selectValue(event) as ViewFilterLogic)}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																	>
																		<option value="all">{$t('forms.filterGroupsMatchAll')}</option>
																		<option value="any">{$t('forms.filterGroupsMatchAny')}</option>
																	</select>
																</label>
																<span
																	class="rounded-md border border-blue-200 bg-blue-50 px-2.5 py-2 text-xs font-semibold text-blue-700 dark:border-blue-800 dark:bg-blue-950 dark:text-blue-200"
																	data-filter-expression-match-count
																	data-record-count={expressionMatchCount ?? ''}
																>
																	{expressionMatchCount === null
																		? $t('forms.filterMatchCountPending')
																		: $t('forms.filterExpressionMatchCount', {
																				values: { count: expressionMatchCount }
																			})}
																</span>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => copyViewDraftFilterExpressionRecordIds(expressionMatchingRecords)}
																	disabled={!expressionRecordIdsText}
																	aria-label={$t('forms.copyFilterExpressionRecordIds')}
																	data-filter-expression-copy-ids
																	data-filter-expression-copied={copiedFilterExpressionRecordIds ===
																	expressionRecordIdsText
																		? 'true'
																		: undefined}
																>
																	<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.copyFilterExpressionRecordIds')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => copyViewDraftFilterExpressionJson(expressionJsonText)}
																	disabled={!expressionJsonText}
																	aria-label={$t('forms.copyFilterExpressionJson')}
																	data-filter-expression-copy-json
																	data-filter-expression-json-copied={copiedFilterExpressionJson ===
																	expressionJsonText
																		? 'true'
																		: undefined}
																>
																	<FileText class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.copyFilterExpressionJson')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => exportViewDraftFilterExpressionConfigJson(expressionJsonText)}
																	disabled={!expressionJsonText}
																	aria-label={$t('forms.exportFilterExpressionJson')}
																	data-filter-expression-export-config-json
																	data-filter-expression-config-json-exported={exportedFilterExpressionJson ===
																	expressionJsonText
																		? 'true'
																		: undefined}
																>
																	<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.exportFilterExpressionJson')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => copyViewDraftFilterExpressionSummary(expressionSummaryText)}
																	disabled={!expressionSummaryText}
																	aria-label={$t('forms.copyFilterExpressionSummary')}
																	data-filter-expression-copy-summary
																	data-filter-expression-summary-copied={copiedFilterExpressionSummary ===
																	expressionSummaryText
																		? 'true'
																		: undefined}
																>
																	<FileText class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.copyFilterExpressionSummary')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => exportViewDraftFilterExpressionSummary(expressionSummaryText)}
																	disabled={!expressionSummaryText}
																	aria-label={$t('forms.exportFilterExpressionSummary')}
																	data-filter-expression-export-summary
																	data-filter-expression-summary-exported={exportedFilterExpressionSummary ===
																	expressionSummaryText
																		? 'true'
																		: undefined}
																>
																	<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.exportFilterExpressionSummary')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => focusViewDraftFilterExpressionRecords(expressionMatchingRecords)}
																	disabled={!expressionRecordIdsText}
																	data-filter-expression-focus-records
																	data-record-count={expressionMatchCount ?? ''}
																>
																	<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.focusFilterExpressionRecords')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => exportViewDraftFilterExpressionCsv(expressionMatchingRecords)}
																	disabled={!expressionRecordIdsText}
																	data-filter-expression-export-csv
																	data-filter-expression-exported={exportedFilterExpressionRecordIds ===
																	expressionRecordIdsText
																		? 'true'
																		: undefined}
																	data-record-count={expressionMatchCount ?? ''}
																>
																	<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.exportFilterExpressionRecords')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => copyViewDraftFilterExpressionCsvRecords(expressionMatchingRecords)}
																	disabled={!expressionRecordIdsText}
																	data-filter-expression-copy-matches-csv
																	data-filter-expression-matches-csv-copied={copiedFilterExpressionCsvRecordIds ===
																	expressionRecordIdsText
																		? 'true'
																		: undefined}
																	data-record-count={expressionMatchCount ?? ''}
																>
																	<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.copyFilterExpressionRecordsCsv')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => copyViewDraftFilterExpressionJsonRecords(expressionMatchingRecords)}
																	disabled={!expressionRecordIdsText}
																	data-filter-expression-copy-matches-json
																	data-filter-expression-matches-json-copied={copiedFilterExpressionJsonRecordIds ===
																	expressionRecordIdsText
																		? 'true'
																		: undefined}
																	data-record-count={expressionMatchCount ?? ''}
																>
																	<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.copyFilterExpressionRecordsJson')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-2 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={() => exportViewDraftFilterExpressionJson(expressionMatchingRecords)}
																	disabled={!expressionRecordIdsText}
																	data-filter-expression-export-json
																	data-filter-expression-json-exported={exportedFilterExpressionJsonRecordIds ===
																	expressionRecordIdsText
																		? 'true'
																		: undefined}
																	data-record-count={expressionMatchCount ?? ''}
																>
																	<FileText class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.exportFilterExpressionRecordsJson')}
																</button>
															</div>
														{/if}
														{#each visibleViewDraftFilterGroups() as group, groupIndex}
															{@const groupMatchCount = viewDraftFilterGroupMatchCount(group)}
															<div
																class={`space-y-2 rounded-md border border-slate-200 bg-white p-2 dark:border-slate-700 dark:bg-slate-900 ${group.disabled ? 'opacity-70 ring-1 ring-amber-300 dark:ring-amber-700' : ''}`}
																data-filter-group-disabled-state={group.disabled ? 'true' : 'false'}
															>
																<div class="flex flex-wrap items-end gap-2">
																	<label class="min-w-36 flex-1 space-y-1">
																		<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																			{$t('forms.filterGroup')} {groupIndex + 1}
																		</span>
																		<select
																			value={group.logic}
																			onchange={(event) =>
																				updateViewDraftFilterGroupLogic(
																					groupIndex,
																					selectValue(event) as ViewFilterLogic
																				)}
																			class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																		>
																			<option value="all">{$t('forms.filterMatchAll')}</option>
																			<option value="any">{$t('forms.filterMatchAny')}</option>
																		</select>
																	</label>
																	<Button
																		size="sm"
																		variant="secondary"
																		onclick={() => addViewDraftFilter(groupIndex)}
																		disabled={hasReachedFilterLimit()}
																	>
																		<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
																		{$t('forms.addFilter')}
																	</Button>
																	<span
																		class="self-end rounded-md border border-slate-200 bg-slate-50 px-2.5 py-2 text-xs font-medium text-slate-600 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
																		data-filter-group-match-count={groupIndex + 1}
																		data-record-count={groupMatchCount ?? ''}
																	>
																		{groupMatchCount === null
																			? $t('forms.filterMatchCountPending')
																			: $t('forms.filterMatchCount', {
																					values: { count: groupMatchCount }
																				})}
																	</span>
																	<div class="flex items-center rounded-md border border-slate-200 bg-slate-50 p-0.5 dark:border-slate-700 dark:bg-slate-950">
																		<button
																			type="button"
																			class="rounded p-1.5 text-slate-500 hover:bg-white hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => toggleViewDraftFilterGroupDisabled(groupIndex)}
																			aria-label={`${group.disabled ? $t('forms.enableFilterGroup') : $t('forms.disableFilterGroup')} ${groupIndex + 1}`}
																			title={group.disabled ? $t('forms.enableFilterGroup') : $t('forms.disableFilterGroup')}
																			data-filter-group-toggle-disabled={groupIndex + 1}
																			data-filter-group-disabled={group.disabled ? 'true' : 'false'}
																		>
																			{#if group.disabled}
																				<Eye class="h-4 w-4" aria-hidden="true" />
																			{:else}
																				<EyeOff class="h-4 w-4" aria-hidden="true" />
																			{/if}
																		</button>
																		<button
																			type="button"
																			class="rounded p-1.5 text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => mergeViewDraftFilterGroupWithPrevious(groupIndex)}
																			disabled={!canMergeViewDraftFilterGroup(groupIndex)}
																			aria-label={`${$t('forms.mergeFilterGroup')} ${groupIndex + 1}`}
																			data-filter-group-merge-previous={groupIndex + 1}
																		>
																			<Link class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="rounded p-1.5 text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => duplicateViewDraftFilterGroup(groupIndex)}
																			disabled={!canDuplicateViewDraftFilterGroup(groupIndex)}
																			aria-label={`${$t('forms.duplicateFilterGroup')} ${groupIndex + 1}`}
																			data-filter-group-duplicate={groupIndex + 1}
																		>
																			<Copy class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="rounded p-1.5 text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => moveViewDraftFilterGroup(groupIndex, -1)}
																			disabled={groupIndex === 0}
																			aria-label={`${$t('forms.moveFilterGroupUp')} ${groupIndex + 1}`}
																			data-filter-group-move-up={groupIndex + 1}
																		>
																			<ArrowUp class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="rounded p-1.5 text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => moveViewDraftFilterGroup(groupIndex, 1)}
																			disabled={groupIndex >= visibleViewDraftFilterGroups().length - 1}
																			aria-label={`${$t('forms.moveFilterGroupDown')} ${groupIndex + 1}`}
																			data-filter-group-move-down={groupIndex + 1}
																		>
																			<ArrowDown class="h-4 w-4" aria-hidden="true" />
																		</button>
																	</div>
																	{#if viewDraft.filterGroups.length > 1}
																		<button
																			type="button"
																			class="rounded-md p-2 text-slate-500 hover:bg-slate-100 hover:text-red-600 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-red-300"
																			onclick={() => removeViewDraftFilterGroup(groupIndex)}
																			aria-label={$t('forms.removeFilterGroup')}
																			data-filter-group-remove={groupIndex + 1}
																		>
																			<X class="h-4 w-4" aria-hidden="true" />
																		</button>
																	{/if}
																</div>
																{#each group.filters as filter, filterIndex}
																	{@const filterMatchCount = viewDraftFilterMatchCount(filter)}
																	<div
																		class={`grid gap-2 rounded-md border border-slate-200 bg-slate-50 p-2 dark:border-slate-700 dark:bg-slate-950 sm:grid-cols-[minmax(0,1fr)_150px_minmax(0,1fr)_auto_auto_auto_auto_auto_auto] ${filter.disabled ? 'opacity-70 ring-1 ring-amber-300 dark:ring-amber-700' : ''}`}
																		data-filter-disabled-state={filter.disabled ? 'true' : 'false'}
																	>
																		<label class="space-y-1">
																			<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																				{$t('forms.filterField')} {filterLabelSuffix(groupIndex, filterIndex)}
																			</span>
																			<select
																				value={filter.field}
																				onchange={(event) =>
																					updateViewDraftFilter(filterIndex, { field: selectValue(event) }, groupIndex)}
																				class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																			>
																				<option value="">{$t('forms.viewNoRule')}</option>
																				{#each fields.filter((field) => field.visibility?.list !== false) as field}
																					<option value={field.key}>{fieldLabel(field)}</option>
																				{/each}
																			</select>
																		</label>
																		<label class="space-y-1">
																			<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																				{$t('forms.filterOperator')} {filterLabelSuffix(groupIndex, filterIndex)}
																			</span>
																			<select
																				value={filter.operator}
																				disabled={!filter.field}
																				onchange={(event) =>
																					updateViewDraftFilter(
																						filterIndex,
																						{ operator: selectValue(event) as ViewFilterOperator },
																						groupIndex
																					)}
																				class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-950"
																			>
																				<option value="contains">{$t('forms.filterContains')}</option>
																				<option value="equals">{$t('forms.filterEquals')}</option>
																				<option value="not_equals">{$t('forms.filterNotEquals')}</option>
																				<option value="not_empty">{$t('forms.filterNotEmpty')}</option>
																			</select>
																		</label>
																		<label class="space-y-1">
																			<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																				{$t('forms.filterValue')} {filterLabelSuffix(groupIndex, filterIndex)}
																			</span>
																			<input
																				class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-950"
																				value={filter.value}
																				disabled={!filter.field || filter.operator === 'not_empty'}
																				oninput={(event) =>
																					updateViewDraftFilter(
																						filterIndex,
																						{ value: inputValue(event) },
																						groupIndex
																					)}
																			/>
																		</label>
																		<span
																			class="self-end rounded-md border border-slate-200 bg-white px-2.5 py-2 text-xs font-medium text-slate-600 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300"
																			data-filter-match-count={`${groupIndex + 1}.${filterIndex + 1}`}
																			data-record-count={filterMatchCount ?? ''}
																		>
																			{filterMatchCount === null
																				? $t('forms.filterMatchCountPending')
																				: $t('forms.filterMatchCount', {
																						values: { count: filterMatchCount }
																					})}
																		</span>
																		<button
																			type="button"
																			class="self-end rounded-md p-2 text-slate-500 hover:bg-slate-100 hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => toggleViewDraftFilterDisabled(groupIndex, filterIndex)}
																			aria-label={`${filter.disabled ? $t('forms.enableFilter') : $t('forms.disableFilter')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																			title={filter.disabled ? $t('forms.enableFilter') : $t('forms.disableFilter')}
																			data-filter-toggle-disabled={`${groupIndex + 1}.${filterIndex + 1}`}
																			data-filter-disabled={filter.disabled ? 'true' : 'false'}
																		>
																			{#if filter.disabled}
																				<Eye class="h-4 w-4" aria-hidden="true" />
																			{:else}
																				<EyeOff class="h-4 w-4" aria-hidden="true" />
																			{/if}
																		</button>
																		<button
																			type="button"
																			class="self-end rounded-md p-2 text-slate-500 hover:bg-slate-100 hover:text-red-600 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-red-300"
																			onclick={() => removeViewDraftFilter(filterIndex, groupIndex)}
																			aria-label={$t('forms.removeFilter')}
																			data-filter-remove={`${groupIndex + 1}.${filterIndex + 1}`}
																		>
																			<X class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="self-end rounded-md p-2 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => duplicateViewDraftFilter(groupIndex, filterIndex)}
																			disabled={hasReachedFilterLimit()}
																			aria-label={`${$t('forms.duplicateFilter')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																			data-filter-duplicate={`${groupIndex + 1}.${filterIndex + 1}`}
																		>
																			<Copy class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="self-end rounded-md p-2 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																			onclick={() => splitViewDraftFilterToGroup(groupIndex, filterIndex)}
																			disabled={!canSplitViewDraftFilterToGroup(groupIndex, filterIndex)}
																			aria-label={`${$t('forms.splitFilterToGroup')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																			data-filter-split-group={`${groupIndex + 1}.${filterIndex + 1}`}
																		>
																			<ArrowLeftRight class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<div class="self-end flex items-center rounded-md border border-slate-200 bg-white p-0.5 dark:border-slate-700 dark:bg-slate-900">
																			<button
																				type="button"
																				class="rounded p-1.5 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																				onclick={() => moveViewDraftFilter(groupIndex, filterIndex, -1)}
																				disabled={filterIndex === 0}
																				aria-label={`${$t('forms.moveFilterUp')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																				data-filter-move-up={`${groupIndex + 1}.${filterIndex + 1}`}
																			>
																				<ArrowUp class="h-4 w-4" aria-hidden="true" />
																			</button>
																			<button
																				type="button"
																				class="rounded p-1.5 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																				onclick={() => moveViewDraftFilter(groupIndex, filterIndex, 1)}
																				disabled={filterIndex >= group.filters.length - 1}
																				aria-label={`${$t('forms.moveFilterDown')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																				data-filter-move-down={`${groupIndex + 1}.${filterIndex + 1}`}
																			>
																				<ArrowDown class="h-4 w-4" aria-hidden="true" />
																			</button>
																		</div>
																	</div>
																{/each}
															</div>
														{/each}
													</div>
												<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.sortField')}
														</span>
														<select
															value={viewDraft.sortField}
															onchange={(event) => (viewDraft = { ...viewDraft, sortField: selectValue(event) })}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each fields.filter((field) => field.visibility?.list !== false) as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
														</select>
													</label>
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.sortDirection')}
														</span>
														<select
															value={viewDraft.sortDirection}
															disabled={!viewDraft.sortField}
															onchange={(event) =>
																(viewDraft = {
																	...viewDraft,
																	sortDirection: selectValue(event) as ViewSortDirection
																})}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
														>
															<option value="asc">{$t('forms.sortAscending')}</option>
															<option value="desc">{$t('forms.sortDescending')}</option>
														</select>
													</label>
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.groupField')}
														</span>
														<select
															value={viewDraft.groupBy}
															onchange={(event) => {
																const groupBy = selectValue(event);
																viewDraft = {
																	...viewDraft,
																	groupBy,
																	swimlaneBy: viewDraft.swimlaneBy === groupBy ? '' : viewDraft.swimlaneBy,
																	wipLimits: {}
																};
															}}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each fields.filter((field) => field.visibility?.list !== false) as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
														</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.pivotRowField')}
															</span>
															<select
																value={viewDraft.pivotRowField}
																disabled={viewDraft.view_type !== 'pivot'}
																onchange={(event) => {
																	const pivotRowField = selectValue(event);
																	viewDraft = {
																		...viewDraft,
																		pivotRowField,
																		pivotColumnField:
																			viewDraft.pivotColumnField === pivotRowField ? '' : viewDraft.pivotColumnField
																	};
																}}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each viewDraftPivotDimensionFieldCandidates() as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.pivotColumnField')}
															</span>
															<select
																value={viewDraft.pivotColumnField}
																disabled={viewDraft.view_type !== 'pivot'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		pivotColumnField: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each viewDraftPivotDimensionFieldCandidates().filter((field) => field.key !== viewDraft.pivotRowField) as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.pivotValueField')}
															</span>
															<select
																value={viewDraft.pivotValueField}
																disabled={viewDraft.view_type !== 'pivot'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		pivotValueField: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each viewDraftPivotValueFieldCandidates() as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.pivotAggregate')}
															</span>
															<select
																value={viewDraft.pivotAggregate}
																disabled={viewDraft.view_type !== 'pivot'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		pivotAggregate: viewPivotAggregate(selectValue(event))
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="sum">{$t('forms.pivotAggregateSum')}</option>
																<option value="count">{$t('forms.pivotAggregateCount')}</option>
																<option value="avg">{$t('forms.pivotAggregateAvg')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.pivotTotals')}
															</span>
															<select
																value={viewDraft.pivotTotals}
																disabled={viewDraft.view_type !== 'pivot'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		pivotTotals: viewPivotTotalsMode(selectValue(event))
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
														<option value="all">{$t('forms.pivotTotalsAll')}</option>
														<option value="rows">{$t('forms.pivotTotalsRows')}</option>
														<option value="columns">{$t('forms.pivotTotalsColumns')}</option>
														<option value="none">{$t('forms.pivotTotalsNone')}</option>
													</select>
												</label>
												<label class="space-y-1">
													<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
														{$t('forms.pivotValueDisplay')}
													</span>
													<select
														value={viewDraft.pivotValueDisplay}
														disabled={viewDraft.view_type !== 'pivot'}
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																pivotValueDisplay: viewPivotValueDisplay(selectValue(event))
															})}
														class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
													>
														<option value="value">{$t('forms.pivotValueDisplayValue')}</option>
														<option value="percent_row">{$t('forms.pivotValueDisplayPercentRow')}</option>
														<option value="percent_column">{$t('forms.pivotValueDisplayPercentColumn')}</option>
														<option value="percent_total">{$t('forms.pivotValueDisplayPercentTotal')}</option>
													</select>
												</label>
												<label class="space-y-1">
													<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
														{$t('forms.pivotSort')}
													</span>
													<select
														value={viewDraft.pivotSort}
														disabled={viewDraft.view_type !== 'pivot'}
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																pivotSort: viewPivotSortMode(selectValue(event))
															})}
														class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
													>
														<option value="label">{$t('forms.pivotSortLabel')}</option>
														<option value="row_total_desc">{$t('forms.pivotSortRowTotalDesc')}</option>
														<option value="row_total_asc">{$t('forms.pivotSortRowTotalAsc')}</option>
														<option value="column_total_desc">{$t('forms.pivotSortColumnTotalDesc')}</option>
														<option value="column_total_asc">{$t('forms.pivotSortColumnTotalAsc')}</option>
													</select>
												</label>
												<label class="space-y-1">
													<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
														{$t('forms.pivotRowLimit')}
													</span>
													<select
														value={String(viewDraft.pivotRowLimit)}
														disabled={viewDraft.view_type !== 'pivot'}
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																pivotRowLimit: viewPivotRowLimit(selectValue(event))
															})}
														class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
													>
														<option value="0">{$t('forms.pivotRowLimitAll')}</option>
														<option value="2">{$t('forms.pivotRowLimitTwo')}</option>
														<option value="5">{$t('forms.pivotRowLimitFive')}</option>
														<option value="10">{$t('forms.pivotRowLimitTen')}</option>
													</select>
												</label>
												<label class="space-y-1">
													<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
														{$t('forms.pivotColumnLimit')}
													</span>
													<select
														value={String(viewDraft.pivotColumnLimit)}
														disabled={viewDraft.view_type !== 'pivot'}
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																pivotColumnLimit: viewPivotColumnLimit(selectValue(event))
															})}
														class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
													>
														<option value="0">{$t('forms.pivotColumnLimitAll')}</option>
														<option value="2">{$t('forms.pivotColumnLimitTwo')}</option>
														<option value="5">{$t('forms.pivotColumnLimitFive')}</option>
														<option value="10">{$t('forms.pivotColumnLimitTen')}</option>
													</select>
												</label>
												<label
													class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
												>
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-slate-300 text-blue-600"
														checked={viewDraft.pivotEmptyBuckets}
														disabled={viewDraft.view_type !== 'pivot'}
														data-pivot-empty-buckets-control
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																pivotEmptyBuckets:
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
															})}
														/>
														<span class="min-w-0 truncate">{$t('forms.pivotEmptyBuckets')}</span>
													</label>
													<label
														class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
													>
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-slate-300 text-blue-600"
															checked={viewDraft.pivotHideZeroBuckets}
															disabled={viewDraft.view_type !== 'pivot'}
															data-pivot-hide-zero-buckets-control
															onchange={(event) =>
																(viewDraft = {
																	...viewDraft,
																	pivotHideZeroBuckets:
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																})}
														/>
														<span class="min-w-0 truncate">{$t('forms.pivotHideZeroBuckets')}</span>
													</label>
													<label
														class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
													>
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-slate-300 text-blue-600"
														checked={viewDraft.pivotShowCounts}
														disabled={viewDraft.view_type !== 'pivot'}
														data-pivot-show-counts-control
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																pivotShowCounts:
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
															})}
													/>
													<span class="min-w-0 truncate">{$t('forms.pivotShowCounts')}</span>
												</label>
												<label
													class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
												>
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-slate-300 text-blue-600"
														checked={viewDraft.pivotHeatmap}
														disabled={viewDraft.view_type !== 'pivot'}
														data-pivot-heatmap-control
														onchange={(event) =>
															(viewDraft = {
																...viewDraft,
																pivotHeatmap:
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
															})}
													/>
													<span class="min-w-0 truncate">{$t('forms.pivotHeatmap')}</span>
												</label>
												<div class="flex items-end">
													<button
																type="button"
																disabled={
																	viewDraft.view_type !== 'pivot' ||
																	!viewDraft.pivotRowField ||
																	!viewDraft.pivotColumnField
																}
																class="inline-flex h-10 items-center gap-2 rounded-md border border-slate-300 bg-white px-3 text-sm font-medium text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-200 dark:hover:bg-slate-700"
																data-pivot-swap-axes
																onclick={swapViewDraftPivotAxes}
															>
																<ArrowLeftRight class="h-4 w-4" aria-hidden="true" />
																{$t('forms.pivotSwapAxes')}
															</button>
														</div>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.ganttStartField')}
															</span>
															<select
																value={viewDraft.ganttStartField}
																disabled={viewDraft.view_type !== 'gantt'}
																onchange={(event) => {
																	const ganttStartField = selectValue(event);
																	viewDraft = {
																		...viewDraft,
																		ganttStartField,
																		ganttEndField:
																			viewDraft.ganttEndField === ganttStartField ? '' : viewDraft.ganttEndField,
																		ganttGroupBy:
																			viewDraft.ganttGroupBy === ganttStartField ? '' : viewDraft.ganttGroupBy,
																		ganttDependencyField:
																			viewDraft.ganttDependencyField === ganttStartField
																				? ''
																				: viewDraft.ganttDependencyField
																	};
																}}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each viewDraftGanttDateFieldCandidates() as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.ganttEndField')}
															</span>
															<select
																value={viewDraft.ganttEndField}
																disabled={viewDraft.view_type !== 'gantt'}
																onchange={(event) => {
																	const ganttEndField = selectValue(event);
																	viewDraft = {
																		...viewDraft,
																		ganttEndField,
																		ganttGroupBy:
																			viewDraft.ganttGroupBy === ganttEndField ? '' : viewDraft.ganttGroupBy,
																		ganttDependencyField:
																			viewDraft.ganttDependencyField === ganttEndField
																				? ''
																				: viewDraft.ganttDependencyField
																	};
																}}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.ganttNoEndField')}</option>
																{#each viewDraftGanttDateFieldCandidates().filter((field) => field.key !== viewDraft.ganttStartField) as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.ganttGroupField')}
															</span>
															<select
																value={viewDraft.ganttGroupBy}
																disabled={viewDraft.view_type !== 'gantt'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		ganttGroupBy: selectValue(event),
																		ganttDependencyField:
																			viewDraft.ganttDependencyField === selectValue(event)
																				? ''
																				: viewDraft.ganttDependencyField
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each viewDraftGanttGroupFieldCandidates() as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.ganttDependencyField')}
															</span>
															<select
																value={viewDraft.ganttDependencyField}
																disabled={viewDraft.view_type !== 'gantt'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		ganttDependencyField: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each viewDraftGanttDependencyFieldCandidates() as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.ganttScale')}
															</span>
															<select
																value={viewDraft.ganttScale}
																disabled={viewDraft.view_type !== 'gantt'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		ganttScale: selectValue(event) as GanttScale
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="day">{$t('forms.ganttScaleDay')}</option>
																<option value="week">{$t('forms.ganttScaleWeek')}</option>
																<option value="month">{$t('forms.ganttScaleMonth')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.swimlaneField')}
														</span>
														<select
															value={viewDraft.swimlaneBy}
															disabled={viewDraft.view_type !== 'kanban'}
															onchange={(event) =>
																(viewDraft = { ...viewDraft, swimlaneBy: selectValue(event) })}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each viewDraftSwimlaneFieldCandidates() as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
														</select>
													</label>
													<label class="space-y-1 sm:col-span-2 xl:col-span-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.dateField')}
														</span>
														<select
															value={viewDraft.dateField}
															onchange={(event) => {
																const dateField = selectValue(event);
																viewDraft = {
																	...viewDraft,
																	dateField,
																	calendarEndField:
																		viewDraft.calendarEndField === dateField
																			? ''
																			: viewDraft.calendarEndField,
																	calendarResourceBy:
																		viewDraft.calendarResourceBy === dateField
																			? ''
																			: viewDraft.calendarResourceBy
																};
															}}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each fields.filter((field) => ['date', 'datetime'].includes(field.type ?? 'text')) as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
														</select>
													</label>
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.calendarEndField')}
														</span>
														<select
															value={viewDraft.calendarEndField}
															disabled={viewDraft.view_type !== 'calendar'}
															onchange={(event) =>
																(viewDraft = {
																	...viewDraft,
																	calendarEndField: selectValue(event)
																})}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each viewDraftCalendarEndFieldCandidates() as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
														</select>
													</label>
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.calendarResourceField')}
														</span>
														<select
															value={viewDraft.calendarResourceBy}
															disabled={viewDraft.view_type !== 'calendar'}
															onchange={(event) =>
																(viewDraft = {
																	...viewDraft,
																	calendarResourceBy: selectValue(event)
																})}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each viewDraftCalendarResourceFieldCandidates() as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
														</select>
													</label>
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.calendarRange')}
														</span>
														<select
															value={viewDraft.calendarRange}
															disabled={viewDraft.view_type !== 'calendar'}
															onchange={(event) =>
																(viewDraft = {
																	...viewDraft,
																	calendarRange: selectValue(event) as ViewCalendarRange
																})}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
														>
															<option value="week">{$t('forms.calendarRangeWeek')}</option>
															<option value="month">{$t('forms.calendarRangeMonth')}</option>
														</select>
													</label>
													<div class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.calendarFocusDate')}
														</span>
														<div class="grid grid-cols-[auto_minmax(0,1fr)_auto] gap-1">
															<button
																type="button"
																class="inline-flex h-9 w-9 items-center justify-center rounded-md border border-slate-300 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-600 dark:text-slate-300 dark:hover:bg-slate-800"
																disabled={viewDraft.view_type !== 'calendar'}
																onclick={() => shiftViewDraftCalendarFocus(-1)}
																aria-label={$t('forms.calendarPreviousRange')}
																title={$t('forms.calendarPreviousRange')}
																data-calendar-focus-shift="-1"
															>
																<ChevronLeft class="h-4 w-4" aria-hidden="true" />
															</button>
															<input
																type="date"
																value={viewDraft.calendarFocusDate}
																disabled={viewDraft.view_type !== 'calendar'}
																aria-label={$t('forms.calendarFocusDate')}
																data-calendar-focus-date-control
																oninput={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		calendarFocusDate: inputValue(event)
																	})}
																class="min-w-0 rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															/>
															<button
																type="button"
																class="inline-flex h-9 w-9 items-center justify-center rounded-md border border-slate-300 text-slate-600 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-600 dark:text-slate-300 dark:hover:bg-slate-800"
																disabled={viewDraft.view_type !== 'calendar'}
																onclick={() => shiftViewDraftCalendarFocus(1)}
																aria-label={$t('forms.calendarNextRange')}
																title={$t('forms.calendarNextRange')}
																data-calendar-focus-shift="1"
															>
																<ChevronRight class="h-4 w-4" aria-hidden="true" />
															</button>
														</div>
													</div>
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.calendarRecurrence')}
														</span>
														<select
															value={viewDraft.calendarRecurrence}
															disabled={viewDraft.view_type !== 'calendar'}
															onchange={(event) =>
																(viewDraft = {
																	...viewDraft,
																	calendarRecurrence: selectValue(event) as ViewCalendarRecurrence
																})}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="none">{$t('forms.calendarRecurrenceNone')}</option>
																<option value="daily">{$t('forms.calendarRecurrenceDaily')}</option>
																<option value="weekly">{$t('forms.calendarRecurrenceWeekly')}</option>
																<option value="monthly">{$t('forms.calendarRecurrenceMonthly')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.calendarDayScope')}
															</span>
															<select
																value={viewDraft.calendarDayScope}
																disabled={viewDraft.view_type !== 'calendar'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		calendarDayScope: selectValue(event) as ViewCalendarDayScope
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="all">{$t('forms.calendarDayScopeAll')}</option>
																<option value="workweek">{$t('forms.calendarDayScopeWorkweek')}</option>
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.calendarLayout')}
															</span>
															<select
																value={viewDraft.calendarLayout}
																disabled={viewDraft.view_type !== 'calendar'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		calendarLayout: selectValue(event) as ViewCalendarLayout
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
																>
																	<option value="grid">{$t('forms.calendarLayoutGrid')}</option>
																	<option value="agenda">{$t('forms.calendarLayoutAgenda')}</option>
																</select>
															</label>
															<label
																class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
															>
																<input
																	type="checkbox"
																	class="h-4 w-4 rounded border-slate-300 text-blue-600"
																	checked={viewDraft.calendarHideEmptyDays}
																	disabled={viewDraft.view_type !== 'calendar'}
																	data-calendar-hide-empty-days-control
																	onchange={(event) =>
																		(viewDraft = {
																			...viewDraft,
																			calendarHideEmptyDays:
																				event.currentTarget instanceof HTMLInputElement
																					? event.currentTarget.checked
																					: false
																		})}
																/>
																<span class="min-w-0 truncate">{$t('forms.calendarHideEmptyDays')}</span>
															</label>
															<div class="space-y-2 rounded-md border border-slate-200 p-3 dark:border-slate-700 lg:col-span-2">
															<div class="grid gap-2 md:grid-cols-[minmax(0,1fr)_180px_auto]">
																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																		{$t('forms.calendarExceptionRecord')}
																	</span>
																	<select
																		value={viewDraft.calendarExceptionRecordId}
																		disabled={viewDraft.view_type !== 'calendar'}
																		onchange={(event) =>
																			(viewDraft = {
																				...viewDraft,
																				calendarExceptionRecordId: selectValue(event)
																			})}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>
																		{#each viewRecords as record}
																			<option value={record.id}>{record.title}</option>
																		{/each}
																	</select>
																</label>
																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																		{$t('forms.cardFieldLimit')}
																	</span>
																	<select
																		value={String(viewDraft.cardFieldLimit)}
																		disabled={viewDraft.view_type !== 'card'}
																		onchange={(event) =>
																			(viewDraft = {
																				...viewDraft,
																				cardFieldLimit: viewCardFieldLimit(selectValue(event))
																			})}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
																	>
																		<option value="3">{$t('forms.cardFieldLimitThree')}</option>
																		<option value="6">{$t('forms.cardFieldLimitSix')}</option>
																	<option value="9">{$t('forms.cardFieldLimitNine')}</option>
																	<option value="99">{$t('forms.cardFieldLimitAll')}</option>
																</select>
															</label>
															<label
																class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
															>
																<input
																	type="checkbox"
																	class="h-4 w-4 rounded border-slate-300 text-blue-600"
																	checked={viewDraft.cardHideEmptyFields}
																	disabled={viewDraft.view_type !== 'card'}
																	data-card-hide-empty-fields-control
																	onchange={(event) =>
																		(viewDraft = {
																			...viewDraft,
																			cardHideEmptyFields:
																				event.currentTarget instanceof HTMLInputElement
																					? event.currentTarget.checked
																					: false
																		})}
																	/>
																	<span class="min-w-0 truncate">{$t('forms.cardHideEmptyFields')}</span>
																</label>
																<label
																	class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
																>
																	<input
																		type="checkbox"
																		class="h-4 w-4 rounded border-slate-300 text-blue-600"
																		checked={viewDraft.cardShowTimestamps}
																		disabled={viewDraft.view_type !== 'card'}
																		data-card-show-timestamps-control
																		onchange={(event) =>
																			(viewDraft = {
																				...viewDraft,
																				cardShowTimestamps:
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																			})}
																	/>
																<span class="min-w-0 truncate">{$t('forms.cardShowTimestamps')}</span>
															</label>
															<label
																class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
															>
																<input
																	type="checkbox"
																	class="h-4 w-4 rounded border-slate-300 text-blue-600"
																	checked={viewDraft.cardShowRecordId}
																	disabled={viewDraft.view_type !== 'card'}
																	data-card-show-record-id-control
																	onchange={(event) =>
																		(viewDraft = {
																			...viewDraft,
																			cardShowRecordId:
																				event.currentTarget instanceof HTMLInputElement
																					? event.currentTarget.checked
																					: false
																		})}
																/>
																<span class="min-w-0 truncate">{$t('forms.cardShowRecordId')}</span>
															</label>
																	<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																		{$t('forms.calendarExceptionDate')}
																	</span>
																	<input
																		type="date"
																		value={viewDraft.calendarExceptionDate}
																		disabled={viewDraft.view_type !== 'calendar'}
																		oninput={(event) =>
																			(viewDraft = {
																				...viewDraft,
																				calendarExceptionDate: inputValue(event)
																			})}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
																	/>
																</label>
																<div class="flex items-end">
																	<button
																		type="button"
																		disabled={
																			viewDraft.view_type !== 'calendar' ||
																			!viewDraft.calendarExceptionRecordId ||
																			!isIsoDate(viewDraft.calendarExceptionDate)
																		}
																		class="inline-flex h-10 items-center gap-2 rounded-md bg-slate-900 px-3 text-sm font-medium text-white disabled:opacity-50 dark:bg-slate-100 dark:text-slate-900"
																		onclick={addViewDraftCalendarException}
																	>
																		<Plus class="h-4 w-4" aria-hidden="true" />
																		{$t('forms.calendarAddException')}
																	</button>
																</div>
															</div>
															<div class="space-y-1">
																<div class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																	{$t('forms.calendarExceptions')}
																</div>
																{#if viewDraftCalendarExceptionItems().length === 0}
																	<p class="text-xs text-slate-500 dark:text-slate-400">
																		{$t('forms.calendarNoExceptions')}
																	</p>
																{:else}
																	<div class="flex flex-wrap gap-2">
																		{#each viewDraftCalendarExceptionItems() as exception (`${exception.recordId}:${exception.date}`)}
																			<span class="inline-flex max-w-full items-center gap-2 rounded-md bg-slate-100 px-2 py-1 text-xs text-slate-700 dark:bg-slate-800 dark:text-slate-200">
																				<span class="truncate">{exception.label} · {exception.date}</span>
																				<button
																					type="button"
																					class="rounded p-0.5 text-slate-500 hover:bg-slate-200 hover:text-slate-900 dark:hover:bg-slate-700 dark:hover:text-slate-100"
																					onclick={() =>
																						removeViewDraftCalendarException(exception.recordId, exception.date)}
																					aria-label={$t('forms.calendarRemoveException')}
																				>
																					<X class="h-3 w-3" aria-hidden="true" />
																				</button>
																			</span>
																		{/each}
																	</div>
																{/if}
															</div>
															</div>
															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																	{$t('forms.cardLayout')}
																</span>
																<select
																	value={viewDraft.cardLayout}
																	disabled={viewDraft.view_type !== 'card'}
																	onchange={(event) =>
																		(viewDraft = {
																			...viewDraft,
																			cardLayout: selectValue(event) as CardLayoutMode
																		})}
																	class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
																>
																	<option value="standard">{$t('forms.cardLayoutStandard')}</option>
																	<option value="gallery">{$t('forms.cardLayoutGallery')}</option>
																	<option value="compact">{$t('forms.cardLayoutCompact')}</option>
																	<option value="list">{$t('forms.cardLayoutList')}</option>
																</select>
															</label>
															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																	{$t('forms.cardTitleField')}
														</span>
														<select
															value={viewDraft.cardTitleField}
															disabled={viewDraft.view_type !== 'card'}
															onchange={(event) => {
																const cardTitleField = selectValue(event);
																viewDraft = {
																	...viewDraft,
																		cardTitleField,
																		cardSubtitleField:
																			viewDraft.cardSubtitleField === cardTitleField
																				? ''
																				: viewDraft.cardSubtitleField,
																		cardBadgeField:
																			viewDraft.cardBadgeField === cardTitleField ? '' : viewDraft.cardBadgeField,
																		cardFieldKeys:
																			cardTitleField && viewDraft.cardFieldKeys[0] !== cardTitleField
																				? [
																					cardTitleField,
																					...viewDraft.cardFieldKeys.filter((key) => key !== cardTitleField)
																				]
																				: viewDraft.cardFieldKeys
																	};
																}}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
														>
															<option value="">{$t('forms.recordTitle')}</option>
															{#each fields.filter((field) => field.visibility?.list !== false) as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
														</select>
													</label>
													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.cardSubtitleField')}
														</span>
														<select
															value={viewDraft.cardSubtitleField}
															disabled={viewDraft.view_type !== 'card'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		cardSubtitleField: selectValue(event),
																		cardBadgeField:
																			viewDraft.cardBadgeField === selectValue(event) ? '' : viewDraft.cardBadgeField
																	})}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each fields.filter((field) => field.visibility?.list !== false && field.key !== viewDraft.cardTitleField) as field}
																<option value={field.key}>{fieldLabel(field)}</option>
															{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.cardBadgeField')}
															</span>
															<select
																value={viewDraft.cardBadgeField}
																disabled={viewDraft.view_type !== 'card'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		cardBadgeField: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each fields.filter((field) => field.visibility?.list !== false && field.key !== viewDraft.cardTitleField && field.key !== viewDraft.cardSubtitleField) as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.cardCoverField')}
															</span>
															<select
																value={viewDraft.cardCoverField}
																disabled={viewDraft.view_type !== 'card'}
																onchange={(event) =>
																	(viewDraft = {
																		...viewDraft,
																		cardCoverField: selectValue(event)
																	})}
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 disabled:bg-slate-100 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100 dark:disabled:bg-slate-900"
															>
																<option value="">{$t('forms.viewNoRule')}</option>
																{#each fields.filter((field) => field.visibility?.list !== false && ['image', 'attachment'].includes(field.type ?? 'text')) as field}
																	<option value={field.key}>{fieldLabel(field)}</option>
																{/each}
															</select>
														</label>
													</div>
													{#if viewDraft.view_type === 'card' && viewDraftCardFields().length > 0}
														<div class="space-y-2 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900">
															<div>
																<h5 class="text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
																	{$t('forms.cardFieldOrder')}
																</h5>
															</div>
															<div class="grid gap-2 md:grid-cols-2 xl:grid-cols-3">
																{#each viewDraftCardFields() as field, fieldIndex}
																	<div
																		class="flex min-w-0 items-center justify-between gap-2 rounded-md border border-slate-200 bg-slate-50 px-2 py-1.5 text-sm dark:border-slate-700 dark:bg-slate-950"
																		data-card-field-order={`${fieldIndex + 1}:${field.key}`}
																	>
																		<span class="min-w-0 truncate text-slate-700 dark:text-slate-200">
																			{fieldIndex + 1}. {fieldLabel(field)}
																		</span>
																		<div class="flex shrink-0 items-center rounded-md border border-slate-200 bg-white p-0.5 dark:border-slate-700 dark:bg-slate-900">
																			<button
																				type="button"
																				class="rounded p-1 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																				disabled={fieldIndex === 0}
																				onclick={() => moveViewDraftCardField(fieldIndex, -1)}
																				aria-label={`${$t('forms.moveCardFieldUp')} ${fieldLabel(field)}`}
																				data-card-field-move-up={field.key}
																			>
																				<ArrowUp class="h-3.5 w-3.5" aria-hidden="true" />
																			</button>
																			<button
																				type="button"
																				class="rounded p-1 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																				disabled={fieldIndex >= viewDraftCardFields().length - 1}
																				onclick={() => moveViewDraftCardField(fieldIndex, 1)}
																				aria-label={`${$t('forms.moveCardFieldDown')} ${fieldLabel(field)}`}
																				data-card-field-move-down={field.key}
																			>
																				<ArrowDown class="h-3.5 w-3.5" aria-hidden="true" />
																			</button>
																		</div>
																	</div>
																{/each}
															</div>
														</div>
													{/if}
												{#if viewDraftKanbanWipOptions().length > 0}
													<div class="space-y-3 rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900">
														<div>
															<h5 class="text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.kanbanWipPolicy')}
															</h5>
															<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
																{$t('forms.kanbanWipPolicyDescription')}
															</p>
														</div>
														<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
															{#each viewDraftKanbanWipOptions() as option}
																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																		{$t('forms.kanbanWipLimitFor', { values: { option } })}
																	</span>
																	<input
																		type="number"
																		min="0"
																		step="1"
																		inputmode="numeric"
																		placeholder={$t('forms.kanbanNoWipLimit')}
																		value={viewDraft.wipLimits[option] ?? ''}
																		oninput={(event) => setViewDraftWipLimit(option, inputValue(event))}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																	/>
																</label>
															{/each}
														</div>
													</div>
												{/if}
											</div>
											{#if selectedView}
												<Button
												size="sm"
												variant="danger"
												loading={deletingViewId === selectedView.id}
												onclick={deleteSelectedView}
											>
												<Trash2 class="mr-2 h-4 w-4" aria-hidden="true" />
												{$t('forms.deleteView')}
											</Button>
										{/if}
											</div>
										</details>
										<details
											class="rounded-md border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900"
											open
											data-view-columns-disclosure
										>
											<summary
												class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 marker:hidden"
											>
												<span class="text-sm font-semibold text-slate-900 dark:text-slate-100">
													{$t('forms.visibleColumns')}
												</span>
												<span class="text-xs text-slate-500 dark:text-slate-400">
													{$t('forms.selectedColumns', { values: { count: viewDraft.columns.length } })}
												</span>
											</summary>
											<div class="border-t border-slate-100 p-3 dark:border-slate-800">
											<div class="mb-2 flex items-center justify-between gap-3">
												<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
													{$t('forms.visibleColumns')}
											</span>
											<span class="text-xs text-slate-500 dark:text-slate-400">
												{$t('forms.selectedColumns', { values: { count: viewDraft.columns.length } })}
											</span>
										</div>
											<div class="grid gap-2 sm:grid-cols-2">
											{#each fields.filter((field) => field.visibility?.list !== false) as field}
												<div
													class="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
												>
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-slate-300 text-blue-600"
														checked={viewDraftHasColumn(field.key)}
														aria-label={`${$t('forms.visibleColumns')}: ${fieldLabel(field)}`}
														onchange={(event) =>
															setViewDraftColumn(
																field.key,
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
															)}
													/>
													<span class="min-w-0 flex-1 truncate">{fieldLabel(field)}</span>
													{#if memberFieldHidden(field)}
														<span
															class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
															data-view-column-permission-hidden={field.key}
														>
															{$t('forms.fieldHiddenBadge')}
														</span>
													{/if}
													{#if memberFieldLocked(field)}
														<span
															class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
															data-view-column-permission-locked={field.key}
														>
															{$t('forms.fieldLockedBadge')}
														</span>
													{/if}
													{#if fieldHasConditionalHidden(field)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-view-column-conditional-hidden={field.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if fieldHasConditionalReadOnly(field)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-view-column-conditional-readonly={field.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
													<span class="ml-auto hidden font-mono text-[11px] text-slate-400 2xl:inline">{field.key}</span>
												</div>
												{/each}
											</div>
											</div>
											</details>
											</div>
								</section>
									{/if}
								{#if recordEditorOpen}
									<div
										class="fixed inset-0 z-40 flex justify-end bg-slate-950/30 backdrop-blur-[1px]"
										data-record-editor-drawer
										role="dialog"
										aria-modal="true"
									>
										<button
											type="button"
											class="absolute inset-0 cursor-default"
											aria-hidden="true"
											tabindex="-1"
											onclick={closeRecordEditorDrawer}
										></button>
										<section
											class="relative z-10 flex h-full w-full max-w-3xl flex-col overflow-y-auto bg-white p-4 shadow-xl dark:bg-slate-900"
										>
									<div class="mb-3 flex items-center justify-between gap-3">
										<div>
											<h3 class="text-base font-semibold text-slate-950 dark:text-slate-100">
											{editingRecordId ? $t('forms.editRecord') : $t('forms.newRecord')}
										</h3>
										<span class="text-xs text-slate-500 dark:text-slate-400"
												>{$t('forms.decimalHint')}</span
											>
										</div>
										<Button size="sm" variant="secondary" onclick={closeRecordEditorDrawer}>
											<X class="mr-2 h-4 w-4" aria-hidden="true" />
											{$t('common.cancel')}
										</Button>
								</div>
								<div class="grid gap-3 md:grid-cols-2">
									<label class="space-y-1 md:col-span-2">
										<span class="text-sm font-medium text-slate-700 dark:text-slate-300"
											>{$t('forms.titleOverride')}</span
										>
										<input
											class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											bind:value={recordTitle}
											placeholder={selectedForm.title_template}
										/>
									</label>
									{#each fields as field}
										<label
											class="space-y-1 {fieldType(field) === 'textarea' ||
											fieldType(field) === 'rich_text' ||
											fieldType(field) === 'signature' ||
											fieldType(field) === 'child_table'
												? 'md:col-span-2'
												: ''} {fieldHiddenByCondition(field, recordDraft) ? 'hidden' : ''}"
											data-record-edit-field={field.key}
											data-conditional-hidden={fieldHiddenByCondition(field, recordDraft) ? field.key : undefined}
											data-conditional-readonly={fieldReadOnlyByCondition(field, recordDraft) ? field.key : undefined}
										>
											<span class="flex flex-wrap items-center gap-2 text-sm font-medium text-slate-700 dark:text-slate-300">
												<span>{fieldLabel(field)}</span>
												{#if field.required}<span class="text-red-500">*</span>{/if}
												{#if memberFieldHidden(field)}
													<span
														class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[11px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
														data-record-edit-permission-hidden={field.key}
													>
														{$t('forms.fieldHiddenBadge')}
													</span>
												{/if}
												{#if memberFieldLocked(field)}
													<span
														class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[11px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
														data-record-edit-permission-locked={field.key}
													>
														{$t('forms.fieldLockedBadge')}
													</span>
												{/if}
											</span>
											{#if fieldType(field) === 'boolean'}
												<input
													type="checkbox"
													checked={Boolean(recordDraft[field.key])}
													disabled={fieldReadOnlyByCondition(field, recordDraft)}
													onchange={(event) =>
														setDraftValue(
															field.key,
															event.currentTarget instanceof HTMLInputElement
																? event.currentTarget.checked
																: false
														)}
													class="h-5 w-5 rounded border-slate-300 text-blue-600"
												/>
											{:else if fieldType(field) === 'single_select'}
												<select
													value={draftString(field.key)}
													disabled={fieldReadOnlyByCondition(field, recordDraft)}
													onchange={(event) => setDraftValue(field.key, selectValue(event))}
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												>
													<option value="">{$t('forms.selectPlaceholder')}</option>
													{#each recordSelectOptions(field, recordDraft) as option}
														<option
															value={option}
															disabled={optionDisabledForSingleDraft(field, option)}
															data-option-disabled={optionDisabled(field, option) ? option : undefined}
															data-option-archived={optionArchived(field, option) ? option : undefined}
															data-option-description={optionDescription(field, option) || undefined}
															data-option-group={optionGroup(field, option) || undefined}
														>
															{optionDisplayLabel(field, option)}
														</option>
													{/each}
												</select>
											{:else if fieldType(field) === 'multi_select' && fieldOptions(field).length > 0}
												<div
													class="grid gap-2 rounded-md border border-slate-300 bg-white p-2 text-sm dark:border-slate-600 dark:bg-slate-800"
												>
													{#each recordSelectOptions(field, recordDraft) as option}
														<label
															class="flex items-start gap-2 text-slate-700 dark:text-slate-300"
															data-option-description={optionDescription(field, option) || undefined}
															data-option-group={optionGroup(field, option) || undefined}
														>
															<input
																type="checkbox"
																class="h-4 w-4 rounded border-slate-300 text-blue-600"
																checked={isDraftOptionSelected(field.key, option)}
																disabled={fieldReadOnlyByCondition(field, recordDraft) || optionDisabledForMultiDraft(field, option)}
																data-option-disabled={optionDisabled(field, option) ? option : undefined}
																data-option-archived={optionArchived(field, option) ? option : undefined}
																onchange={(event) =>
																	toggleDraftOption(
																		field.key,
																		option,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																	)}
															/>
															<span class="min-w-0">
																<span class="block">{optionDisplayLabel(field, option)}</span>
																{#if optionDescription(field, option)}
																	<span class="block text-xs text-slate-500 dark:text-slate-400">
																		{optionDescription(field, option)}
																	</span>
																{/if}
															</span>
														</label>
													{/each}
												</div>
											{:else if fieldType(field) === 'relation' && field.relation?.form_key}
												<select
													value={relationDraftValue(field)}
													disabled={fieldReadOnlyByCondition(field, recordDraft) || relationTargetsLoading}
													onchange={(event) => setDraftValue(field.key, selectValue(event))}
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												>
													<option value="">
														{relationTargetsLoading
															? $t('forms.loadingRelationTargets')
															: $t('forms.selectRelationTarget')}
													</option>
													{#each relationTargetOptions(field) as target}
														<option value={target.record_id}>
															{target.display || target.title}
														</option>
													{/each}
												</select>
											{:else if fieldType(field) === 'child_table'}
												<div
													class="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-600 dark:border-slate-700 dark:bg-slate-800/60 dark:text-slate-300"
													data-child-table-field={field.key}
												>
													{$t('forms.childRecords')}
												</div>
											{:else if fieldType(field) === 'member'}
												<select
													value={draftString(field.key)}
													disabled={fieldReadOnlyByCondition(field, recordDraft) || workspaceMembersLoading}
													onchange={(event) => setDraftValue(field.key, selectValue(event))}
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												>
													<option value="">
														{workspaceMembersLoading
															? $t('forms.loadingMembers')
															: $t('forms.selectMemberPlaceholder')}
													</option>
													{#each workspaceMembers as member (member.user_id)}
														<option value={member.user_id}>{member.name || member.email || member.user_id}</option>
													{/each}
												</select>
											{:else if ['attachment', 'image'].includes(fieldType(field))}
												<div class="space-y-2" data-attachment-field={field.key}>
													<div class="flex gap-2">
														<input
															type="url"
															class="min-w-0 flex-1 rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
															value={attachmentUrl(field)}
															placeholder={fieldPlaceholder(field) || $t('forms.attachmentUrlPlaceholder')}
															readonly={fieldReadOnlyByCondition(field, recordDraft)}
															oninput={(event) => setDraftValue(field.key, inputValue(event))}
															data-attachment-url-input={field.key}
														/>
														<Button
															size="sm"
															variant="secondary"
															disabled={fieldReadOnlyByCondition(field, recordDraft) || !editingRecordId || !attachmentUrl(field)}
															loading={attachmentSubmittingField === field.key}
															onclick={() => createAttachmentForField(field)}
														>
															<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('forms.attachmentLink')}
														</Button>
													</div>
													<div class="flex flex-wrap items-center gap-2">
														<label class="inline-flex cursor-pointer items-center rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800">
															<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('forms.attachmentChooseFile')}
															<input
																type="file"
																class="sr-only"
																accept={attachmentAcceptAttr(field)}
																disabled={fieldReadOnlyByCondition(field, recordDraft) || !editingRecordId}
																onchange={(event) => createAttachmentFromLocalFile(field, event)}
																data-attachment-file-input={field.key}
															/>
														</label>
														{#if field.attachment?.max_size_mb}
															<span class="text-xs text-slate-500 dark:text-slate-400">
																{$t('forms.attachmentMaxSizeLabel', { values: { size: field.attachment.max_size_mb } })}
															</span>
														{/if}
													</div>
													{#if !editingRecordId}
														<p class="text-xs text-slate-500 dark:text-slate-400">
															{$t('forms.attachmentSaveRecordFirst')}
														</p>
													{/if}
													{#if attachmentUrl(field) && fieldType(field) === 'image'}
														<img
															src={attachmentUrl(field)}
															alt={fieldLabel(field)}
															class="h-24 w-full rounded-md border border-slate-200 object-cover dark:border-slate-700"
															data-attachment-preview={field.key}
														/>
													{/if}
													{#if fieldAttachments(field.key).length > 0}
														<div class="space-y-2">
															{#each fieldAttachments(field.key) as attachment}
																<div class="flex flex-wrap items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs dark:border-slate-700 dark:bg-slate-950" data-attachment-item={attachment.id}>
																	{#if attachment.url && attachmentIsPreviewableImage(attachment)}
																		<img
																			src={attachmentThumbnailUrl(attachment)}
																			alt={attachment.file_name}
																			class="h-12 w-12 shrink-0 rounded border border-slate-200 object-cover dark:border-slate-700"
																			data-attachment-thumbnail={attachment.id}
																		/>
																	{:else}
																		<FileText class="h-4 w-4 shrink-0 text-slate-500 dark:text-slate-400" aria-hidden="true" />
																	{/if}
																	<div class="min-w-0 flex-1">
																		<div class="truncate font-medium text-slate-900 dark:text-slate-100">{attachment.file_name}</div>
																		<div class="truncate font-mono text-[11px] text-slate-500 dark:text-slate-400">{attachment.content_type || attachment.storage_key}</div>
																	</div>
																		{#if attachment.url}
																			<a
																				href={attachmentSignedUrl(attachment)}
																				target="_blank"
																				rel="noreferrer"
																				class="inline-flex items-center rounded-md px-2 py-1 text-xs font-medium text-blue-700 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																				data-attachment-open={attachment.id}
																				data-attachment-signed-url={attachmentSignedUrl(attachment)}
																			>
																				{$t('forms.attachmentOpen')}
																			</a>
																			<a
																				href={attachmentSignedUrl(attachment)}
																				download={attachment.file_name}
																				class="inline-flex items-center rounded-md px-2 py-1 text-xs font-medium text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
																				data-attachment-download={attachment.id}
																				data-attachment-signed-download={attachmentSignedUrl(attachment)}
																			>
																			<FileDown class="mr-1 h-3.5 w-3.5" aria-hidden="true" />
																			{$t('forms.attachmentDownload')}
																		</a>
																	{/if}
																	<Button
																		size="sm"
																		variant="danger"
																		disabled={fieldReadOnlyByCondition(field, recordDraft)}
																		loading={deletingAttachmentId === attachment.id}
																		onclick={() => removeAttachment(field, attachment)}
																	>
																		{$t('forms.attachmentRemove')}
																	</Button>
																</div>
															{/each}
														</div>
													{/if}
												</div>
											{:else if fieldType(field) === 'textarea' || fieldType(field) === 'rich_text'}
												<textarea
													class="min-h-24 w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
													value={draftString(field.key)}
													placeholder={fieldPlaceholder(field)}
													readonly={fieldReadOnlyByCondition(field, recordDraft)}
													maxlength={field.validation?.max_length}
													oninput={(event) => setDraftValue(field.key, inputValue(event))}
												></textarea>
											{:else if fieldType(field) === 'signature'}
												<div class="space-y-2" data-signature-field={field.key}>
													<canvas
														class="h-28 w-full touch-none rounded-md border border-dashed border-slate-300 bg-white dark:border-slate-600 dark:bg-slate-900"
														data-signature-canvas={field.key}
														aria-label={$t('forms.signatureCanvas')}
														onpointerdown={(event) =>
															startSignatureStroke(event, field.key, 'record', fieldReadOnlyByCondition(field, recordDraft))}
														onpointermove={(event) =>
															moveSignatureStroke(event, field.key, 'record', fieldReadOnlyByCondition(field, recordDraft))}
														onpointerup={(event) =>
															endSignatureStroke(event, field.key, 'record', fieldReadOnlyByCondition(field, recordDraft))}
														onpointercancel={(event) =>
															endSignatureStroke(event, field.key, 'record', fieldReadOnlyByCondition(field, recordDraft))}
													></canvas>
													<div class="flex flex-wrap items-center gap-2">
														<Button
															size="sm"
															variant="secondary"
															disabled={fieldReadOnlyByCondition(field, recordDraft)}
															onclick={() => clearSignatureCanvas(field.key, 'record', fieldReadOnlyByCondition(field, recordDraft))}
														>
															{$t('forms.signatureClear')}
														</Button>
														{#if signatureImageValue(recordDraft, field.key)}
															<img
																src={signatureImageValue(recordDraft, field.key)}
																alt={fieldLabel(field)}
																class="h-10 max-w-48 rounded border border-slate-200 bg-white object-contain dark:border-slate-700"
																data-signature-preview={field.key}
															/>
														{/if}
													</div>
													<textarea
														class="min-h-16 w-full rounded-md border border-slate-300 bg-white px-3 py-2 font-mono text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
														value={draftString(field.key)}
														placeholder={fieldPlaceholder(field)}
														readonly={fieldReadOnlyByCondition(field, recordDraft)}
														maxlength={field.validation?.max_length}
														oninput={(event) => setDraftValue(field.key, inputValue(event))}
														data-signature-value={field.key}
													></textarea>
												</div>
											{:else if fieldType(field) === 'autonumber'}
												<input
													type="text"
													class="w-full rounded-md border border-slate-300 bg-slate-50 px-3 py-2 text-sm text-slate-500 dark:border-slate-600 dark:bg-slate-800/60 dark:text-slate-400"
													value={draftString(field.key)}
													placeholder={$t('forms.autonumberGeneratedAfterSave')}
													readonly
													data-autonumber-field={field.key}
												/>
											{:else}
												<input
													type={fieldType(field) === 'date'
														? 'date'
														: fieldType(field) === 'datetime'
															? 'datetime-local'
															: fieldType(field) === 'email'
																? 'email'
																: fieldType(field) === 'phone'
																	? 'tel'
																: ['amount', 'number', 'integer', 'rating', 'progress'].includes(fieldType(field))
																	? 'number'
																	: 'text'}
														step={['integer', 'rating', 'progress'].includes(fieldType(field))
															? '1'
															: ['amount', 'number'].includes(fieldType(field))
																? '0.01'
																: undefined}
													class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
													value={draftString(field.key)}
													placeholder={fieldPlaceholder(field)}
													readonly={fieldReadOnlyByCondition(field, recordDraft)}
													min={field.validation?.min}
													max={field.validation?.max}
													pattern={field.validation?.pattern}
													maxlength={field.validation?.max_length}
													oninput={(event) => setDraftValue(field.key, inputValue(event))}
												/>
											{/if}
											{#if field.help_text}
												<span class="block text-xs text-slate-500 dark:text-slate-400">
													{field.help_text}
												</span>
											{/if}
											{#if fieldType(field) === 'multi_select' && fieldOptions(field).length > 0}
												<span class="block text-xs text-slate-500 dark:text-slate-400"
													>{$t('forms.options', { values: { options: fieldOptions(field).join(', ') } })}</span
												>
											{/if}
										</label>
									{/each}
								</div>
								<div class="mt-4 flex justify-end">
									<Button loading={recordSubmitting} onclick={handleSaveRecord}>
										{#if editingRecordId}
											<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
											{$t('forms.saveRecord')}
										{:else}
											<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
											{$t('forms.addRecord')}
										{/if}
									</Button>
								</div>
										</section>
									</div>
								{/if}

									<section
										class="w-full overflow-hidden rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900"
										data-record-list-panel
									>
										{#if focusedRecords.length > 0}
										<div
											class="flex flex-wrap items-center justify-between gap-3 border-b border-slate-200 bg-blue-50 px-4 py-2 text-sm text-blue-900 dark:border-slate-700 dark:bg-blue-950/30 dark:text-blue-100"
											data-record-focus={focusedRecords.map((record) => record.id).join('|')}
										>
											<span class="min-w-0 truncate">
												{#if focusedRecords.length === 1}
													{$t('forms.focusedRecord')}: {focusedRecords[0].title}
												{:else}
													{$t('forms.focusedRecords', {
														values: { count: focusedRecords.length }
													})}
												{/if}
											</span>
											<button
												type="button"
												class="rounded-md px-2 py-1 text-xs font-medium text-blue-700 hover:bg-blue-100 dark:text-blue-200 dark:hover:bg-blue-900/40"
												onclick={clearFocusedRecord}
												data-record-focus-clear
											>
												{$t('forms.clearFocusedRecord')}
											</button>
										</div>
									{/if}
										{#if records.length === 0}
											<div class="p-8 text-center text-sm text-slate-500 dark:text-slate-400">
												<FileText class="mx-auto mb-3 h-8 w-8 text-slate-400" aria-hidden="true" />
												{$t('forms.noRecords')}
											</div>
										{:else if viewRecords.length === 0}
											<div class="p-8 text-center text-sm text-slate-500 dark:text-slate-400" data-view-empty-filter>
												<FileText class="mx-auto mb-3 h-8 w-8 text-slate-400" aria-hidden="true" />
												{$t('forms.noMatchingRecords')}
											</div>
										{:else if selectedViewType === 'grid'}
											<div class="overflow-x-auto" data-view-renderer="grid">
											<table
												class="min-w-full divide-y divide-slate-200 text-sm dark:divide-slate-700"
											>
											<thead class="bg-slate-50 dark:bg-slate-950">
												<tr>
													<th
														class="w-56 px-4 py-3 text-left font-medium text-slate-600 dark:text-slate-300"
														>{$t('forms.recordTitle')}</th
													>
													{#each visibleFields as field}
														<th
															class="px-4 py-3 text-left font-medium text-slate-600 dark:text-slate-300"
														>
															<div class="flex flex-wrap items-center gap-1.5">
																<span>{fieldLabel(field)}</span>
																{#if memberFieldHidden(field)}
																	<span
																		class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																		data-grid-header-permission-hidden={field.key}
																	>
																		{$t('forms.fieldHiddenBadge')}
																	</span>
																{/if}
																{#if memberFieldLocked(field)}
																	<span
																		class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																		data-grid-header-permission-locked={field.key}
																	>
																		{$t('forms.fieldLockedBadge')}
																	</span>
																{/if}
																{#if fieldHasConditionalHidden(field)}
																	<span
																		class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																		data-grid-header-conditional-hidden={field.key}
																	>
																		{$t('forms.fieldConditionHiddenBadge')}
																	</span>
																{/if}
																{#if fieldHasConditionalReadOnly(field)}
																	<span
																		class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																		data-grid-header-conditional-readonly={field.key}
																	>
																		{$t('forms.fieldConditionReadOnlyBadge')}
																	</span>
																{/if}
															</div>
														</th>
													{/each}
													<th
														class="px-4 py-3 text-right font-medium text-slate-600 dark:text-slate-300"
														>{$t('common.actions')}</th
													>
												</tr>
											</thead>
												<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
														{#each paginatedViewRecords as record}
														<tr class="hover:bg-slate-50 dark:hover:bg-slate-800/70">
															<td class="px-4 py-3">
																<button
																	type="button"
																	class="text-left font-medium text-blue-700 hover:text-blue-800 dark:text-blue-300"
																	onclick={() => openRecordEditDrawer(record)}
																>
																	{record.title}
																</button>
															<div class="mt-1 font-mono text-xs text-slate-400">
																{record.id.slice(0, 8)}
															</div>
														</td>
														{#each visibleFields as field}
															<td
																class="max-w-64 truncate px-4 py-3 text-slate-700 dark:text-slate-300"
															>
																<div class="flex min-w-0 flex-wrap items-center gap-1.5">
																	<span class="min-w-0 truncate">
																		<OptionValueBadges {field} value={record.values[field.key]} />
																	</span>
																	{#if recordFieldHiddenByCondition(field, record)}
																		<span
																			class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																			data-grid-cell-conditional-hidden={`${record.id}:${field.key}`}
																		>
																			{$t('forms.fieldConditionHiddenBadge')}
																		</span>
																	{/if}
																	{#if recordFieldReadOnlyByCondition(field, record)}
																		<span
																			class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																			data-grid-cell-conditional-readonly={`${record.id}:${field.key}`}
																		>
																			{$t('forms.fieldConditionReadOnlyBadge')}
																		</span>
																	{/if}
																</div>
															</td>
														{/each}
															<td class="px-4 py-3 text-right">
																<button
																	type="button"
																	class="mr-1 inline-flex items-center rounded-md p-1.5 text-blue-700 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																	onclick={() => openRecordEditDrawer(record)}
																	aria-label={$t('forms.editRecord')}
																>
																	<Pencil class="h-4 w-4" aria-hidden="true" />
																</button>
																<button
																	type="button"
																	class="inline-flex items-center rounded-md p-1.5 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																disabled={deletingRecordId === record.id}
																onclick={() => handleDeleteRecord(record)}
																aria-label={$t('forms.deleteRecord')}
															>
																<Trash2 class="h-4 w-4" aria-hidden="true" />
															</button>
														</td>
													</tr>
												{/each}
											</tbody>
											</table>
											</div>
											{#if recordPageCount > 1}
												<div
													class="flex flex-col gap-2 border-t border-slate-200 px-4 py-3 text-sm dark:border-slate-700 sm:flex-row sm:items-center sm:justify-between"
													data-record-pagination
												>
													<span class="text-slate-500 dark:text-slate-400">
														{$t('forms.recordPageLabel', {
															values: {
																page: normalizedRecordPage,
																total: recordPageCount,
																count: viewRecords.length
															}
														})}
													</span>
													<div class="flex items-center gap-2">
														<Button
															size="sm"
															variant="secondary"
															disabled={normalizedRecordPage <= 1}
															onclick={() => goToRecordPage(normalizedRecordPage - 1)}
														>
															<ChevronLeft class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('forms.previousPage')}
														</Button>
														<Button
															size="sm"
															variant="secondary"
															disabled={normalizedRecordPage >= recordPageCount}
															onclick={() => goToRecordPage(normalizedRecordPage + 1)}
														>
															{$t('forms.nextPage')}
															<ChevronRight class="ml-2 h-4 w-4" aria-hidden="true" />
														</Button>
													</div>
												</div>
											{/if}
										{:else if selectedViewType === 'gantt'}
										<div class="overflow-x-auto p-4" data-view-renderer="gantt">
											{#if !ganttTimeline.startField}
												<div class="rounded-md border border-dashed border-slate-300 bg-slate-50 px-4 py-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-950/50 dark:text-slate-400">
													{$t('forms.ganttConfigureFields')}
												</div>
											{:else}
												<div class="mb-3 flex flex-wrap items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
													<span data-gantt-start-field={ganttTimeline.startField.key}>
														{$t('forms.ganttStartField')}: {fieldLabel(ganttTimeline.startField)}
													</span>
													{#if fieldHasConditionalHidden(ganttTimeline.startField)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-gantt-start-field-conditional-hidden={ganttTimeline.startField.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if fieldHasConditionalReadOnly(ganttTimeline.startField)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-gantt-start-field-conditional-readonly={ganttTimeline.startField.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
													<span>·</span>
													<span data-gantt-end-field={ganttTimeline.endField?.key ?? ''}>
														{$t('forms.ganttEndField')}:
														{ganttTimeline.endField ? fieldLabel(ganttTimeline.endField) : $t('forms.ganttNoEndField')}
													</span>
													{#if ganttTimeline.endField && fieldHasConditionalHidden(ganttTimeline.endField)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-gantt-end-field-conditional-hidden={ganttTimeline.endField.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if ganttTimeline.endField && fieldHasConditionalReadOnly(ganttTimeline.endField)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-gantt-end-field-conditional-readonly={ganttTimeline.endField.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
													<span>·</span>
													<span data-gantt-group-field={ganttTimeline.groupField?.key ?? ''}>
														{$t('forms.ganttGroupField')}:
														{ganttTimeline.groupField ? fieldLabel(ganttTimeline.groupField) : $t('forms.ganttUngrouped')}
													</span>
													{#if ganttTimeline.groupField && fieldHasConditionalHidden(ganttTimeline.groupField)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-gantt-group-field-conditional-hidden={ganttTimeline.groupField.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if ganttTimeline.groupField && fieldHasConditionalReadOnly(ganttTimeline.groupField)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-gantt-group-field-conditional-readonly={ganttTimeline.groupField.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
													<span>·</span>
													<span data-gantt-dependency-field={ganttTimeline.dependencyField?.key ?? ''}>
														{$t('forms.ganttDependencyField')}:
														{ganttTimeline.dependencyField ? fieldLabel(ganttTimeline.dependencyField) : $t('forms.viewNoRule')}
													</span>
													{#if ganttTimeline.dependencyField && fieldHasConditionalHidden(ganttTimeline.dependencyField)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-gantt-dependency-field-conditional-hidden={ganttTimeline.dependencyField.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if ganttTimeline.dependencyField && fieldHasConditionalReadOnly(ganttTimeline.dependencyField)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-gantt-dependency-field-conditional-readonly={ganttTimeline.dependencyField.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
												</div>
												{#if ganttTimeline.dates.length === 0}
													<div class="rounded-md border border-dashed border-slate-300 bg-slate-50 px-4 py-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-950/50 dark:text-slate-400">
														{$t('forms.noRecords')}
													</div>
												{:else}
													<div
														class="min-w-[980px] rounded-md border border-slate-200 bg-white text-sm dark:border-slate-700 dark:bg-slate-900"
														data-gantt-scale={ganttTimeline.scale}
														data-gantt-start={ganttTimeline.dates[0]}
														data-gantt-end={ganttTimeline.dates[ganttTimeline.dates.length - 1]}
													>
														<div class="grid grid-cols-[180px_minmax(0,1fr)] border-b border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-950">
															<div class="px-4 py-3 text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
																{$t('forms.records')}
															</div>
															<div
																class="grid divide-x divide-slate-200 dark:divide-slate-700"
																style={`grid-template-columns: repeat(${ganttTimeline.dates.length}, minmax(72px, 1fr));`}
															>
																{#each ganttTimeline.dates as date}
																	<div
																		class="px-2 py-3 text-center text-xs font-medium text-slate-500 dark:text-slate-400"
																		data-gantt-date={date}
																		role="button"
																		tabindex="0"
																		aria-label={`${$t('forms.ganttStartField')} ${date}`}
																		ondragover={handleGanttDragOver}
																		ondrop={(event) => handleGanttDrop(event, date)}
																	>
																		{ganttDateLabel(date, ganttTimeline.scale)}
																	</div>
																{/each}
															</div>
														</div>
														{#each ganttTimeline.lanes as lane}
															<div class="border-b border-slate-100 last:border-b-0 dark:border-slate-800" data-gantt-lane={lane.label}>
																<div class="bg-slate-50 px-4 py-2 text-xs font-semibold uppercase text-slate-500 dark:bg-slate-950 dark:text-slate-400">
																	{lane.label}
																</div>
																{#if lane.tasks.length === 0}
																	<div class="grid grid-cols-[180px_minmax(0,1fr)]">
																		<div class="px-4 py-3 text-xs text-slate-400">{$t('forms.noRecords')}</div>
																		<div class="min-h-10"></div>
																	</div>
																{/if}
																{#each lane.tasks as task}
																	<div class="grid grid-cols-[180px_minmax(0,1fr)] items-center border-t border-slate-100 dark:border-slate-800">
																		<div class="min-w-0 px-4 py-3">
																			<div class="truncate font-medium text-slate-900 dark:text-slate-100">{task.record.title}</div>
																			<div class="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
																				{task.start}{task.end !== task.start ? ` - ${task.end}` : ''}
																			</div>
																			<div class="mt-1 flex flex-wrap items-center gap-1.5">
																				{#if recordFieldHiddenByCondition(ganttTimeline.startField, task.record)}
																					<span
																						class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																						data-gantt-task-start-conditional-hidden={`${task.record.id}:${ganttTimeline.startField.key}`}
																					>
																						{$t('forms.fieldConditionHiddenBadge')}
																					</span>
																				{/if}
																				{#if recordFieldReadOnlyByCondition(ganttTimeline.startField, task.record)}
																					<span
																						class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																						data-gantt-task-start-conditional-readonly={`${task.record.id}:${ganttTimeline.startField.key}`}
																					>
																						{$t('forms.fieldConditionReadOnlyBadge')}
																					</span>
																				{/if}
																				{#if ganttTimeline.endField && recordFieldHiddenByCondition(ganttTimeline.endField, task.record)}
																					<span
																						class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																						data-gantt-task-end-conditional-hidden={`${task.record.id}:${ganttTimeline.endField.key}`}
																					>
																						{$t('forms.fieldConditionHiddenBadge')}
																					</span>
																				{/if}
																				{#if ganttTimeline.endField && recordFieldReadOnlyByCondition(ganttTimeline.endField, task.record)}
																					<span
																						class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																						data-gantt-task-end-conditional-readonly={`${task.record.id}:${ganttTimeline.endField.key}`}
																					>
																						{$t('forms.fieldConditionReadOnlyBadge')}
																					</span>
																				{/if}
																			</div>
																			{#if ganttTimeline.dependencyField && task.dependencyValue && task.dependencyValue !== '-'}
																				<div class="mt-1 truncate text-xs text-slate-500 dark:text-slate-400" data-gantt-dependency={task.record.id}>
																					{$t('forms.ganttDependency')}: {task.dependencyValue}
																				</div>
																			{/if}
																			{#if ganttTimeline.dependencyField && recordFieldHiddenByCondition(ganttTimeline.dependencyField, task.record)}
																				<span
																					class="mt-1 inline-flex rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																					data-gantt-task-dependency-conditional-hidden={`${task.record.id}:${ganttTimeline.dependencyField.key}`}
																				>
																					{$t('forms.fieldConditionHiddenBadge')}
																				</span>
																			{/if}
																			{#if ganttTimeline.dependencyField && recordFieldReadOnlyByCondition(ganttTimeline.dependencyField, task.record)}
																				<span
																					class="mt-1 inline-flex rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																					data-gantt-task-dependency-conditional-readonly={`${task.record.id}:${ganttTimeline.dependencyField.key}`}
																				>
																					{$t('forms.fieldConditionReadOnlyBadge')}
																				</span>
																			{/if}
																		</div>
																		<div
																			class="grid min-h-14 items-center gap-y-1 px-2"
																			style={`grid-template-columns: repeat(${ganttTimeline.dates.length}, minmax(72px, 1fr));`}
																		>
																				<div
																				class="flex items-center justify-between gap-2 rounded-md bg-emerald-600 px-3 py-2 text-xs font-semibold text-white shadow-sm transition-opacity dark:bg-emerald-500 dark:text-slate-950"
																				style={`grid-column: ${task.startOffset + 1} / span ${task.span};`}
																				draggable="true"
																				role="button"
																				tabindex="0"
																				aria-label={task.record.title}
																				ondragstart={(event) => handleGanttDragStart(event, task.record)}
																				ondragend={handleGanttDragEnd}
																				data-gantt-bar={task.record.id}
																				data-gantt-bar-start={task.start}
																				data-gantt-bar-end={task.end}
																				data-gantt-bar-dependency={task.dependencyValue}
																				data-gantt-updating={updatingGanttRecordId === task.record.id ? 'true' : 'false'}
																			>
																				<span class="min-w-0 truncate">{task.record.title}</span>
																				{#if ganttTimeline.dependencyField && task.dependencyValue && task.dependencyValue !== '-'}
																					<span class="hidden shrink-0 rounded bg-white/15 px-1.5 py-0.5 text-[11px] font-medium text-white/90 md:inline dark:text-slate-950" data-gantt-bar-dependency-label={task.record.id}>
																						{task.dependencyValue}
																					</span>
																				{/if}
																				{#if ganttTimeline.endField}
																					<button
																						type="button"
																						class="shrink-0 rounded bg-white/20 px-1.5 py-0.5 text-[11px] leading-none text-white hover:bg-white/30 focus:outline-none focus:ring-2 focus:ring-white dark:text-slate-950 dark:focus:ring-slate-950"
																						onclick={(event) => {
																							event.stopPropagation();
																							extendGanttTaskEnd(task);
																						}}
																						disabled={updatingGanttRecordId === task.record.id}
																						aria-label={$t('forms.ganttResizeEnd')}
																						data-gantt-resize-end={task.record.id}
																					>
																						+1
																					</button>
																				{/if}
																			</div>
																		</div>
																	</div>
																{/each}
															</div>
														{/each}
													</div>
												{/if}
											{/if}
										</div>
									{:else if selectedViewType === 'pivot'}
										<div class="overflow-x-auto p-4" data-view-renderer="pivot">
											{#if !pivotTable.rowField || !pivotTable.columnField || !pivotTable.valueField}
												<div class="rounded-md border border-dashed border-slate-300 bg-slate-50 px-4 py-6 text-sm text-slate-500 dark:border-slate-700 dark:bg-slate-950/50 dark:text-slate-400">
													{$t('forms.pivotConfigureFields')}
												</div>
											{:else}
												<div class="mb-3 flex flex-wrap items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
													<span data-pivot-row-field={pivotTable.rowField.key}>
														{$t('forms.pivotRowField')}: {fieldLabel(pivotTable.rowField)}
													</span>
													{#if fieldHasConditionalHidden(pivotTable.rowField)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-pivot-row-field-conditional-hidden={pivotTable.rowField.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if fieldHasConditionalReadOnly(pivotTable.rowField)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-pivot-row-field-conditional-readonly={pivotTable.rowField.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
													<span>·</span>
													<span data-pivot-column-field={pivotTable.columnField.key}>
														{$t('forms.pivotColumnField')}: {fieldLabel(pivotTable.columnField)}
													</span>
													{#if fieldHasConditionalHidden(pivotTable.columnField)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-pivot-column-field-conditional-hidden={pivotTable.columnField.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if fieldHasConditionalReadOnly(pivotTable.columnField)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-pivot-column-field-conditional-readonly={pivotTable.columnField.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
													<span>·</span>
													<span data-pivot-value-field={pivotTable.valueField.key}>
														{$t('forms.pivotValueField')}: {fieldLabel(pivotTable.valueField)}
													</span>
													{#if fieldHasConditionalHidden(pivotTable.valueField)}
														<span
															class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
															data-pivot-value-field-conditional-hidden={pivotTable.valueField.key}
														>
															{$t('forms.fieldConditionHiddenBadge')}
														</span>
													{/if}
													{#if fieldHasConditionalReadOnly(pivotTable.valueField)}
														<span
															class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
															data-pivot-value-field-conditional-readonly={pivotTable.valueField.key}
														>
															{$t('forms.fieldConditionReadOnlyBadge')}
														</span>
													{/if}
													<span>·</span>
													<span data-pivot-aggregate={pivotTable.aggregate}>
														{$t('forms.pivotAggregate')}:
														{pivotTable.aggregate === 'count'
															? $t('forms.pivotAggregateCount')
															: pivotTable.aggregate === 'avg'
																? $t('forms.pivotAggregateAvg')
																: $t('forms.pivotAggregateSum')}
													</span>
													<span>·</span>
													<span data-pivot-totals={pivotTable.totalsMode}>
														{$t('forms.pivotTotals')}:
														{pivotTable.totalsMode === 'rows'
															? $t('forms.pivotTotalsRows')
															: pivotTable.totalsMode === 'columns'
																? $t('forms.pivotTotalsColumns')
																: pivotTable.totalsMode === 'none'
																	? $t('forms.pivotTotalsNone')
																	: $t('forms.pivotTotalsAll')}
													</span>
													<span>·</span>
													<span data-pivot-value-display={pivotTable.valueDisplay}>
														{$t('forms.pivotValueDisplay')}:
														{pivotTable.valueDisplay === 'percent_row'
															? $t('forms.pivotValueDisplayPercentRow')
															: pivotTable.valueDisplay === 'percent_column'
																? $t('forms.pivotValueDisplayPercentColumn')
																: pivotTable.valueDisplay === 'percent_total'
																	? $t('forms.pivotValueDisplayPercentTotal')
																	: $t('forms.pivotValueDisplayValue')}
													</span>
													<span>·</span>
													<span data-pivot-sort={pivotTable.sortMode}>
														{$t('forms.pivotSort')}:
														{pivotTable.sortMode === 'row_total_desc'
															? $t('forms.pivotSortRowTotalDesc')
															: pivotTable.sortMode === 'row_total_asc'
																? $t('forms.pivotSortRowTotalAsc')
																: pivotTable.sortMode === 'column_total_desc'
																	? $t('forms.pivotSortColumnTotalDesc')
																	: pivotTable.sortMode === 'column_total_asc'
																		? $t('forms.pivotSortColumnTotalAsc')
																		: $t('forms.pivotSortLabel')}
													</span>
													<span>·</span>
														<span data-pivot-empty-buckets={pivotTable.emptyBuckets}>
															{$t('forms.pivotEmptyBuckets')}:
															{pivotTable.emptyBuckets ? $t('forms.yes') : $t('forms.no')}
														</span>
														<span>·</span>
														<span data-pivot-hide-zero-buckets={pivotTable.hideZeroBuckets}>
															{$t('forms.pivotHideZeroBuckets')}:
															{pivotTable.hideZeroBuckets ? $t('forms.yes') : $t('forms.no')}
														</span>
														<span>·</span>
														<span data-pivot-show-counts={pivotTable.showCounts}>
															{$t('forms.pivotShowCounts')}:
															{pivotTable.showCounts ? $t('forms.yes') : $t('forms.no')}
													</span>
													<span>·</span>
													<span data-pivot-heatmap={pivotTable.heatmap}>
														{$t('forms.pivotHeatmap')}:
														{pivotTable.heatmap ? $t('forms.yes') : $t('forms.no')}
													</span>
													<span>·</span>
													<span data-pivot-row-limit={pivotTable.rowLimit}>
														{$t('forms.pivotRowLimit')}:
														{pivotTable.rowLimit > 0
															? $t('forms.pivotRowLimitValue', {
																	values: { count: pivotTable.rowLimit }
																})
															: $t('forms.pivotRowLimitAll')}
													</span>
													<span>·</span>
													<span data-pivot-column-limit={pivotTable.columnLimit}>
														{$t('forms.pivotColumnLimit')}:
															{pivotTable.columnLimit > 0
																? $t('forms.pivotColumnLimitValue', {
																		values: { count: pivotTable.columnLimit }
																	})
																: $t('forms.pivotColumnLimitAll')}
														</span>
													</div>
													<div class="mb-3 flex flex-wrap justify-end gap-2">
														<button
															type="button"
															class="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
															onclick={copyPivotTableMatrix}
															data-pivot-copy-table
															data-pivot-table-copied={copiedPivotTableMatrix ? 'true' : undefined}
														>
															<Copy class="h-3.5 w-3.5" aria-hidden="true" />
															{$t('forms.copyPivotTable')}
														</button>
														<button
															type="button"
															class="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
															onclick={exportPivotTableMatrixCsv}
															data-pivot-export-table
															data-pivot-table-exported={exportedPivotTableMatrix ? 'true' : undefined}
														>
															<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
															{$t('forms.exportPivotTable')}
														</button>
													</div>
													<table class="min-w-full divide-y divide-slate-200 text-sm dark:divide-slate-700">
													<thead class="bg-slate-50 text-left text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
														<tr>
															<th class="px-4 py-3 font-medium">
																<div class="flex flex-wrap items-center gap-1.5">
																	<span>{fieldLabel(pivotTable.rowField)}</span>
																	{#if fieldHasConditionalHidden(pivotTable.rowField)}
																		<span
																			class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																			data-pivot-table-row-field-conditional-hidden={pivotTable.rowField.key}
																		>
																			{$t('forms.fieldConditionHiddenBadge')}
																		</span>
																	{/if}
																	{#if fieldHasConditionalReadOnly(pivotTable.rowField)}
																		<span
																			class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																			data-pivot-table-row-field-conditional-readonly={pivotTable.rowField.key}
																		>
																			{$t('forms.fieldConditionReadOnlyBadge')}
																		</span>
																	{/if}
																</div>
															</th>
															{#each pivotTable.columns as column}
																<th class="px-4 py-3 text-right font-medium" data-pivot-column={column.label}>
																	<div class="flex flex-wrap items-center justify-end gap-1.5">
																		<span>{column.label}</span>
																		{#if recordsFieldHiddenByCondition(pivotTable.columnField, pivotColumnRecords(column))}
																			<span
																				class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																				data-pivot-column-conditional-hidden={`${column.label}:${pivotTable.columnField.key}`}
																			>
																				{$t('forms.fieldConditionHiddenBadge')}
																			</span>
																		{/if}
																		{#if recordsFieldReadOnlyByCondition(pivotTable.columnField, pivotColumnRecords(column))}
																			<span
																				class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																				data-pivot-column-conditional-readonly={`${column.label}:${pivotTable.columnField.key}`}
																			>
																				{$t('forms.fieldConditionReadOnlyBadge')}
																			</span>
																		{/if}
																	</div>
																</th>
															{/each}
															{#if pivotShowRowTotals(pivotTable.totalsMode)}
																<th class="px-4 py-3 text-right font-medium">{$t('forms.pivotTotal')}</th>
															{/if}
														</tr>
													</thead>
													<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
														{#each pivotTable.rows as row}
															<tr class="hover:bg-slate-50 dark:hover:bg-slate-800/70" data-pivot-row={row.label}>
																<th class="px-4 py-3 text-left font-medium text-slate-900 dark:text-slate-100">
																	<div class="flex flex-wrap items-center gap-1.5">
																		<span>{row.label}</span>
																		{#if recordsFieldHiddenByCondition(pivotTable.rowField, pivotRowRecords(row))}
																			<span
																				class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																				data-pivot-row-conditional-hidden={`${row.label}:${pivotTable.rowField.key}`}
																			>
																				{$t('forms.fieldConditionHiddenBadge')}
																			</span>
																		{/if}
																		{#if recordsFieldReadOnlyByCondition(pivotTable.rowField, pivotRowRecords(row))}
																			<span
																				class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																				data-pivot-row-conditional-readonly={`${row.label}:${pivotTable.rowField.key}`}
																			>
																				{$t('forms.fieldConditionReadOnlyBadge')}
																			</span>
																		{/if}
																	</div>
																</th>
																{#each pivotTable.columns as column}
																	{@const cellValue = row.cells[column.key] ?? 0}
																	<td
																		class="px-4 py-3 text-right text-slate-700 dark:text-slate-300"
																		data-pivot-cell={`${row.label}:${column.label}`}
																	>
																			<button
																				type="button"
																				class="flex w-full flex-col items-end rounded px-2 py-1 text-right font-medium text-blue-700 hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:text-blue-300 dark:hover:bg-blue-950/40"
																				style={pivotCellHeatmapStyle(cellValue, pivotTable)}
																			onclick={() => selectPivotDrilldown(row, column)}
																			data-pivot-cell-button={`${row.label}:${column.label}`}
																			data-pivot-cell-intensity={pivotCellIntensity(cellValue, pivotTable)}
																		>
																			<span>
																				{formatPivotDisplayValue(
																					cellValue,
																					row.total,
																					column.total,
																					pivotTable
																				)}
																			</span>
																			{#if pivotTable.showCounts}
																				<span
																					class="mt-0.5 text-[11px] font-normal text-slate-500 dark:text-slate-400"
																					data-pivot-cell-count={`${row.label}:${column.label}`}
																					data-record-count={row.cellCounts[column.key] ?? 0}
																				>
																					{$t('forms.pivotCellRecordCount', {
																						values: { count: row.cellCounts[column.key] ?? 0 }
																					})}
																				</span>
																				{/if}
																			{#if recordsFieldHiddenByCondition(pivotTable.rowField, pivotCellRecords(row, column))}
																				<span
																					class="mt-0.5 rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																					data-pivot-cell-row-conditional-hidden={`${row.label}:${column.label}:${pivotTable.rowField.key}`}
																				>
																					{$t('forms.fieldConditionHiddenBadge')}
																				</span>
																			{/if}
																			{#if recordsFieldReadOnlyByCondition(pivotTable.columnField, pivotCellRecords(row, column))}
																				<span
																					class="mt-0.5 rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																					data-pivot-cell-column-conditional-readonly={`${row.label}:${column.label}:${pivotTable.columnField.key}`}
																				>
																					{$t('forms.fieldConditionReadOnlyBadge')}
																				</span>
																			{/if}
																			</button>
																			<button
																				type="button"
																				class="mt-1 inline-flex items-center rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:hover:bg-slate-800 dark:hover:text-slate-200"
																				onclick={() => copyPivotCellValue(row, column, cellValue)}
																				aria-label={$t('forms.copyPivotCellValue')}
																				data-pivot-cell-copy={`${row.label}:${column.label}`}
																				data-pivot-cell-copied={copiedPivotCellKey === `${row.label}:${column.label}` ? 'true' : undefined}
																			>
																				<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																			</button>
																		</td>
																	{/each}
																	{#if pivotShowRowTotals(pivotTable.totalsMode)}
																		<td class="px-4 py-3 text-right font-semibold text-slate-900 dark:text-slate-100" data-pivot-row-total={row.label}>
																			<button
																				type="button"
																				class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-blue-700 hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:text-blue-300 dark:hover:bg-blue-950/40"
																				onclick={() => selectPivotDrilldown(row, null)}
																				data-pivot-row-total-button={row.label}
																			>
																				{formatPivotTotalDisplayValue(
																					row.total,
																					'row',
																					pivotTable
																				)}
																			</button>
																		</td>
																	{/if}
															</tr>
														{/each}
													</tbody>
													{#if pivotShowColumnTotals(pivotTable.totalsMode)}
														<tfoot class="bg-slate-50 text-sm font-semibold text-slate-900 dark:bg-slate-900 dark:text-slate-100">
															<tr>
																	<th class="px-4 py-3 text-left">{$t('forms.pivotTotal')}</th>
																	{#each pivotTable.columns as column}
																		<td class="px-4 py-3 text-right" data-pivot-column-total={column.label}>
																			<button
																				type="button"
																				class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-blue-700 hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:text-blue-300 dark:hover:bg-blue-950/40"
																				onclick={() => selectPivotDrilldown(null, column)}
																				data-pivot-column-total-button={column.label}
																			>
																				{formatPivotTotalDisplayValue(
																					column.total,
																					'column',
																					pivotTable
																				)}
																			</button>
																		</td>
																	{/each}
																	{#if pivotShowRowTotals(pivotTable.totalsMode)}
																		<td class="px-4 py-3 text-right" data-pivot-grand-total>
																			<button
																				type="button"
																				class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-blue-700 hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:text-blue-300 dark:hover:bg-blue-950/40"
																				onclick={() => selectPivotDrilldown(null, null)}
																				data-pivot-grand-total-button
																			>
																				{formatPivotTotalDisplayValue(
																					pivotTable.grandTotal,
																					'grand',
																					pivotTable
																				)}
																			</button>
																		</td>
																	{/if}
															</tr>
														</tfoot>
													{/if}
												</table>
												{#if pivotDrilldown}
													<section
														class="mt-4 rounded-md border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
															data-pivot-drilldown={`${pivotDrilldown.rowLabel}:${pivotDrilldown.columnLabel}`}
															data-pivot-drilldown-copied={copiedPivotDrilldownKey ===
															pivotDrilldownKey()
																? `${pivotDrilldown.rowLabel}:${pivotDrilldown.columnLabel}`
																: undefined}
															data-pivot-drilldown-exported={exportedPivotDrilldownKey ===
															pivotDrilldownKey()
																? `${pivotDrilldown.rowLabel}:${pivotDrilldown.columnLabel}`
																: undefined}
															data-pivot-drilldown-filters-applied={appliedPivotDrilldownFilterKey ===
															pivotDrilldownKey()
																? `${pivotDrilldown.rowLabel}:${pivotDrilldown.columnLabel}`
																: undefined}
														data-record-count={pivotDrilldownRecords.length}
													>
														<div class="mb-3 flex items-start justify-between gap-3">
															<div>
																<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
																	{$t('forms.pivotDrilldown')}
																</h3>
																<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
																	{$t('forms.pivotDrilldownFor', {
																		values: {
																			row: pivotDrilldown.rowLabel,
																			column: pivotDrilldown.columnLabel,
																			count: pivotDrilldownRecords.length
																		}
																	})}
																</p>
															</div>
															<div class="flex shrink-0 items-center gap-2">
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={copyPivotDrilldownRecordIds}
																	disabled={pivotDrilldownRecords.length === 0}
																	data-pivot-drilldown-copy-ids
																>
																	<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.copyPivotDrilldownRecordIds')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={exportPivotDrilldownRecordsCsv}
																	disabled={pivotDrilldownRecords.length === 0}
																	data-pivot-drilldown-export-csv
																	data-pivot-drilldown-exported={exportedPivotDrilldownKey ===
																	pivotDrilldownKey()
																		? 'true'
																		: undefined}
																>
																	<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.exportPivotDrilldownRecords')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={applyPivotDrilldownFilters}
																	disabled={!canApplyPivotDrilldownFilters()}
																	data-pivot-drilldown-apply-filters
																	data-pivot-drilldown-filters-applied={appliedPivotDrilldownFilterKey ===
																	pivotDrilldownKey()
																		? 'true'
																		: undefined}
																>
																	<Filter class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.applyPivotDrilldownFilters')}
																</button>
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
																	onclick={focusPivotDrilldownRecords}
																	disabled={pivotDrilldownRecords.length === 0}
																	data-pivot-drilldown-focus
																>
																	<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.focusPivotDrilldown')}
																</button>
																<button
																	type="button"
																	class="rounded-md p-1.5 text-slate-500 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
																	onclick={() => (pivotDrilldown = null)}
																	aria-label={$t('common.close')}
																>
																	x
																</button>
															</div>
														</div>
														{#if pivotDrilldownRecords.length === 0}
															<p class="text-sm text-slate-500 dark:text-slate-400">
																{$t('forms.pivotDrilldownEmpty')}
															</p>
														{:else}
															<div class="divide-y divide-slate-100 dark:divide-slate-800">
																{#each pivotDrilldownRecords as record}
																	<div class="flex items-center justify-between gap-3 py-2" data-pivot-drilldown-record={record.id}>
																		<div class="min-w-0">
																			<div class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">
																				{record.title}
																			</div>
																			<div class="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
																				{record.id}
																			</div>
																			<div class="mt-1 flex flex-wrap items-center gap-1.5">
																				{#if recordFieldHiddenByCondition(pivotTable.rowField, record)}
																					<span
																						class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																						data-pivot-drilldown-row-conditional-hidden={`${record.id}:${pivotTable.rowField.key}`}
																					>
																						{$t('forms.fieldConditionHiddenBadge')}
																					</span>
																				{/if}
																				{#if recordFieldReadOnlyByCondition(pivotTable.rowField, record)}
																					<span
																						class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																						data-pivot-drilldown-row-conditional-readonly={`${record.id}:${pivotTable.rowField.key}`}
																					>
																						{$t('forms.fieldConditionReadOnlyBadge')}
																					</span>
																				{/if}
																				{#if recordFieldHiddenByCondition(pivotTable.columnField, record)}
																					<span
																						class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																						data-pivot-drilldown-column-conditional-hidden={`${record.id}:${pivotTable.columnField.key}`}
																					>
																						{$t('forms.fieldConditionHiddenBadge')}
																					</span>
																				{/if}
																				{#if recordFieldReadOnlyByCondition(pivotTable.columnField, record)}
																					<span
																						class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																						data-pivot-drilldown-column-conditional-readonly={`${record.id}:${pivotTable.columnField.key}`}
																					>
																						{$t('forms.fieldConditionReadOnlyBadge')}
																					</span>
																				{/if}
																			</div>
																		</div>
																		<button
																			type="button"
																			class="shrink-0 rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
																			onclick={() => selectRecord(record)}
																			data-pivot-drilldown-open={record.id}
																		>
																			{$t('forms.openRecord')}
																		</button>
																	</div>
																{/each}
															</div>
														{/if}
													</section>
												{/if}
											{/if}
										</div>
										{:else if selectedViewType === 'detail'}
											<div class="divide-y divide-slate-100 dark:divide-slate-800" data-view-renderer="detail">
												{#each viewRecords as record}
												<article class="p-4 hover:bg-slate-50 dark:hover:bg-slate-800/60">
													<div class="flex items-start justify-between gap-3">
														<button
															type="button"
															class="min-w-0 text-left"
															onclick={() => selectRecord(record)}
															aria-label={`${$t('forms.openRecord')}: ${record.title}`}
														>
															<span class="block truncate font-medium text-blue-700 dark:text-blue-300">
																{record.title}
															</span>
															<span class="mt-1 block text-xs text-slate-500 dark:text-slate-400">
																{$t('forms.updatedAt', { values: { date: formatDate(record.updated_at) } })}
															</span>
														</button>
														<button
															type="button"
															class="inline-flex shrink-0 items-center rounded-md p-1.5 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
															disabled={deletingRecordId === record.id}
															onclick={() => handleDeleteRecord(record)}
															aria-label={$t('forms.deleteRecord')}
														>
															<Trash2 class="h-4 w-4" aria-hidden="true" />
														</button>
													</div>
													<div class="mt-3 space-y-3">
														{#each detailLayoutSections() as section}
															<details
																open={!section.collapsible || !section.defaultCollapsed}
																ontoggle={(event) => keepDetailSectionOpen(event, section.collapsible)}
																data-detail-view-section={`${record.id}:${section.key}`}
																data-detail-view-section-density={`${record.id}:${section.key}:${section.density}`}
																data-detail-view-section-collapsible={`${record.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
																data-detail-view-section-open={`${record.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
															>
																<summary
																	class={section.collapsible
																		? 'cursor-pointer list-none'
																		: 'cursor-default list-none'}
																>
																	<h4 class="mb-2 inline text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
																		{section.title}
																	</h4>
																</summary>
																{#if section.description}
																	<p
																		class="mb-3 text-sm text-slate-500 dark:text-slate-400"
																		data-detail-view-section-description={`${record.id}:${section.key}`}
																	>
																		{section.description}
																	</p>
																{/if}
																<dl
																	class={detailLayoutSectionGridClass(section.columns, section.density)}
																	data-detail-view-section-columns={`${record.id}:${section.key}:${section.columns}`}
																>
																	{#each detailLayoutSectionFieldsForRecord(section, record) as field}
																		<div
																			class={detailLayoutSectionFieldClass(field, section.columns)}
																			data-detail-view-field={`${record.id}:${section.key}:${field.key}`}
																		>
																			<dt class="truncate text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																				{fieldLabel(field)}
																			</dt>
																			<dd class="mt-1 truncate text-sm text-slate-900 dark:text-slate-100">
																				<OptionValueBadges {field} value={record.values[field.key]} />
																			</dd>
																		</div>
																	{/each}
																</dl>
															</details>
														{/each}
													</div>
												</article>
											{/each}
										</div>
									{:else if selectedViewType === 'kanban'}
										<div class="overflow-x-auto p-4" data-view-renderer="kanban">
											<div class="mb-3 text-xs text-slate-500 dark:text-slate-400">
												{$t('forms.viewGroupBy', {
													values: { field: kanbanGroupField ? fieldLabel(kanbanGroupField) : $t('forms.recordTitle') }
												})}
											</div>
											{#if kanbanGroupField && kanbanMoveOptions(kanbanGroupField).length > 0}
												<div
													class="mb-3 flex min-w-[760px] flex-wrap items-end gap-2 rounded-md border border-slate-200 bg-white p-2 dark:border-slate-700 dark:bg-slate-900"
												>
													<div class="min-w-40 flex-1 text-xs text-slate-600 dark:text-slate-400">
														<span class="font-medium text-slate-900 dark:text-slate-100">
															{$t('forms.kanbanSelected', {
																values: { count: selectedKanbanRecords().length }
															})}
														</span>
													</div>
													<label class="min-w-56 space-y-1">
														<span class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															{$t('forms.kanbanBulkTarget', {
																values: { field: fieldLabel(kanbanGroupField) }
															})}
														</span>
														<select
															value={bulkKanbanTarget}
															onchange={(event) => (bulkKanbanTarget = selectValue(event))}
															class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
														>
															<option value="">{$t('forms.viewNoRule')}</option>
															{#each kanbanMoveOptions(kanbanGroupField) as option}
																<option value={option}>{option}</option>
															{/each}
														</select>
													</label>
													<Button
														size="sm"
														onclick={bulkMoveKanbanRecords}
														loading={bulkMovingKanban}
														disabled={
															selectedKanbanRecords().length === 0 ||
															!bulkKanbanTarget ||
															!kanbanBulkMoveAllowed(selectedKanbanRecords(), bulkKanbanTarget)
														}
													>
														{$t('forms.kanbanBulkMove')}
													</Button>
													<Button
														size="sm"
														variant="secondary"
														onclick={clearKanbanSelection}
														disabled={selectedKanbanRecords().length === 0 || bulkMovingKanban}
													>
														{$t('forms.clearSelection')}
													</Button>
												</div>
											{/if}
												<div class="min-w-[760px] space-y-4">
													{#each kanbanSwimlaneRows() as swimlane}
														<section
															class={kanbanSwimlaneField
																? 'rounded-lg border border-slate-200 bg-slate-100/70 p-3 dark:border-slate-700 dark:bg-slate-950/40'
																: ''}
															data-kanban-swimlane={kanbanSwimlaneField ? swimlane.label : undefined}
															data-record-count={swimlane.records.length}
														>
															{#if kanbanSwimlaneField}
																<div class="mb-3 flex items-center justify-between gap-3">
																	<div class="min-w-0">
																		<div class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																			{$t('forms.viewSwimlaneBy', {
																				values: { field: fieldLabel(kanbanSwimlaneField) }
																			})}
																		</div>
																		<h4 class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
																			{swimlane.label}
																		</h4>
																	</div>
																	<span class="rounded bg-white px-2 py-0.5 text-xs text-slate-500 dark:bg-slate-900 dark:text-slate-400">
																		{$t('forms.groupRecordCount', { values: { count: swimlane.records.length } })}
																	</span>
																</div>
															{/if}
															<div class="grid gap-3 lg:grid-cols-3 xl:grid-cols-4">
																{#each swimlane.groups as group}
													<section
														class={`rounded-lg border bg-slate-50 transition-colors dark:bg-slate-950/60 ${
															kanbanColumnWipExceeded(group.label)
																? 'border-amber-300 dark:border-amber-700'
																: 'border-slate-200 dark:border-slate-700'
														}`}
														data-record-count={group.records.length}
														data-kanban-column={group.label}
														data-wip-limited={kanbanColumnWipExceeded(group.label)}
														role="list"
														ondragover={(event) => handleKanbanDragOver(event, group.label)}
														ondrop={(event) => handleKanbanDrop(event, group.label)}
													>
														<div
															class="flex items-center justify-between gap-3 border-b border-slate-200 px-3 py-2 dark:border-slate-700"
														>
															<h4 class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
																{group.label}
																</h4>
																<span
																	class={`rounded px-2 py-0.5 text-xs ${
																		kanbanColumnWipExceeded(group.label)
																			? 'bg-amber-100 text-amber-800 dark:bg-amber-950/60 dark:text-amber-200'
																			: 'bg-white text-slate-500 dark:bg-slate-900 dark:text-slate-400'
																	}`}
																>
																	{kanbanWipLabel(group.label, kanbanColumnRecordCount(group.label))}
																</span>
															</div>
														<div class="space-y-2 p-2">
															{#each group.records as record}
																<article
																	class={`rounded-md border border-slate-200 bg-white p-3 shadow-sm transition-opacity dark:border-slate-700 dark:bg-slate-900 ${
																		draggingKanbanRecordId === record.id ? 'opacity-60' : ''
																	}`}
																	draggable={Boolean(kanbanGroupField)}
																	data-kanban-card={record.id}
																	role="listitem"
																	ondragstart={(event) => handleKanbanDragStart(event, record)}
																	ondragend={handleKanbanDragEnd}
																>
																	<div class="flex items-start justify-between gap-2">
																		<div class="flex min-w-0 items-start gap-2">
																			<input
																				type="checkbox"
																				class="mt-0.5 h-4 w-4 rounded border-slate-300 text-blue-600"
																				checked={isKanbanRecordSelected(record.id)}
																				disabled={bulkMovingKanban}
																				onchange={(event) =>
																					setKanbanRecordSelected(
																						record.id,
																						event.currentTarget instanceof HTMLInputElement
																							? event.currentTarget.checked
																							: false
																					)}
																				aria-label={`${$t('forms.kanbanSelectCard')}: ${record.title}`}
																			/>
																			<button
																				type="button"
																				class="min-w-0 text-left text-sm font-medium text-blue-700 dark:text-blue-300"
																				onclick={() => selectRecord(record)}
																				aria-label={`${$t('forms.openRecord')}: ${record.title}`}
																			>
																				<span class="block truncate">{record.title}</span>
																			</button>
																		</div>
																		<button
																			type="button"
																			class="inline-flex shrink-0 items-center rounded-md p-1 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																			disabled={deletingRecordId === record.id}
																			onclick={() => handleDeleteRecord(record)}
																			aria-label={$t('forms.deleteRecord')}
																		>
																			<Trash2 class="h-3.5 w-3.5" aria-hidden="true" />
																		</button>
																	</div>
																	<div class="mt-2 space-y-1">
																		{#each cardFields.slice(0, 3) as field}
																			<p class="truncate text-xs text-slate-600 dark:text-slate-400">
																				<span class="inline-flex items-center gap-1 font-medium">
																					<span>{fieldLabel(field)}:</span>
																					{#if memberFieldHidden(field)}
																						<span
																							class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																							data-kanban-card-field-permission-hidden={`${record.id}:${field.key}`}
																						>
																							{$t('forms.fieldHiddenBadge')}
																						</span>
																					{/if}
																					{#if memberFieldLocked(field)}
																						<span
																							class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																							data-kanban-card-field-permission-locked={`${record.id}:${field.key}`}
																						>
																							{$t('forms.fieldLockedBadge')}
																						</span>
																					{/if}
																					{#if recordFieldHiddenByCondition(field, record)}
																						<span
																							class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																							data-kanban-card-field-conditional-hidden={`${record.id}:${field.key}`}
																						>
																							{$t('forms.fieldConditionHiddenBadge')}
																						</span>
																					{/if}
																					{#if recordFieldReadOnlyByCondition(field, record)}
																						<span
																							class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																							data-kanban-card-field-conditional-readonly={`${record.id}:${field.key}`}
																						>
																							{$t('forms.fieldConditionReadOnlyBadge')}
																						</span>
																					{/if}
																				</span>
																				<OptionValueBadges {field} value={record.values[field.key]} />
																			</p>
																		{/each}
																	</div>
																	{#if kanbanGroupField && kanbanMoveOptions(kanbanGroupField).length > 0}
																		<select
																			value={String(record.values[kanbanGroupField.key] ?? '')}
																			disabled={updatingKanbanRecordId === record.id}
																			aria-label={`${$t('forms.kanbanMoveTo')} ${fieldLabel(kanbanGroupField)}`}
																			onchange={(event) =>
																				updateKanbanRecordField(
																					record,
																					kanbanGroupField,
																					selectValue(event)
																				)}
																			class="mt-3 w-full rounded-md border border-slate-200 bg-slate-50 px-2 py-1.5 text-xs text-slate-700 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200"
																		>
																			{#each kanbanMoveOptions(kanbanGroupField) as option}
																				<option value={option}>{option}</option>
																			{/each}
																		</select>
																	{/if}
																</article>
															{/each}
														</div>
														</section>
																{/each}
															</div>
														</section>
													{/each}
												</div>
										</div>
									{:else if selectedViewType === 'calendar'}
										<div class="overflow-x-auto p-4" data-view-renderer="calendar">
											<div class="mb-3 flex flex-wrap items-center gap-1.5 text-xs text-slate-500 dark:text-slate-400">
												{$t('forms.viewDateBy', {
													values: { field: calendarDateField ? fieldLabel(calendarDateField) : $t('forms.updated') }
												})}
												{#if calendarDateField && memberFieldHidden(calendarDateField)}
													<span
														class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
														data-calendar-date-field-permission-hidden={calendarDateField.key}
													>
														{$t('forms.fieldHiddenBadge')}
													</span>
												{/if}
												{#if calendarDateField && memberFieldLocked(calendarDateField)}
													<span
														class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
														data-calendar-date-field-permission-locked={calendarDateField.key}
													>
														{$t('forms.fieldLockedBadge')}
													</span>
												{/if}
												{#if calendarDateField && fieldHasConditionalHidden(calendarDateField)}
													<span
														class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
														data-calendar-date-field-conditional-hidden={calendarDateField.key}
													>
														{$t('forms.fieldConditionHiddenBadge')}
													</span>
												{/if}
												{#if calendarDateField && fieldHasConditionalReadOnly(calendarDateField)}
													<span
														class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
														data-calendar-date-field-conditional-readonly={calendarDateField.key}
													>
														{$t('forms.fieldConditionReadOnlyBadge')}
													</span>
												{/if}
											</div>
											<div
												class="min-w-[980px] space-y-4"
													data-calendar-range={viewCalendarRange(selectedView?.config?.calendar_range)}
													data-calendar-day-scope={viewCalendarDayScope(selectedView?.config?.calendar_day_scope)}
													data-calendar-layout={selectedCalendarLayout}
													data-calendar-hide-empty-days={selectedCalendarHideEmptyDays}
													data-calendar-focus-date={viewCalendarFocusDate()}
												>
												{#each calendarResourceRows() as resource}
													<section
														class={calendarResourceField
															? 'rounded-lg border border-slate-200 bg-slate-100/70 p-3 dark:border-slate-700 dark:bg-slate-950/40'
															: ''}
														data-calendar-resource={calendarResourceField ? resource.label : undefined}
														data-record-count={resource.records.length}
													>
														{#if calendarResourceField}
															<div class="mb-3 flex items-center justify-between gap-3">
																<div class="min-w-0">
																	<div class="flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																		{$t('forms.viewCalendarResourceBy', {
																			values: { field: fieldLabel(calendarResourceField) }
																		})}
																		{#if memberFieldHidden(calendarResourceField)}
																			<span
																				class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																				data-calendar-resource-field-permission-hidden={calendarResourceField.key}
																			>
																				{$t('forms.fieldHiddenBadge')}
																			</span>
																		{/if}
																		{#if memberFieldLocked(calendarResourceField)}
																			<span
																				class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																				data-calendar-resource-field-permission-locked={calendarResourceField.key}
																			>
																				{$t('forms.fieldLockedBadge')}
																			</span>
																		{/if}
																		{#if fieldHasConditionalHidden(calendarResourceField)}
																			<span
																				class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																				data-calendar-resource-field-conditional-hidden={calendarResourceField.key}
																			>
																				{$t('forms.fieldConditionHiddenBadge')}
																			</span>
																		{/if}
																		{#if fieldHasConditionalReadOnly(calendarResourceField)}
																			<span
																				class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																				data-calendar-resource-field-conditional-readonly={calendarResourceField.key}
																			>
																				{$t('forms.fieldConditionReadOnlyBadge')}
																			</span>
																		{/if}
																	</div>
																	<h4 class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
																		{resource.label}
																	</h4>
																</div>
																<span class="rounded bg-white px-2 py-0.5 text-xs text-slate-500 dark:bg-slate-900 dark:text-slate-400">
																	{$t('forms.groupRecordCount', { values: { count: resource.records.length } })}
																</span>
															</div>
														{/if}
														{#if selectedCalendarLayout === 'agenda'}
															<div
																class="space-y-3"
																data-calendar-agenda-resource={calendarResourceField ? resource.label : '__all__'}
																data-record-count={calendarAgendaRecordCount(resource)}
															>
																{#each calendarAgendaGroups(resource) as group}
																	<section
																		class="rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900"
																		data-calendar-agenda-date={group.date}
																		data-record-count={group.records.length}
																	>
																		<div class="flex items-center justify-between gap-2 border-b border-slate-200 px-3 py-2 dark:border-slate-700">
																			<h4 class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
																				{group.label}
																			</h4>
																			<div class="flex shrink-0 items-center gap-1">
																				<button
																					type="button"
																					class="rounded-md p-1.5 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																					onclick={() => applyCalendarBucketFilters(group.date, resource)}
																					disabled={!canApplyCalendarBucketFilters(group.date, resource)}
																					aria-label={$t('forms.applyCalendarBucketFilters')}
																					title={$t('forms.applyCalendarBucketFilters')}
																					data-calendar-agenda-apply-filters={`${group.date}:${calendarResourceField ? resource.key : '__all__'}`}
																					data-calendar-filters-applied={appliedCalendarFilterKey ===
																					calendarBucketFilterKey(group.date, resource)
																						? 'true'
																						: undefined}
																				>
																					<Filter class="h-3.5 w-3.5" aria-hidden="true" />
																				</button>
																				<span class="rounded bg-slate-50 px-2 py-0.5 text-xs text-slate-500 dark:bg-slate-950 dark:text-slate-400">
																					{$t('forms.groupRecordCount', { values: { count: group.records.length } })}
																				</span>
																			</div>
																		</div>
																		<div class="divide-y divide-slate-100 dark:divide-slate-800">
																			{#each group.records as occurrence (occurrence.key)}
																				{@const record = occurrence.record}
																				<article
																					class="grid gap-3 px-3 py-3 sm:grid-cols-[8rem_minmax(0,1fr)_auto]"
																					data-calendar-agenda-occurrence={occurrence.spanStart ? `${record.id}:${occurrence.startDate}` : occurrence.key}
																					data-calendar-card={record.id}
																					data-calendar-recurring={occurrence.recurring}
																					data-calendar-start={occurrence.startDate}
																					data-calendar-end={occurrence.endDate}
																					data-calendar-span-days={occurrence.spanDays}
																					data-calendar-schedule-copied={copiedCalendarScheduleKey ===
																					calendarScheduleKey(occurrence)
																						? 'true'
																						: undefined}
																					data-calendar-shifted={shiftedCalendarScheduleKey === calendarScheduleKey(occurrence) ? 'true' : undefined}
																				>
																					<div class="min-w-0 text-xs font-medium text-slate-500 dark:text-slate-400">
																						<div class="truncate">{occurrence.startDate}</div>
																						{#if occurrence.spanDays > 1}
																							<div class="mt-1 truncate text-emerald-700 dark:text-emerald-300">
																								{$t('forms.calendarDateSpan', {
																									values: {
																										start: occurrence.startDate,
																										end: occurrence.endDate
																									}
																								})}
																							</div>
																						{/if}
																					</div>
																					<button
																						type="button"
																						class="min-w-0 text-left"
																						onclick={() => selectRecord(record)}
																						aria-label={`${$t('forms.openRecord')}: ${record.title}`}
																					>
																						<span class="block truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
																							{record.title}
																						</span>
																						<span class="mt-0.5 block truncate text-xs text-slate-500 dark:text-slate-400">
																							{visibleFields[0] ? recordDisplay(record, visibleFields[0]) : record.id.slice(0, 8)}
																						</span>
																						{#if visibleFields[0] && recordFieldHiddenByCondition(visibleFields[0], record)}
																							<span
																								class="mt-1 inline-flex rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																								data-calendar-summary-conditional-hidden={`${record.id}:${visibleFields[0].key}`}
																							>
																								{$t('forms.fieldConditionHiddenBadge')}
																							</span>
																						{/if}
																						{#if visibleFields[0] && recordFieldReadOnlyByCondition(visibleFields[0], record)}
																							<span
																								class="mt-1 inline-flex rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																								data-calendar-summary-conditional-readonly={`${record.id}:${visibleFields[0].key}`}
																							>
																								{$t('forms.fieldConditionReadOnlyBadge')}
																							</span>
																						{/if}
																						{#if occurrence.recurring}
																							<span class="mt-2 inline-flex rounded bg-blue-50 px-2 py-0.5 text-xs font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200">
																								{$t('forms.calendarRecurringOccurrence')}
																							</span>
																						{/if}
																					</button>
																					<div class="flex items-center justify-end gap-1">
																						<button
																							type="button"
																							class="inline-flex h-8 w-8 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																							onclick={() => shiftCalendarOccurrence(occurrence, -1)}
																							disabled={updatingCalendarRecordId === record.id || !canShiftCalendarOccurrence(occurrence)}
																							aria-label={$t('forms.calendarShiftPreviousDay')}
																							title={$t('forms.calendarShiftPreviousDay')}
																							data-calendar-shift-previous={calendarScheduleKey(occurrence)}
																						>
																							<ChevronLeft class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																						<button
																							type="button"
																							class="inline-flex h-8 w-8 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																							onclick={() => shiftCalendarOccurrence(occurrence, 1)}
																							disabled={updatingCalendarRecordId === record.id || !canShiftCalendarOccurrence(occurrence)}
																							aria-label={$t('forms.calendarShiftNextDay')}
																							title={$t('forms.calendarShiftNextDay')}
																							data-calendar-shift-next={calendarScheduleKey(occurrence)}
																						>
																							<ChevronRight class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																						<button
																							type="button"
																							class="inline-flex h-8 w-8 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																							onclick={() => copyCalendarSchedule(occurrence)}
																							aria-label={$t('forms.copyCalendarSchedule')}
																							title={$t('forms.copyCalendarSchedule')}
																							data-calendar-copy-schedule={calendarScheduleKey(occurrence)}
																						>
																							<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																						<button
																							type="button"
																							class="inline-flex h-8 w-8 items-center justify-center rounded-md text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																							disabled={deletingRecordId === record.id}
																							onclick={() => handleDeleteRecord(record)}
																							aria-label={$t('forms.deleteRecord')}
																						>
																							<Trash2 class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																					</div>
																				</article>
																			{/each}
																		</div>
																	</section>
																{/each}
															</div>
														{:else}
														<div
															class={`grid gap-3 ${
																viewCalendarRange(selectedView?.config?.calendar_range) === 'month'
																	? 'grid-cols-2 md:grid-cols-4 xl:grid-cols-7'
																	: 'lg:grid-cols-7'
															}`}
														>
															{#each resource.groups as group}
																<section
																	class="min-h-48 rounded-lg border border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-950/60"
																	data-calendar-date={group.date}
																	data-record-count={group.records.length}
																	role="list"
																	ondragover={handleCalendarDragOver}
																	ondrop={(event) => handleCalendarDrop(event, group.date)}
																>
																	<div class="flex items-center justify-between gap-2 border-b border-slate-200 px-3 py-2 dark:border-slate-700">
																		<h4 class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
																			{group.label}
																		</h4>
																		<div class="flex shrink-0 items-center gap-1">
																			<button
																				type="button"
																				class="rounded-md p-1.5 text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																				onclick={() => applyCalendarBucketFilters(group.date, resource)}
																				disabled={!canApplyCalendarBucketFilters(group.date, resource)}
																				aria-label={$t('forms.applyCalendarBucketFilters')}
																				title={$t('forms.applyCalendarBucketFilters')}
																				data-calendar-apply-filters={`${group.date}:${calendarResourceField ? resource.key : '__all__'}`}
																				data-calendar-filters-applied={appliedCalendarFilterKey ===
																				calendarBucketFilterKey(group.date, resource)
																					? 'true'
																					: undefined}
																			>
																				<Filter class="h-3.5 w-3.5" aria-hidden="true" />
																			</button>
																			<span class="rounded bg-white px-2 py-0.5 text-xs text-slate-500 dark:bg-slate-900 dark:text-slate-400">
																				{$t('forms.groupRecordCount', { values: { count: group.records.length } })}
																			</span>
																		</div>
																	</div>
																	<div class="space-y-2 p-2">
																		{#each group.records as occurrence (occurrence.key)}
																			{@const record = occurrence.record}
																			<article
																				class={`rounded-md border border-slate-200 bg-white p-3 shadow-sm transition-opacity dark:border-slate-700 dark:bg-slate-900 ${
																					draggingCalendarRecordId === record.id ? 'opacity-60' : ''
																				}`}
																				draggable={Boolean(calendarDateField) && !occurrence.recurring}
																				data-calendar-card={record.id}
																				data-calendar-occurrence={occurrence.spanStart ? `${record.id}:${occurrence.startDate}` : occurrence.key}
																				data-calendar-recurring={occurrence.recurring}
																				data-calendar-start={occurrence.startDate}
																				data-calendar-end={occurrence.endDate}
																				data-calendar-span-start={occurrence.spanStart}
																				data-calendar-span-end={occurrence.spanEnd}
																				data-calendar-span-days={occurrence.spanDays}
																				data-calendar-schedule-copied={copiedCalendarScheduleKey ===
																				calendarScheduleKey(occurrence)
																					? 'true'
																					: undefined}
																				data-calendar-shifted={shiftedCalendarScheduleKey === calendarScheduleKey(occurrence) ? 'true' : undefined}
																				role="listitem"
																				ondragstart={(event) => handleCalendarDragStart(event, record)}
																				ondragend={handleCalendarDragEnd}
																			>
																				<div class="flex items-start justify-between gap-2">
																					<button
																						type="button"
																						class="min-w-0 text-left"
																						onclick={() => selectRecord(record)}
																						aria-label={`${$t('forms.openRecord')}: ${record.title}`}
																					>
																						<span class="block truncate text-sm font-medium text-blue-700 dark:text-blue-300">
																							{record.title}
																						</span>
																						<span class="mt-0.5 block truncate text-xs text-slate-500 dark:text-slate-400">
																							{visibleFields[0] ? recordDisplay(record, visibleFields[0]) : record.id.slice(0, 8)}
																						</span>
																						{#if visibleFields[0] && recordFieldHiddenByCondition(visibleFields[0], record)}
																							<span
																								class="mt-1 inline-flex rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																								data-calendar-summary-conditional-hidden={`${record.id}:${visibleFields[0].key}`}
																							>
																								{$t('forms.fieldConditionHiddenBadge')}
																							</span>
																						{/if}
																						{#if visibleFields[0] && recordFieldReadOnlyByCondition(visibleFields[0], record)}
																							<span
																								class="mt-1 inline-flex rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																								data-calendar-summary-conditional-readonly={`${record.id}:${visibleFields[0].key}`}
																							>
																								{$t('forms.fieldConditionReadOnlyBadge')}
																							</span>
																						{/if}
																					</button>
																					<div class="flex shrink-0 items-center gap-1">
																						<button
																							type="button"
																							class="inline-flex items-center rounded-md p-1 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																							onclick={() => shiftCalendarOccurrence(occurrence, -1)}
																							disabled={updatingCalendarRecordId === record.id || !canShiftCalendarOccurrence(occurrence)}
																							aria-label={$t('forms.calendarShiftPreviousDay')}
																							title={$t('forms.calendarShiftPreviousDay')}
																							data-calendar-shift-previous={calendarScheduleKey(occurrence)}
																						>
																							<ChevronLeft class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																						<button
																							type="button"
																							class="inline-flex items-center rounded-md p-1 text-slate-500 hover:bg-slate-100 hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																							onclick={() => shiftCalendarOccurrence(occurrence, 1)}
																							disabled={updatingCalendarRecordId === record.id || !canShiftCalendarOccurrence(occurrence)}
																							aria-label={$t('forms.calendarShiftNextDay')}
																							title={$t('forms.calendarShiftNextDay')}
																							data-calendar-shift-next={calendarScheduleKey(occurrence)}
																						>
																							<ChevronRight class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																						<button
																							type="button"
																							class="inline-flex items-center rounded-md p-1 text-slate-500 hover:bg-slate-100 hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																							onclick={() => copyCalendarSchedule(occurrence)}
																							aria-label={$t('forms.copyCalendarSchedule')}
																							title={$t('forms.copyCalendarSchedule')}
																							data-calendar-copy-schedule={calendarScheduleKey(occurrence)}
																						>
																							<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																						<button
																							type="button"
																							class="inline-flex items-center rounded-md p-1 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																							disabled={deletingRecordId === record.id}
																							onclick={() => handleDeleteRecord(record)}
																							aria-label={$t('forms.deleteRecord')}
																						>
																							<Trash2 class="h-3.5 w-3.5" aria-hidden="true" />
																						</button>
																					</div>
																				</div>
																				{#if occurrence.recurring}
																					<div class="mt-3 space-y-2">
																						<span class="inline-flex rounded bg-blue-50 px-2 py-0.5 text-xs font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200">
																							{$t('forms.calendarRecurringOccurrence')}
																						</span>
																						{#if calendarDateField && ['date', 'datetime'].includes(calendarDateField.type ?? 'text')}
																							<label class="block space-y-1">
																								<span class="text-[11px] font-medium uppercase text-slate-500 dark:text-slate-400">
																									{$t('forms.calendarSeriesDate')}
																								</span>
																								<input
																									type="date"
																									value={calendarDateInputValue(record, calendarDateField)}
																									disabled={updatingCalendarRecordId === record.id}
																									aria-label={`${$t('forms.calendarSeriesDate')} ${record.title}`}
																									data-calendar-series-date={`${record.id}:${occurrence.date}`}
																									onchange={(event) =>
																										updateCalendarSeriesDate(record, calendarDateField, inputValue(event))}
																									class="w-full rounded-md border border-blue-200 bg-blue-50 px-2 py-1.5 text-xs text-blue-900 disabled:opacity-50 dark:border-blue-900/70 dark:bg-blue-950/30 dark:text-blue-100"
																								/>
																							</label>
																						{/if}
																					</div>
																				{/if}
																				{#if occurrence.spanDays > 1}
																					<div class="mt-3 inline-flex max-w-full items-center gap-1 rounded bg-emerald-50 px-2 py-0.5 text-xs font-medium text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-200">
																						<span class="truncate">
																							{$t('forms.calendarDateSpan', {
																								values: {
																									start: occurrence.startDate,
																									end: occurrence.endDate
																								}
																							})}
																						</span>
																					</div>
																				{/if}
																				{#if calendarDateField && ['date', 'datetime'].includes(calendarDateField.type ?? 'text') && !occurrence.recurring}
																					<input
																						type="date"
																						value={calendarDateInputValue(record, calendarDateField)}
																						disabled={updatingCalendarRecordId === record.id}
																						aria-label={`${$t('forms.calendarSetDate')} ${fieldLabel(calendarDateField)}`}
																						onchange={(event) =>
																							updateCalendarRecordDate(record, calendarDateField, inputValue(event))}
																						class="mt-3 w-full rounded-md border border-slate-200 bg-slate-50 px-2 py-1.5 text-xs text-slate-700 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200"
																					/>
																				{/if}
																				{#if calendarDateField && calendarEndField && ['date', 'datetime'].includes(calendarEndField.type ?? 'text') && occurrence.startDate === calendarRecordIsoDate(record, calendarDateField)}
																					<label class="mt-3 block space-y-1">
																						<span class="text-[11px] font-medium uppercase text-slate-500 dark:text-slate-400">
																							{$t('forms.calendarEndDate')}
																						</span>
																						<input
																							type="date"
																							value={calendarDateInputValue(record, calendarEndField)}
																							disabled={updatingCalendarRecordId === record.id}
																							aria-label={`${$t('forms.calendarEndDate')} ${record.title}`}
																							data-calendar-end-date={`${record.id}:${occurrence.startDate}`}
																							onchange={(event) =>
																								updateCalendarRecordEndDate(record, calendarEndField, inputValue(event))}
																							class="w-full rounded-md border border-emerald-200 bg-emerald-50 px-2 py-1.5 text-xs text-emerald-900 disabled:opacity-50 dark:border-emerald-900/70 dark:bg-emerald-950/30 dark:text-emerald-100"
																						/>
																					</label>
																				{/if}
																			</article>
																		{/each}
																	</div>
																</section>
															{/each}
														</div>
														{/if}
													</section>
												{/each}
											</div>
										</div>
										{:else if selectedViewType === 'card'}
											{#snippet cardRecord(record: FormRecord)}
												{@const badgeField = viewCardBadgeField()}
												{@const badge = cardBadge(record)}
												{@const coverUrl = cardCoverUrl(record)}
												<article
													class={selectedCardLayout === 'gallery'
														? 'overflow-hidden rounded-lg border border-slate-200 bg-white shadow-sm ring-1 ring-transparent transition-shadow hover:shadow-md dark:border-slate-700 dark:bg-slate-900'
														: selectedCardLayout === 'compact' || selectedCardLayout === 'list'
															? 'overflow-visible rounded-md border border-slate-200 bg-white shadow-sm dark:border-slate-700 dark:bg-slate-900'
													: 'overflow-hidden rounded-lg border border-slate-200 bg-white shadow-sm dark:border-slate-700 dark:bg-slate-900'}
													data-card-copied={copiedCardRecordId === record.id ? record.id : undefined}
													data-card-exported={exportedCardRecordId === record.id ? record.id : undefined}
													data-card-compact-row={selectedCardLayout === 'compact' ? record.id : undefined}
													data-card-list-row={selectedCardLayout === 'list' ? record.id : undefined}
													data-card-cover-aspect={selectedCardCoverAspect}
													data-card-cover-fit={selectedCardCoverFit}
													data-card-cover-position={selectedCardCoverPosition}
												>
													{#if coverUrl && selectedCardLayout !== 'compact' && selectedCardLayout !== 'list' && selectedCardCoverPosition === 'top'}
														<img
															src={coverUrl}
															alt={cardTitle(record)}
															class={cardCoverImageClass(
																selectedCardLayout,
																selectedCardCoverAspect,
																selectedCardCoverFit
															)}
															data-card-cover={record.id}
															data-card-cover-aspect={selectedCardCoverAspect}
															data-card-cover-fit={selectedCardCoverFit}
															data-card-cover-position={selectedCardCoverPosition}
														/>
													{/if}
													<div
														class={selectedCardLayout === 'compact' || selectedCardLayout === 'list'
															? 'flex items-center justify-between gap-3 px-3 py-2'
															: selectedCardCoverPosition === 'left' && coverUrl
																? 'grid grid-cols-[7rem_minmax(0,1fr)_auto] items-start gap-3 p-4 pb-0'
															: 'flex items-start justify-between gap-3 p-4 pb-0'}
													>
														{#if coverUrl && selectedCardLayout !== 'compact' && selectedCardLayout !== 'list' && selectedCardCoverPosition === 'left'}
															<img
																src={coverUrl}
																alt={cardTitle(record)}
																class={`h-24 w-28 shrink-0 rounded-md ${cardCoverObjectClass(selectedCardCoverFit)}`}
																data-card-cover={record.id}
																data-card-cover-aspect={selectedCardCoverAspect}
																data-card-cover-fit={selectedCardCoverFit}
																data-card-cover-position={selectedCardCoverPosition}
															/>
														{/if}
														{#if coverUrl && selectedCardLayout === 'compact' && selectedCardCoverPosition !== 'hidden'}
															<img
																src={coverUrl}
																alt={cardTitle(record)}
																class={`h-12 w-12 shrink-0 rounded-md ${cardCoverObjectClass(selectedCardCoverFit)}`}
															data-card-cover={record.id}
															data-card-cover-aspect={selectedCardCoverAspect}
															data-card-cover-fit={selectedCardCoverFit}
															data-card-cover-position={selectedCardCoverPosition}
														/>
													{/if}
														<button
															type="button"
															class="min-w-0 text-left"
															onclick={() => selectRecord(record)}
															aria-label={`${$t('forms.openRecord')}: ${record.title}`}
														>
															<span
																class={selectedCardLayout === 'gallery'
																	? 'block truncate text-base font-semibold text-slate-950 dark:text-slate-100'
																	: selectedCardLayout === 'compact' || selectedCardLayout === 'list'
																		? 'block truncate text-sm font-semibold text-slate-950 dark:text-slate-100'
																		: 'block truncate font-medium text-blue-700 dark:text-blue-300'}
																data-card-title={record.id}
															>
																{cardTitle(record)}
															</span>
																<span
																	class="mt-1 block truncate text-xs text-slate-500 dark:text-slate-400"
																	data-card-subtitle={record.id}
																>
																	{cardSubtitle(record)}
																</span>
																{#if selectedCardShowTimestamps}
																	<span
																		class="mt-1 flex flex-wrap gap-x-2 gap-y-0.5 text-[11px] text-slate-400 dark:text-slate-500"
																		data-card-timestamps={record.id}
																	>
																		<span data-card-created-at={record.id}>
																			{$t('forms.cardCreatedAt')}: {formatDate(record.created_at)}
																		</span>
																		<span data-card-updated-at={record.id}>
																			{$t('forms.cardUpdatedAt')}: {formatDate(record.updated_at)}
																		</span>
																	</span>
																{/if}
																{#if selectedCardShowRecordId}
																	<span
																		class="mt-1 block truncate font-mono text-[11px] text-slate-400 dark:text-slate-500"
																		data-card-record-id={record.id}
																	>
																		{$t('forms.cardRecordId')}: {record.id}
																	</span>
																{/if}
															</button>
														<div class="flex shrink-0 items-start gap-2">
															{#if badge}
																<span
																	class="max-w-32 truncate"
																	data-card-badge={record.id}
																>
																	<OptionValueBadges field={badgeField} value={badge} />
																</span>
															{/if}
															<details class="group relative shrink-0" data-card-menu={record.id}>
																<summary
																	class="inline-flex cursor-pointer list-none items-center rounded-md p-1.5 text-slate-500 hover:bg-slate-100 hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																	aria-label={$t('forms.cardActions')}
																>
																	<MoreVertical class="h-4 w-4" aria-hidden="true" />
																</summary>
																<div class="absolute right-0 z-20 mt-2 w-40 overflow-hidden rounded-md border border-slate-200 bg-white py-1 text-sm shadow-lg dark:border-slate-700 dark:bg-slate-900">
																	<button
																		type="button"
																		class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																		data-card-action={`${record.id}:open`}
																		onclick={() => selectRecord(record)}
																	>
																		<FileText class="h-4 w-4" aria-hidden="true" />
																		{$t('forms.openRecord')}
																	</button>
																	<button
																		type="button"
																		class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																		data-card-action={`${record.id}:copy-id`}
																		onclick={() => copyRecordId(record)}
																	>
																		<Copy class="h-4 w-4" aria-hidden="true" />
																		{$t('forms.copyRecordId')}
																	</button>
																	<button
																		type="button"
																		class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																		data-card-action={`${record.id}:export-record`}
																		onclick={() => exportCardRecordDetails(record)}
																	>
																		<FileDown class="h-4 w-4" aria-hidden="true" />
																		{$t('forms.exportRecord')}
																	</button>
																	<button
																		type="button"
																		class="flex w-full items-center gap-2 px-3 py-2 text-left text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																		disabled={deletingRecordId === record.id}
																		data-card-action={`${record.id}:delete`}
																		onclick={() => handleDeleteRecord(record)}
																	>
																		<Trash2 class="h-4 w-4" aria-hidden="true" />
																		{$t('forms.deleteRecord')}
																	</button>
																</div>
															</details>
														</div>
													</div>
													<dl
														class={selectedCardLayout === 'compact'
															? 'grid gap-x-4 gap-y-1 border-t border-slate-100 px-3 py-2 sm:grid-cols-2 lg:grid-cols-3 dark:border-slate-800'
															: selectedCardLayout === 'list'
																? 'grid gap-x-4 gap-y-1 border-t border-slate-100 px-3 py-2 sm:grid-cols-3 xl:grid-cols-5 dark:border-slate-800'
																: 'mt-4 space-y-2 px-4'}
													>
														{#each cardDisplayFields(record) as field}
															<div
																class={selectedCardLayout === 'compact' || selectedCardLayout === 'list'
																	? 'min-w-0 text-xs'
																	: 'grid grid-cols-[7rem_minmax(0,1fr)] gap-3 text-sm'}
																data-card-field={`${record.id}:${field.key}`}
															>
																<dt
																	class={selectedCardLayout === 'compact' || selectedCardLayout === 'list'
																		? 'flex min-w-0 flex-wrap items-center gap-1 text-[10px] font-medium uppercase text-slate-500 dark:text-slate-400'
																		: 'flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-slate-500 dark:text-slate-400'}
																>
																	<span class="min-w-0 truncate">{fieldLabel(field)}</span>
																	{#if memberFieldHidden(field)}
																		<span
																			class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																			data-card-field-permission-hidden={`${record.id}:${field.key}`}
																		>
																			{$t('forms.fieldHiddenBadge')}
																		</span>
																	{/if}
																	{#if memberFieldLocked(field)}
																		<span
																			class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																			data-card-field-permission-locked={`${record.id}:${field.key}`}
																		>
																			{$t('forms.fieldLockedBadge')}
																		</span>
																	{/if}
																	{#if recordFieldHiddenByCondition(field, record)}
																		<span
																			class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																			data-card-field-conditional-hidden={`${record.id}:${field.key}`}
																		>
																			{$t('forms.fieldConditionHiddenBadge')}
																		</span>
																	{/if}
																	{#if recordFieldReadOnlyByCondition(field, record)}
																		<span
																			class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																			data-card-field-conditional-readonly={`${record.id}:${field.key}`}
																		>
																			{$t('forms.fieldConditionReadOnlyBadge')}
																		</span>
																	{/if}
																</dt>
																<dd class="truncate text-slate-900 dark:text-slate-100">
																	<OptionValueBadges {field} value={record.values[field.key]} />
																</dd>
															</div>
														{/each}
													</dl>
													{#if cardEditableSelectFields.length > 0}
														<div class="mt-4 space-y-2 border-t border-slate-100 px-4 py-3 dark:border-slate-800">
															{#each cardEditableSelectFields as field}
																<label class="block space-y-1">
																	<span class="flex flex-wrap items-center gap-1.5 text-xs font-medium text-slate-500 dark:text-slate-400">
																		<span>{fieldLabel(field)}</span>
																		{#if memberFieldHidden(field)}
																			<span
																				class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																				data-card-select-permission-hidden={`${record.id}:${field.key}`}
																			>
																				{$t('forms.fieldHiddenBadge')}
																			</span>
																		{/if}
																		{#if memberFieldLocked(field)}
																			<span
																				class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																				data-card-select-permission-locked={`${record.id}:${field.key}`}
																			>
																				{$t('forms.fieldLockedBadge')}
																			</span>
																		{/if}
																		{#if recordFieldHiddenByCondition(field, record)}
																			<span
																				class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																				data-card-select-conditional-hidden={`${record.id}:${field.key}`}
																			>
																				{$t('forms.fieldConditionHiddenBadge')}
																			</span>
																		{/if}
																		{#if recordFieldReadOnlyByCondition(field, record)}
																			<span
																				class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																				data-card-select-conditional-readonly={`${record.id}:${field.key}`}
																			>
																				{$t('forms.fieldConditionReadOnlyBadge')}
																			</span>
																		{/if}
																	</span>
																	<select
																		value={String(record.values[field.key] ?? '')}
																		disabled={updatingCardRecordId === record.id}
																		aria-label={`${$t('forms.cardUpdateField')} ${fieldLabel(field)}`}
																		data-card-select={`${record.id}:${field.key}`}
																		onchange={(event) =>
																			updateCardRecordSelectField(record, field, selectValue(event))}
																		class="w-full rounded-md border border-slate-200 bg-slate-50 px-2 py-1.5 text-xs text-slate-700 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200"
																	>
																		{#each kanbanMoveOptions(field) as option}
																			<option value={option}>{option}</option>
																		{/each}
																	</select>
																</label>
															{/each}
														</div>
													{/if}
												</article>
											{/snippet}
											<div
												class={cardGroupField
													? 'space-y-4 p-4'
													: selectedCardLayout === 'gallery'
														? 'grid gap-4 p-4 md:grid-cols-2 2xl:grid-cols-4'
														: selectedCardLayout === 'compact' || selectedCardLayout === 'list'
															? 'space-y-2 p-4'
															: 'grid gap-3 p-4 md:grid-cols-2 xl:grid-cols-3'}
												data-view-renderer="card"
												data-card-layout={selectedCardLayout}
												data-card-field-limit={viewCardFieldLimit(selectedView?.config?.card_field_limit)}
												data-card-hide-empty-fields={selectedCardHideEmptyFields}
												data-card-show-record-id={selectedCardShowRecordId}
												data-card-group-field={cardGroupField?.key}
											>
													{#if cardGroupField}
														{#each cardGroups as group}
															{@const collapsed = cardGroupCollapsed(group)}
															<section
																class="rounded-lg border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-950/50"
																data-card-group={group.label}
																data-card-group-collapsed={collapsed ? 'true' : 'false'}
																data-record-count={group.records.length}
																data-card-group-record-ids-copied={copiedCardGroupRecordIdsKey ===
																cardGroupFilterKey(group)
																	? 'true'
																	: undefined}
																data-card-group-records-exported={exportedCardGroupRecordsKey ===
																cardGroupFilterKey(group)
																	? 'true'
																	: undefined}
															>
																<div class="mb-3 flex items-center justify-between gap-3">
																	<div class="min-w-0">
																	<div class="text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
																		{$t('forms.viewGroupBy', { values: { field: fieldLabel(cardGroupField) } })}
																	</div>
																	<h4 class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
																			{group.label}
																		</h4>
																	</div>
																	<div class="flex shrink-0 items-center gap-2">
																		<span class="rounded bg-white px-2 py-0.5 text-xs text-slate-500 dark:bg-slate-900 dark:text-slate-400">
																			{$t('forms.groupRecordCount', { values: { count: group.records.length } })}
																		</span>
																		<button
																			type="button"
																			class="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-900 dark:hover:text-slate-100"
																			aria-label={$t('forms.copyCardGroupRecordIds')}
																			title={$t('forms.copyCardGroupRecordIds')}
																			disabled={group.records.length === 0}
																			onclick={() => copyCardGroupRecordIds(group)}
																			data-card-group-copy-record-ids={`${cardGroupField?.key ?? '__none__'}:${group.label}`}
																		>
																			<Copy class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-900 dark:hover:text-slate-100"
																			aria-label={$t('forms.exportCardGroupRecords')}
																			title={$t('forms.exportCardGroupRecords')}
																			disabled={group.records.length === 0}
																			onclick={() => exportCardGroupRecords(group)}
																			data-card-group-export-records={`${cardGroupField?.key ?? '__none__'}:${group.label}`}
																		>
																			<FileDown class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-900 dark:hover:text-slate-100"
																			aria-label={$t('forms.focusCardGroupRecords')}
																			title={$t('forms.focusCardGroupRecords')}
																			disabled={group.records.length === 0}
																			onclick={() => focusCardGroupRecords(group)}
																			data-card-group-focus-records={`${cardGroupField?.key ?? '__none__'}:${group.label}`}
																		>
																			<TableProperties class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 hover:bg-white hover:text-slate-900 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-900 dark:hover:text-slate-100"
																			aria-label={$t('forms.applyCardGroupFilter')}
																			title={$t('forms.applyCardGroupFilter')}
																			disabled={!canApplyCardGroupFilter(group)}
																			onclick={() => applyCardGroupFilter(group)}
																			data-card-group-apply-filter={`${cardGroupField?.key ?? '__none__'}:${group.label}`}
																			data-card-group-filter-applied={appliedCardGroupFilterKey ===
																			cardGroupFilterKey(group)
																				? 'true'
																				: undefined}
																		>
																			<Filter class="h-4 w-4" aria-hidden="true" />
																		</button>
																		<button
																			type="button"
																			class="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 hover:bg-white hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-900 dark:hover:text-slate-100"
																			aria-label={collapsed ? $t('forms.expandCardGroup') : $t('forms.collapseCardGroup')}
																			onclick={() => toggleCardGroup(group)}
																			data-card-group-toggle={group.label}
																		>
																			<ArrowDown
																				class="h-4 w-4 transition-transform {collapsed ? '-rotate-90' : ''}"
																				aria-hidden="true"
																			/>
																		</button>
																	</div>
																</div>
																{#if collapsed}
																	<p class="rounded-md border border-dashed border-slate-200 bg-white px-3 py-2 text-xs text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400">
																		{$t('forms.cardGroupCollapsed', {
																			values: { count: group.records.length }
																		})}
																	</p>
																{:else}
																	<div
																		class={selectedCardLayout === 'gallery'
																			? 'grid gap-4 md:grid-cols-2 2xl:grid-cols-4'
																			: selectedCardLayout === 'compact' || selectedCardLayout === 'list'
																				? 'space-y-2'
																				: 'grid gap-3 md:grid-cols-2 xl:grid-cols-3'}
																	>
																		{#each group.records as record}
																			{@render cardRecord(record)}
																		{/each}
																	</div>
																{/if}
															</section>
														{/each}
												{:else}
													{#each viewRecords as record}
														{@render cardRecord(record)}
													{/each}
												{/if}
											</div>
									{:else}
										<div
											class="divide-y divide-slate-100 dark:divide-slate-800"
											data-view-renderer="timeline"
											data-timeline-source={selectedTimelineSource()}
											data-timeline-event-layout={selectedTimelineEventLayout()}
											data-timeline-event-type-filter={selectedTimelineEventType()}
											data-timeline-event-actor-filter={selectedTimelineEventActor()}
											data-timeline-event-aggregate-filter={selectedTimelineEventAggregate()}
												data-timeline-event-query={selectedTimelineEventQuery()}
												data-timeline-event-from={selectedTimelineEventFrom()}
												data-timeline-event-to={selectedTimelineEventTo()}
												data-timeline-event-pinned-ids={pinnedTimelineEventIds.join('|')}
												data-timeline-events-exported={exportedTimelineEventsCsvCount ?? undefined}
												data-timeline-events-json-exported={exportedTimelineEventsJsonCount ?? undefined}
												data-timeline-event-ids-copied={copiedTimelineEventIdsCount ?? undefined}
												data-timeline-aggregate-ids-copied={copiedTimelineAggregateIdsCount ?? undefined}
											>
											{#if selectedTimelineSource() === 'events'}
												{#if eventsLoading}
													<div class="px-4 py-6 text-sm text-slate-500 dark:text-slate-400">
														{$t('common.loading')}
													</div>
												{:else if timelineEvents.length === 0}
													<div class="px-4 py-6 text-sm text-slate-500 dark:text-slate-400">
														{$t('forms.noRecords')}
													</div>
												{:else}
													<div
														class="flex items-center justify-between gap-3 border-b border-slate-100 px-4 py-2 dark:border-slate-800"
														data-timeline-event-export-toolbar
													>
														<div class="min-w-0 text-xs text-slate-500 dark:text-slate-400">
															{$t('forms.groupRecordCount', { values: { count: timelineEventRows.length } })}
														</div>
														<div class="flex shrink-0 flex-wrap items-center justify-end gap-2">
															<button
																type="button"
																class="inline-flex items-center gap-2 rounded-md border border-slate-200 px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
																disabled={timelineEventRows.length === 0}
																onclick={() => copyTimelineEventIds(timelineEventRows)}
																data-timeline-copy-event-ids
															>
																<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																{$t('forms.copyTimelineEventIds')}
															</button>
															<button
																type="button"
																class="inline-flex items-center gap-2 rounded-md border border-slate-200 px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
																disabled={timelineEventAggregateKeys(timelineEventRows).length === 0}
																onclick={() => copyTimelineAggregateIds(timelineEventRows)}
																data-timeline-copy-aggregate-ids
															>
																<Link class="h-3.5 w-3.5" aria-hidden="true" />
																{$t('forms.copyTimelineAggregateIds')}
															</button>
															<button
																type="button"
																class="inline-flex items-center gap-2 rounded-md border border-slate-200 px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
																disabled={timelineEventLinkedRecordIds(timelineEventRows).length === 0}
																onclick={() => focusTimelineEventRecords(timelineEventRows)}
																data-timeline-focus-event-records
															>
																<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
																{$t('forms.focusTimelineEventRecords')}
															</button>
															<button
																type="button"
																class="inline-flex items-center gap-2 rounded-md border border-slate-200 px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
																disabled={timelineEventRows.length === 0}
																onclick={() => exportTimelineEventsJson(timelineEventRows)}
																data-timeline-export-events-json
															>
																<FileText class="h-3.5 w-3.5" aria-hidden="true" />
																{$t('forms.exportTimelineEventsJson')}
															</button>
															<button
																type="button"
																class="inline-flex items-center gap-2 rounded-md border border-slate-200 px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-40 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
																disabled={timelineEventRows.length === 0}
																onclick={() => exportTimelineEventsCsv(timelineEventRows)}
																data-timeline-export-events-csv
															>
																<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																{$t('forms.exportTimelineEventsCsv')}
															</button>
														</div>
													</div>
														{#each timelineEventRows as event}
															{@const linkedRecord = timelineEventRecord(event)}
															{@const pinned = timelineEventPinned(event)}
															<article
																class="grid grid-cols-[8rem_minmax(0,1fr)] gap-4 px-4 py-3"
																data-timeline-event={event.id}
																data-timeline-event-type={event.event_type}
																data-timeline-event-pinned={pinned ? 'true' : 'false'}
															>
															<div class="text-xs text-slate-500 dark:text-slate-400">
																{formatDate(event.created_at)}
															</div>
															<div class="min-w-0 border-l border-slate-200 pl-4 dark:border-slate-700">
																<div class="flex items-start justify-between gap-3">
																	<div class="min-w-0">
																		<span class="block truncate font-medium text-slate-900 dark:text-slate-100" data-timeline-event-title={event.id}>
																			{timelineEventTitle(event)}
																		</span>
																		<span class="mt-1 block truncate text-xs text-slate-500 dark:text-slate-400" data-timeline-event-subtitle={event.id}>
																			{timelineEventSubtitle(event)}
																		</span>
																	</div>
																	<div class="flex shrink-0 items-center gap-2">
																			<span class="rounded bg-slate-100 px-2 py-0.5 text-xs font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-300">
																				{event.event_type}
																			</span>
																			{#if pinned}
																				<span class="rounded bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-800 dark:bg-amber-950/50 dark:text-amber-200">
																					{$t('forms.timelineEventPinnedBadge')}
																				</span>
																			{/if}
																			<details
																			class="relative"
																			data-timeline-event-menu={event.id}
																			data-timeline-event-copied={copiedTimelineEventAction?.startsWith(`${event.id}:`) ? copiedTimelineEventAction : undefined}
																			data-timeline-event-exported={exportedTimelineEventAction?.startsWith(`${event.id}:`) ? exportedTimelineEventAction : undefined}
																		>
																			<summary
																				class="inline-flex cursor-pointer list-none items-center rounded-md p-1 text-slate-500 hover:bg-slate-100 hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
																				aria-label={$t('forms.timelineEventActions')}
																			>
																				<MoreVertical class="h-4 w-4" aria-hidden="true" />
																			</summary>
																			<div class="absolute right-0 z-20 mt-1 w-48 overflow-hidden rounded-md border border-slate-200 bg-white py-1 text-sm shadow-lg dark:border-slate-700 dark:bg-slate-900">
																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50 dark:text-slate-200 dark:hover:bg-slate-800"
																					disabled={!linkedRecord}
																					data-timeline-event-action={`${event.id}:open-record`}
																					onclick={() => {
																						if (linkedRecord) void selectRecord(linkedRecord);
																					}}
																				>
																					<Link class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.openLinkedRecord')}
																				</button>
																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50 dark:text-slate-200 dark:hover:bg-slate-800"
																					disabled={!linkedRecord}
																					data-timeline-event-action={`${event.id}:focus-record`}
																					onclick={() => focusTimelineEventRecord(event)}
																				>
																					<TableProperties class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.focusLinkedRecord')}
																				</button>
																					<button
																						type="button"
																						class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																						data-timeline-event-action={`${event.id}:${pinned ? 'unpin-event' : 'pin-event'}`}
																						onclick={() => toggleTimelineEventPinned(event)}
																					>
																						<ArrowUp class="h-4 w-4" aria-hidden="true" />
																						{pinned ? $t('forms.unpinTimelineEvent') : $t('forms.pinTimelineEvent')}
																					</button>
																					<button
																						type="button"
																						class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																						data-timeline-event-action={`${event.id}:filter-type`}
																						onclick={() => applyTimelineEventQuickFilter(event, 'type')}
																				>
																					<FileText class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.timelineQuickFilterType')}
																				</button>
																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50 dark:text-slate-200 dark:hover:bg-slate-800"
																					disabled={!event.actor_id}
																					data-timeline-event-action={`${event.id}:filter-actor`}
																					onclick={() => applyTimelineEventQuickFilter(event, 'actor')}
																				>
																					<Shield class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.timelineQuickFilterActor')}
																				</button>
																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																					data-timeline-event-action={`${event.id}:filter-aggregate`}
																					onclick={() => applyTimelineEventQuickFilter(event, 'aggregate')}
																				>
																					<Link class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.timelineQuickFilterAggregate')}
																				</button>
																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																					data-timeline-event-action={`${event.id}:filter-day`}
																					onclick={() => applyTimelineEventDayWindow(event)}
																				>
																					<Filter class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.timelineQuickFilterDay')}
																				</button>
																					<button
																						type="button"
																						class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																						data-timeline-event-action={`${event.id}:copy-event-id`}
																						onclick={() => copyTimelineEventValue(event, event.id, 'event-id')}
																				>
																						<Copy class="h-4 w-4" aria-hidden="true" />
																						{$t('forms.copyEventId')}
																					</button>
																					<button
																						type="button"
																						class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																						data-timeline-event-action={`${event.id}:copy-aggregate`}
																						onclick={() =>
																							copyTimelineEventValue(
																								event,
																								`${event.aggregate_type}:${event.aggregate_id}`,
																								'aggregate'
																							)}
																					>
																						<Copy class="h-4 w-4" aria-hidden="true" />
																						{$t('forms.copyAggregateId')}
																					</button>
																					<button
																						type="button"
																						class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																						data-timeline-event-action={`${event.id}:copy-details`}
																						onclick={() =>
																							copyTimelineEventValue(
																								event,
																								timelineEventDetailsJson(event),
																								'details'
																							)}
																					>
																						<Copy class="h-4 w-4" aria-hidden="true" />
																						{$t('forms.copyEventDetails')}
																					</button>
																					<button
																						type="button"
																						class="flex w-full items-center gap-2 px-3 py-2 text-left text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800"
																						data-timeline-event-action={`${event.id}:export-details`}
																						onclick={() => exportTimelineEventDetails(event)}
																					>
																						<FileDown class="h-4 w-4" aria-hidden="true" />
																						{$t('forms.exportEventDetails')}
																					</button>
																				</div>
																			</details>
																	</div>
																	</div>
																	<details
																		class="mt-3 rounded-md border border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-950/40"
																		data-timeline-event-detail={event.id}
																		open={selectedTimelineEventLayout() === 'detailed'}
																	>
																		<summary class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 text-xs font-medium text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800">
																			<span>{$t('forms.timelineEventDetails')}</span>
																			<span class="text-slate-400">{event.id}</span>
																		</summary>
																		<div class="space-y-3 border-t border-slate-200 p-3 text-xs dark:border-slate-700">
																			<div class="grid gap-2 sm:grid-cols-2">
																				<div>
																					<div class="font-medium text-slate-500 dark:text-slate-400">
																						{$t('forms.timelineEventAggregate')}
																					</div>
																					<div class="mt-0.5 break-all text-slate-900 dark:text-slate-100" data-timeline-event-aggregate={event.id}>
																						{event.aggregate_type}:{event.aggregate_id}
																					</div>
																				</div>
																				<div>
																					<div class="font-medium text-slate-500 dark:text-slate-400">
																						{$t('forms.timelineEventActor')}
																					</div>
																					<div class="mt-0.5 break-all text-slate-900 dark:text-slate-100" data-timeline-event-actor={event.id}>
																						{event.actor_id || '-'}
																					</div>
																				</div>
																			</div>
																			<div class="grid gap-3 lg:grid-cols-3">
																				<div>
																					<div class="mb-1 font-medium text-slate-500 dark:text-slate-400">
																						{$t('forms.timelineEventPayload')}
																					</div>
																					<pre class="max-h-44 overflow-auto rounded bg-white p-2 text-[11px] leading-relaxed text-slate-800 dark:bg-slate-900 dark:text-slate-100" data-timeline-event-payload={event.id}>{timelineEventJson(event.payload)}</pre>
																				</div>
																				<div>
																					<div class="mb-1 font-medium text-slate-500 dark:text-slate-400">
																						{$t('forms.timelineEventMetadata')}
																					</div>
																					<pre class="max-h-44 overflow-auto rounded bg-white p-2 text-[11px] leading-relaxed text-slate-800 dark:bg-slate-900 dark:text-slate-100" data-timeline-event-metadata={event.id}>{timelineEventJson(event.metadata)}</pre>
																				</div>
																				<div>
																					<div class="mb-1 font-medium text-slate-500 dark:text-slate-400">
																						{$t('forms.timelineEventSource')}
																					</div>
																					<pre class="max-h-44 overflow-auto rounded bg-white p-2 text-[11px] leading-relaxed text-slate-800 dark:bg-slate-900 dark:text-slate-100" data-timeline-event-source-detail={event.id}>{timelineEventJson(event.source)}</pre>
																				</div>
																			</div>
																		</div>
																	</details>
																</div>
															</article>
													{/each}
												{/if}
											{:else}
												{#each timelineRecords as record}
												<article class="grid grid-cols-[8rem_minmax(0,1fr)] gap-4 px-4 py-3">
													<div class="text-xs text-slate-500 dark:text-slate-400">
														{formatDate(record.updated_at)}
													</div>
													<div class="min-w-0 border-l border-slate-200 pl-4 dark:border-slate-700">
														<div class="flex items-start justify-between gap-3">
															<button
																type="button"
																class="min-w-0 text-left"
																onclick={() => selectRecord(record)}
																aria-label={`${$t('forms.openRecord')}: ${record.title}`}
															>
																	<span
																		class="block truncate font-medium text-blue-700 dark:text-blue-300"
																		data-timeline-title={record.id}
																	>
																		{timelineTitle(record)}
																	</span>
																	{#if timelineTitlePermissionField}
																		<span class="mt-1 flex flex-wrap items-center gap-1.5">
																			{#if memberFieldHidden(timelineTitlePermissionField)}
																				<span
																					class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																					data-timeline-title-permission-hidden={`${record.id}:${timelineTitlePermissionField.key}`}
																				>
																					{$t('forms.fieldHiddenBadge')}
																				</span>
																			{/if}
																			{#if memberFieldLocked(timelineTitlePermissionField)}
																				<span
																					class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																					data-timeline-title-permission-locked={`${record.id}:${timelineTitlePermissionField.key}`}
																				>
																					{$t('forms.fieldLockedBadge')}
																				</span>
																			{/if}
																			{#if recordFieldHiddenByCondition(timelineTitlePermissionField, record)}
																				<span
																					class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																					data-timeline-title-conditional-hidden={`${record.id}:${timelineTitlePermissionField.key}`}
																				>
																					{$t('forms.fieldConditionHiddenBadge')}
																				</span>
																			{/if}
																			{#if recordFieldReadOnlyByCondition(timelineTitlePermissionField, record)}
																				<span
																					class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																					data-timeline-title-conditional-readonly={`${record.id}:${timelineTitlePermissionField.key}`}
																				>
																					{$t('forms.fieldConditionReadOnlyBadge')}
																				</span>
																			{/if}
																		</span>
																	{/if}
																	<span
																		class="mt-1 block truncate text-xs text-slate-500 dark:text-slate-400"
																		data-timeline-subtitle={record.id}
																	>
																		{timelineSubtitle(record)}
																	</span>
																	{#if timelineSubtitlePermissionField}
																		<span class="mt-1 flex flex-wrap items-center gap-1.5">
																			{#if memberFieldHidden(timelineSubtitlePermissionField)}
																				<span
																					class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																					data-timeline-subtitle-permission-hidden={`${record.id}:${timelineSubtitlePermissionField.key}`}
																				>
																					{$t('forms.fieldHiddenBadge')}
																				</span>
																			{/if}
																			{#if memberFieldLocked(timelineSubtitlePermissionField)}
																				<span
																					class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																					data-timeline-subtitle-permission-locked={`${record.id}:${timelineSubtitlePermissionField.key}`}
																				>
																					{$t('forms.fieldLockedBadge')}
																				</span>
																			{/if}
																			{#if recordFieldHiddenByCondition(timelineSubtitlePermissionField, record)}
																				<span
																					class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																					data-timeline-subtitle-conditional-hidden={`${record.id}:${timelineSubtitlePermissionField.key}`}
																				>
																					{$t('forms.fieldConditionHiddenBadge')}
																				</span>
																			{/if}
																			{#if recordFieldReadOnlyByCondition(timelineSubtitlePermissionField, record)}
																				<span
																					class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																					data-timeline-subtitle-conditional-readonly={`${record.id}:${timelineSubtitlePermissionField.key}`}
																				>
																					{$t('forms.fieldConditionReadOnlyBadge')}
																				</span>
																			{/if}
																		</span>
																	{/if}
															</button>
															<button
																type="button"
																class="inline-flex shrink-0 items-center rounded-md p-1.5 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																disabled={deletingRecordId === record.id}
																onclick={() => handleDeleteRecord(record)}
																aria-label={$t('forms.deleteRecord')}
															>
																<Trash2 class="h-4 w-4" aria-hidden="true" />
															</button>
														</div>
														{#if timelineEditableSelectFields.length > 0}
															<div class="mt-3 grid gap-2 sm:grid-cols-2">
																{#each timelineEditableSelectFields as field}
																	<label class="block space-y-1">
																		<span class="flex flex-wrap items-center gap-1.5 text-xs font-medium text-slate-500 dark:text-slate-400">
																			<span>{fieldLabel(field)}</span>
																			{#if memberFieldHidden(field)}
																				<span
																					class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																					data-timeline-select-permission-hidden={`${record.id}:${field.key}`}
																				>
																					{$t('forms.fieldHiddenBadge')}
																				</span>
																			{/if}
																			{#if memberFieldLocked(field)}
																				<span
																					class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium uppercase text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																					data-timeline-select-permission-locked={`${record.id}:${field.key}`}
																				>
																					{$t('forms.fieldLockedBadge')}
																				</span>
																			{/if}
																			{#if recordFieldHiddenByCondition(field, record)}
																				<span
																					class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																					data-timeline-select-conditional-hidden={`${record.id}:${field.key}`}
																				>
																					{$t('forms.fieldConditionHiddenBadge')}
																				</span>
																			{/if}
																			{#if recordFieldReadOnlyByCondition(field, record)}
																				<span
																					class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium uppercase text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																					data-timeline-select-conditional-readonly={`${record.id}:${field.key}`}
																				>
																					{$t('forms.fieldConditionReadOnlyBadge')}
																				</span>
																			{/if}
																		</span>
																		<select
																			value={String(record.values[field.key] ?? '')}
																			disabled={updatingTimelineRecordId === record.id}
																			aria-label={`${$t('forms.timelineUpdateField')} ${fieldLabel(field)}`}
																			data-timeline-select={`${record.id}:${field.key}`}
																			onchange={(event) =>
																				updateTimelineRecordSelectField(record, field, selectValue(event))}
																			class="w-full rounded-md border border-slate-200 bg-slate-50 px-2 py-1.5 text-xs text-slate-700 disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200"
																		>
																			{#each kanbanMoveOptions(field) as option}
																				<option value={option}>{option}</option>
																			{/each}
																		</select>
																	</label>
																{/each}
															</div>
														{/if}
													</div>
												</article>
											{/each}
											{/if}
										</div>
									{/if}
									</section>
							</div>
							{/if}

							{#if activeMode === 'detail'}
							<aside
								class="rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900"
								data-record-detail-drawer={selectedRecord?.id ?? ''}
								data-record-detail-url-record={detailUrlRecordId}
							>
							{#if selectedRecord}
								<div class="border-b border-slate-200 p-4 dark:border-slate-700">
									<div class="flex items-start justify-between gap-3">
										<div class="min-w-0">
											<h3
												class="truncate text-base font-semibold text-slate-950 dark:text-slate-100"
											>
												{selectedRecord.title}
											</h3>
											<p class="mt-1 font-mono text-xs text-slate-500 dark:text-slate-400">
												{selectedRecord.id}
											</p>
											<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
												{$t('forms.updatedAt', { values: { date: formatDate(selectedRecord.updated_at) } })}
											</p>
										</div>
										<div class="flex shrink-0 items-center gap-2">
											<Button
												size="sm"
												variant="secondary"
												onclick={() => {
													if (selectedRecord) startEditRecord(selectedRecord);
												}}
											>
												<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
												{$t('forms.edit')}
											</Button>
											<button
												type="button"
												class="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-200 text-slate-500 hover:bg-slate-50 hover:text-slate-700 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
												aria-label={$t('common.close')}
												onclick={closeRecordDetail}
												data-record-detail-close={selectedRecord.id}
											>
												<X class="h-4 w-4" aria-hidden="true" />
											</button>
										</div>
									</div>
								</div>
								<div class="divide-y divide-slate-100 dark:divide-slate-800" data-record-detail-layout={selectedRecord.id}>
									{#each detailLayoutSections() as section}
										<details
											class={section.density === 'compact' ? 'px-4 py-3' : 'px-4 py-4'}
											open={!section.collapsible || !section.defaultCollapsed}
											ontoggle={(event) => keepDetailSectionOpen(event, section.collapsible)}
											data-record-detail-section={`${selectedRecord.id}:${section.key}`}
											data-record-detail-section-density={`${selectedRecord.id}:${section.key}:${section.density}`}
											data-record-detail-section-collapsible={`${selectedRecord.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
											data-record-detail-section-open={`${selectedRecord.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
										>
											<summary
												class={section.collapsible ? 'cursor-pointer list-none' : 'cursor-default list-none'}
											>
												<h4 class="mb-3 inline text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
													{section.title}
												</h4>
											</summary>
											{#if section.description}
												<p
													class="mb-3 text-sm text-slate-500 dark:text-slate-400"
													data-record-detail-section-description={`${selectedRecord.id}:${section.key}`}
												>
													{section.description}
												</p>
											{/if}
											<dl
												class={detailLayoutSectionGridClass(section.columns, section.density)}
												data-record-detail-section-columns={`${selectedRecord.id}:${section.key}:${section.columns}`}
											>
												{#each detailLayoutSectionFieldsForRecord(section, selectedRecord) as field}
													<div
														class={detailLayoutSectionFieldClass(field, section.columns)}
														data-record-detail-field={`${selectedRecord.id}:${section.key}:${field.key}`}
													>
														<dt class="flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-slate-500 dark:text-slate-400">
															<span class="min-w-0 truncate">{fieldLabel(field)}</span>
															{#if memberFieldHidden(field)}
																<span
																	class="rounded-sm bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium text-amber-800 dark:bg-amber-950/50 dark:text-amber-200"
																	data-record-detail-permission-hidden={field.key}
																>
																	{$t('forms.fieldHiddenBadge')}
																</span>
															{/if}
															{#if memberFieldLocked(field)}
																<span
																	class="rounded-sm bg-slate-100 px-1.5 py-0.5 text-[10px] font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																	data-record-detail-permission-locked={field.key}
																>
																	{$t('forms.fieldLockedBadge')}
																</span>
															{/if}
															{#if recordFieldHiddenByCondition(field, selectedRecord)}
																<span
																	class="rounded-sm bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-950/40 dark:text-blue-200"
																	data-record-detail-conditional-hidden={field.key}
																>
																	{$t('forms.fieldConditionHiddenBadge')}
																</span>
															{/if}
															{#if recordFieldReadOnlyByCondition(field, selectedRecord)}
																<span
																	class="rounded-sm bg-cyan-50 px-1.5 py-0.5 text-[10px] font-medium text-cyan-700 dark:bg-cyan-950/40 dark:text-cyan-200"
																	data-record-detail-conditional-readonly={field.key}
																>
																	{$t('forms.fieldConditionReadOnlyBadge')}
																</span>
															{/if}
														</dt>
														<dd class="mt-1 break-words text-sm text-slate-900 dark:text-slate-100">
															{#if fieldType(field) === 'child_table'}
																{@const childRecords = childRecordsForRelationField(field)}
																{@const childFields = childGridFieldsForRelationField(field)}
																{@const childForm = childFormForRelationField(field)}
																<div
																	class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700"
																	data-record-detail-child-table-widget={`${selectedRecord.id}:${section.key}:${field.key}`}
																	data-record-detail-child-table-relation-key={childRelationKeyForField(field)}
																	data-record-detail-child-table-child-form={childForm?.key}
																>
																	<div
																		class="flex flex-wrap items-center justify-between gap-2 border-b border-slate-200 bg-slate-50 px-3 py-2 dark:border-slate-700 dark:bg-slate-950"
																	>
																		<div class="min-w-0">
																			<div
																				class="truncate text-xs font-semibold uppercase text-slate-600 dark:text-slate-300"
																				data-record-detail-child-table-title={`${selectedRecord.id}:${field.key}`}
																			>
																				{fieldLabel(field)}
																			</div>
																			<div
																				class="mt-0.5 text-[11px] text-slate-500 dark:text-slate-400"
																				data-record-detail-child-table-meta={`${selectedRecord.id}:${field.key}`}
																			>
																				{childForm?.name ?? childRelationKeyForField(field)} · {childRelationKeyForField(field)}
																			</div>
																		</div>
																		<span
																			class="rounded bg-white px-2 py-0.5 text-[11px] font-medium text-slate-600 ring-1 ring-slate-200 dark:bg-slate-900 dark:text-slate-300 dark:ring-slate-700"
																			data-record-detail-child-table-count={`${field.key}:${childRecords.length}`}
																		>
																			{$t('forms.groupRecordCount', { values: { count: childRecords.length } })}
																		</span>
																	</div>
																	{#if childRecords.length > 0}
																		<div class="overflow-x-auto">
																			<table
																				class="min-w-full divide-y divide-slate-200 text-xs dark:divide-slate-700"
																				data-record-detail-child-table-grid={field.key}
																			>
																				<thead class="bg-white dark:bg-slate-900">
																					<tr>
																						<th class="min-w-40 px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
																							{$t('forms.recordTitle')}
																						</th>
																						{#each childFields as childField}
																							<th
																								class="min-w-32 px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300"
																								data-record-detail-child-table-column={`${field.key}:${childField.key}`}
																							>
																								{fieldLabel(childField)}
																							</th>
																						{/each}
																						<th class="w-20 px-3 py-2 text-right font-medium text-slate-600 dark:text-slate-300">
																							{$t('common.actions')}
																						</th>
																					</tr>
																				</thead>
																				<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
																					{#each childRecords as child}
																						<tr data-record-detail-child-table-row={`${field.key}:${child.record.id}`}>
																							<td class="px-3 py-2 font-medium text-slate-900 dark:text-slate-100">
																								{child.record.title}
																							</td>
																							{#each childFields as childField}
																								<td
																									class="max-w-56 px-3 py-2 text-slate-700 dark:text-slate-300"
																									data-record-detail-child-table-cell={`${child.record.id}:${childField.key}`}
																								>
																									<span class="line-clamp-2 break-words">
																										{displayValue(child.record.values[childField.key])}
																									</span>
																								</td>
																							{/each}
																							<td class="px-3 py-2 text-right">
																								<div class="flex justify-end gap-1">
																									<button
																										type="button"
																										class="inline-flex items-center rounded-md p-1.5 text-blue-700 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																										onclick={() => startEditChildRecord(child)}
																										aria-label={$t('forms.editChildRecord')}
																										data-record-detail-child-table-edit-row={`${field.key}:${child.record.id}`}
																									>
																										<Pencil class="h-4 w-4" aria-hidden="true" />
																									</button>
																									<button
																										type="button"
																										class="inline-flex items-center rounded-md p-1.5 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																										disabled={deletingChildRecordId === child.record.id}
																										onclick={() => deleteChildRecord(child)}
																										aria-label={$t('forms.deleteChildRecord')}
																										data-record-detail-child-table-delete-row={`${field.key}:${child.record.id}`}
																									>
																										<Trash2 class="h-4 w-4" aria-hidden="true" />
																									</button>
																								</div>
																							</td>
																						</tr>
																					{/each}
																				</tbody>
																			</table>
																		</div>
																	{:else}
																		<p
																			class="px-3 py-3 text-sm text-slate-500 dark:text-slate-400"
																			data-record-detail-child-table-empty={field.key}
																		>
																			{$t('forms.noChildRecords')}
																		</p>
																	{/if}
																</div>
															{:else}
																<OptionValueBadges {field} value={selectedRecord.values[field.key]} />
															{/if}
														</dd>
													</div>
												{/each}
											</dl>
										</details>
									{/each}
									{#if parentChildRelationFields().length > 0}
										<section class="px-4 py-4" data-record-detail-section={`${selectedRecord.id}:child_tables`}>
											<h4 class="mb-3 text-xs font-semibold uppercase text-slate-500 dark:text-slate-400">
												{$t('forms.childRecords')}
											</h4>
											<div class="space-y-3" data-record-detail-child-table-widgets>
												{#each parentChildRelationFields() as relationField}
													{@const childRecords = childRecordsForRelationField(relationField)}
													{@const childFields = childGridFieldsForRelationField(relationField)}
													{@const childForm = childFormForRelationField(relationField)}
													<div
														class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700"
														data-record-detail-child-table-widget={`${selectedRecord.id}:child_tables:${relationField.key}`}
														data-record-detail-child-table-relation-key={childRelationKeyForField(relationField)}
														data-record-detail-child-table-child-form={childForm?.key}
													>
														<div
															class="flex flex-wrap items-center justify-between gap-2 border-b border-slate-200 bg-slate-50 px-3 py-2 dark:border-slate-700 dark:bg-slate-950"
														>
															<div class="min-w-0">
																<div
																	class="truncate text-xs font-semibold uppercase text-slate-600 dark:text-slate-300"
																	data-record-detail-child-table-title={`${selectedRecord.id}:${relationField.key}`}
																>
																	{fieldLabel(relationField)}
																</div>
																<div
																	class="mt-0.5 text-[11px] text-slate-500 dark:text-slate-400"
																	data-record-detail-child-table-meta={`${selectedRecord.id}:${relationField.key}`}
																>
																	{childForm?.name ?? childRelationKeyForField(relationField)} · {childRelationKeyForField(relationField)}
																</div>
															</div>
															<span
																class="rounded bg-white px-2 py-0.5 text-[11px] font-medium text-slate-600 ring-1 ring-slate-200 dark:bg-slate-900 dark:text-slate-300 dark:ring-slate-700"
																data-record-detail-child-table-count={`${relationField.key}:${childRecords.length}`}
															>
																{$t('forms.groupRecordCount', { values: { count: childRecords.length } })}
															</span>
														</div>
														{#if childRecords.length > 0}
															<div class="overflow-x-auto">
																<table
																	class="min-w-full divide-y divide-slate-200 text-xs dark:divide-slate-700"
																	data-record-detail-child-table-grid={relationField.key}
																>
																	<thead class="bg-white dark:bg-slate-900">
																		<tr>
																			<th class="min-w-40 px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
																				{$t('forms.recordTitle')}
																			</th>
																			{#each childFields as childField}
																				<th
																					class="min-w-32 px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300"
																					data-record-detail-child-table-column={`${relationField.key}:${childField.key}`}
																				>
																					{fieldLabel(childField)}
																				</th>
																			{/each}
																			<th class="w-20 px-3 py-2 text-right font-medium text-slate-600 dark:text-slate-300">
																				{$t('common.actions')}
																			</th>
																		</tr>
																	</thead>
																	<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
																		{#each childRecords as child}
																			<tr data-record-detail-child-table-row={`${relationField.key}:${child.record.id}`}>
																				<td class="px-3 py-2 font-medium text-slate-900 dark:text-slate-100">
																					{child.record.title}
																				</td>
																				{#each childFields as childField}
																					<td
																						class="max-w-56 px-3 py-2 text-slate-700 dark:text-slate-300"
																						data-record-detail-child-table-cell={`${child.record.id}:${childField.key}`}
																					>
																						<span class="line-clamp-2 break-words">
																							{displayValue(child.record.values[childField.key])}
																						</span>
																					</td>
																				{/each}
																				<td class="px-3 py-2 text-right">
																					<div class="flex justify-end gap-1">
																						<button
																							type="button"
																							class="inline-flex items-center rounded-md p-1.5 text-blue-700 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																							onclick={() => startEditChildRecord(child)}
																							aria-label={$t('forms.editChildRecord')}
																							data-record-detail-child-table-edit-row={`${relationField.key}:${child.record.id}`}
																						>
																							<Pencil class="h-4 w-4" aria-hidden="true" />
																						</button>
																						<button
																							type="button"
																							class="inline-flex items-center rounded-md p-1.5 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																							disabled={deletingChildRecordId === child.record.id}
																							onclick={() => deleteChildRecord(child)}
																							aria-label={$t('forms.deleteChildRecord')}
																							data-record-detail-child-table-delete-row={`${relationField.key}:${child.record.id}`}
																						>
																							<Trash2 class="h-4 w-4" aria-hidden="true" />
																						</button>
																					</div>
																				</td>
																			</tr>
																		{/each}
																	</tbody>
																</table>
															</div>
														{:else}
															<p
																class="px-3 py-3 text-sm text-slate-500 dark:text-slate-400"
																data-record-detail-child-table-empty={relationField.key}
															>
																{$t('forms.noChildRecords')}
															</p>
														{/if}
													</div>
												{/each}
											</div>
										</section>
									{/if}
								</div>
								<div class="border-t border-slate-200 p-4 dark:border-slate-700">
										<div class="mb-5">
											<div class="mb-3 flex flex-wrap items-center justify-between gap-2">
												<div class="flex items-center gap-2">
													<TableProperties class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
													<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
														{$t('forms.childRecords')}
													</h4>
												</div>
												<div class="flex flex-wrap items-center gap-2">
													{#each parentChildRelationFields() as relationField}
														<Button
															size="sm"
															variant="secondary"
															onclick={() => startAddChildRecord(relationField)}
														>
															<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('forms.addChildRecord')} · {fieldLabel(relationField)}
														</Button>
													{/each}
												</div>
											</div>
											{#if childEditor}
												<div
													class="mb-3 rounded-md border border-blue-200 bg-blue-50 p-3 dark:border-blue-900/60 dark:bg-blue-950/30"
												>
													<div class="mb-3 flex items-center justify-between gap-3">
														<h5 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
															{childEditor.recordId ? $t('forms.editChildRecord') : $t('forms.addChildRecord')}
														</h5>
														<Button size="sm" variant="secondary" onclick={cancelChildEditor}>
															<X class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('common.cancel')}
														</Button>
													</div>
													<div class="grid gap-3 md:grid-cols-2">
														<label class="space-y-1 text-sm">
															<span class="font-medium text-slate-700 dark:text-slate-300">
																{$t('forms.titleOverride')}
															</span>
															<input
																class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																value={childEditor.title}
																oninput={(event) => updateChildTitle(inputValue(event))}
															/>
														</label>
														{#each childFieldsForForm(childEditor.childFormId) as childField}
															<label
																class="space-y-1 text-sm {fieldHiddenByCondition(childField, childEditor.draft) ? 'hidden' : ''}"
																data-child-record-edit-field={childField.key}
																data-conditional-hidden={fieldHiddenByCondition(childField, childEditor.draft)
																	? childField.key
																	: undefined}
																data-conditional-readonly={fieldReadOnlyByCondition(childField, childEditor.draft)
																	? childField.key
																	: undefined}
															>
																<span class="font-medium text-slate-700 dark:text-slate-300">
																	{fieldLabel(childField)}
																	{#if childField.required}<span class="text-red-500">*</span>{/if}
																</span>
																{#if fieldType(childField) === 'boolean'}
																	<div
																		class="flex min-h-10 items-center rounded-md border border-slate-300 bg-white px-3 py-2 dark:border-slate-600 dark:bg-slate-800"
																	>
																		<input
																			type="checkbox"
																			class="h-4 w-4 rounded border-slate-300 text-blue-600"
																			checked={childEditor.draft[childField.key] === true}
																			disabled={fieldReadOnlyByCondition(childField, childEditor.draft)}
																			onchange={(event) =>
																				updateChildDraftValue(
																					childField.key,
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																				)}
																		/>
																	</div>
																{:else if fieldType(childField) === 'single_select' && fieldOptions(childField).length > 0}
																	<select
																		value={draftStringFrom(childEditor.draft, childField.key)}
																		disabled={fieldReadOnlyByCondition(childField, childEditor.draft)}
																		onchange={(event) => updateChildDraftValue(childField.key, selectValue(event))}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																	>
																		<option value="">{$t('forms.selectPlaceholder')}</option>
																		{#each recordSelectOptions(childField, childEditor.draft) as option}
																			<option
																				value={option}
																				disabled={optionDisabledForChildSingleDraft(childField, option)}
																				data-option-disabled={optionDisabled(childField, option) ? option : undefined}
																				data-option-archived={optionArchived(childField, option) ? option : undefined}
																				data-option-description={optionDescription(childField, option) || undefined}
																				data-option-group={optionGroup(childField, option) || undefined}
																			>
																				{optionDisplayLabel(childField, option)}
																			</option>
																		{/each}
																	</select>
																{:else if fieldType(childField) === 'multi_select' && fieldOptions(childField).length > 0}
																	<div
																		class="grid gap-2 rounded-md border border-slate-300 bg-white p-2 text-sm dark:border-slate-600 dark:bg-slate-800"
																	>
																		{#each recordSelectOptions(childField, childEditor.draft) as option}
																			<label
																				class="flex items-start gap-2 text-slate-700 dark:text-slate-300"
																				data-option-description={optionDescription(childField, option) || undefined}
																				data-option-group={optionGroup(childField, option) || undefined}
																			>
																				<input
																					type="checkbox"
																					class="h-4 w-4 rounded border-slate-300 text-blue-600"
																					checked={isChildDraftOptionSelected(childField.key, option)}
																					disabled={
																						fieldReadOnlyByCondition(childField, childEditor.draft) ||
																						optionDisabledForChildMultiDraft(childField, option)
																					}
																					data-option-disabled={optionDisabled(childField, option) ? option : undefined}
																					data-option-archived={optionArchived(childField, option) ? option : undefined}
																					onchange={(event) =>
																						toggleChildDraftOption(
																							childField.key,
																							option,
																							event.currentTarget instanceof HTMLInputElement
																								? event.currentTarget.checked
																								: false
																						)}
																				/>
																				<span class="min-w-0">
																					<span class="block">{optionDisplayLabel(childField, option)}</span>
																					{#if optionDescription(childField, option)}
																						<span class="block text-xs text-slate-500 dark:text-slate-400">
																							{optionDescription(childField, option)}
																						</span>
																					{/if}
																				</span>
																			</label>
																		{/each}
																	</div>
																{:else if fieldType(childField) === 'member'}
																	<select
																		value={draftStringFrom(childEditor.draft, childField.key)}
																		disabled={fieldReadOnlyByCondition(childField, childEditor.draft) || workspaceMembersLoading}
																		onchange={(event) => updateChildDraftValue(childField.key, selectValue(event))}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																	>
																		<option value="">
																			{workspaceMembersLoading
																				? $t('forms.loadingMembers')
																				: $t('forms.selectMemberPlaceholder')}
																		</option>
																		{#each workspaceMembers as member (member.user_id)}
																			<option value={member.user_id}>{member.name || member.email || member.user_id}</option>
																		{/each}
																	</select>
																	{:else if fieldType(childField) === 'textarea' || fieldType(childField) === 'rich_text'}
																	<textarea
																		class="min-h-20 w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																		value={draftStringFrom(childEditor.draft, childField.key)}
																		placeholder={fieldPlaceholder(childField)}
																		readonly={fieldReadOnlyByCondition(childField, childEditor.draft)}
																		maxlength={childField.validation?.max_length}
																		oninput={(event) => updateChildDraftValue(childField.key, inputValue(event))}
																	></textarea>
																{:else if fieldType(childField) === 'signature'}
																	<div class="space-y-2" data-child-signature-field={childField.key}>
																		<canvas
																			class="h-24 w-full touch-none rounded-md border border-dashed border-slate-300 bg-white dark:border-slate-600 dark:bg-slate-900"
																			data-child-signature-canvas={childField.key}
																			aria-label={$t('forms.signatureCanvas')}
																			onpointerdown={(event) =>
																				startSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					childEditor ? fieldReadOnlyByCondition(childField, childEditor.draft) : true
																				)}
																			onpointermove={(event) =>
																				moveSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					childEditor ? fieldReadOnlyByCondition(childField, childEditor.draft) : true
																				)}
																			onpointerup={(event) =>
																				endSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					childEditor ? fieldReadOnlyByCondition(childField, childEditor.draft) : true
																				)}
																			onpointercancel={(event) =>
																				endSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					childEditor ? fieldReadOnlyByCondition(childField, childEditor.draft) : true
																				)}
																		></canvas>
																		<div class="flex flex-wrap items-center gap-2">
																			<Button
																				size="sm"
																				variant="secondary"
																				disabled={fieldReadOnlyByCondition(childField, childEditor.draft)}
																				onclick={() =>
																					clearSignatureCanvas(
																						childField.key,
																						'child',
																						childEditor ? fieldReadOnlyByCondition(childField, childEditor.draft) : true
																					)}
																			>
																				{$t('forms.signatureClear')}
																			</Button>
																			{#if signatureImageValue(childEditor.draft, childField.key)}
																				<img
																					src={signatureImageValue(childEditor.draft, childField.key)}
																					alt={fieldLabel(childField)}
																					class="h-10 max-w-48 rounded border border-slate-200 bg-white object-contain dark:border-slate-700"
																					data-child-signature-preview={childField.key}
																				/>
																			{/if}
																		</div>
																		<textarea
																			class="min-h-16 w-full rounded-md border border-slate-300 bg-white px-3 py-2 font-mono text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																			value={draftStringFrom(childEditor.draft, childField.key)}
																			placeholder={fieldPlaceholder(childField)}
																			readonly={fieldReadOnlyByCondition(childField, childEditor.draft)}
																			maxlength={childField.validation?.max_length}
																			oninput={(event) => updateChildDraftValue(childField.key, inputValue(event))}
																			data-child-signature-value={childField.key}
																		></textarea>
																	</div>
																{:else if fieldType(childField) === 'autonumber'}
																	<input
																		type="text"
																		class="w-full rounded-md border border-slate-300 bg-slate-50 px-3 py-2 text-sm text-slate-500 dark:border-slate-600 dark:bg-slate-800/60 dark:text-slate-400"
																		value={draftStringFrom(childEditor.draft, childField.key)}
																		placeholder={$t('forms.autonumberGeneratedAfterSave')}
																		readonly
																		data-child-autonumber-field={childField.key}
																	/>
																{:else if fieldType(childField) === 'child_table'}
																	<div
																		class="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-600 dark:border-slate-700 dark:bg-slate-800/60 dark:text-slate-300"
																		data-child-table-field={childField.key}
																	>
																		{$t('forms.childRecords')}
																	</div>
																{:else}
																	<input
																		type={fieldType(childField) === 'date'
																			? 'date'
																			: fieldType(childField) === 'datetime'
																				? 'datetime-local'
																				: fieldType(childField) === 'email'
																					? 'email'
																					: fieldType(childField) === 'phone'
																						? 'tel'
																					: ['amount', 'number', 'integer', 'rating', 'progress'].includes(fieldType(childField))
																						? 'number'
																						: 'text'}
																			step={['integer', 'rating', 'progress'].includes(fieldType(childField))
																				? '1'
																				: ['amount', 'number'].includes(fieldType(childField))
																					? '0.01'
																					: undefined}
																		class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
																		value={draftStringFrom(childEditor.draft, childField.key)}
																		placeholder={fieldPlaceholder(childField)}
																		readonly={fieldReadOnlyByCondition(childField, childEditor.draft)}
																		min={childField.validation?.min}
																		max={childField.validation?.max}
																		pattern={childField.validation?.pattern}
																		maxlength={childField.validation?.max_length}
																		oninput={(event) => updateChildDraftValue(childField.key, inputValue(event))}
																	/>
																{/if}
																{#if childField.help_text}
																	<span class="block text-xs text-slate-500 dark:text-slate-400">
																		{childField.help_text}
																	</span>
																{/if}
															</label>
														{/each}
													</div>
													<div class="mt-3 flex justify-end gap-2">
														<Button size="sm" variant="secondary" onclick={cancelChildEditor}>
															{$t('common.cancel')}
														</Button>
														<Button size="sm" loading={childSubmitting} onclick={saveChildRecord}>
															<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('forms.saveRecord')}
														</Button>
													</div>
												</div>
											{/if}
											{#if parentChildRelationFields().length > 0}
												<div class="space-y-3" data-child-table-grids>
													{#each parentChildRelationFields() as relationField}
														{@const childRecords = childRecordsForRelationField(relationField)}
														{@const childFields = childGridFieldsForRelationField(relationField)}
														{@const childForm = childFormForRelationField(relationField)}
														<section
															class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700"
															data-child-table-grid={relationField.key}
															data-child-table-relation-key={childRelationKeyForField(relationField)}
															data-child-table-child-form={childForm?.key}
														>
															<div
																class="flex flex-wrap items-center justify-between gap-2 border-b border-slate-200 bg-slate-50 px-3 py-2 dark:border-slate-700 dark:bg-slate-950"
															>
																<div class="min-w-0">
																	<div
																		class="truncate text-xs font-semibold uppercase text-slate-600 dark:text-slate-300"
																		data-child-table-grid-title={relationField.key}
																	>
																		{fieldLabel(relationField)}
																	</div>
																	<div
																		class="mt-0.5 text-[11px] text-slate-500 dark:text-slate-400"
																		data-child-table-grid-meta={relationField.key}
																	>
																		{childForm?.name ?? childRelationKeyForField(relationField)} · {childRelationKeyForField(relationField)}
																	</div>
																</div>
																<span
																	class="rounded bg-white px-2 py-0.5 text-[11px] font-medium text-slate-600 ring-1 ring-slate-200 dark:bg-slate-900 dark:text-slate-300 dark:ring-slate-700"
																	data-child-table-grid-count={`${relationField.key}:${childRecords.length}`}
																>
																	{$t('forms.groupRecordCount', { values: { count: childRecords.length } })}
																</span>
															</div>
															{#if childRecords.length > 0}
																<div class="overflow-x-auto">
																	<table
																		class="min-w-full divide-y divide-slate-200 text-xs dark:divide-slate-700"
																		data-child-table-grid-table={relationField.key}
																	>
																		<thead class="bg-white dark:bg-slate-900">
																			<tr>
																				<th class="min-w-40 px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
																					{$t('forms.recordTitle')}
																				</th>
																				{#each childFields as childField}
																					<th
																						class="min-w-32 px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300"
																						data-child-table-grid-column={`${relationField.key}:${childField.key}`}
																					>
																						{fieldLabel(childField)}
																					</th>
																				{/each}
																				<th class="w-20 px-3 py-2 text-right font-medium text-slate-600 dark:text-slate-300">
																					{$t('common.actions')}
																				</th>
																			</tr>
																		</thead>
																		<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
																			{#each childRecords as child}
																				<tr data-child-table-grid-row={`${relationField.key}:${child.record.id}`}>
																					<td class="px-3 py-2 font-medium text-slate-900 dark:text-slate-100">
																						{child.record.title}
																					</td>
																					{#each childFields as childField}
																						<td
																							class="max-w-56 px-3 py-2 text-slate-700 dark:text-slate-300"
																							data-child-table-grid-cell={`${child.record.id}:${childField.key}`}
																						>
																							<span class="line-clamp-2 break-words">
																								{displayValue(child.record.values[childField.key])}
																							</span>
																						</td>
																					{/each}
																					<td class="px-3 py-2 text-right">
																						<div class="flex justify-end gap-1">
																							<button
																								type="button"
																								class="inline-flex items-center rounded-md p-1.5 text-blue-700 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																								onclick={() => startEditChildRecord(child)}
																								aria-label={$t('forms.editChildRecord')}
																								data-child-table-edit-row={`${relationField.key}:${child.record.id}`}
																							>
																								<Pencil class="h-4 w-4" aria-hidden="true" />
																							</button>
																							<button
																								type="button"
																								class="inline-flex items-center rounded-md p-1.5 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																								disabled={deletingChildRecordId === child.record.id}
																								onclick={() => deleteChildRecord(child)}
																								aria-label={$t('forms.deleteChildRecord')}
																								data-child-table-delete-row={`${relationField.key}:${child.record.id}`}
																							>
																								<Trash2 class="h-4 w-4" aria-hidden="true" />
																							</button>
																						</div>
																					</td>
																				</tr>
																			{/each}
																		</tbody>
																	</table>
																</div>
															{:else}
																<p
																	class="px-3 py-3 text-sm text-slate-500 dark:text-slate-400"
																	data-child-table-grid-empty={relationField.key}
																>
																	{$t('forms.noChildRecords')}
																</p>
															{/if}
														</section>
													{/each}
												</div>
											{:else if selectedRecordChildren.length > 0}
												<div class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700">
													<table class="min-w-full divide-y divide-slate-200 text-xs dark:divide-slate-700">
														<thead class="bg-slate-50 dark:bg-slate-950">
															<tr>
																<th class="px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
																	{$t('forms.recordTitle')}
																</th>
																<th class="px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
																	{$t('forms.relationKey')}
																</th>
																<th class="px-3 py-2 text-left font-medium text-slate-600 dark:text-slate-300">
																	{$t('forms.fields')}
																</th>
																<th class="px-3 py-2 text-right font-medium text-slate-600 dark:text-slate-300">
																	{$t('common.actions')}
																</th>
															</tr>
														</thead>
														<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
															{#each selectedRecordChildren as child}
																<tr>
																	<td class="px-3 py-2 font-medium text-slate-900 dark:text-slate-100">
																		{child.record.title}
																	</td>
																	<td class="px-3 py-2 font-mono text-slate-500 dark:text-slate-400">
																		{child.relation_key}
																	</td>
																	<td class="px-3 py-2 text-slate-700 dark:text-slate-300">
																		{#each childRecordFields(child).slice(0, 4) as childField, index}
																			{#if index > 0}<span class="mx-1 text-slate-300">/</span>{/if}
																			<span>{fieldLabel(childField)}: {displayValue(child.record.values[childField.key])}</span>
																		{/each}
																	</td>
																	<td class="px-3 py-2 text-right">
																		<div class="flex justify-end gap-1">
																			<button
																				type="button"
																				class="inline-flex items-center rounded-md p-1.5 text-blue-700 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																				onclick={() => startEditChildRecord(child)}
																				aria-label={$t('forms.editChildRecord')}
																			>
																				<Pencil class="h-4 w-4" aria-hidden="true" />
																			</button>
																			<button
																				type="button"
																				class="inline-flex items-center rounded-md p-1.5 text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950/30"
																				disabled={deletingChildRecordId === child.record.id}
																				onclick={() => deleteChildRecord(child)}
																				aria-label={$t('forms.deleteChildRecord')}
																			>
																				<Trash2 class="h-4 w-4" aria-hidden="true" />
																			</button>
																		</div>
																	</td>
																</tr>
															{/each}
														</tbody>
													</table>
												</div>
											{:else}
												<p class="text-sm text-slate-500 dark:text-slate-400">{$t('forms.noChildRecords')}</p>
											{/if}
										</div>
									<div class="mb-3 flex items-center gap-2">
										<Link class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
										<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
											{$t('forms.recordLinks')}
										</h4>
									</div>
									<div class="space-y-2">
										{#each selectedRecordLinks as item}
											<div
												class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
											>
												<div class="font-medium text-slate-800 dark:text-slate-200">
													{item.relation_key} / {item.relation_type}
												</div>
												<div class="mt-1 truncate font-mono text-slate-500 dark:text-slate-400">
													{item.target_type}:{item.target_id}
												</div>
											</div>
										{/each}
										{#if selectedRecordLinks.length === 0}
											<p class="text-sm text-slate-500 dark:text-slate-400">{$t('forms.noLinkedData')}</p>
										{/if}
									</div>
									<div class="mt-3 space-y-2">
										<input
											class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
											bind:value={linkDraft.target_id}
											placeholder={$t('forms.targetPlaceholder')}
										/>
										<div class="grid grid-cols-2 gap-2">
											<input
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												bind:value={linkDraft.relation_key}
												placeholder={$t('forms.relationKeyPlaceholder')}
											/>
											<input
												class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
												bind:value={linkDraft.relation_type}
												placeholder={$t('forms.relationTypePlaceholder')}
											/>
										</div>
										<Button
											size="sm"
											variant="secondary"
											loading={linkSubmitting}
											disabled={!linkDraft.target_id.trim()}
											onclick={handleCreateLink}
										>
											<Link class="mr-2 h-4 w-4" aria-hidden="true" />
											{$t('forms.addLink')}
										</Button>
									</div>
									<div
										class="mt-4 border-t border-slate-100 pt-4 dark:border-slate-800"
										data-record-detail-comments={selectedRecord.id}
										data-record-detail-comment-count={selectedRecordComments.length}
									>
										<div class="mb-3 flex items-center gap-2">
											<FileText class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
											<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.recordComments')}
											</h4>
										</div>
											<div class="space-y-2">
											{#each selectedRecordComments.slice(0, 5) as comment}
												<div
													class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
													data-record-detail-comment={comment.id}
												>
													<div class="flex flex-wrap items-center justify-between gap-2">
														<span
															class="font-medium text-slate-800 dark:text-slate-200"
															data-record-detail-comment-author={comment.id}
														>
															{comment.author_name || comment.author_email || comment.author_id?.slice(0, 8) || 'system'}
														</span>
														<span
															class="font-mono text-[11px] text-slate-500 dark:text-slate-400"
															data-record-detail-comment-created={comment.id}
														>
															{formatDate(comment.created_at)}
														</span>
													</div>
													<p
														class="mt-2 whitespace-pre-wrap text-sm leading-relaxed text-slate-700 dark:text-slate-300"
														data-record-detail-comment-body={comment.id}
													>
														{comment.body}
													</p>
												</div>
											{/each}
											{#if recordCommentsLoading && selectedRecordComments.length === 0}
												<p class="text-sm text-slate-500 dark:text-slate-400">
													{$t('common.loading')}
												</p>
											{:else if selectedRecordComments.length === 0}
												<p class="text-sm text-slate-500 dark:text-slate-400" data-record-detail-comments-empty={selectedRecord.id}>
													{$t('forms.noRecordComments')}
												</p>
											{/if}
											<div class="space-y-2 pt-1">
												<textarea
													class="min-h-20 w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
													bind:value={recordCommentDraft}
													placeholder={$t('forms.recordCommentPlaceholder')}
													data-record-detail-comment-input={selectedRecord.id}
												></textarea>
												<span data-record-detail-comment-submit={selectedRecord.id}>
													<Button
														size="sm"
														variant="secondary"
														loading={recordCommentSubmitting}
														disabled={!recordCommentDraft.trim()}
														onclick={handleCreateRecordComment}
													>
														{$t('forms.addComment')}
													</Button>
												</span>
											</div>
										</div>
									</div>
									{#if selectedRecordAttachmentFields().length > 0}
										<div
											class="mt-4 border-t border-slate-100 pt-4 dark:border-slate-800"
											data-record-detail-attachments={selectedRecord.id}
											data-record-detail-attachment-count={recordAttachments.filter((attachment) => !attachment.archived_at).length}
										>
											<div class="mb-3 flex items-center gap-2">
												<FileDown class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
												<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
													{$t('forms.attachmentConfig')}
												</h4>
											</div>
											<div class="space-y-3">
												{#each selectedRecordAttachmentFields() as field}
													{@const metadataAttachments = fieldAttachments(field.key)}
													{@const valueUrls = selectedRecordAttachmentValueUrls(field)}
													<section
														class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
														data-record-detail-attachment-field={field.key}
														data-record-detail-attachment-field-count={`${field.key}:${metadataAttachments.length + valueUrls.length}`}
													>
														<div class="mb-2 font-medium text-slate-800 dark:text-slate-200">
															{fieldLabel(field)}
														</div>
														<div class="space-y-2">
															{#each metadataAttachments as attachment}
																<div
																	class="flex flex-wrap items-center gap-2 rounded bg-slate-50 px-2 py-1.5 dark:bg-slate-950"
																	data-record-detail-attachment-item={attachment.id}
																>
																	{#if attachmentIsPreviewableImage(attachment)}
																		<img
																			src={attachmentThumbnailUrl(attachment)}
																			alt={attachment.file_name}
																			class="h-10 w-10 rounded object-cover ring-1 ring-slate-200 dark:ring-slate-700"
																			data-record-detail-attachment-thumbnail={attachment.id}
																		/>
																	{/if}
																	<div class="min-w-0 flex-1">
																		<div class="truncate font-medium text-slate-700 dark:text-slate-200">
																			{attachment.file_name}
																		</div>
																		<div
																			class="truncate font-mono text-[11px] text-slate-500 dark:text-slate-400"
																			data-record-detail-attachment-url={attachment.id}
																		>
																			{attachment.url}
																		</div>
																		</div>
																		<a
																			href={attachmentSignedUrl(attachment)}
																			target="_blank"
																			rel="noreferrer"
																			class="rounded px-2 py-1 font-medium text-blue-700 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																			data-record-detail-attachment-open={attachment.id}
																			data-record-detail-attachment-signed-url={attachmentSignedUrl(attachment)}
																		>
																			{$t('forms.attachmentOpen')}
																		</a>
																		<a
																			href={attachmentSignedUrl(attachment)}
																			download={attachment.file_name}
																			class="rounded px-2 py-1 font-medium text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
																			data-record-detail-attachment-download={attachment.id}
																			data-record-detail-attachment-signed-download={attachmentSignedUrl(attachment)}
																		>
																		{$t('forms.attachmentDownload')}
																	</a>
																</div>
															{/each}
															{#each valueUrls as url}
																<div
																	class="flex flex-wrap items-center gap-2 rounded bg-slate-50 px-2 py-1.5 dark:bg-slate-950"
																	data-record-detail-attachment-value={`${field.key}:${url}`}
																>
																	<div class="min-w-0 flex-1 truncate font-mono text-[11px] text-slate-500 dark:text-slate-400">
																		{url}
																	</div>
																	<a
																		href={url}
																		target="_blank"
																		rel="noreferrer"
																		class="rounded px-2 py-1 font-medium text-blue-700 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/30"
																		data-record-detail-attachment-value-open={field.key}
																	>
																		{$t('forms.attachmentOpen')}
																	</a>
																</div>
															{/each}
																{#if metadataAttachments.length === 0 && valueUrls.length === 0}
																	<p class="text-sm text-slate-500 dark:text-slate-400" data-record-detail-attachment-empty={field.key}>
																		{$t('forms.noRecordAttachments')}
																	</p>
								{/if}
															</div>
					</section>
												{/each}
											</div>
										</div>
									{/if}
									<div
										class="mt-4 border-t border-slate-100 pt-4 dark:border-slate-800"
										data-record-detail-automation-history={selectedRecord.id}
										data-record-detail-automation-count={selectedRecordAutomationInvocations.length}
									>
										<div class="mb-3 flex items-center gap-2">
											<Activity class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
											<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.recordAutomationHistory')}
											</h4>
										</div>
										<div class="space-y-2">
											{#each selectedRecordAutomationInvocations.slice(0, 5) as invocation}
												<details
													class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
													data-record-detail-automation-invocation={invocation.id}
													data-record-detail-automation-status={invocation.status}
												>
													<summary class="cursor-pointer list-none">
														<div class="flex flex-wrap items-start justify-between gap-2">
															<div class="min-w-0">
																<div
																	class="truncate font-medium text-slate-800 dark:text-slate-200"
																	data-record-detail-automation-connector={invocation.id}
																>
																	{invocationConnectorLabel(invocation)}
																</div>
																<div
																	class="mt-1 font-mono text-[11px] text-slate-500 dark:text-slate-400"
																	data-record-detail-automation-trigger={invocation.id}
																>
																	{invocation.trigger_ref_type || '-'}:{invocation.trigger_ref_id || '-'}
																</div>
															</div>
															<span
																class="rounded bg-slate-100 px-2 py-0.5 font-mono text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																data-record-detail-automation-updated={invocation.id}
															>
																{formatDate(invocation.updated_at)}
															</span>
														</div>
														<div class="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-slate-500 dark:text-slate-400">
															<span data-record-detail-automation-kind={invocation.id}>{invocation.trigger_kind}</span>
															<span data-record-detail-automation-delivery={invocation.id}>
																{invocationDeliveryStatus(invocation)}
															</span>
															{#if invocation.actor_id}
																<span data-record-detail-automation-actor={invocation.id}>
																	{$t('forms.eventActor')} {invocation.actor_id}
																</span>
															{/if}
														</div>
													</summary>
													<div class="mt-3 grid gap-2 lg:grid-cols-2">
														<pre
															class="max-h-40 overflow-auto rounded bg-slate-50 p-2 font-mono text-[11px] text-slate-600 dark:bg-slate-950 dark:text-slate-300"
															data-record-detail-automation-payload={invocation.id}
														>{timelineEventJson(invocation.payload)}</pre>
														<pre
															class="max-h-40 overflow-auto rounded bg-slate-50 p-2 font-mono text-[11px] text-slate-600 dark:bg-slate-950 dark:text-slate-300"
															data-record-detail-automation-result={invocation.id}
														>{timelineEventJson(invocation.result)}</pre>
													</div>
												</details>
											{/each}
											{#if automationInvocationsLoading && selectedRecordAutomationInvocations.length === 0}
												<p class="text-sm text-slate-500 dark:text-slate-400">
													{$t('common.loading')}
												</p>
											{:else if selectedRecordAutomationInvocations.length === 0}
												<p class="text-sm text-slate-500 dark:text-slate-400" data-record-detail-automation-empty={selectedRecord.id}>
													{$t('forms.noRecordAutomationHistory')}
												</p>
											{/if}
										</div>
									</div>
									<div
										class="mt-4 border-t border-slate-100 pt-4 dark:border-slate-800"
										data-record-detail-event-history={selectedRecord.id}
										data-record-detail-event-count={selectedRecordEvents.length}
									>
										<div class="mb-3 flex items-center gap-2">
											<FileText class="h-4 w-4 text-blue-600 dark:text-blue-300" aria-hidden="true" />
											<h4 class="text-sm font-semibold text-slate-950 dark:text-slate-100">
												{$t('forms.timelineSourceEvents')}
											</h4>
										</div>
										<div class="space-y-2">
											{#each selectedRecordEvents.slice(0, 5) as event}
												<details
													class="rounded-md border border-slate-200 px-3 py-2 text-xs dark:border-slate-700"
													data-record-detail-event={event.id}
													data-record-detail-event-type={event.event_type}
												>
													<summary class="cursor-pointer list-none">
														<div class="flex flex-wrap items-start justify-between gap-2">
															<div class="min-w-0">
																<div
																	class="truncate font-medium text-slate-800 dark:text-slate-200"
																	data-record-detail-event-title={event.id}
																>
																	{timelineEventTitle(event)}
																</div>
																<div
																	class="mt-1 font-mono text-[11px] text-slate-500 dark:text-slate-400"
																	data-record-detail-event-aggregate={event.id}
																>
																	{timelineEventAggregateKey(event)}
																</div>
															</div>
															<span
																class="rounded bg-slate-100 px-2 py-0.5 font-mono text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300"
																data-record-detail-event-created={event.id}
															>
																{formatDate(event.created_at)}
															</span>
														</div>
														<div class="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-slate-500 dark:text-slate-400">
															<span data-record-detail-event-kind={event.id}>{event.event_type}</span>
															{#if event.actor_id}
																<span data-record-detail-event-actor={event.id}>
																	{$t('forms.eventActor')} {event.actor_id}
																</span>
															{/if}
														</div>
													</summary>
													<div class="mt-3 grid gap-2 lg:grid-cols-2">
														<pre
															class="max-h-40 overflow-auto rounded bg-slate-50 p-2 font-mono text-[11px] text-slate-600 dark:bg-slate-950 dark:text-slate-300"
															data-record-detail-event-payload={event.id}
														>{timelineEventJson(event.payload)}</pre>
														<pre
															class="max-h-40 overflow-auto rounded bg-slate-50 p-2 font-mono text-[11px] text-slate-600 dark:bg-slate-950 dark:text-slate-300"
															data-record-detail-event-source={event.id}
														>{timelineEventJson(event.source)}</pre>
													</div>
												</details>
											{/each}
											{#if selectedRecordEvents.length === 0}
												<p class="text-sm text-slate-500 dark:text-slate-400" data-record-detail-event-empty={selectedRecord.id}>
													{$t('forms.noRecordEvents')}
												</p>
											{/if}
										</div>
									</div>
								</div>
							{:else}
								<div class="p-6 text-sm text-slate-500 dark:text-slate-400">
									{$t('forms.selectRecordPrompt')}
								</div>
								{/if}
							</aside>
							{/if}
							</div>
							{/if}
				</section>
		{/if}
	{/if}
	</div>

	<Modal
		bind:open={showImportRecords}
		title={$t('forms.importRecords')}
		maxWidthClass="max-w-4xl"
		onclose={closeImportRecords}
	>
		<div class="space-y-4" data-import-wizard data-import-wizard-step={importWizardStep}>
			<div class="grid gap-2 sm:grid-cols-4" aria-label={$t('forms.importWizardSteps')}>
				{#each ['source', 'mapping', 'preview', 'commit'] as step, index}
					{@const current = importWizardStep === step}
					{@const complete =
						(step === 'source' && importText.trim().length > 0) ||
						(step === 'mapping' && (importMappingApplied || importMappingHeaders.length === 0)) ||
						(step === 'preview' && Boolean(importPreview)) ||
						(step === 'commit' && (importPreview?.valid_rows ?? 0) > 0 && importPreview?.invalid_rows === 0)}
					<div
						class={current
							? 'rounded-md border border-blue-200 bg-blue-50 p-3 text-blue-900 dark:border-blue-800 dark:bg-blue-950/40 dark:text-blue-100'
							: complete
								? 'rounded-md border border-emerald-200 bg-emerald-50 p-3 text-emerald-900 dark:border-emerald-900/60 dark:bg-emerald-950/30 dark:text-emerald-100'
								: 'rounded-md border border-slate-200 bg-white p-3 text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400'}
						data-import-step={step}
						data-import-step-active={current ? 'true' : undefined}
					>
						<p class="text-xs font-semibold uppercase tracking-wide">
							{$t('forms.importStepCounter', { values: { step: index + 1 } })}
						</p>
						<p class="mt-1 text-sm font-medium">{$t(`forms.importSteps.${step}.title`)}</p>
						<p class="mt-1 text-xs opacity-80">{$t(`forms.importSteps.${step}.description`)}</p>
					</div>
				{/each}
			</div>

			{#if importWizardStep === 'source'}
				<section class="space-y-3" data-import-wizard-panel="source">
					<div class="flex flex-wrap items-center justify-between gap-2">
						<label class="block text-sm font-medium text-slate-700 dark:text-slate-300" for="form-import-text">
							{$t('forms.importPayload')}
						</label>
						<div class="flex flex-wrap items-center gap-2">
							<label
								class="inline-flex cursor-pointer items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
								for="form-import-file"
							>
								<FileUp class="h-3.5 w-3.5" aria-hidden="true" />
								{$t('forms.uploadImportFile')}
							</label>
							<input
								id="form-import-file"
								class="sr-only"
								type="file"
								accept=".csv,.json,text/csv,application/json"
								onchange={loadImportFile}
								data-import-file-upload
								data-import-file-loaded={importedImportFileLoaded ? 'true' : undefined}
								data-import-file-name={importedImportFileName}
							/>
							<button
								type="button"
								class="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
								onclick={exportImportTemplate}
								disabled={!selectedForm || visibleFields.length === 0}
								data-import-template-export
								data-import-template-exported={exportedImportTemplate ? 'true' : undefined}
							>
								<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
								{$t('forms.downloadImportTemplate')}
							</button>
						</div>
					</div>
					{#if importedImportFileName}
						<p class="text-xs text-slate-500 dark:text-slate-400">
							{$t('forms.importFileLoadedName', { values: { name: importedImportFileName } })}
						</p>
					{/if}
					<textarea
						id="form-import-text"
						class="min-h-72 w-full rounded-md border border-slate-300 bg-white px-3 py-2 font-mono text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
						value={importText}
						oninput={(event) => {
							importText = inputValue(event);
							importedImportFileName = '';
							importedImportFileLoaded = false;
							resetImportPreviewForSourceChange();
							refreshImportMappingFromText();
						}}
					></textarea>
					<p class="text-xs text-slate-500 dark:text-slate-400">
						{$t('forms.importPayloadHint')}
					</p>
				</section>
			{/if}

			{#if importWizardStep === 'mapping'}
				<section
					class="space-y-3 rounded-md border border-slate-200 bg-slate-50/60 p-3 dark:border-slate-700 dark:bg-slate-950/40"
					data-import-wizard-panel="mapping"
					data-import-mapping-wizard
					data-import-mapping-applied={importMappingApplied ? 'true' : undefined}
					data-import-mapping-template-available={importMappingTemplateAvailable ? 'true' : undefined}
					data-import-mapping-template-saved={importMappingTemplateSaved ? 'true' : undefined}
					data-import-mapping-template-restored={importMappingTemplateRestored ? 'true' : undefined}
				>
					<div class="flex flex-wrap items-center justify-between gap-2">
						<div>
							<h3 class="text-sm font-medium text-slate-900 dark:text-slate-100">
								{$t('forms.importMapping')}
							</h3>
							<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
								{$t('forms.importMappingHint')}
							</p>
						</div>
						<div class="flex flex-wrap items-center gap-2">
							<button
								type="button"
								class="inline-flex items-center rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
								onclick={saveImportMappingTemplate}
								data-import-mapping-template-save
							>
								{$t('forms.saveImportMappingTemplate')}
							</button>
							<button
								type="button"
								class="inline-flex items-center rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
								onclick={restoreImportMappingTemplate}
								disabled={!importMappingTemplateAvailable}
								data-import-mapping-template-restore
							>
								{$t('forms.restoreImportMappingTemplate')}
							</button>
							<button
								type="button"
								class="inline-flex items-center rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
								onclick={applyImportMapping}
								data-import-mapping-apply
							>
								{$t('forms.applyImportMapping')}
							</button>
						</div>
					</div>
					<div class="grid max-h-[46vh] gap-2 overflow-auto pr-1 sm:grid-cols-2">
						{#each importMappingHeaders as header, index}
							<label class="block space-y-1 text-xs" data-import-mapping-row={index}>
								<span class="font-medium text-slate-600 dark:text-slate-300">
									{header || $t('forms.importUnnamedColumn')}
								</span>
								<select
									class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
									bind:value={importMappingSelections[index]}
									onchange={() => {
										importMappingApplied = false;
									}}
									data-import-mapping-column={index}
								>
									<option value="__skip">{$t('forms.importMappingSkip')}</option>
									<option value="__title">{$t('forms.recordTitle')}</option>
									{#each fields as field}
										<option value={field.key}>{fieldLabel(field)}</option>
									{/each}
								</select>
								<span class="font-medium text-slate-600 dark:text-slate-300">
									{$t('forms.importMappingTransform')}
								</span>
								<select
									class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-xs text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
									bind:value={importMappingTransforms[index]}
									onchange={() => {
										importMappingApplied = false;
									}}
									data-import-mapping-transform={index}
								>
									<option value="none">{$t('forms.importMappingTransformNone')}</option>
									<option value="trim">{$t('forms.importMappingTransformTrim')}</option>
									<option value="uppercase">{$t('forms.importMappingTransformUppercase')}</option>
									<option value="lowercase">{$t('forms.importMappingTransformLowercase')}</option>
									<option value="capitalize">{$t('forms.importMappingTransformCapitalize')}</option>
									<option value="titlecase">{$t('forms.importMappingTransformTitlecase')}</option>
									<option value="normalize_spaces">{$t('forms.importMappingTransformNormalizeSpaces')}</option>
									<option value="strip_non_digits">{$t('forms.importMappingTransformStripNonDigits')}</option>
									<option value="slugify">{$t('forms.importMappingTransformSlugify')}</option>
								</select>
							</label>
						{/each}
					</div>
				</section>
			{/if}

			{#if importWizardStep === 'preview'}
				<section class="space-y-3" data-import-wizard-panel="preview">
					{#if importPreview}
						<div class="grid gap-2 sm:grid-cols-3">
							<div class="rounded-md border border-slate-200 bg-white p-3 dark:border-slate-700 dark:bg-slate-900">
								<p class="text-xs text-slate-500 dark:text-slate-400">{$t('forms.importTotalRows')}</p>
								<p class="mt-1 text-2xl font-semibold text-slate-950 dark:text-slate-100">{importPreview.total_rows}</p>
							</div>
							<div class="rounded-md border border-emerald-200 bg-emerald-50 p-3 dark:border-emerald-900/60 dark:bg-emerald-950/30">
								<p class="text-xs text-emerald-700 dark:text-emerald-200">{$t('forms.importValidRows')}</p>
								<p class="mt-1 text-2xl font-semibold text-emerald-800 dark:text-emerald-100">{importPreview.valid_rows}</p>
							</div>
							<div class="rounded-md border border-red-200 bg-red-50 p-3 dark:border-red-900/60 dark:bg-red-950/30">
								<p class="text-xs text-red-700 dark:text-red-200">{$t('forms.importInvalidRows')}</p>
								<p class="mt-1 text-2xl font-semibold text-red-800 dark:text-red-100">{importPreview.invalid_rows}</p>
							</div>
						</div>
						<div class="rounded-md border border-slate-200 dark:border-slate-700">
							<div class="flex flex-wrap items-center justify-between gap-2 border-b border-slate-200 px-3 py-2 text-sm dark:border-slate-700">
								<span class="font-medium text-slate-900 dark:text-slate-100">
									{$t('forms.importPreview')}
								</span>
								<span class="text-slate-500 dark:text-slate-400">
									{$t('forms.importPreviewSummary', {
										values: {
											total: importPreview.total_rows,
											valid: importPreview.valid_rows,
											invalid: importPreview.invalid_rows
										}
									})}
								</span>
							</div>
							<div class="max-h-64 overflow-auto">
								<table class="min-w-full divide-y divide-slate-200 text-sm dark:divide-slate-700">
									<thead class="bg-slate-50 text-left text-xs font-medium uppercase text-slate-500 dark:bg-slate-950 dark:text-slate-400">
										<tr>
											<th class="px-3 py-2">{$t('forms.importRow')}</th>
											<th class="px-3 py-2">{$t('common.status')}</th>
											<th class="px-3 py-2">{$t('forms.recordTitle')}</th>
											<th class="px-3 py-2">{$t('forms.importMessage')}</th>
										</tr>
									</thead>
									<tbody class="divide-y divide-slate-100 dark:divide-slate-800">
										{#each importPreview.rows as row}
											<tr>
												<td class="px-3 py-2 font-mono text-xs text-slate-500">{row.row_number}</td>
												<td class="px-3 py-2">
													<span
														class={row.valid
															? 'rounded-full bg-emerald-50 px-2 py-0.5 text-xs font-medium text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300'
															: 'rounded-full bg-red-50 px-2 py-0.5 text-xs font-medium text-red-700 dark:bg-red-950/40 dark:text-red-300'}
													>
														{row.valid ? $t('forms.importValid') : $t('forms.importInvalid')}
													</span>
												</td>
												<td class="max-w-56 truncate px-3 py-2 text-slate-800 dark:text-slate-200">
													{row.title ?? '-'}
												</td>
												<td class="px-3 py-2 text-slate-500 dark:text-slate-400">
													{row.error ?? '-'}
												</td>
											</tr>
										{/each}
									</tbody>
								</table>
							</div>
						</div>
					{/if}
				</section>
			{/if}

			{#if importWizardStep === 'commit'}
				<section class="rounded-md border border-emerald-200 bg-emerald-50 p-4 dark:border-emerald-900/60 dark:bg-emerald-950/30" data-import-wizard-panel="commit">
					<p class="text-sm font-semibold text-emerald-900 dark:text-emerald-100">
						{$t('forms.importReadyToCommit')}
					</p>
					<p class="mt-1 text-sm text-emerald-700 dark:text-emerald-200">
						{#if importPreview}
							{$t('forms.importReadyToCommitDescription', { values: { count: importPreview.valid_rows } })}
						{/if}
					</p>
				</section>
			{/if}

			<div class="flex flex-col-reverse gap-2 border-t border-slate-200 pt-3 dark:border-slate-700 sm:flex-row sm:items-center sm:justify-between">
				<div class="flex gap-2">
					{#if importWizardStep !== 'source'}
						<Button
							variant="secondary"
							onclick={() => {
								importWizardStep =
									importWizardStep === 'mapping' ? 'source' : importWizardStep === 'preview' ? 'mapping' : 'preview';
							}}
						>
							<ChevronLeft class="mr-2 h-4 w-4" aria-hidden="true" />
							{$t('common.back')}
						</Button>
					{/if}
				</div>
				<div class="flex justify-end gap-2">
					<Button variant="secondary" onclick={closeImportRecords}>{$t('common.cancel')}</Button>
					{#if importWizardStep === 'source'}
						<Button disabled={!importText.trim()} loading={importPreviewing} onclick={continueImportFromSource}>
							{$t('forms.importContinueToMapping')}
							<ChevronRight class="ml-2 h-4 w-4" aria-hidden="true" />
						</Button>
					{:else if importWizardStep === 'mapping'}
						<Button loading={importPreviewing} onclick={continueImportFromMapping}>
							{$t('forms.previewImport')}
							<ChevronRight class="ml-2 h-4 w-4" aria-hidden="true" />
						</Button>
					{:else}
						<Button
							loading={importingRecords}
							disabled={!importPreview || importPreview.invalid_rows > 0 || importPreview.valid_rows === 0}
							onclick={commitImportRecords}
						>
							<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
							{$t('forms.commitImport')}
						</Button>
					{/if}
				</div>
			</div>
		</div>
	</Modal>

	<Modal
		bind:open={designerConfirmOpen}
	title={$t('forms.schemaChangeConfirmTitle')}
	maxWidthClass="max-w-3xl"
	onclose={closeDesignerConfirm}
>
	<div class="space-y-4">
		<div class="rounded-md border border-amber-200 bg-amber-50 p-3 dark:border-amber-900/60 dark:bg-amber-950/30">
			<div class="flex items-start gap-2 text-sm text-amber-900 dark:text-amber-100">
				<AlertTriangle class="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
				<p>{$t('forms.schemaChangeConfirmDescription')}</p>
			</div>
		</div>
		<div class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700">
			<table class="min-w-full divide-y divide-slate-200 text-sm dark:divide-slate-700">
				<thead class="bg-slate-50 text-left text-xs font-medium uppercase text-slate-500 dark:bg-slate-900 dark:text-slate-400">
					<tr>
						<th class="px-3 py-2">{$t('forms.changeType')}</th>
						<th class="px-3 py-2">{$t('forms.field')}</th>
						<th class="px-3 py-2">{$t('forms.change')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.valueUsage')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.dependencyUsage')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.blockingDependencies')}</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-100 bg-white dark:divide-slate-800 dark:bg-slate-900">
					{#each pendingDesignerChanges as change}
						<tr>
							<td class="px-3 py-2 text-slate-700 dark:text-slate-300">
								{#if change.kind === 'delete'}
									{$t('forms.changeDelete')}
								{:else if change.kind === 'rename'}
									{$t('forms.changeRename')}
								{:else}
									{$t('forms.changeTypeChange')}
								{/if}
							</td>
							<td class="px-3 py-2">
								<div class="font-medium text-slate-950 dark:text-slate-100">{change.fieldLabel}</div>
								<div class="font-mono text-xs text-slate-500 dark:text-slate-400">{change.fieldKey}</div>
							</td>
							<td class="px-3 py-2 font-mono text-xs text-slate-600 dark:text-slate-300">
								{#if change.to}
									{change.from} -> {change.to}
								{:else}
									{change.from}
								{/if}
							</td>
							<td class="px-3 py-2 text-right font-mono text-slate-700 dark:text-slate-300">
								{change.valueCount}
							</td>
							<td class="px-3 py-2 text-right font-mono text-slate-700 dark:text-slate-300">
								{change.dependencyCount}
							</td>
							<td class="px-3 py-2 text-right font-mono {change.blockingDependencyCount > 0 ? 'text-red-600 dark:text-red-300' : 'text-slate-700 dark:text-slate-300'}">
								{change.blockingDependencyCount}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</div>

	{#snippet footer()}
		<Button variant="secondary" onclick={closeDesignerConfirm}>{$t('common.cancel')}</Button>
		<Button variant="dangerSolid" loading={designerSaving} onclick={confirmDesignerSchemaSave}>
			{$t('forms.confirmSchemaSave')}
		</Button>
	{/snippet}
</Modal>

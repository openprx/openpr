import { apiClient, type ApiResult, type PaginatedData } from './client';

export type FormFieldType =
	| 'text'
	| 'phone'
	| 'email'
	| 'address'
	| 'location'
	| 'scan'
	| 'signature'
	| 'autonumber'
	| 'member'
	| 'textarea'
	| 'rich_text'
	| 'number'
	| 'integer'
	| 'amount'
	| 'rating'
	| 'progress'
	| 'date'
	| 'datetime'
	| 'single_select'
	| 'multi_select'
	| 'boolean'
	| 'attachment'
	| 'image'
	| 'relation'
	| 'child_table'
	| 'formula'
	| 'ai_summary';

export interface AmountFieldConfig {
	currency?: string;
	scale?: number;
	allow_negative?: boolean;
	rounding?: string;
}

export interface FormFieldCondition {
	field?: string;
	equals?: string;
	operator?: 'equals' | 'not_equals';
	value?: string;
}

export interface FormFieldConditionGroup {
	logic?: 'all' | 'any';
	conditions?: FormFieldCondition[];
}

export interface FormField {
	field_id?: string;
	key: string;
	label?: string;
	type?: FormFieldType;
	required?: boolean;
	options?: string[];
	option_colors?: Record<string, string>;
	option_disabled?: string[];
	option_archived?: string[];
	option_descriptions?: Record<string, string>;
	option_groups?: Record<string, string>;
	amount?: AmountFieldConfig;
	placeholder?: string;
	help_text?: string;
	default_value?: unknown;
	validation?: {
		min?: number | string;
		max?: number | string;
		pattern?: string;
		max_length?: number;
	};
	visibility?: {
		list?: boolean;
		detail?: boolean;
	};
	conditional?: {
		hidden_when?: FormFieldCondition | FormFieldConditionGroup;
		read_only_when?: FormFieldCondition | FormFieldConditionGroup;
	};
	policy?: {
		read_only?: boolean;
	};
	formula?: {
		op?: string;
		args?: unknown[];
		scale?: number;
		relation_key?: string;
		field?: string;
	};
	relation?: {
		target_type?: string;
		form_key?: string;
		relation_key?: string;
		relation_type?: string;
		display_field?: string;
	};
	attachment?: {
		accept?: string[];
		max_size_mb?: number;
		multiple?: boolean;
		preview?: boolean;
		preview_format?: string;
		preview_max_dimension?: number;
		thumbnail?: boolean;
		thumbnail_format?: string;
		thumbnail_max_dimension?: number;
		variants?: Array<{
			kind?: string;
			max_dimension?: number;
			format?: string;
			enabled?: boolean;
		}>;
		storage_policy?: string;
		retention_days?: number;
		signed_url_ttl_minutes?: number;
	};
}

export interface UniversalForm {
	id: string;
	workspace_id: string;
	project_id: string;
	key: string;
	name: string;
	description: string;
	icon?: string | null;
	color?: string | null;
	title_template: string;
	schema: Record<string, unknown>;
	detail_layout: Record<string, unknown>;
	schema_version: number;
	created_by?: string | null;
	archived_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface FormRecord {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	title: string;
	values: Record<string, unknown>;
	source: Record<string, unknown>;
	schema_version?: number | null;
	created_by?: string | null;
	updated_by?: string | null;
	archived_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface FormView {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	key: string;
	name: string;
	view_type: 'grid' | 'detail' | 'kanban' | 'calendar' | 'card' | 'timeline' | 'pivot' | 'gantt';
	config: Record<string, unknown>;
	created_by?: string | null;
	archived_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface FormRecordsExport {
	form_id: string;
	format: 'csv' | 'json';
	file_name: string;
	export_policy: {
		scope: 'view' | 'all_accessible';
		view_id?: string | null;
		include_archived: boolean;
		row_limit: number;
		row_count: number;
		truncated: boolean;
	};
	columns: Array<{ key: string; label: string }>;
	rows: Array<{ record_id: string; title: string; values: Record<string, string> }>;
	csv?: string;
}

export interface FormExportJob {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	status: 'queued' | 'running' | 'completed' | 'failed';
	input: Record<string, unknown>;
	result?: FormRecordsExport | null;
	error?: string | null;
	created_by?: string | null;
	started_at?: string | null;
	completed_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface CreateFormExportJobRequest {
	view_id?: string;
	columns?: string[];
	format?: 'csv' | 'json';
	scope?: 'view' | 'all_accessible';
	include_archived?: boolean;
	limit?: number;
}

export interface FormAttachmentPackageJob {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	status: 'queued' | 'running' | 'completed' | 'failed';
	input: Record<string, unknown>;
	result?: {
		file_name: string;
		stored_file_name: string;
		download_url: string;
		content_type: string;
		byte_size: number;
		attachment_count: number;
		binary_file_count: number;
		generated_at: string;
		expires_at: string;
	} | null;
	error?: string | null;
	created_by?: string | null;
	started_at?: string | null;
	completed_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface CreateFormAttachmentPackageJobRequest {
	view_id?: string;
}

export interface FormRecordImportRow {
	row_number?: number;
	title?: string;
	values: Record<string, unknown>;
	source?: Record<string, unknown>;
}

export interface FormRecordImportPreviewRow {
	row_number: number;
	valid: boolean;
	title?: string | null;
	normalized_values?: Record<string, unknown> | null;
	error?: string | null;
}

export interface FormRecordImportPreview {
	form_id: string;
	schema_version: number;
	total_rows: number;
	valid_rows: number;
	invalid_rows: number;
	rows: FormRecordImportPreviewRow[];
}

export interface FormRecordImportFileRequest {
	file_url: string;
	mapping_template_id?: string;
	idempotency_key?: string;
}

export interface FormImportMappingTemplate {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	name: string;
	header_signature: string;
	headers: string[];
	selections: string[];
	transforms: string[];
	shared: boolean;
	created_by?: string | null;
	archived_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface SaveFormImportMappingTemplateRequest {
	id?: string;
	name?: string;
	header_signature?: string;
	headers: string[];
	selections: string[];
	transforms: string[];
	shared?: boolean;
}

export interface FormRecordImportResult extends FormRecordImportPreview {
	created_count: number;
	records: FormRecord[];
}

export interface FormImportJob {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	status: 'queued' | 'running' | 'completed' | 'failed';
	input: Record<string, unknown>;
	result?: FormRecordImportResult | null;
	error?: string | null;
	created_by?: string | null;
	started_at?: string | null;
	completed_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface CreateFormImportJobRequest {
	rows?: FormRecordImportRow[];
	file_url?: string;
	upload_url?: string;
	mapping_template_id?: string;
	idempotency_key?: string;
}

export type FormPermissionAction =
	| 'form.view'
	| 'form.design'
	| 'record.create'
	| 'record.update'
	| 'record.delete'
	| 'record.export';

export interface FormPermissionPolicy {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	subject_type: 'role';
	subject_id: 'owner' | 'admin' | 'member';
	policy: {
		actions?: Partial<Record<FormPermissionAction, boolean>>;
		record_scope?: 'all' | 'owned';
		fields?: Record<string, Partial<Record<'read' | 'write', boolean>>>;
		[key: string]: unknown;
	};
	created_at: string;
	updated_at: string;
}

export interface FormPermissions {
	form_id: string;
	policies: FormPermissionPolicy[];
	effective: {
		role: string;
		is_bot: boolean;
		actions: Record<FormPermissionAction, boolean>;
		record_scope: 'all' | 'owned';
		fields: Record<string, Partial<Record<'read' | 'write', boolean>>>;
	};
}

export interface UpsertFormPermissionPolicy {
	subject_type: 'role';
	subject_id: 'owner' | 'admin' | 'member';
	policy: {
		actions?: Partial<Record<FormPermissionAction, boolean>>;
		record_scope?: 'all' | 'owned';
		fields?: Record<string, Partial<Record<'read' | 'write', boolean>>>;
		[key: string]: unknown;
	};
}

export interface FormAggregate {
	form_id: string;
	field_key: string;
	field_type: string;
	aggregate: string;
	decimal?: string | null;
	count: number;
	currency?: string | null;
	scale?: number | null;
}

export interface FormRecalculatePreview {
	form_id: string;
	schema_version: number;
	values: Record<string, unknown>;
	normalized_values: Record<string, unknown>;
}

export interface FormAttachment {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	record_id?: string | null;
	field_id: string;
	field_key: string;
	file_name: string;
	content_type: string;
	byte_size: number;
	storage_key: string;
	url: string;
	thumbnail_url?: string | null;
	media_metadata?: Record<string, unknown> | null;
	created_by?: string | null;
	archived_at?: string | null;
	created_at: string;
	updated_at: string;
}

export interface FormAttachmentSignedDownload {
	url: string;
	expires_at: string;
}

export interface FormFieldSummary {
	field_id: string;
	key: string;
	label: string;
	field_type: FormFieldType | string;
	required: boolean;
	indexed: boolean;
	list_visible: boolean;
	detail_visible: boolean;
}

export interface FormSchemaSummary {
	form_id: string;
	schema_version: number;
	field_count: number;
	view_count: number;
	record_count: number;
	archived_record_count: number;
	fields: FormFieldSummary[];
}

export interface FormFieldUsage {
	field_id: string;
	field_key: string;
	field_type: FormFieldType | string;
	value_count: number;
	indexed_count: number;
	dependency_count: number;
}

export interface FormFieldUsageResult {
	form_id: string;
	schema_version: number;
	fields: FormFieldUsage[];
}

export interface FormFieldDependency {
	field_id: string;
	field_key: string;
	source_type: string;
	source_id?: string | null;
	source_key?: string | null;
	reason: string;
	blocking: boolean;
}

export interface FormFieldDependenciesResult {
	form_id: string;
	schema_version: number;
	dependencies: FormFieldDependency[];
}

export interface FormRecordLink {
	id: string;
	workspace_id: string;
	project_id: string;
	source_record_id: string;
	target_type: string;
	target_id: string;
	relation_key: string;
	relation_type: string;
	metadata: Record<string, unknown>;
	created_by?: string | null;
	created_at: string;
}

export interface FormRelationTarget {
	record_id: string;
	form_id: string;
	form_key: string;
	form_name: string;
	title: string;
	display: string;
	values: Record<string, unknown>;
	updated_at: string;
}

export interface FormChildRecord {
	link_id: string;
	relation_key: string;
	relation_type: string;
	record: FormRecord;
}

export interface BusinessEvent {
	id: string;
	workspace_id: string;
	project_id?: string | null;
	event_type: string;
	aggregate_type: string;
	aggregate_id: string;
	actor_id?: string | null;
	source: Record<string, unknown>;
	payload: Record<string, unknown>;
	metadata: Record<string, unknown>;
	correlation_id?: string | null;
	causation_id?: string | null;
	idempotency_key?: string | null;
	created_at: string;
}

export interface FormRecordComment {
	id: string;
	workspace_id: string;
	project_id: string;
	form_id: string;
	record_id: string;
	author_id?: string | null;
	author_name?: string | null;
	author_email?: string | null;
	body: string;
	metadata: Record<string, unknown>;
	archived_at?: string | null;
	created_at: string;
	updated_at: string;
}

export const formsApi = {
	list(
		projectId: string,
		params?: { page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<UniversalForm>>> {
		const query = new URLSearchParams();
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());
		const endpoint = query.size
			? `/api/v1/projects/${projectId}/forms?${query.toString()}`
			: `/api/v1/projects/${projectId}/forms`;
		return apiClient.get<PaginatedData<UniversalForm>>(endpoint);
	},

	get(formId: string): Promise<ApiResult<UniversalForm>> {
		return apiClient.get<UniversalForm>(`/api/v1/forms/${formId}`);
	},

	getPermissions(formId: string): Promise<ApiResult<FormPermissions>> {
		return apiClient.get<FormPermissions>(`/api/v1/forms/${formId}/permissions`);
	},

	updatePermissions(
		formId: string,
		policies: UpsertFormPermissionPolicy[]
	): Promise<ApiResult<FormPermissions>> {
		return apiClient.patch<FormPermissions>(`/api/v1/forms/${formId}/permissions`, { policies });
	},

	create(
		projectId: string,
		data: {
			key: string;
			name: string;
			description?: string;
			icon?: string;
			color?: string;
			title_template?: string;
			schema?: Record<string, unknown>;
			detail_layout?: Record<string, unknown>;
		}
	): Promise<ApiResult<UniversalForm>> {
		return apiClient.post<UniversalForm>(`/api/v1/projects/${projectId}/forms`, data);
	},

	createFromTemplate(
		projectId: string,
		data: {
			template_key: string;
			key?: string;
			name?: string;
			description?: string;
			title_template?: string;
		}
	): Promise<ApiResult<UniversalForm>> {
		return apiClient.post<UniversalForm>(`/api/v1/projects/${projectId}/forms/from-template`, data);
	},

	duplicate(
		formId: string,
		data: {
			key?: string;
			name?: string;
			description?: string;
		} = {}
	): Promise<ApiResult<UniversalForm>> {
		return apiClient.post<UniversalForm>(`/api/v1/forms/${formId}/duplicate`, data);
	},

	update(
		formId: string,
		data: {
			name?: string;
			description?: string;
			icon?: string;
			color?: string;
			title_template?: string;
			schema?: Record<string, unknown>;
			detail_layout?: Record<string, unknown>;
			expected_schema_version?: number;
		}
	): Promise<ApiResult<UniversalForm>> {
		return apiClient.patch<UniversalForm>(`/api/v1/forms/${formId}`, data);
	},

	delete(formId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/forms/${formId}`);
	},

	restore(formId: string): Promise<ApiResult<UniversalForm>> {
		return apiClient.post<UniversalForm>(`/api/v1/forms/${formId}/restore`, {});
	},

	listViews(formId: string): Promise<ApiResult<FormView[]>> {
		return apiClient.get<FormView[]>(`/api/v1/forms/${formId}/views`);
	},

	createView(
		formId: string,
		data: {
			key: string;
			name: string;
			view_type: FormView['view_type'];
			config?: Record<string, unknown>;
		}
	): Promise<ApiResult<FormView>> {
		return apiClient.post<FormView>(`/api/v1/forms/${formId}/views`, data);
	},

	updateView(
		viewId: string,
		data: {
			name?: string;
			view_type?: FormView['view_type'];
			config?: Record<string, unknown>;
			expected_updated_at?: string;
		}
	): Promise<ApiResult<FormView>> {
		return apiClient.patch<FormView>(`/api/v1/form-views/${viewId}`, data);
	},

	deleteView(viewId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/form-views/${viewId}`);
	},

		exportRecords(
			formId: string,
			params?: {
				view_id?: string;
				columns?: string[];
				format?: 'csv' | 'json';
				scope?: 'view' | 'all_accessible';
				include_archived?: boolean;
				limit?: number;
		}
	): Promise<ApiResult<FormRecordsExport>> {
		const query = new URLSearchParams();
		if (params?.view_id) query.append('view_id', params.view_id);
		if (params?.format) query.append('format', params.format);
		if (params?.scope) query.append('scope', params.scope);
		if (params?.include_archived !== undefined) {
			query.append('include_archived', String(params.include_archived));
		}
		if (params?.limit !== undefined) query.append('limit', String(params.limit));
		if (params?.columns && params.columns.length > 0) {
			query.append('columns', params.columns.join(','));
		}
			const endpoint = query.size
				? `/api/v1/forms/${formId}/records/export?${query.toString()}`
					: `/api/v1/forms/${formId}/records/export`;
				return apiClient.get<FormRecordsExport>(endpoint);
			},

			listExportJobs(formId: string): Promise<ApiResult<FormExportJob[]>> {
				return apiClient.get<FormExportJob[]>(`/api/v1/forms/${formId}/records/export-jobs`);
			},

			createExportJob(formId: string, request: CreateFormExportJobRequest): Promise<ApiResult<FormExportJob>> {
				return apiClient.post<FormExportJob>(`/api/v1/forms/${formId}/records/export-jobs`, request);
			},

			getExportJob(jobId: string): Promise<ApiResult<FormExportJob>> {
				return apiClient.get<FormExportJob>(`/api/v1/form-export-jobs/${jobId}`);
			},

			listAttachmentPackageJobs(formId: string): Promise<ApiResult<FormAttachmentPackageJob[]>> {
				return apiClient.get<FormAttachmentPackageJob[]>(
					`/api/v1/forms/${formId}/attachments/package-jobs`
				);
			},

			createAttachmentPackageJob(
				formId: string,
				request: CreateFormAttachmentPackageJobRequest
			): Promise<ApiResult<FormAttachmentPackageJob>> {
				return apiClient.post<FormAttachmentPackageJob>(
					`/api/v1/forms/${formId}/attachments/package-jobs`,
					request
				);
			},

			getAttachmentPackageJob(jobId: string): Promise<ApiResult<FormAttachmentPackageJob>> {
				return apiClient.get<FormAttachmentPackageJob>(`/api/v1/form-attachment-package-jobs/${jobId}`);
			},

			previewImportRecords(
				formId: string,
				rows: FormRecordImportRow[]
			): Promise<ApiResult<FormRecordImportPreview>> {
			return apiClient.post<FormRecordImportPreview>(
				`/api/v1/forms/${formId}/records/import-preview`,
				{ rows }
			);
		},

		importRecords(formId: string, rows: FormRecordImportRow[]): Promise<ApiResult<FormRecordImportResult>> {
			return apiClient.post<FormRecordImportResult>(`/api/v1/forms/${formId}/records/import`, {
				rows
			});
		},

		previewImportFile(
			formId: string,
			request: FormRecordImportFileRequest
		): Promise<ApiResult<FormRecordImportPreview>> {
			return apiClient.post<FormRecordImportPreview>(
				`/api/v1/forms/${formId}/records/import-file-preview`,
				request
			);
		},

		importFile(
			formId: string,
			request: FormRecordImportFileRequest
		): Promise<ApiResult<FormRecordImportResult>> {
			return apiClient.post<FormRecordImportResult>(`/api/v1/forms/${formId}/records/import-file`, request);
		},

		listImportJobs(formId: string): Promise<ApiResult<FormImportJob[]>> {
			return apiClient.get<FormImportJob[]>(`/api/v1/forms/${formId}/records/import-jobs`);
		},

		createImportJob(formId: string, request: CreateFormImportJobRequest): Promise<ApiResult<FormImportJob>> {
			return apiClient.post<FormImportJob>(`/api/v1/forms/${formId}/records/import-jobs`, request);
		},

		getImportJob(jobId: string): Promise<ApiResult<FormImportJob>> {
			return apiClient.get<FormImportJob>(`/api/v1/form-import-jobs/${jobId}`);
		},

		listImportMappingTemplates(
			formId: string,
			params?: { header_signature?: string }
		): Promise<ApiResult<FormImportMappingTemplate[]>> {
			const query = new URLSearchParams();
			if (params?.header_signature) query.append('header_signature', params.header_signature);
			const endpoint = query.size
				? `/api/v1/forms/${formId}/import-mapping-templates?${query.toString()}`
				: `/api/v1/forms/${formId}/import-mapping-templates`;
			return apiClient.get<FormImportMappingTemplate[]>(endpoint);
		},

		saveImportMappingTemplate(
			formId: string,
			request: SaveFormImportMappingTemplateRequest
		): Promise<ApiResult<FormImportMappingTemplate>> {
			return apiClient.post<FormImportMappingTemplate>(
				`/api/v1/forms/${formId}/import-mapping-templates`,
				request
			);
		},

		deleteImportMappingTemplate(templateId: string): Promise<ApiResult<{ id: string }>> {
			return apiClient.delete<{ id: string }>(`/api/v1/form-import-mapping-templates/${templateId}`);
		},

		listRecords(
		formId: string,
		params?: {
			page?: number;
			per_page?: number;
			view_id?: string;
			sort?: string;
			filter_field?: string;
			filter_op?: 'eq' | 'neq' | 'not_equals' | 'contains' | 'gt' | 'gte' | 'lt' | 'lte' | 'not_empty';
			filter_value?: string;
		}
	): Promise<ApiResult<PaginatedData<FormRecord>>> {
		const query = new URLSearchParams();
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());
		if (params?.view_id) query.append('view_id', params.view_id);
		if (params?.sort) query.append('sort', params.sort);
		if (params?.filter_field) query.append('filter_field', params.filter_field);
		if (params?.filter_op) query.append('filter_op', params.filter_op);
		if (params?.filter_value) query.append('filter_value', params.filter_value);
		const endpoint = query.size
			? `/api/v1/forms/${formId}/records?${query.toString()}`
			: `/api/v1/forms/${formId}/records`;
		return apiClient.get<PaginatedData<FormRecord>>(endpoint);
	},

	createRecord(
		formId: string,
		data: { values: Record<string, unknown>; title?: string; source?: Record<string, unknown> }
	): Promise<ApiResult<FormRecord>> {
		return apiClient.post<FormRecord>(`/api/v1/forms/${formId}/records`, data);
	},

	previewRecalculate(
		formId: string,
		data: { values: Record<string, unknown> }
	): Promise<ApiResult<FormRecalculatePreview>> {
		return apiClient.post<FormRecalculatePreview>(
			`/api/v1/forms/${formId}/records/recalculate-preview`,
			data
		);
	},

	getRecord(recordId: string): Promise<ApiResult<FormRecord>> {
		return apiClient.get<FormRecord>(`/api/v1/form-records/${recordId}`);
	},

	updateRecord(
		recordId: string,
		data: { values?: Record<string, unknown>; title?: string; source?: Record<string, unknown> }
	): Promise<ApiResult<FormRecord>> {
		return apiClient.patch<FormRecord>(`/api/v1/form-records/${recordId}`, data);
	},

	deleteRecord(recordId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/form-records/${recordId}`);
	},

	restoreRecord(recordId: string): Promise<ApiResult<FormRecord>> {
		return apiClient.post<FormRecord>(`/api/v1/form-records/${recordId}/restore`, {});
	},

	recalculateRecord(recordId: string): Promise<ApiResult<FormRecord>> {
		return apiClient.post<FormRecord>(`/api/v1/form-records/${recordId}/recalculate`, {});
	},

	aggregate(
		formId: string,
		fieldKey: string,
		aggregate = 'sum'
	): Promise<ApiResult<FormAggregate>> {
		const query = new URLSearchParams({ field_key: fieldKey, aggregate });
		return apiClient.get<FormAggregate>(`/api/v1/forms/${formId}/aggregate?${query.toString()}`);
	},

	getSchemaSummary(formId: string): Promise<ApiResult<FormSchemaSummary>> {
		return apiClient.get<FormSchemaSummary>(`/api/v1/forms/${formId}/schema-summary`);
	},

	getFieldUsage(formId: string): Promise<ApiResult<FormFieldUsageResult>> {
		return apiClient.get<FormFieldUsageResult>(`/api/v1/forms/${formId}/field-usage`);
	},

	getFieldDependencies(formId: string): Promise<ApiResult<FormFieldDependenciesResult>> {
		return apiClient.get<FormFieldDependenciesResult>(
			`/api/v1/forms/${formId}/field-dependencies`
		);
	},

	listRelationTargets(
		formId: string,
		params?: {
			field_key?: string;
			form_key?: string;
			q?: string;
			page?: number;
			per_page?: number;
		}
	): Promise<ApiResult<PaginatedData<FormRelationTarget>>> {
		const query = new URLSearchParams();
		if (params?.field_key) query.append('field_key', params.field_key);
		if (params?.form_key) query.append('form_key', params.form_key);
		if (params?.q) query.append('q', params.q);
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());
		const endpoint = query.size
			? `/api/v1/forms/${formId}/relation-targets?${query.toString()}`
			: `/api/v1/forms/${formId}/relation-targets`;
		return apiClient.get<PaginatedData<FormRelationTarget>>(endpoint);
	},

	createAttachment(
		formId: string,
		data: {
			field_key: string;
			record_id?: string;
			file_name: string;
			content_type?: string;
			byte_size?: number;
			storage_key: string;
			url?: string;
			thumbnail_url?: string;
		}
	): Promise<ApiResult<FormAttachment>> {
		return apiClient.post<FormAttachment>(`/api/v1/forms/${formId}/attachments`, data);
	},

	listAttachments(
		formId: string,
		params?: {
			record_id?: string;
			field_key?: string;
			include_archived?: boolean;
			page?: number;
			per_page?: number;
		}
	): Promise<ApiResult<PaginatedData<FormAttachment>>> {
		const query = new URLSearchParams();
		if (params?.record_id) query.append('record_id', params.record_id);
		if (params?.field_key) query.append('field_key', params.field_key);
		if (params?.include_archived !== undefined) {
			query.append('include_archived', String(params.include_archived));
		}
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());
		const endpoint = query.size
			? `/api/v1/forms/${formId}/attachments?${query.toString()}`
			: `/api/v1/forms/${formId}/attachments`;
		return apiClient.get<PaginatedData<FormAttachment>>(endpoint);
	},

	deleteAttachment(attachmentId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/form-attachments/${attachmentId}`);
	},

	restoreAttachment(attachmentId: string): Promise<ApiResult<FormAttachment>> {
		return apiClient.post<FormAttachment>(`/api/v1/form-attachments/${attachmentId}/restore`, {});
	},

	getAttachmentSignedDownload(attachmentId: string): Promise<ApiResult<FormAttachmentSignedDownload>> {
		return apiClient.get<FormAttachmentSignedDownload>(`/api/v1/form-attachments/${attachmentId}/signed-url`);
	},

	listEvents(formId: string): Promise<ApiResult<PaginatedData<BusinessEvent>>> {
		return apiClient.get<PaginatedData<BusinessEvent>>(`/api/v1/forms/${formId}/events`);
	},

	listRecordComments(
		recordId: string,
		params?: { page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<FormRecordComment>>> {
		const query = new URLSearchParams();
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());
		const endpoint = query.size
			? `/api/v1/form-records/${recordId}/comments?${query.toString()}`
			: `/api/v1/form-records/${recordId}/comments`;
		return apiClient.get<PaginatedData<FormRecordComment>>(endpoint);
	},

	createRecordComment(
		recordId: string,
		data: { body: string; metadata?: Record<string, unknown> }
	): Promise<ApiResult<FormRecordComment>> {
		return apiClient.post<FormRecordComment>(`/api/v1/form-records/${recordId}/comments`, data);
	},

	listRecordLinks(recordId: string): Promise<ApiResult<FormRecordLink[]>> {
		return apiClient.get<FormRecordLink[]>(`/api/v1/form-records/${recordId}/links`);
	},

	listRecordChildren(
		recordId: string,
		params?: {
			relation_key?: string;
			child_form_key?: string;
			page?: number;
			per_page?: number;
		}
	): Promise<ApiResult<PaginatedData<FormChildRecord>>> {
		const query = new URLSearchParams();
		if (params?.relation_key) query.append('relation_key', params.relation_key);
		if (params?.child_form_key) query.append('child_form_key', params.child_form_key);
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());
		const endpoint = query.size
			? `/api/v1/form-records/${recordId}/children?${query.toString()}`
			: `/api/v1/form-records/${recordId}/children`;
		return apiClient.get<PaginatedData<FormChildRecord>>(endpoint);
	},

	createRecordLink(
		recordId: string,
		data: {
			target_type: string;
			target_id: string;
			relation_key: string;
			relation_type: string;
			metadata?: Record<string, unknown>;
		}
	): Promise<ApiResult<FormRecordLink>> {
		return apiClient.post<FormRecordLink>(`/api/v1/form-records/${recordId}/links`, data);
	}
};

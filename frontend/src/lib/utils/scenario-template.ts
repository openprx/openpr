import type { Project } from '$lib/api/projects';

export interface ScenarioFieldDef {
	key: string;
	label: string;
	type: string;
	required: boolean;
	options: string[];
}

export interface EmbeddedScenarioTemplate {
	key: string;
	name: string;
	industry: string;
	project_type_key: string;
	audience_label: string;
	workflow_template: Record<string, unknown>;
	field_schema: Record<string, unknown>;
	resource_schema: Record<string, unknown>;
	ai_roles: unknown[];
	governance_policy: Record<string, unknown>;
	connector_suggestions: unknown[];
	sample_data: Record<string, unknown>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function getProjectScenarioTemplate(project: Project | null | undefined): EmbeddedScenarioTemplate | null {
	const raw = project?.type_settings?.scenario_template;
	if (!isRecord(raw) || typeof raw.key !== 'string' || typeof raw.name !== 'string') {
		return null;
	}

	return {
		key: raw.key,
		name: raw.name,
		industry: typeof raw.industry === 'string' ? raw.industry : 'general',
		project_type_key: typeof raw.project_type_key === 'string' ? raw.project_type_key : project?.type_key ?? '',
		audience_label: typeof raw.audience_label === 'string' ? raw.audience_label : 'Assistant',
		workflow_template: isRecord(raw.workflow_template) ? raw.workflow_template : {},
		field_schema: isRecord(raw.field_schema) ? raw.field_schema : {},
		resource_schema: isRecord(raw.resource_schema) ? raw.resource_schema : {},
		ai_roles: Array.isArray(raw.ai_roles) ? raw.ai_roles : [],
		governance_policy: isRecord(raw.governance_policy) ? raw.governance_policy : {},
		connector_suggestions: Array.isArray(raw.connector_suggestions) ? raw.connector_suggestions : [],
		sample_data: isRecord(raw.sample_data) ? raw.sample_data : {}
	};
}

export function templateArray(template: EmbeddedScenarioTemplate | null | undefined, section: keyof EmbeddedScenarioTemplate, key: string): unknown[] {
	const sectionValue = template?.[section];
	if (!isRecord(sectionValue)) {
		return [];
	}
	const value = sectionValue[key];
	return Array.isArray(value) ? value : [];
}

export function getScenarioFields(template: EmbeddedScenarioTemplate | null | undefined): ScenarioFieldDef[] {
	return templateArray(template, 'field_schema', 'fields')
		.filter(isRecord)
		.map((field) => {
			const key = typeof field.key === 'string' ? field.key.trim() : '';
			const label = typeof field.label === 'string' && field.label.trim() ? field.label.trim() : key;
			const type = typeof field.type === 'string' && field.type.trim() ? field.type.trim() : 'text';
			const options = Array.isArray(field.options)
				? field.options.filter((item): item is string => typeof item === 'string')
				: [];
			return {
				key,
				label,
				type,
				required: Boolean(field.required),
				options
			};
		})
		.filter((field) => field.key.length > 0);
}

export function scenarioItemLabel(item: unknown, fallback: string): string {
	if (!isRecord(item)) {
		return fallback;
	}
	for (const key of ['label', 'name', 'key', 'kind', 'type']) {
		const value = item[key];
		if (typeof value === 'string' && value.trim()) {
			return value.trim();
		}
	}
	return fallback;
}

import type { ProjectType, ScenarioTemplate } from '$lib/api/projects';
import type { EmbeddedScenarioTemplate } from '$lib/utils/scenario-template';

type Translate = (id: string) => string;

function translated(t: Translate, key: string, fallback: string): string {
	const value = t(key);
	return value && value !== key ? value : fallback;
}

function fallbackName(value: string): string {
	return value.replaceAll('_', ' ');
}

export function projectTypeName(
	typeKey: string,
	projectTypes: ProjectType[],
	t: Translate
): string {
	const projectType = projectTypes.find((item) => item.key === typeKey);
	const fallback = projectType?.name ?? fallbackName(typeKey);
	return translated(t, `projectTypes.systemNames.${typeKey}`, fallback);
}

export function projectTypeDescription(projectType: ProjectType, t: Translate): string {
	return translated(t, `projectTypes.systemDescriptions.${projectType.key}`, projectType.description);
}

export function projectTypeDomain(domain: string, t: Translate): string {
	return translated(t, `projectTypes.domains.${domain}`, fallbackName(domain));
}

export function scenarioTemplateName(
	template: Pick<ScenarioTemplate | EmbeddedScenarioTemplate, 'key' | 'name'>,
	t: Translate
): string {
	return translated(t, `scenarioTemplates.systemNames.${template.key}`, template.name);
}

export function scenarioTemplateDescription(template: ScenarioTemplate, t: Translate): string {
	return translated(t, `scenarioTemplates.systemDescriptions.${template.key}`, template.description);
}

export function scenarioTemplateIndustry(industry: string, t: Translate): string {
	return translated(t, `scenarioTemplates.industries.${industry}`, fallbackName(industry));
}

export function scenarioTemplateAudience(audience: string, templateKey: string, t: Translate): string {
	return translated(t, `scenarioTemplates.audiences.${templateKey}`, audience);
}

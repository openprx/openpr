import { projectOptionsStore, type ProjectOption } from '$lib/stores/project-options';

export type { ProjectOption };
export interface ProjectOptionGroup {
	workspaceName: string;
	items: ProjectOption[];
}

export async function loadProjectOptions(force = false): Promise<ProjectOption[]> {
	return projectOptionsStore.ensureLoaded(force);
}

export function groupProjectOptionsByWorkspace(items: ProjectOption[]): ProjectOptionGroup[] {
	const grouped = new Map<string, ProjectOption[]>();
	for (const item of items) {
		const existing = grouped.get(item.workspace_name);
		if (existing) {
			existing.push(item);
			continue;
		}
		grouped.set(item.workspace_name, [item]);
	}

	return Array.from(grouped.entries()).map(([workspaceName, workspaceItems]) => ({
		workspaceName,
		items: workspaceItems
	}));
}

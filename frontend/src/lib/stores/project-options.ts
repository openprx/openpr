import { get, writable } from 'svelte/store';
import { projectsApi, type Project } from '$lib/api/projects';
import { workspacesApi } from '$lib/api/workspaces';

export interface ProjectOption {
	id: string;
	name: string;
	workspace_id: string;
	workspace_name: string;
}

interface ProjectOptionsState {
	items: ProjectOption[];
	loading: boolean;
	loaded: boolean;
	error: string | null;
}

const initialState: ProjectOptionsState = {
	items: [],
	loading: false,
	loaded: false,
	error: null
};

function createProjectOptionsStore() {
	const { subscribe, set, update } = writable<ProjectOptionsState>(initialState);
	let inFlight: Promise<ProjectOption[]> | null = null;

	async function fetchProjectOptions(): Promise<ProjectOption[]> {
		const wsRes = await workspacesApi.list();
		if (wsRes.code !== 0 || !wsRes.data) {
			return [];
		}

		const workspaceItems = wsRes.data.items ?? [];
		const projectResults = await Promise.all(
			workspaceItems.map(async (workspace) => {
				const projectRes = await projectsApi.list(workspace.id, { page: 1, per_page: 100 });
				if (projectRes.code !== 0 || !projectRes.data) {
					return [] as ProjectOption[];
				}
				return (projectRes.data.items ?? []).map((project: Project) => ({
					id: project.id,
					name: project.name,
					workspace_id: workspace.id,
					workspace_name: workspace.name
				}));
			})
		);

		return projectResults
			.flat()
			.sort((a, b) => `${a.workspace_name}/${a.name}`.localeCompare(`${b.workspace_name}/${b.name}`));
	}

	async function ensureLoaded(force = false): Promise<ProjectOption[]> {
		const current = get({ subscribe });
		if (!force && current.loaded) {
			return current.items;
		}
		if (!force && inFlight) {
			return inFlight;
		}

		update((state) => ({ ...state, loading: true, error: null }));
		inFlight = (async () => {
			try {
				const items = await fetchProjectOptions();
				update(() => ({ items, loading: false, loaded: true, error: null }));
				return items;
			} catch (error) {
				const message = error instanceof Error ? error.message : 'Failed to load project options';
				update((state) => ({ ...state, loading: false, error: message }));
				return current.items;
			} finally {
				inFlight = null;
			}
		})();

		return inFlight;
	}

	return {
		subscribe,
		ensureLoaded,
		refresh: () => ensureLoaded(true),
		reset: () => set(initialState)
	};
}

export const projectOptionsStore = createProjectOptionsStore();

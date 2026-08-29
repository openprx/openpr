// `flow_enabled` gate for BOTH navigation and direct URL access
// (`contracts/ui-surface-v1.md` "flag false：导航不显示且 direct URL 显示 not-found-safe 页面；
// route guard 与 API/WS 各自检查"). `(app)/+layout.ts` already sets `ssr=false` for this whole
// route group, so this load always runs client-side with a real access token available.
//
// v0.4 baseline note: `GET /workspaces/{id}/features/flow` is in the frozen `rest-api-v1.md`
// table but is not routed server-side at this repo's baseline -- calling it fails (no matching
// route), and this load treats ANY non-success result as `flowEnabled: false`
// (`api/flow.ts`'s header comment has the detail). That is the fail-closed default the backend's
// own `fetch_flow_enabled` already uses for a workspace with no settings row, so this route guard
// behaves correctly today (Flow shows as unavailable, which is true) and will start reflecting
// the real flag with no code change once the endpoint ships.

import { flowApi } from '$lib/api/flow';
import { requireRouteParam } from '$lib/utils/route-params';
import type { LayoutLoad } from './$types';

export const load: LayoutLoad = async ({ params }) => {
	const workspaceId = requireRouteParam(params.workspaceId, 'workspaceId');
	const result = await flowApi.getFeatureFlags(workspaceId);
	const flowEnabled = result.code === 0 ? (result.data?.flow_enabled ?? false) : false;
	return { workspaceId, flowEnabled };
};

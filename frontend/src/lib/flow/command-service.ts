// `CommandService`: object create and other server-governed/high-risk actions; never performs a
// direct CRDT mutation (`contracts/ui-surface-v1.md` "职责边界"). Components never build a REST
// URL themselves -- they always go through this.
//
// v0.4 delivery scope: only `createObject` is wired up (navigator "New Page"). `POST
// .../commands` (`set_title|insert_block|update_block|delete_block|move_block|archive|restore`)
// is in the frozen `rest-api-v1.md` table but is not routed server-side at this repo's baseline,
// and this session's task brief does not ask for archive/restore UI -- content/title edits
// instead go through `ObjectSession`/`ObjectRepository.setTitle` as raw CRDT updates, which the
// server surface at this baseline actually serves. `execute()` is intentionally not implemented
// here rather than wired to a 404; wiring UI to a non-existent endpoint would be a worse failure
// mode than a clear compile-time absence.

import { flowApi, type CreateFlowObjectInput, type AcceptedChange } from '$lib/api/flow';
import type { FlowError } from './types';

function newIdempotencyKey(): string {
	return typeof crypto !== 'undefined' && 'randomUUID' in crypto
		? crypto.randomUUID()
		: `idem-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export class FlowCommandService {
	async createObject(
		workspaceId: string,
		input: Omit<CreateFlowObjectInput, 'idempotency_key'>
	): Promise<AcceptedChange> {
		const result = await flowApi.createObject(workspaceId, {
			...input,
			idempotency_key: newIdempotencyKey()
		});
		if (result.code !== 0 || !result.data) {
			throw mapCreateError(result.code);
		}
		return result.data;
	}
}

function mapCreateError(code: number): FlowError {
	switch (code) {
		case 401:
			return { code: 'unauthenticated', recoverable: true };
		case 403:
			return { code: 'forbidden', recoverable: false };
		case 404:
			return { code: 'not_found', recoverable: false };
		case 409:
			return { code: 'stale_frontier', recoverable: true };
		default:
			return { code: 'invalid_update', recoverable: false };
	}
}

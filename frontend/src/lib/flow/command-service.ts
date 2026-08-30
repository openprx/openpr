// `CommandService`: object create, ticket issuance, and all other server-governed/high-risk
// actions; never performs a direct CRDT mutation (`contracts/ui-surface-v1.md` "职责边界").
// Components -- and `ObjectSession`, for ticket issuance -- never build a REST URL themselves;
// they always go through this, which is the only thing that imports `$lib/api/flow` outside that
// module's own tests. Read-only listing/history calls used by components are exposed here too
// (`listObjects`/`getHistory`) even though `ui-surface-v1.md`'s frozen `CommandService` interface
// sketch does not list them, so `FlowNavigator`/`FlowContextPanel` have exactly one adapter entry
// point instead of splitting reads through `flowApi` and writes through this class.
//
// v0.4 delivery scope:
// - `createObject`, `createTicket`, `execute` are fully wired against routes this repo's baseline
//   actually serves (`apps/api/src/routes/flow.rs::post_flow_object_command`,
//   `routes::collab::create_ticket`).
// - `diff` calls `flowApi.getDiff`, which is typed against the frozen contract but NOT routed
//   server-side at this baseline (`GET .../diff` -- see `api/flow.ts`'s header comment); calling
//   it returns a normal non-success `ApiResult`/thrown `FlowError`, not a compile-time absence,
//   since the contract requires this member to exist on the interface.
// - Error mapping below is coarse (HTTP-status-shaped) on purpose: `apps/api/src/error.rs`'s
//   `ApiError` only has 5 status-shaped variants (400/401/403/404/409) with a free-text message,
//   no machine-readable discriminator field. That collapses several distinct `FlowErrorCode`s onto
//   the same HTTP code (403 covers `forbidden`/`feature_disabled`/`policy_rejected`; 409 covers
//   `stale_frontier`/`resync_required`; 400 covers `invalid_update`/`limit_exceeded`/
//   `unsupported_protocol`). `contracts/error-mapping-v1.md` forbids branching on the message
//   string to recover the fine-grained code, so this file picks the single most common variant
//   per HTTP status and documents the collapse rather than guessing from text. Finer-grained
//   mapping needs a backend `ApiError` change (out of this package's file ownership).

import {
	flowApi,
	type AcceptedChange,
	type CollabTicket,
	type CreateFlowObjectInput,
	type CreateTicketInput,
	type ExecuteFlowCommandInput,
	type FlowDiffResponse,
	type FlowHistoryResponse,
	type FlowObjectListResponse,
	type ListFlowObjectsQuery
} from '$lib/api/flow';
import type { ApiResult } from '$lib/api/client';
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
			throw mapErrorCode(result.code);
		}
		return result.data;
	}

	/**
	 * Server-governed lifecycle/content commands (`set_title|insert_block|update_block|
	 * delete_block|move_block|archive|restore`). `options.idempotencyKey` matches the frozen
	 * `CommandService.execute` signature; callers that do not supply one get a fresh one, same as
	 * `createObject`.
	 */
	async execute(
		objectId: string,
		command: ExecuteFlowCommandInput['command'],
		options: { idempotencyKey?: string; expectedFrontier?: string; message?: string } = {}
	): Promise<AcceptedChange> {
		const result = await flowApi.executeCommand(objectId, {
			command,
			expected_frontier: options.expectedFrontier,
			idempotency_key: options.idempotencyKey ?? newIdempotencyKey(),
			message: options.message
		});
		if (result.code !== 0 || !result.data) {
			throw mapErrorCode(result.code);
		}
		return result.data;
	}

	/** Typed against the frozen contract; NOT routed server-side at this baseline -- see this
	 * file's header comment. Throws the same way `execute`/`createObject` do on a non-success
	 * result, rather than returning a partial/undefined diff. */
	async diff(objectId: string, fromSeq: number, toSeq: number): Promise<FlowDiffResponse> {
		const result = await flowApi.getDiff(objectId, { from_seq: fromSeq, to_seq: toSeq });
		if (result.code !== 0 || !result.data) {
			throw mapErrorCode(result.code);
		}
		return result.data;
	}

	/**
	 * Issues a collab WebSocket ticket. Returns the raw `ApiResult` (not throw-on-error like the
	 * other members above) because `ObjectSession` needs to branch on the exact status
	 * (401 vs. 403 vs. transient) to pick reconnect/backoff behaviour, which a thrown `FlowError`
	 * would flatten -- a deliberate narrowing of the frozen `Promise<CollabTicket>` signature,
	 * matching this codebase's existing convention of documenting such narrowings (see
	 * `types.ts`'s "narrowed to what this v0.4 delivery actually implements").
	 */
	async createTicket(input: CreateTicketInput): Promise<ApiResult<CollabTicket>> {
		return flowApi.createTicket(input);
	}

	async listObjects(workspaceId: string, query: ListFlowObjectsQuery = {}): Promise<ApiResult<FlowObjectListResponse>> {
		return flowApi.listObjects(workspaceId, query);
	}

	async getHistory(
		objectId: string,
		query: { before_seq?: number; limit?: number } = {}
	): Promise<ApiResult<FlowHistoryResponse>> {
		return flowApi.getHistory(objectId, query);
	}
}

function mapErrorCode(code: number): FlowError {
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

// `ObjectSession`: ticket/WS, ack/resume/backoff, presence, accepted/rejected/resync state machine
// (`contracts/ui-surface-v1.md` "职责边界"). Speaks the exact wire shape the server implements
// (`apps/api/src/flow/collab/frame.rs`: one JSON text WebSocket message per frame, tagged `type`,
// base64 byte fields) and the handshake order `apps/api/src/flow/collab/session.rs::run` actually
// executes: client `hello` -> server `hello` -> client `open` -> server `snapshot` -> steady state.
//
// v0.4 scope note (documented, not silent): full mid-stream mismatch resync
// (`collab-protocol-v1.md` "客户端仅在 accepted.seq == last_applied_seq+1 时应用...立即停止该
// document apply并请求 resync/bootstrap") is implemented here as "close and reconnect", which gets
// a fresh, consistent `snapshot` from the same loader REST `Bootstrap` would use
// (`rest-api-v1.md`: "WS `open` 与 REST endpoint 使用同一 loader"). It does not attempt to splice a
// partial tail into a live document. `GET .../bootstrap` IS routed server-side at this repo's
// baseline now (`ObjectRepository.bootstrap`/`replaceWithAccepted` use it directly for the
// recovery-draft flow), but reusing it here instead of a WS reconnect would only change which
// transport re-delivers the same snapshot+tail -- the actually-missing piece for a true
// `stale_frontier/resync_required` "persist intent -> bootstrap -> replaceWithAccepted -> replay"
// flow is a local pending-update outbox to replay, which is explicitly out of scope this round:
// Offline pending/outbox is out of scope (`ui-surface-v1.md` "v0.5：presence/cursor、offline
// pending...").

import type { CollabTicket } from '$lib/api/flow';
import { apiClient, type ApiResult } from '$lib/api/client';
import { writable, type Readable } from 'svelte/store';
import type {
	EngineUpdate,
	FlowError,
	FlowLimitsNegotiation,
	FlowLimitsV1,
	ObjectHandle,
	ObjectSessionContract,
	SemanticIntent,
	SyncState
} from './types';
import { DEFAULT_FLOW_LIMITS, checkUpdateBytes } from './limits';
import {
	clientError,
	drainDisposition,
	flowErrorFromCloseCode,
	flowErrorFromEnvelope,
	flowErrorFromRejectedFrame
} from './errors';

export interface AcceptedNotice {
	readonly headSeq: number;
	readonly headFrontier: string;
	readonly projectionSeq: number;
	readonly eventId: string;
	/** Present only for updates this connection did not itself submit (remote/other-session commits). */
	readonly remoteBytes?: Uint8Array;
}

export interface SnapshotPayload {
	readonly snapshotSeq: number;
	readonly headSeq: number;
	readonly snapshotBytes: Uint8Array;
	readonly tailUpdates: ReadonlyArray<{ seq: number; bytes: Uint8Array }>;
	readonly headFrontier: string;
}

export type DisconnectReason = 'manual' | 'network' | 'stale_frontier' | 'auth_expired';

interface SessionHooks {
	onSnapshot(payload: SnapshotPayload): void;
	onAccepted(notice: AcceptedNotice): void;
	onFlowError(error: FlowError): void;
}

const PROTOCOL_VERSION = 1;
/** How long `connect()` waits for the first `snapshot` before giving up on this attempt. */
const CONNECT_TIMEOUT_MS = 15_000;
const BACKOFF_STEPS_MS = [250, 500, 1000, 2000, 5000];

function toBase64(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}

function fromBase64(value: string): Uint8Array {
	const binary = atob(value);
	const out = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
	return out;
}

function wsBaseUrl(): string {
	const configured = import.meta.env.VITE_API_BASE_URL as string | undefined;
	if (configured) {
		return configured.replace(/^http/, 'ws');
	}
	if (typeof window === 'undefined') return '';
	return window.location.origin.replace(/^http/, 'ws');
}

type PendingResolve = { resolve: () => void; reject: (error: FlowError) => void };

/** The subset of `CommandService` `ObjectSession` needs for ticket issuance -- kept narrow so
 * this module depends on an interface, not the concrete class, matching `types.ts`'s existing
 * "narrowed to what this v0.4 delivery actually implements" convention. */
export interface TicketIssuer {
	createTicket(input: {
		workspace_id: string;
		document_id: string;
		client_id: string;
		origin: string;
	}): Promise<ApiResult<CollabTicket>>;
}

export class LoroObjectSession implements ObjectSessionContract {
	private readonly hooks: SessionHooks;
	private readonly commandService: TicketIssuer;
	private readonly clientId: string;
	private socket: WebSocket | null = null;
	private handle: ObjectHandle | null = null;
	private readonly pending = new Map<string, PendingResolve>();
	private reconnectAttempt = 0;
	private lastReconnectDelayMs: number | null = null;
	private connectTimeout: ReturnType<typeof setTimeout> | null = null;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private disposed = false;
	private manualClose = false;
	private readonly stateStore = writable<SyncState>('local');
	/** Effective ceilings for this session's client-side pre-checks. Starts at the pre-bootstrap
	 * fallback and is replaced wholesale the first time a real `Bootstrap.limits` payload is
	 * negotiated (`adoptLimits`); never merged field-by-field. */
	private limits: FlowLimitsV1 = DEFAULT_FLOW_LIMITS;
	/** True once a `Bootstrap.limits` payload was rejected by version negotiation. Local writes
	 * are blocked while set (`limits-v1.md`: "Client 不得自行放宽或缓存跨 `version` limits"). */
	private unknownLimitsVersion: Extract<
		FlowLimitsNegotiation,
		{ outcome: 'unknownVersion' }
	> | null = null;
	/** Resolved/rejected the first time this connection reaches `snapshot` or a terminal failure.
	 * `connect()` awaits this so callers (`ObjectRepository.open`) never hand a doc to
	 * `EditorAdapter`/a navigator reorder before the doc actually has server content imported --
	 * without this, `LoroSyncPlugin` can initialize a ProseMirror doc against a still-empty local
	 * Loro doc and `doc.subscribeLocalUpdates` ships that empty-init as a spurious `update` frame
	 * with `base_frontier: ""` before the real snapshot has even arrived. Found via manual E2E,
	 * not by inspection -- see the delivery report. */
	private firstSync: { resolve: () => void; reject: (error: FlowError) => void } | null = null;

	constructor(hooks: SessionHooks, commandService: TicketIssuer) {
		this.hooks = hooks;
		this.commandService = commandService;
		this.clientId =
			typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID()
				: `client-${Math.random().toString(36).slice(2)}`;
	}

	get state(): Readable<SyncState> {
		return { subscribe: this.stateStore.subscribe };
	}

	private setState(next: SyncState): void {
		this.stateStore.set(next);
	}

	/** Adopts (or refuses) a live `Bootstrap.limits` payload for this session.
	 *
	 * On `supported` the server's numbers replace whatever this session was using -- including
	 * undoing an earlier read-only degradation, because a later bootstrap that negotiates cleanly
	 * is proof the mismatch is gone. On `unknownVersion` the session drops to the `read_only`
	 * sync state and `submit()` refuses every local write from then on, rather than falling back
	 * to this build's compiled-in ceilings. */
	adoptLimits(negotiation: FlowLimitsNegotiation): void {
		if (negotiation.outcome === 'supported') {
			this.limits = negotiation.limits;
			this.unknownLimitsVersion = null;
			return;
		}
		this.unknownLimitsVersion = negotiation;
		this.setState('read_only');
	}

	/** The ceilings this session currently pre-checks against. */
	get effectiveLimits(): FlowLimitsV1 {
		return this.limits;
	}

	/** Whether local writes are blocked because `Bootstrap.limits` failed version negotiation. */
	get isReadOnly(): boolean {
		return this.unknownLimitsVersion !== null;
	}

	async connect(handle: ObjectHandle): Promise<void> {
		this.handle = handle;
		this.manualClose = false;
		const firstSync = new Promise<void>((resolve, reject) => {
			this.firstSync = { resolve, reject };
		});
		const timeout = new Promise<void>((_, reject) => {
			// A connect that never completes is a LOCAL condition, so it is marked as such. It used
			// to reject with a hand-built `server_draining{reason:'contention'}` -- a required,
			// server-produced discriminator manufactured by the client, which would have shown a
			// server-contention banner for what is really "this browser got no answer", and would
			// have made every "the UI honoured the server's reason" assertion pass vacuously.
			this.connectTimeout = setTimeout(
				() => reject(clientError('resync_required')),
				CONNECT_TIMEOUT_MS
			);
		});
		await this.openSocket();
		try {
			await Promise.race([firstSync, timeout]);
		} finally {
			this.firstSync = null;
			if (this.connectTimeout) {
				clearTimeout(this.connectTimeout);
				this.connectTimeout = null;
			}
		}
	}

	private async openSocket(): Promise<void> {
		if (this.disposed || !this.handle) return;
		this.setState('reconnecting');

		const clientToken = apiClient.getToken();
		if (!clientToken) {
			this.setState('auth_required');
			this.firstSync?.reject({ code: 'unauthenticated', recoverable: true });
			return;
		}

		let ticketResult = await this.commandService.createTicket({
			workspace_id: this.handle.workspaceId,
			document_id: this.handle.documentId,
			client_id: this.clientId,
			origin: typeof window !== 'undefined' ? window.location.origin : 'sylvode-flow-web'
		});

		if (ticketResult.code === 401) {
			// `ui-surface-v1.md` "连接、refresh 与恢复状态机" step 3: "ticket 401：最多一次
			// `ensureFreshAccessToken` + 新 ticket；仍失败进入 `auth_required`，停止自动 loop."
			const refreshed = await apiClient.ensureFreshAccessToken();
			if (refreshed) {
				ticketResult = await this.commandService.createTicket({
					workspace_id: this.handle.workspaceId,
					document_id: this.handle.documentId,
					client_id: this.clientId,
					origin: typeof window !== 'undefined' ? window.location.origin : 'sylvode-flow-web'
				});
			}
		}

		if (ticketResult.code === 401) {
			this.setState('auth_required');
			this.firstSync?.reject({ code: 'unauthenticated', recoverable: true });
			return;
		}
		if (ticketResult.code === 403) {
			// `forbidden`/`feature_disabled`: not recoverable by retrying (`error-mapping-v1.md`
			// UI invariant "auth/feature/forbidden 不自动无限重连"), unlike a transient failure.
			// Both map to HTTP-equivalent 403, so the stable `error_code` -- not the status, and
			// never the message -- decides which one this is.
			this.setState('error');
			const error: FlowError = flowErrorFromEnvelope(ticketResult) ?? {
				code: 'forbidden',
				recoverable: false
			};
			this.hooks.onFlowError(error);
			this.firstSync?.reject(error);
			return;
		}
		if (ticketResult.code !== 0 || !ticketResult.data) {
			// A typed rejection on the ticket endpoint is the REST half of the same producer the
			// WebSocket `rejected` frame carries -- most importantly `server_draining`, whose
			// required `details.reason` decides whether this client waits out a drain or retries a
			// contention. Reconnecting blindly here would discard that discriminator.
			const rejection = flowErrorFromEnvelope(ticketResult);
			if (rejection) {
				this.applyFlowError(rejection);
				return;
			}
			this.scheduleReconnect();
			return;
		}

		const url = `${wsBaseUrl()}${ticketResult.data.websocket_url}`;
		const socket = new WebSocket(url);
		this.socket = socket;

		socket.addEventListener('open', () => {
			this.sendFrame({
				type: 'hello',
				protocol_version: PROTOCOL_VERSION,
				capabilities: [],
				client_id: this.clientId,
				session_id: this.clientId
			});
		});

		let sentOpen = false;
		socket.addEventListener('message', (event: MessageEvent<string>) => {
			let frame: Record<string, unknown>;
			try {
				frame = JSON.parse(event.data) as Record<string, unknown>;
			} catch {
				return;
			}

			if (frame.type === 'hello' && !sentOpen) {
				sentOpen = true;
				this.sendFrame({
					type: 'open',
					protocol_version: PROTOCOL_VERSION,
					document_id: this.handle?.documentId,
					known_seq: null,
					known_frontier: null
				});
				return;
			}

			this.handleFrame(frame);
		});

		socket.addEventListener('close', (event) => {
			this.socket = null;
			if (this.manualClose || this.disposed) return;
			// The frozen WS close codes (`error-mapping-v1.md`'s "WS close/control" column) are
			// machine-readable discriminators in their own right, including 4410's drain payload.
			const closed = flowErrorFromCloseCode(event.code, event.reason ?? '');
			if (closed) {
				this.applyFlowError(closed);
				return;
			}
			this.scheduleReconnect();
		});

		socket.addEventListener('error', () => {
			// Handled by the subsequent `close` event; nothing actionable here.
		});
	}

	private handleFrame(frame: Record<string, unknown>): void {
		switch (frame.type) {
			case 'snapshot': {
				this.setState('resyncing');
				const tailUpdates = (frame.tail_updates as Array<Record<string, unknown>>).map((u) => ({
					seq: u.seq as number,
					bytes: fromBase64(u.bytes as string)
				}));
				this.hooks.onSnapshot({
					snapshotSeq: frame.snapshot_seq as number,
					headSeq: frame.head_seq as number,
					snapshotBytes: fromBase64(frame.snapshot as string),
					tailUpdates,
					headFrontier: frame.head_frontier as string
				});
				this.reconnectAttempt = 0;
				// `snapshot` establishes the baseline document state, not a save confirmation.
				// `contracts/ui-surface-v1.md` "连接、refresh 与恢复状态机": "只有带 matching
				// update id 和 events-v1.md event id 的 accepted frame 可进入 saved" -- entering
				// `saved` here would tell the user an edit was durably persisted before this
				// connection has ever received a matching `accepted` frame for one. `local`
				// reflects "loaded and mirrors the server as of this snapshot, no in-flight or
				// confirmed local edit" without making that false claim.
				this.setState('local');
				this.firstSync?.resolve();
				break;
			}
			case 'accepted': {
				const updateId = frame.update_id as string;
				const waiter = this.pending.get(updateId);
				if (waiter) {
					this.pending.delete(updateId);
					waiter.resolve();
					this.hooks.onAccepted({
						headSeq: frame.head_seq as number,
						headFrontier: frame.head_frontier as string,
						projectionSeq: frame.projection_seq as number,
						eventId: frame.event_id as string
					});
					this.setState('saved');
				} else {
					// This `accepted` is for an update THIS connection did not submit -- i.e. another
					// session committed a change. `apps/api/src/flow/collab/session.rs` broadcasts
					// `Frame::Accepted` (head_seq/frontier/event_id only, no `bytes`) to every other
					// session on the document; there is no second broadcast frame carrying the actual
					// update bytes at this repo's baseline. A full reconnect (fresh `snapshot`) is
					// therefore the only way this client can pick up someone else's content -- the
					// same recovery path `resync_required` already uses. Documented backend gap, not
					// a client bug: true incremental multi-session fan-out needs a server-side change
					// outside `frontend/**`.
					this.setState('resyncing');
					void this.reconnect('stale_frontier');
				}
				break;
			}
			case 'rejected': {
				const error = flowErrorFromRejectedFrame(frame);
				if (!error) break;
				const updateId = frame.update_id as string | undefined;
				if (updateId) {
					const waiter = this.pending.get(updateId);
					if (waiter) {
						this.pending.delete(updateId);
						waiter.reject(error);
					}
				}
				this.applyFlowError(error);
				break;
			}
			case 'resync': {
				this.setState('resyncing');
				this.reconnect('stale_frontier');
				break;
			}
			case 'ping': {
				this.sendFrame({ type: 'pong', protocol_version: PROTOCOL_VERSION, nonce: frame.nonce });
				break;
			}
			default:
				break;
		}
	}

	/** The single place a `FlowError` -- from a REST envelope, a `rejected` control frame or a
	 * close code -- turns into a sync state and a user-visible report.
	 *
	 * `server_draining`'s two reasons share a stable code and MUST diverge here, in exactly the
	 * ways `error-mapping-v1.md` freezes: `drain` gives up this connection and waits out the
	 * advertised `retry_after_ms` before reconnecting; `contention` keeps the socket and the
	 * accepted head untouched and lets the caller retry the write. A `server_draining` with a
	 * missing or unknown `details.reason` is a protocol violation, not a third behaviour: it is
	 * reported and NOT acted on, because both possible guesses are harmful (retrying into a
	 * draining instance, or showing maintenance for a lock conflict). */
	private applyFlowError(error: FlowError): void {
		if (error.code === 'server_draining') {
			const disposition = drainDisposition(error);
			this.hooks.onFlowError(error);
			// An error that arrives before this connection ever reached `snapshot` means the OPEN
			// itself failed, whatever the reason says about retrying afterwards. Resolving that
			// distinction here is what stops `connect()` from hanging until its timeout on a drain.
			this.firstSync?.reject(error);
			if (!disposition) return;
			this.setState(disposition.syncState);
			if (!disposition.keepConnection) this.closeCurrentSocket();
			if (disposition.reconnects) this.scheduleReconnect(disposition.retryAfterMs);
			return;
		}
		if (error.code === 'resync_required') {
			this.setState('resyncing');
			void this.reconnect('stale_frontier');
			return;
		}
		if (error.code === 'unauthenticated') {
			this.setState('auth_required');
			this.firstSync?.reject(error);
			return;
		}
		this.setState('error');
		this.hooks.onFlowError(error);
		this.firstSync?.reject(error);
	}

	/** Closes this connection on purpose.
	 *
	 * The resulting `close` event must NOT run the generic lost-connection path: that path
	 * schedules a plain jittered backoff, and because `scheduleReconnect` is a no-op while a timer
	 * is already pending, it would win the race against the drain's own `retry_after_ms` floor and
	 * silently reconnect ~60 ms into a 1.5 s drain. Found by the UI state test, not by reading. */
	private closeCurrentSocket(): void {
		const socket = this.socket;
		this.socket = null;
		if (!socket) return;
		this.manualClose = true;
		socket.close();
		this.manualClose = false;
	}

	private sendFrame(frame: Record<string, unknown>): void {
		if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return;
		this.socket.send(JSON.stringify(frame));
	}

	async submit(update: EngineUpdate, _intent: SemanticIntent): Promise<void> {
		if (!this.socket || this.socket.readyState !== WebSocket.OPEN || !this.handle) {
			// No socket is a local condition, not a server rejection -- see `clientError`.
			throw clientError('resync_required');
		}
		// `Bootstrap.limits` failed version negotiation: this client cannot know the real
		// ceilings, and `limits-v1.md` forbids writing under its own cached/compiled-in ones
		// ("Client 不得自行放宽或缓存跨 `version` limits"). Refuse locally instead of sending.
		const unknownVersion = this.unknownLimitsVersion;
		if (unknownVersion) {
			throw {
				code: 'policy_rejected',
				recoverable: false,
				details: {
					reason: 'unknown_limits_version',
					limits_version: unknownVersion.version,
					negotiation: unknownVersion.reason
				}
			} satisfies FlowError;
		}
		// Client-side pre-check (`ui-surface-v1.md` "Session/CommandService 在编码前按
		// limits-v1.md 检查 update/frame...超限不进入 IndexedDB outbox、不发网络请求"): reject an
		// obviously over-budget update before spending a round trip. This does not replace the
		// server's own re-validation of the same limit.
		const violation = checkUpdateBytes(update.bytes, this.limits);
		if (violation) {
			throw {
				code: 'limit_exceeded',
				recoverable: false,
				details: {
					limit_kind: violation.limitKind,
					limit: violation.limit,
					observed: violation.observed
				}
			} satisfies FlowError;
		}
		const updateId =
			typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID()
				: `${Date.now()}-${Math.random()}`;

		this.setState('saving');
		return new Promise<void>((resolve, reject) => {
			this.pending.set(updateId, { resolve, reject });
			this.sendFrame({
				type: 'update',
				protocol_version: PROTOCOL_VERSION,
				document_id: this.handle?.documentId,
				update_id: updateId,
				base_frontier: update.baseFrontier,
				bytes: toBase64(update.bytes),
				idempotency_key: updateId,
				origin: 'local',
				message: null
			});
		});
	}

	setPresence(): void {
		// Presence is v0.5 scope (`ui-surface-v1.md`); intentionally not implemented in v0.4.
	}

	async reconnect(_reason: DisconnectReason): Promise<void> {
		this.manualClose = false;
		this.socket?.close();
		this.socket = null;
		await this.openSocket();
	}

	/** `ui-surface-v1.md` step 6: full-jitter exponential backoff 250ms->5s, with
	 * `server_draining.details.retry_after_ms` acting as a LOWER bound when the server supplied
	 * one -- reconnecting sooner than the server asked is exactly what a drain is trying to
	 * prevent, so the advertised floor wins over a shorter jittered delay. */
	private scheduleReconnect(retryAfterMs: number | null = null): void {
		if (this.disposed || this.reconnectTimer) return;
		this.setState('reconnecting');
		const step = BACKOFF_STEPS_MS[Math.min(this.reconnectAttempt, BACKOFF_STEPS_MS.length - 1)];
		const jitter = Math.random() * step;
		const delay = Math.max(jitter, retryAfterMs ?? 0);
		this.reconnectAttempt += 1;
		this.lastReconnectDelayMs = delay;
		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			void this.openSocket();
		}, delay);
	}

	/** The delay the most recent reconnect was scheduled with. Read by the UI state tests to
	 * prove `retry_after_ms` was honoured as a floor rather than silently dropped. */
	get lastScheduledReconnectDelayMs(): number | null {
		return this.lastReconnectDelayMs;
	}

	async dispose(): Promise<void> {
		this.disposed = true;
		this.manualClose = true;
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		if (this.connectTimeout) {
			clearTimeout(this.connectTimeout);
			this.connectTimeout = null;
		}
		for (const waiter of this.pending.values()) {
			// Teardown, not a server drain: the caller navigated away or the route was destroyed.
			waiter.reject(clientError('resync_required'));
		}
		this.pending.clear();
		this.socket?.close();
		this.socket = null;
	}
}

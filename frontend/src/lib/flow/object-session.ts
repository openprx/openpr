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
// partial tail into a live document. Given `POST .../bootstrap` is not yet routed server-side at
// this repo's baseline, this is also the only bootstrap path Web v0.4 has.
// Offline pending/outbox is out of scope (`ui-surface-v1.md` "v0.5：presence/cursor、offline
// pending...").

import { flowApi } from '$lib/api/flow';
import { apiClient } from '$lib/api/client';
import { writable, type Readable } from 'svelte/store';
import type {
	EngineUpdate,
	FlowError,
	FlowErrorCode,
	ObjectHandle,
	ObjectSessionContract,
	SemanticIntent,
	SyncState
} from './types';

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

export class LoroObjectSession implements ObjectSessionContract {
	private readonly hooks: SessionHooks;
	private readonly clientId: string;
	private socket: WebSocket | null = null;
	private handle: ObjectHandle | null = null;
	private readonly pending = new Map<string, PendingResolve>();
	private reconnectAttempt = 0;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private disposed = false;
	private manualClose = false;
	private readonly stateStore = writable<SyncState>('local');
	/** Resolved/rejected the first time this connection reaches `snapshot` or a terminal failure.
	 * `connect()` awaits this so callers (`ObjectRepository.open`) never hand a doc to
	 * `EditorAdapter`/a navigator reorder before the doc actually has server content imported --
	 * without this, `LoroSyncPlugin` can initialize a ProseMirror doc against a still-empty local
	 * Loro doc and `doc.subscribeLocalUpdates` ships that empty-init as a spurious `update` frame
	 * with `base_frontier: ""` before the real snapshot has even arrived. Found via manual E2E,
	 * not by inspection -- see the delivery report. */
	private firstSync: { resolve: () => void; reject: (error: FlowError) => void } | null = null;

	constructor(hooks: SessionHooks) {
		this.hooks = hooks;
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

	async connect(handle: ObjectHandle): Promise<void> {
		this.handle = handle;
		this.manualClose = false;
		const firstSync = new Promise<void>((resolve, reject) => {
			this.firstSync = { resolve, reject };
		});
		const timeout = new Promise<void>((_, reject) => {
			setTimeout(() => reject({ code: 'server_draining', recoverable: true, details: { reason: 'contention' } }), 15_000);
		});
		await this.openSocket();
		try {
			await Promise.race([firstSync, timeout]);
		} finally {
			this.firstSync = null;
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

		const ticketResult = await flowApi.createTicket({
			workspace_id: this.handle.workspaceId,
			document_id: this.handle.documentId,
			client_id: this.clientId,
			origin: typeof window !== 'undefined' ? window.location.origin : 'sylvode-flow-web'
		});

		if (ticketResult.code === 401) {
			this.setState('auth_required');
			this.firstSync?.reject({ code: 'unauthenticated', recoverable: true });
			return;
		}
		if (ticketResult.code === 403) {
			// `forbidden`/`feature_disabled`: not recoverable by retrying (`error-mapping-v1.md`
			// UI invariant "auth/feature/forbidden 不自动无限重连"), unlike a transient failure.
			this.setState('error');
			const error: FlowError = { code: 'forbidden', recoverable: false };
			this.hooks.onFlowError(error);
			this.firstSync?.reject(error);
			return;
		}
		if (ticketResult.code !== 0 || !ticketResult.data) {
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
			if (event.code === 4401) {
				this.setState('auth_required');
				this.firstSync?.reject({ code: 'unauthenticated', recoverable: true });
				return;
			}
			if (event.code === 4403 || event.code === 4404) {
				this.setState('error');
				const error: FlowError = {
					code: event.code === 4403 ? 'forbidden' : 'feature_disabled',
					recoverable: false
				};
				this.hooks.onFlowError(error);
				this.firstSync?.reject(error);
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
				this.setState('saved');
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
				const code = frame.code as FlowErrorCode;
				const updateId = frame.update_id as string | undefined;
				const error: FlowError = { code, recoverable: Boolean(frame.recoverable), details: frame.details };
				if (updateId) {
					const waiter = this.pending.get(updateId);
					if (waiter) {
						this.pending.delete(updateId);
						waiter.reject(error);
					}
				}
				if (code === 'server_draining') {
					const details = frame.details as { reason?: string } | undefined;
					if (details?.reason === 'drain') {
						this.setState('reconnecting');
					}
					// reason === "contention": connection stays open, caller can retry the write.
				} else if (code === 'resync_required') {
					this.setState('resyncing');
					this.reconnect('stale_frontier');
				} else {
					this.hooks.onFlowError(error);
				}
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

	private sendFrame(frame: Record<string, unknown>): void {
		if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return;
		this.socket.send(JSON.stringify(frame));
	}

	async submit(update: EngineUpdate, _intent: SemanticIntent): Promise<void> {
		if (!this.socket || this.socket.readyState !== WebSocket.OPEN || !this.handle) {
			throw { code: 'server_draining', recoverable: true, details: { reason: 'contention' } } satisfies FlowError;
		}
		const updateId =
			typeof crypto !== 'undefined' && 'randomUUID' in crypto ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`;

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

	private scheduleReconnect(): void {
		if (this.disposed || this.reconnectTimer) return;
		this.setState('reconnecting');
		const step = BACKOFF_STEPS_MS[Math.min(this.reconnectAttempt, BACKOFF_STEPS_MS.length - 1)];
		const jitter = Math.random() * step;
		this.reconnectAttempt += 1;
		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			void this.openSocket();
		}, jitter);
	}

	async dispose(): Promise<void> {
		this.disposed = true;
		this.manualClose = true;
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		for (const waiter of this.pending.values()) {
			waiter.reject({ code: 'server_draining', recoverable: true, details: { reason: 'drain' } });
		}
		this.pending.clear();
		this.socket?.close();
		this.socket = null;
	}
}

/**
 * Hard gate `web_ime_undo_selection_and_sync_state` -- the automatable half -- and the UI leg of
 * `gates/gate-commands.md`'s v0.4 Error verifier requirement:
 *
 *   "Error verifier 必须用同一 producer fixture 在 REST、MCP HTTP/SSE/stdio、CLI JSON/table 与
 *    UI state tests 分别注入 `server_draining.details.reason=drain|contention`：两者稳定 code 相同
 *    且 CLI 均 exit 9，但 WS close/keep-open、JSON discriminator、human/UI i18n key 与 retry 状态
 *    必须不同；missing/unknown reason、message-based branching 或只测 WS 均失败"
 *
 * The fixture below (`DRAIN_FIXTURE`) is a transcription of what the server's ONE shared drain
 * producer emits -- `apps/api/src/flow/collab/runtime.rs::CollabRuntime::{begin_workspace_drain,
 * ensure_workspace_accepting}`, whose own test `shared_workspace_drain_fixture_produces_rest_and_
 * ws_drain_wire_shapes` asserts all three shapes from a single guard: the REST envelope
 * (`error_code:"server_draining"`, `details:{reason,retry_after_ms}`, HTTP 200), the WS `rejected`
 * control frame (`details.reason`), and the 4410 close whose `reason` string is the same JSON
 * (`apps/api/src/flow/collab/frame.rs::DrainSignal::{details,close_reason}`).
 *
 * "只测 WS 均失败" is why this suite injects that one fixture on THREE UI entry points -- the REST
 * ticket envelope, the WebSocket `rejected` frame, and the WebSocket close code -- and requires
 * the same verdict from each.
 *
 * NOT COVERED HERE, and not claimed to be: IME composition behaviour, undo/redo through the
 * ProseMirror+Loro binding, and selection restore all require a mounted `EditorView` in a real
 * browser with a real input method. Those are the manual `page_editor` sign-off key
 * (`gate-commands.md` v0.4 "人工 keys"), and the checks are `skip()`ped below by name rather than
 * silently omitted.
 *
 * Run standalone: `bun tests/flow-ui-state.test.ts`
 */

import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	assertNotEqual,
	finish,
	waitFor
} from './support/harness';
import {
	DRAIN_CLOSE_CODE,
	drainDisposition,
	flowErrorFromCloseCode,
	flowErrorFromEnvelope,
	flowErrorFromRejectedFrame,
	flowErrorI18nKey,
	isServerReported,
	parseRetryAfterMs,
	parseServerDrainingReason
} from '../src/lib/flow/errors';
import type { FlowError, SyncState } from '../src/lib/flow/types';

const suite = new Suite('web_ime_undo_selection_and_sync_state');

const { clientError: clientErrorRef } = await import('../src/lib/flow/errors');

suite.check('authorization churn is retryable and machine-distinct from forbidden on REST and WS', () => {
	const envelope = flowErrorFromEnvelope({
		code: 409,
		error_code: 'authorization_churn',
		details: { retry_after_ms: 200 }
	});
	const frame = flowErrorFromRejectedFrame({
		type: 'rejected',
		code: 'authorization_churn',
		recoverable: true,
		details: { retry_after_ms: 200 }
	});
	const forbidden = flowErrorFromEnvelope({ code: 403, error_code: 'forbidden' });
	for (const [surface, error] of [
		['REST', envelope],
		['WS', frame]
	] as const) {
		assert(error !== null, `${surface} must recognize authorization_churn`);
		assertEqual(error.code, 'authorization_churn', `${surface} stable code`);
		assertEqual(error.recoverable, true, `${surface} retry disposition`);
		assertEqual(parseRetryAfterMs(error.details), 200, `${surface} retry hint`);
		assertEqual(flowErrorI18nKey(error), 'flow.error.authorization_churn', `${surface} UI key`);
	}
	assert(forbidden !== null, 'forbidden fixture must parse');
	assertEqual(forbidden.recoverable, false, 'forbidden is not retryable');
	assertNotEqual(envelope?.code, forbidden.code, 'stable codes must not collapse');
});

// =============================================================================================
// The one producer fixture, in the three shapes the server emits it in.
// =============================================================================================

const RETRY_AFTER_MS = 1_500;

/** `apps/api/src/flow/collab/frame.rs::DrainSignal::details()` -- `{"reason","retry_after_ms"}`. */
function drainDetails(
	reason: string,
	retryAfterMs: number = RETRY_AFTER_MS
): Record<string, unknown> {
	return { reason, retry_after_ms: retryAfterMs };
}

/** The REST envelope shape (`apps/api/src/error.rs::IntoResponse for ApiError`): transport status
 * is HTTP 200, envelope `code` is the business code, `error_code` is the stable code, `details`
 * carries the required discriminator. */
function restEnvelope(reason: string, message = 'server_draining') {
	return {
		code: 409,
		message,
		data: null,
		error_code: 'server_draining',
		details: drainDetails(reason)
	};
}

/** The WS control frame (`Frame::Rejected`). */
function rejectedFrame(reason: string, message = 'server_draining'): Record<string, unknown> {
	return {
		type: 'rejected',
		protocol_version: 1,
		code: 'server_draining',
		recoverable: true,
		message,
		details: drainDetails(reason)
	};
}

/** The WS close (`DRAIN_CLOSE_CODE` 4410 with `DrainSignal::close_reason()` as the reason). */
function drainClose(reason: string): { code: number; reason: string } {
	return { code: DRAIN_CLOSE_CODE, reason: JSON.stringify(drainDetails(reason)) };
}

const SURFACES = [
	{ name: 'REST envelope', read: (reason: string) => flowErrorFromEnvelope(restEnvelope(reason)) },
	{
		name: 'WS rejected frame',
		read: (reason: string) => flowErrorFromRejectedFrame(rejectedFrame(reason))
	},
	{ name: 'WS close code', read: (reason: string) => flowErrorFromCloseCode(...closeArgs(reason)) }
] as const;

function closeArgs(reason: string): [number, string] {
	const close = drainClose(reason);
	return [close.code, close.reason];
}

// ---- 1. same code everywhere, different key / retry state / connection fate ------------------

suite.check('every surface reads the same fixture into the same stable code', () => {
	for (const surface of SURFACES) {
		for (const reason of ['drain', 'contention']) {
			const error = surface.read(reason);
			assert(error !== null, `${surface.name} did not produce a Flow error for reason=${reason}`);
			assertEqual(
				error.code,
				'server_draining',
				`${surface.name} reason=${reason} produced the wrong stable code`
			);
			assertEqual(
				error.recoverable,
				true,
				`${surface.name} reason=${reason} must stay recoverable`
			);
		}
	}
});

suite.check('every surface marks the fixture as server-reported, and only the wire can', () => {
	// Without this, every assertion below would also pass against a build that ignores the wire
	// and mints `server_draining{reason}` locally -- which is precisely what this client used to
	// do in its connect timeout and its teardown path.
	for (const surface of SURFACES) {
		for (const reason of ['drain', 'contention']) {
			const error = surface.read(reason) as FlowError;
			assertEqual(
				error.origin,
				'server',
				`${surface.name} reason=${reason} is not marked server-reported`
			);
			assertEqual(isServerReported(error), true, `${surface.name} reason=${reason}`);
		}
	}
	const local = clientErrorRef('resync_required');
	assertEqual(
		isServerReported(local),
		false,
		'a locally synthesised error must not read as server-reported'
	);
});

suite.check('drain and contention resolve to DIFFERENT i18n keys on every surface', () => {
	for (const surface of SURFACES) {
		const drainKey = flowErrorI18nKey(surface.read('drain') as FlowError);
		const contentionKey = flowErrorI18nKey(surface.read('contention') as FlowError);
		assertEqual(drainKey, 'flow.error.server_draining.drain', `${surface.name} drain key`);
		assertEqual(
			contentionKey,
			'flow.error.server_draining.contention',
			`${surface.name} contention key`
		);
		assertNotEqual(drainKey, contentionKey, `${surface.name} collapsed both reasons onto one key`);
	}
});

suite.check('drain and contention resolve to DIFFERENT retry states and connection fates', () => {
	for (const surface of SURFACES) {
		const drain = drainDisposition(surface.read('drain') as FlowError);
		const contention = drainDisposition(surface.read('contention') as FlowError);
		assert(
			drain !== null && contention !== null,
			`${surface.name} failed to classify a well-formed fixture`
		);

		assertEqual(
			drain.syncState,
			'reconnecting',
			`${surface.name}: drain must show a reconnect/maintenance state`
		);
		assertEqual(
			contention.syncState,
			'local',
			`${surface.name}: contention must not impersonate maintenance`
		);
		assertNotEqual(
			drain.syncState,
			contention.syncState,
			`${surface.name}: both reasons show the same retry state`
		);

		assertEqual(drain.keepConnection, false, `${surface.name}: drain must give up the connection`);
		assertEqual(
			contention.keepConnection,
			true,
			`${surface.name}: contention must keep the connection`
		);

		assertEqual(drain.closeCode, DRAIN_CLOSE_CODE, `${surface.name}: drain's close code`);
		assertEqual(contention.closeCode, null, `${surface.name}: contention has no close code`);

		assertEqual(
			drain.retryAfterMs,
			RETRY_AFTER_MS,
			`${surface.name}: drain must carry the advertised retry floor`
		);
	}
});

// ---- 2. missing / unknown reason fails closed ------------------------------------------------

suite.check('a missing or unknown reason is refused, never defaulted to contention', () => {
	const malformed: Array<[string, unknown]> = [
		['no details at all', undefined],
		['empty details', {}],
		['null reason', { reason: null }],
		['numeric reason', { reason: 9 }],
		['unknown reason', { reason: 'maintenance' }],
		['reason nested wrongly', { details: { reason: 'drain' } }],
		['reason only in the message', { retry_after_ms: 500 }]
	];
	for (const [label, details] of malformed) {
		assertEqual(parseServerDrainingReason(details), null, `${label}: reason must not be inferred`);
		const error: FlowError = { code: 'server_draining', recoverable: true, details };
		assertEqual(drainDisposition(error), null, `${label}: must not produce a disposition`);
		const key = flowErrorI18nKey(error);
		assertNotEqual(
			key,
			'flow.error.server_draining.contention',
			`${label}: silently rendered as a contention retry`
		);
		assertNotEqual(
			key,
			'flow.error.server_draining.drain',
			`${label}: silently rendered as a drain`
		);
		assertNotEqual(key, 'flow.error.server_draining', `${label}: rendered the forbidden base key`);
		assertEqual(
			key,
			'flow.error.unsupported_protocol',
			`${label}: must fail closed onto a protocol-violation key`
		);
	}
});

suite.check('a 4410 close with an unreadable payload still fails closed', () => {
	for (const payload of ['', 'not json', '{"retry_after_ms":10}', '{"reason":"nope"}']) {
		const error = flowErrorFromCloseCode(DRAIN_CLOSE_CODE, payload);
		assert(
			error !== null,
			`close 4410 with payload ${JSON.stringify(payload)} must still be a Flow error`
		);
		assertEqual(error.code, 'server_draining', 'the frozen close code is itself the stable code');
		assertEqual(
			drainDisposition(error),
			null,
			`payload ${JSON.stringify(payload)} must not yield a disposition`
		);
	}
});

// ---- 3. no message-based branching -------------------------------------------------------------

suite.check('the verdict is identical no matter what the message says', () => {
	// `error-mapping-v1.md`: "不得用 message 推断". These messages are deliberately misleading.
	const messages = [
		'',
		'server_draining',
		'maintenance in progress',
		'contention',
		'drain',
		'正在维护',
		'drain drain drain'
	];
	for (const reason of ['drain', 'contention']) {
		const baseline = drainDisposition(flowErrorFromEnvelope(restEnvelope(reason)) as FlowError);
		for (const message of messages) {
			const viaRest = drainDisposition(
				flowErrorFromEnvelope(restEnvelope(reason, message)) as FlowError
			);
			const viaFrame = drainDisposition(
				flowErrorFromRejectedFrame(rejectedFrame(reason, message)) as FlowError
			);
			assertDeepEqual(
				viaRest,
				baseline,
				`REST reason=${reason} changed verdict for message ${JSON.stringify(message)}`
			);
			assertDeepEqual(
				viaFrame,
				baseline,
				`WS reason=${reason} changed verdict for message ${JSON.stringify(message)}`
			);
		}
	}
	// The strongest form: a message that says one thing while the discriminator says the other.
	const contradictory = flowErrorFromRejectedFrame(
		rejectedFrame('contention', 'the server is draining for maintenance')
	);
	assertEqual(
		drainDisposition(contradictory as FlowError)?.reason,
		'contention',
		'the message overrode the required discriminator'
	);
});

suite.check('retry_after_ms is read only from details, and only when usable', () => {
	assertEqual(parseRetryAfterMs(drainDetails('drain', 250)), 250, 'a plain value is read');
	assertEqual(parseRetryAfterMs({ reason: 'drain' }), null, 'an absent value is null, not zero');
	assertEqual(parseRetryAfterMs({ retry_after_ms: -1 }), null, 'a negative value is refused');
	assertEqual(parseRetryAfterMs({ retry_after_ms: 'soon' }), null, 'a string value is refused');
	assertEqual(parseRetryAfterMs({ retry_after_ms: Number.NaN }), null, 'NaN is refused');
});

// ---- 3b. the REST client must actually carry the discriminator ------------------------------

await suite.checkAsync('the REST client carries error_code and details off the wire', async () => {
	// A counter-proof caught this hole: every REST assertion above feeds `flowErrorFromEnvelope`
	// a hand-built envelope, so deleting the passthrough in `ApiClient.request` -- the code that
	// actually reads the response body -- changed nothing. This drives the real client against a
	// real Response, which is where the fields either survive or are dropped.
	const { apiClient } = await import('../src/lib/api/client');
	const realFetch = globalThis.fetch;
	try {
		globalThis.fetch = (async () =>
			new Response(JSON.stringify(restEnvelope('drain')), {
				status: 200,
				headers: { 'Content-Type': 'application/json' }
			})) as typeof globalThis.fetch;
		const result = await apiClient.post('/api/v1/flow/collab/tickets', {});
		assertEqual(result.error_code, 'server_draining', 'ApiClient dropped the stable error code');
		assertEqual(
			parseServerDrainingReason(result.details),
			'drain',
			'ApiClient dropped the required server_draining discriminator'
		);
		const error = flowErrorFromEnvelope(result);
		assert(error !== null, 'a typed REST rejection must become a Flow error');
		assertEqual(
			flowErrorI18nKey(error),
			'flow.error.server_draining.drain',
			'the wire reason must reach the UI key'
		);
	} finally {
		globalThis.fetch = realFetch;
	}
});

await suite.checkAsync('a success envelope carries no error fields', async () => {
	const { apiClient } = await import('../src/lib/api/client');
	const realFetch = globalThis.fetch;
	try {
		globalThis.fetch = (async () =>
			new Response(JSON.stringify({ code: 0, message: 'ok', data: { id: 1 } }), {
				status: 200,
				headers: { 'Content-Type': 'application/json' }
			})) as typeof globalThis.fetch;
		const result = await apiClient.get('/api/v1/flow/objects');
		assertEqual(result.error_code, undefined, 'a success envelope must not invent an error code');
		assertEqual(result.details, undefined, 'a success envelope must not invent details');
		assertEqual(flowErrorFromEnvelope(result), null, 'a success envelope is not a Flow error');
	} finally {
		globalThis.fetch = realFetch;
	}
});

// ---- 4. the same fixture driven through the real session state machine -----------------------

class MockWebSocket {
	static readonly CONNECTING = 0;
	static readonly OPEN = 1;
	static readonly CLOSING = 2;
	static readonly CLOSED = 3;
	static instances: MockWebSocket[] = [];

	readonly url: string;
	readyState = MockWebSocket.CONNECTING;
	readonly sent: Array<Record<string, unknown>> = [];
	closedByClient = false;
	private readonly listeners = new Map<string, Array<(event: unknown) => void>>();

	constructor(url: string) {
		this.url = url;
		MockWebSocket.instances.push(this);
	}

	addEventListener(type: string, handler: (event: unknown) => void): void {
		const bucket = this.listeners.get(type) ?? [];
		bucket.push(handler);
		this.listeners.set(type, bucket);
	}

	send(payload: string): void {
		this.sent.push(JSON.parse(payload) as Record<string, unknown>);
	}

	close(): void {
		this.closedByClient = true;
		this.readyState = MockWebSocket.CLOSED;
		this.emit('close', { code: 1000, reason: '' });
	}

	/** Test-side: the socket finishes connecting. */
	open(): void {
		this.readyState = MockWebSocket.OPEN;
		this.emit('open', {});
	}

	/** Test-side: the server sends one frame. */
	deliver(frame: Record<string, unknown>): void {
		this.emit('message', { data: JSON.stringify(frame) });
	}

	/** Test-side: the server closes the connection. */
	serverClose(code: number, reason: string): void {
		this.readyState = MockWebSocket.CLOSED;
		this.emit('close', { code, reason });
	}

	private emit(type: string, event: unknown): void {
		for (const handler of this.listeners.get(type) ?? []) handler(event);
	}
}

interface Harness {
	session: import('../src/lib/flow/object-session').LoroObjectSession;
	states: SyncState[];
	errors: FlowError[];
	socket(): MockWebSocket;
	dispose(): Promise<void>;
}

async function connectedSession(
	options: { ticketEnvelope?: ReturnType<typeof restEnvelope> } = {}
): Promise<Harness> {
	MockWebSocket.instances = [];
	const { LoroObjectSession } = await import('../src/lib/flow/object-session');
	const { apiClient } = await import('../src/lib/api/client');
	apiClient.setToken('test-access-token');

	const states: SyncState[] = [];
	const errors: FlowError[] = [];
	const session = new LoroObjectSession(
		{
			onSnapshot: () => {},
			onAccepted: () => {},
			onFlowError: (error) => errors.push(error)
		},
		{
			createTicket: async () =>
				options.ticketEnvelope ??
				({
					code: 0,
					message: 'ok',
					data: { websocket_url: '/api/v1/collab/ws?ticket=t', expires_at: '', ticket: 't' }
				} as never)
		}
	);
	session.state.subscribe((state) => states.push(state));

	const handle = {
		objectId: '11111111-1111-4111-8111-111111111111',
		documentId: '22222222-2222-4222-8222-222222222222',
		workspaceId: '33333333-3333-4333-8333-333333333333',
		objectType: 'page',
		object: {} as never
	} as never;

	const connecting = session.connect(handle).catch((error: FlowError) => {
		errors.push(error);
	});

	if (!options.ticketEnvelope) {
		await waitFor(() => MockWebSocket.instances.length > 0, 'the session opened a socket');
		const socket = MockWebSocket.instances[0];
		socket.open();
		await waitFor(() => socket.sent.some((frame) => frame.type === 'hello'), 'client hello');
		socket.deliver({ type: 'hello', protocol_version: 1 });
		await waitFor(() => socket.sent.some((frame) => frame.type === 'open'), 'client open');
		socket.deliver({
			type: 'snapshot',
			protocol_version: 1,
			document_id: '22222222-2222-4222-8222-222222222222',
			snapshot_seq: 1,
			head_seq: 1,
			snapshot: '',
			tail_updates: [],
			head_frontier: 'f1'
		});
	}
	await connecting;

	return {
		session,
		states,
		errors,
		socket: () => MockWebSocket.instances[MockWebSocket.instances.length - 1],
		dispose: () => session.dispose()
	};
}

const realWebSocket = globalThis.WebSocket;
(globalThis as { WebSocket: unknown }).WebSocket = MockWebSocket;

await suite.checkAsync('a clean handshake lands on the loaded-not-saved state', async () => {
	const harness = await connectedSession();
	try {
		assertEqual(harness.states.at(-1), 'local', 'a fresh snapshot is not a save confirmation');
		assert(harness.states.includes('reconnecting'), 'the connect attempt is visible as a state');
		assert(harness.states.includes('resyncing'), 'applying the snapshot is visible as a state');
		assertEqual(harness.errors.length, 0, 'a clean handshake reports no error');
	} finally {
		await harness.dispose();
	}
});

await suite.checkAsync(
	'submit -> accepted walks saving -> saved with the matching update id',
	async () => {
		const harness = await connectedSession();
		try {
			const socket = harness.socket();
			const submitted = harness.session.submit(
				{ bytes: new Uint8Array([1, 2, 3]), baseFrontier: 'f1' },
				{
					kind: 'content_edit'
				}
			);
			await waitFor(
				() => socket.sent.some((frame) => frame.type === 'update'),
				'the update frame was sent'
			);
			assertEqual(harness.states.at(-1), 'saving', 'an in-flight update shows as saving');
			const update = socket.sent.find((frame) => frame.type === 'update');
			socket.deliver({
				type: 'accepted',
				protocol_version: 1,
				update_id: update?.update_id,
				head_seq: 2,
				head_frontier: 'f2',
				projection_seq: 2,
				event_id: 'evt-1'
			});
			await submitted;
			assertEqual(harness.states.at(-1), 'saved', 'a matching accepted frame confirms the save');
		} finally {
			await harness.dispose();
		}
	}
);

await suite.checkAsync(
	'WS drain: connection given up, reconnect scheduled at the advertised floor',
	async () => {
		const harness = await connectedSession();
		try {
			const socket = harness.socket();
			const before = harness.states.length;
			socket.deliver(rejectedFrame('drain'));
			await waitFor(() => harness.errors.length > 0, 'the drain was reported to the UI');
			assertEqual(
				flowErrorI18nKey(harness.errors.at(-1) as FlowError),
				'flow.error.server_draining.drain',
				'drain key'
			);
			assert(
				harness.states.slice(before).includes('reconnecting'),
				'drain must show the reconnect state'
			);
			assertEqual(socket.closedByClient, true, 'drain must give up this connection');
			const delay = harness.session.lastScheduledReconnectDelayMs;
			assert(
				delay !== null && delay >= RETRY_AFTER_MS,
				`reconnect was scheduled in ${String(delay)} ms, below the advertised ${RETRY_AFTER_MS} ms floor`
			);
		} finally {
			await harness.dispose();
		}
	}
);

await suite.checkAsync(
	'WS contention: same code, connection kept, no reconnect, no maintenance state',
	async () => {
		const harness = await connectedSession();
		try {
			const socket = harness.socket();
			const before = harness.states.length;
			socket.deliver(rejectedFrame('contention'));
			await waitFor(() => harness.errors.length > 0, 'the contention was reported to the UI');
			const error = harness.errors.at(-1) as FlowError;
			assertEqual(error.code, 'server_draining', 'contention shares the drain stable code');
			assertEqual(
				flowErrorI18nKey(error),
				'flow.error.server_draining.contention',
				'contention key'
			);
			assertEqual(socket.closedByClient, false, 'contention must keep the existing connection');
			assert(
				!harness.states.slice(before).includes('reconnecting'),
				'contention must not impersonate maintenance'
			);
			assertEqual(
				harness.session.lastScheduledReconnectDelayMs,
				null,
				'contention must not schedule a reconnect'
			);
		} finally {
			await harness.dispose();
		}
	}
);

await suite.checkAsync(
	'a rejected update rejects exactly its own waiter with the discriminator intact',
	async () => {
		const harness = await connectedSession();
		try {
			const socket = harness.socket();
			const submitted = harness.session.submit(
				{ bytes: new Uint8Array([7]), baseFrontier: 'f1' },
				{
					kind: 'content_edit'
				}
			);
			await waitFor(
				() => socket.sent.some((frame) => frame.type === 'update'),
				'the update frame was sent'
			);
			const update = socket.sent.find((frame) => frame.type === 'update');
			socket.deliver({ ...rejectedFrame('contention'), update_id: update?.update_id });
			const rejection = await submitted.then(
				() => null,
				(error: FlowError) => error
			);
			assert(rejection !== null, 'the submit promise must reject');
			assertEqual(rejection.code, 'server_draining', 'the waiter gets the stable code');
			assertEqual(
				parseServerDrainingReason(rejection.details),
				'contention',
				'the waiter gets the discriminator'
			);
		} finally {
			await harness.dispose();
		}
	}
);

await suite.checkAsync(
	'the 4410 drain close is honoured as a drain, not a generic disconnect',
	async () => {
		const harness = await connectedSession();
		try {
			const socket = harness.socket();
			const close = drainClose('drain');
			socket.serverClose(close.code, close.reason);
			await waitFor(() => harness.errors.length > 0, 'the drain close was reported to the UI');
			assertEqual(
				flowErrorI18nKey(harness.errors.at(-1) as FlowError),
				'flow.error.server_draining.drain',
				'a 4410 close must surface the drain key, not silently reconnect'
			);
			const delay = harness.session.lastScheduledReconnectDelayMs;
			assert(
				delay !== null && delay >= RETRY_AFTER_MS,
				'the close payload retry floor was ignored'
			);
		} finally {
			await harness.dispose();
		}
	}
);

await suite.checkAsync(
	'the REST ticket surface carries the same discriminator into the same states',
	async () => {
		// "只测 WS 均失败": the drain the user actually hits first is usually the ticket POST, whose
		// envelope must not be flattened to "some non-zero code, retry later".
		for (const [reason, expectedState, expectedKey] of [
			['drain', 'reconnecting', 'flow.error.server_draining.drain'],
			['contention', 'local', 'flow.error.server_draining.contention']
		] as const) {
			const harness = await connectedSession({ ticketEnvelope: restEnvelope(reason) });
			try {
				await waitFor(() => harness.errors.length > 0, `the REST ${reason} was reported to the UI`);
				const error = harness.errors.find((candidate) => candidate.code === 'server_draining');
				assert(
					error !== undefined,
					`REST ${reason} did not surface as a Flow server_draining error`
				);
				assertEqual(flowErrorI18nKey(error), expectedKey, `REST ${reason} key`);
				assertEqual(harness.states.at(-1), expectedState, `REST ${reason} retry state`);
				assertEqual(
					MockWebSocket.instances.length,
					0,
					'a drained ticket must not have produced a WebSocket connection attempt'
				);
			} finally {
				await harness.dispose();
			}
		}
	}
);

// ---- 5. the rest of the sync state machine ----------------------------------------------------

await suite.checkAsync('frozen close codes map onto their frozen states', async () => {
	const cases: Array<[number, SyncState, string]> = [
		[4401, 'auth_required', 'unauthenticated'],
		[4403, 'error', 'forbidden'],
		[4404, 'error', 'feature_disabled'],
		[4406, 'error', 'unsupported_protocol']
	];
	for (const [code, expectedState, expectedCode] of cases) {
		const harness = await connectedSession();
		try {
			harness.socket().serverClose(code, '');
			await waitFor(
				() => harness.states.at(-1) === expectedState,
				`close ${code} -> ${expectedState}`
			);
			assertEqual(harness.states.at(-1), expectedState, `close ${code} state`);
			if (expectedState === 'error') {
				assertEqual(harness.errors.at(-1)?.code, expectedCode, `close ${code} code`);
			}
			assertEqual(
				harness.session.lastScheduledReconnectDelayMs,
				null,
				`close ${code} must not start an automatic reconnect loop`
			);
		} finally {
			await harness.dispose();
		}
	}
});

await suite.checkAsync(
	'teardown rejects in-flight writes as a LOCAL error, not a fabricated drain',
	async () => {
		const harness = await connectedSession();
		const socket = harness.socket();
		const inflight = harness.session.submit(
			{ bytes: new Uint8Array([1]), baseFrontier: 'f1' },
			{
				kind: 'content_edit'
			}
		);
		await waitFor(
			() => socket.sent.some((frame) => frame.type === 'update'),
			'the update frame was sent'
		);
		await harness.dispose();
		const rejection = await inflight.then(
			() => null,
			(error: FlowError) => error
		);
		assert(rejection !== null, 'disposing must reject in-flight writes');
		assertEqual(rejection.origin, 'client', 'a teardown is not something the server reported');
		assertNotEqual(
			rejection.code,
			'server_draining',
			'a teardown must not impersonate a server drain'
		);
		assertEqual(
			drainDisposition(rejection),
			null,
			'a locally synthesised teardown must not classify as either drain reason'
		);
	}
);

await suite.checkAsync(
	'an unknown limits version degrades to read_only and refuses local writes',
	async () => {
		const harness = await connectedSession();
		try {
			const { negotiateFlowLimitsVersion } = await import('../src/lib/flow/limits');
			harness.session.adoptLimits(
				negotiateFlowLimitsVersion({ version: 'sylvode.flow.limits.v9' })
			);
			assertEqual(
				harness.states.at(-1),
				'read_only',
				'an unknown limits version must degrade the indicator'
			);
			assertEqual(harness.session.isReadOnly, true, 'the session reports itself read-only');
			const refused = await harness.session
				.submit({ bytes: new Uint8Array([1]), baseFrontier: 'f1' }, { kind: 'content_edit' })
				.then(
					() => null,
					(error: FlowError) => error
				);
			assert(refused !== null, 'a write under an unknown limits version must be refused locally');
			assertEqual(refused.code, 'policy_rejected', 'the refusal uses the stable policy code');
			assertEqual(
				harness.socket().sent.some((frame) => frame.type === 'update'),
				false,
				'the refused write must never reach the wire'
			);
		} finally {
			await harness.dispose();
		}
	}
);

await suite.checkAsync('an over-budget update is refused before it reaches the wire', async () => {
	const harness = await connectedSession();
	try {
		const {
			negotiateFlowLimitsVersion,
			DEFAULT_FLOW_LIMITS,
			FLOW_LIMITS_VERSION,
			wireFieldName,
			FLOW_LIMITS_FIELDS
		} = await import('../src/lib/flow/limits');
		const payload: Record<string, unknown> = { version: FLOW_LIMITS_VERSION };
		for (const field of FLOW_LIMITS_FIELDS)
			payload[wireFieldName(field)] = DEFAULT_FLOW_LIMITS[field];
		payload.update_bytes_max = 4;
		harness.session.adoptLimits(negotiateFlowLimitsVersion(payload));
		const refused = await harness.session
			.submit({ bytes: new Uint8Array(5), baseFrontier: 'f1' }, { kind: 'content_edit' })
			.then(
				() => null,
				(error: FlowError) => error
			);
		assert(refused !== null, 'an over-budget update must be refused');
		assertEqual(refused.code, 'limit_exceeded', 'the refusal uses the stable limit code');
		assertDeepEqual(
			refused.details,
			{ limit_kind: 'update_bytes', limit: 4, observed: 5 },
			'the refusal carries safe numbers only'
		);
		assertEqual(
			harness.socket().sent.some((frame) => frame.type === 'update'),
			false,
			'the refused write must never reach the wire'
		);
	} finally {
		await harness.dispose();
	}
});

// ---- 6. what a real browser and a real human still have to sign off ---------------------------

suite.skip(
	'IME composition does not trigger the slash menu mid-composition',
	'needs a mounted ProseMirror EditorView and a real input method (EditorView.composing is only ' +
		'set by native compositionstart/compositionend); manual sign-off key `page_editor`'
);
suite.skip(
	'local undo/redo through the Loro binding',
	'needs a mounted EditorView -- loroPm.undo/redo operate on a live EditorState/dispatch pair; ' +
		'manual sign-off key `page_editor`'
);
suite.skip(
	'selection survives a remote change and focus falls back to the nearest valid block',
	'needs a real DOM with real focus (the fallback path reads document.activeElement); manual ' +
		'sign-off key `page_editor`'
);

(globalThis as { WebSocket: unknown }).WebSocket = realWebSocket;

export const result = suite.result();

if (import.meta.main) finish(result);

/**
 * A dependency-free Chrome DevTools Protocol client, plus a static file server that reproduces
 * the production reverse proxy's behaviour, for the `vite_wasm_static_build_and_deep_route` gate.
 *
 * `contracts/ui-surface-v1.md` is explicit that a dev server is not evidence ("build 和 preview
 * E2E 必须通过，禁止只靠 dev server") and that the CSP evidence must come from actually running
 * the candidate under the reference policy while collecting browser `securitypolicyviolation`
 * events -- "只跑 dev server、只静态 grep、未实际触发 editor/CRDT 路径或遗漏 CSP violation 均失败".
 * So this drives a real Chromium over CDP against real `vite build` output served with the real
 * `deploy/caddy/Caddyfile` policy. No Puppeteer/Playwright dependency is added: the protocol is a
 * WebSocket and Bun already speaks it.
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { createServer, type Server } from 'node:http';
import { existsSync, mkdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { extname, join, normalize } from 'node:path';

const CHROME_CANDIDATES = [
	'/usr/bin/chromium',
	'/usr/bin/chromium-browser',
	'/usr/bin/google-chrome'
];

/** Scratch root for the browser profile. Deliberately NOT under /tmp: this repo's /tmp is a
 * shared 16 GB tmpfs that a Chromium profile plus a build cache can and has exhausted. */
const SCRATCH = process.env.FLOW_V04_SCRATCH ?? '/opt/worker/.cache/w9';

export function findChrome(): string | null {
	return CHROME_CANDIDATES.find((candidate) => existsSync(candidate)) ?? null;
}

const MIME: Record<string, string> = {
	'.html': 'text/html; charset=utf-8',
	'.js': 'text/javascript; charset=utf-8',
	'.css': 'text/css; charset=utf-8',
	'.json': 'application/json; charset=utf-8',
	'.svg': 'image/svg+xml',
	'.wasm': 'application/wasm',
	'.woff2': 'font/woff2',
	'.txt': 'text/plain; charset=utf-8'
};

export interface StaticSite {
	readonly origin: string;
	/** Every path this server was asked for, with the status it answered. */
	readonly requests: ReadonlyArray<{ path: string; status: number }>;
	stop(): void;
}

/**
 * Serves a directory the way the production chain does: the Caddy security headers (including the
 * CSP this gate is judged under) in front of `adapter-static` output, with SvelteKit's `fallback:
 * 'index.html'` for any path that is not a real file -- which is exactly what makes a cold deep
 * route load rather than 404.
 */
export function serveStatic(root: string, cspPolicy: string): StaticSite {
	const requests: Array<{ path: string; status: number }> = [];
	// `node:http` rather than `Bun.serve` so the file type-checks under the repo's existing
	// `@types/node` without adding a Bun typings dependency just for a test fixture.
	const server: Server = createServer((request, response) => {
		const url = new URL(request.url ?? '/', 'http://127.0.0.1');
		const headers: Record<string, string> = {
			'Content-Security-Policy': cspPolicy,
			'X-Content-Type-Options': 'nosniff',
			'Referrer-Policy': 'strict-origin-when-cross-origin'
		};
		// Path traversal guard: a served path must stay inside `root`.
		const relative = normalize(decodeURIComponent(url.pathname)).replace(/^(\.\.[/\\])+/, '');
		const candidate = join(root, relative);
		const isFile =
			candidate.startsWith(root) && existsSync(candidate) && statSync(candidate).isFile();

		if (isFile) {
			headers['Content-Type'] = MIME[extname(candidate)] ?? 'application/octet-stream';
			requests.push({ path: url.pathname, status: 200 });
			response.writeHead(200, headers);
			response.end(readFileSync(candidate));
			return;
		}
		// `/api/**` is the one path the real chain proxies elsewhere; answering 404 here keeps the
		// app's own boot-time API calls from being counted as missing static assets.
		if (url.pathname.startsWith('/api/')) {
			headers['Content-Type'] = MIME['.json'];
			requests.push({ path: url.pathname, status: 404 });
			response.writeHead(404, headers);
			response.end('{"code":404,"message":"not found","data":null}');
			return;
		}
		// SvelteKit's `fallback: 'index.html'` -- what makes a cold deep route load, not 404.
		headers['Content-Type'] = MIME['.html'];
		requests.push({ path: url.pathname, status: 200 });
		response.writeHead(200, headers);
		response.end(readFileSync(join(root, 'index.html')));
	});
	server.listen(0, '127.0.0.1');
	const address = server.address();
	const port = typeof address === 'object' && address !== null ? address.port : 0;
	if (port === 0) throw new Error('the static site server did not bind a port');
	return {
		origin: `http://127.0.0.1:${port}`,
		requests,
		stop: () => server.close()
	};
}

interface CdpMessage {
	id?: number;
	method?: string;
	params?: Record<string, unknown>;
	result?: Record<string, unknown>;
	error?: { message: string };
	sessionId?: string;
}

/** One cold page load's observations. */
export interface PageObservation {
	/** `securitypolicyviolation` events the page itself reported. */
	readonly cspViolations: Array<{ directive: string; blockedURI: string }>;
	/** Console entries at `error` level. */
	readonly consoleErrors: string[];
	/** Uncaught exceptions. */
	readonly pageErrors: string[];
	/** Requests the network layer failed outright. */
	readonly failedRequests: Array<{ url: string; error: string }>;
	/** Every response, so the caller can assert on status per URL prefix. */
	readonly responses: Array<{ url: string; status: number }>;
}

export class Browser {
	private readonly process: ChildProcess;
	private socket: WebSocket | null = null;
	private nextId = 1;
	private readonly pending = new Map<
		number,
		{ resolve: (value: Record<string, unknown>) => void; reject: (error: Error) => void }
	>();
	private readonly listeners: Array<(message: CdpMessage) => void> = [];
	private readonly profileDir: string;

	private constructor(process: ChildProcess, profileDir: string) {
		this.process = process;
		this.profileDir = profileDir;
	}

	static async launch(executable: string): Promise<Browser> {
		const profileDir = join(SCRATCH, `chrome-profile-${process.pid}-${Date.now()}`);
		rmSync(profileDir, { recursive: true, force: true });
		mkdirSync(profileDir, { recursive: true });
		const port = 9400 + (process.pid % 400);
		const child = spawn(
			executable,
			[
				'--headless=new',
				'--no-sandbox',
				'--disable-gpu',
				'--disable-dev-shm-usage',
				'--no-first-run',
				'--disable-background-networking',
				'--disable-sync',
				`--user-data-dir=${profileDir}`,
				`--remote-debugging-port=${port}`,
				'about:blank'
			],
			{ stdio: ['ignore', 'ignore', 'pipe'] }
		);
		const browser = new Browser(child, profileDir);

		const deadline = Date.now() + 30_000;
		let webSocketDebuggerUrl: string | null = null;
		while (Date.now() < deadline && !webSocketDebuggerUrl) {
			try {
				const response = await fetch(`http://127.0.0.1:${port}/json/version`);
				const body = (await response.json()) as { webSocketDebuggerUrl?: string };
				webSocketDebuggerUrl = body.webSocketDebuggerUrl ?? null;
			} catch {
				await new Promise((resolve) => setTimeout(resolve, 100));
			}
		}
		if (!webSocketDebuggerUrl) {
			browser.close();
			throw new Error(`chromium never exposed a DevTools endpoint on port ${port}`);
		}
		await browser.connect(webSocketDebuggerUrl);
		return browser;
	}

	private connect(url: string): Promise<void> {
		return new Promise((resolve, reject) => {
			const socket = new WebSocket(url);
			this.socket = socket;
			socket.addEventListener('open', () => resolve());
			socket.addEventListener('error', () => reject(new Error('CDP socket failed')));
			socket.addEventListener('message', (event: MessageEvent<string>) => {
				const message = JSON.parse(event.data) as CdpMessage;
				if (typeof message.id === 'number') {
					const waiter = this.pending.get(message.id);
					if (waiter) {
						this.pending.delete(message.id);
						if (message.error) waiter.reject(new Error(message.error.message));
						else waiter.resolve(message.result ?? {});
					}
					return;
				}
				for (const listener of this.listeners) listener(message);
			});
		});
	}

	send(
		method: string,
		params: Record<string, unknown> = {},
		sessionId?: string
	): Promise<Record<string, unknown>> {
		const socket = this.socket;
		if (!socket) return Promise.reject(new Error('CDP socket is not connected'));
		const id = this.nextId++;
		const payload: Record<string, unknown> = { id, method, params };
		if (sessionId) payload.sessionId = sessionId;
		return new Promise((resolve, reject) => {
			this.pending.set(id, { resolve, reject });
			socket.send(JSON.stringify(payload));
			setTimeout(() => {
				if (this.pending.delete(id)) reject(new Error(`CDP ${method} timed out`));
			}, 30_000);
		});
	}

	on(listener: (message: CdpMessage) => void): () => void {
		this.listeners.push(listener);
		return () => {
			const index = this.listeners.indexOf(listener);
			if (index >= 0) this.listeners.splice(index, 1);
		};
	}

	/**
	 * Opens a BRAND NEW browser context and page, navigates it once, and returns what the page
	 * reported. A fresh context per call is what makes each load genuinely cold -- no HTTP cache,
	 * no module cache, no service worker, no storage carried over from a previous assertion.
	 */
	async coldLoad(
		url: string
	): Promise<{ observation: PageObservation; sessionId: string; contextId: string }> {
		const context = (await this.send('Target.createBrowserContext', {
			disposeOnDetach: false
		})) as {
			browserContextId: string;
		};
		const target = (await this.send('Target.createTarget', {
			url: 'about:blank',
			browserContextId: context.browserContextId
		})) as { targetId: string };
		const attached = (await this.send('Target.attachToTarget', {
			targetId: target.targetId,
			flatten: true
		})) as {
			sessionId: string;
		};
		const sessionId = attached.sessionId;

		const observation: PageObservation = {
			cspViolations: [],
			consoleErrors: [],
			pageErrors: [],
			failedRequests: [],
			responses: []
		};

		const off = this.on((message) => {
			if (message.sessionId !== sessionId) return;
			const params = (message.params ?? {}) as Record<string, unknown>;
			switch (message.method) {
				case 'Runtime.consoleAPICalled': {
					if (params.type === 'error') {
						const args = (params.args ?? []) as Array<{ value?: unknown; description?: string }>;
						observation.consoleErrors.push(
							args.map((arg) => String(arg.value ?? arg.description ?? '')).join(' ')
						);
					}
					break;
				}
				case 'Runtime.exceptionThrown': {
					const detail = params.exceptionDetails as {
						text?: string;
						exception?: { description?: string };
					};
					observation.pageErrors.push(
						detail?.exception?.description ?? detail?.text ?? 'unknown exception'
					);
					break;
				}
				case 'Network.loadingFailed': {
					observation.failedRequests.push({
						url: String((params.request as { url?: string })?.url ?? params.requestId ?? ''),
						error: String(params.errorText ?? '')
					});
					break;
				}
				case 'Network.responseReceived': {
					const response = params.response as { url?: string; status?: number };
					observation.responses.push({
						url: String(response?.url ?? ''),
						status: Number(response?.status ?? 0)
					});
					break;
				}
				default:
					break;
			}
		});

		await this.send('Runtime.enable', {}, sessionId);
		await this.send('Network.enable', {}, sessionId);
		await this.send('Page.enable', {}, sessionId);
		await this.send('Network.setCacheDisabled', { cacheDisabled: true }, sessionId);
		// The page cannot be asked for `securitypolicyviolation` events after the fact -- they are
		// DOM events, so the listener has to exist before the first byte of the document runs.
		await this.send(
			'Page.addScriptToEvaluateOnNewDocument',
			{
				source: `window.__cspViolations = [];
document.addEventListener('securitypolicyviolation', (event) => {
  window.__cspViolations.push({ directive: event.effectiveDirective || event.violatedDirective, blockedURI: event.blockedURI });
});`
			},
			sessionId
		);

		const loaded = this.waitForEvent('Page.loadEventFired', sessionId, 30_000);
		await this.send('Page.navigate', { url }, sessionId);
		await loaded;
		// One extra turn so the SPA's own boot promises (dynamic imports) settle.
		await new Promise((resolve) => setTimeout(resolve, 800));

		const violations = (await this.evaluate(
			sessionId,
			'JSON.stringify(window.__cspViolations || [])'
		)) as string;
		observation.cspViolations.push(
			...(JSON.parse(violations) as Array<{ directive: string; blockedURI: string }>)
		);

		off();
		return { observation, sessionId, contextId: context.browserContextId };
	}

	/** Requires the network layer's own record of the navigation response. */
	async evaluate(sessionId: string, expression: string): Promise<unknown> {
		const result = (await this.send(
			'Runtime.evaluate',
			{ expression, returnByValue: true, awaitPromise: true },
			sessionId
		)) as {
			result?: { value?: unknown };
			exceptionDetails?: { exception?: { description?: string }; text?: string };
		};
		if (result.exceptionDetails) {
			throw new Error(
				result.exceptionDetails.exception?.description ??
					result.exceptionDetails.text ??
					'evaluate threw'
			);
		}
		return result.result?.value;
	}

	private waitForEvent(method: string, sessionId: string, budgetMs: number): Promise<void> {
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				off();
				reject(new Error(`timed out waiting for ${method}`));
			}, budgetMs);
			const off = this.on((message) => {
				if (message.method === method && message.sessionId === sessionId) {
					clearTimeout(timer);
					off();
					resolve();
				}
			});
		});
	}

	close(): void {
		try {
			this.socket?.close();
		} catch {
			// the socket may already be gone; the process kill below is what actually matters
		}
		this.process.kill('SIGKILL');
		rmSync(this.profileDir, { recursive: true, force: true });
	}
}

/** Extracts the production `Content-Security-Policy` value from the deployment's Caddyfile.
 *
 * Read from the real file rather than transcribed, so a silent relaxation of the deployed policy
 * changes what this gate runs under instead of leaving the gate testing a stale copy. */
export function productionCsp(caddyfilePath: string): string {
	const source = readFileSync(caddyfilePath, 'utf8');
	const match = /Content-Security-Policy\s+"([^"]+)"/.exec(source);
	if (!match) throw new Error(`no Content-Security-Policy directive found in ${caddyfilePath}`);
	return match[1];
}

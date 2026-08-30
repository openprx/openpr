/**
 * Hard gate `vite_wasm_static_build_and_deep_route`.
 *
 * Judged against `contracts/ui-surface-v1.md` "事实基线" items 1-6 and "CSP-clean bundle 约束":
 * bundler/browser engine entry (no Node entry), `build.target = 'es2022'`, engine/binding in
 * `optimizeDeps.exclude`, "WASM URL/worker/chunk 在 `adapter-static` base path 下可加载，刷新 deep
 * route 不 404", "`bun run check`、`bun run build`、preview cold-load、offline reload 均纳入 gate",
 * and "engine chunk 只从 `(app)/flow` 动态 import，login/Forms route bundle 不携带 engine";
 * plus, from the CSP paragraph, no executable inline script, no `eval`/indirect eval/`new
 * Function`, same-origin connections only, and the browser's own `securitypolicyviolation` events
 * collected while the page really runs -- "只跑 dev server、只静态 grep、未实际触发 editor/CRDT
 * 路径或遗漏 CSP violation 均失败".
 *
 * Two layers, both required:
 *
 *   1. ARTIFACT -- scans the real `vite build` output and the Vite client manifest.
 *   2. BROWSER -- serves that output under the real `deploy/caddy/Caddyfile` policy and cold-loads
 *      a deep Flow route in headless Chromium, then instantiates the WASM engine IN THE PAGE.
 *
 * The build runs as part of the suite. Set `FLOW_V04_REUSE_BUILD=1` to reuse an existing `build/`
 * while iterating locally; the gate run must not set it, and the result JSON records which was
 * used.
 *
 * Run standalone: `bun tests/flow-static-build-deep-route.test.ts`
 */

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { Suite, assert, assertEqual, finish } from './support/harness';
import { Browser, findChrome, productionCsp, serveStatic } from './support/browser';

const ROOT = new URL('..', import.meta.url).pathname.replace(/\/$/, '');
const REPO = join(ROOT, '..');
const BUILD = join(ROOT, 'build');
const MANIFEST = join(ROOT, '.svelte-kit/output/client/.vite/manifest.json');
const GENERATED_NODES = join(ROOT, '.svelte-kit/generated/client-optimized/nodes');
const CADDYFILE = join(REPO, 'deploy/caddy/Caddyfile');

const suite = new Suite('vite_wasm_static_build_and_deep_route');
const reuseBuild = process.env.FLOW_V04_REUSE_BUILD === '1';

// =============================================================================================
// Layer 0 -- the build itself is part of the gate.
// =============================================================================================

suite.check(`production build (${reuseBuild ? 'reused' : 'fresh'})`, () => {
	if (reuseBuild) {
		assert(
			existsSync(join(BUILD, 'index.html')),
			'FLOW_V04_REUSE_BUILD=1 but there is no build/index.html to reuse'
		);
		return;
	}
	const result = spawnSync('bun', ['run', 'build'], {
		cwd: ROOT,
		encoding: 'utf8',
		// Never /tmp: this repo's /tmp is a shared 16 GB tmpfs that a Vite build has exhausted.
		env: { ...process.env, TMPDIR: process.env.FLOW_V04_SCRATCH ?? '/opt/worker/.cache/w9/tmp' },
		maxBuffer: 64 * 1024 * 1024
	});
	assert(
		result.status === 0,
		`bun run build exited ${String(result.status)}\n${(result.stderr ?? '').slice(-4000)}`
	);
	assert(existsSync(join(BUILD, 'index.html')), 'the build produced no build/index.html');
});

// =============================================================================================
// Layer 1 -- artifact scan.
// =============================================================================================

function walkBuild(dir: string, out: string[] = []): string[] {
	for (const entry of readdirSync(dir)) {
		const full = join(dir, entry);
		if (statSync(full).isDirectory()) walkBuild(full, out);
		else out.push(full);
	}
	return out;
}

interface ManifestEntry {
	file: string;
	imports?: string[];
	dynamicImports?: string[];
}

const manifest = JSON.parse(readFileSync(MANIFEST, 'utf8')) as Record<string, ManifestEntry>;

const buildFiles = existsSync(BUILD) ? walkBuild(BUILD) : [];
const jsFiles = buildFiles.filter((file) => file.endsWith('.js'));
const htmlFiles = buildFiles.filter((file) => file.endsWith('.html'));

suite.check('no emitted HTML carries an executable inline script', () => {
	assert(htmlFiles.length > 0, 'the build emitted no HTML at all');
	for (const file of htmlFiles) {
		const html = readFileSync(file, 'utf8');
		for (const match of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)) {
			const attributes = match[1];
			const body = match[2].trim();
			if (body.length === 0) continue; // `<script src=...></script>` is fine
			// A `type` the browser does not execute (importmap, application/json) is not a script.
			const type = /type\s*=\s*"([^"]*)"/.exec(attributes)?.[1] ?? '';
			assert(
				type === 'application/json' || type === 'importmap' || type === 'speculationrules',
				`${relative(ROOT, file)} contains an executable inline script under CSP script-src 'self':\n${body.slice(0, 200)}`
			);
		}
	}
});

/** Every eval-family construct in the emitted JS, with the chunk it lives in.
 *
 * `\beval(` catches direct eval; `(0,eval)` catches the indirect form; `new Function(` catches the
 * Function constructor. All three are blocked by a policy without `'unsafe-eval'`, so a REACHED
 * one is a runtime failure, not a style question. */
function evalFamilySites(): Array<{ file: string; construct: string; excerpt: string }> {
	const sites: Array<{ file: string; construct: string; excerpt: string }> = [];
	const patterns: Array<[string, RegExp]> = [
		['eval', /\beval\s*\(/g],
		['indirect eval', /\(\s*0\s*,\s*eval\s*\)/g],
		['new Function', /new\s+Function\s*\(/g]
	];
	for (const file of jsFiles) {
		const source = readFileSync(file, 'utf8');
		for (const [construct, pattern] of patterns) {
			for (const match of source.matchAll(pattern)) {
				sites.push({
					file: relative(ROOT, file),
					construct,
					excerpt: source.slice(Math.max(0, match.index - 60), match.index + 80)
				});
			}
		}
	}
	return sites;
}

/** Emitted chunk files whose manifest key is a `node_modules/**` module -- i.e. third-party code
 * this repo does not author. Everything else is treated as first-party and held to the strict
 * rule, including shared chunks with no manifest key of their own. */
function thirdPartyChunkFiles(): Map<string, string> {
	const byFile = new Map<string, string>();
	for (const [key, entry] of Object.entries(manifest)) {
		if (key.startsWith('node_modules/') && entry.file) byFile.set(entry.file, key);
	}
	return byFile;
}

/**
 * The ONE tolerated eval-family occurrence, declared explicitly rather than by loosening the
 * pattern above.
 *
 * `__wbg_newnoargs_*` is wasm-bindgen's standard import shim for `js_sys::Function::new_no_args`.
 * It is emitted into every wasm-bindgen bundle that links `js_sys`, whether or not the Rust side
 * ever calls it, and `js_sys`'s own `global()` only reaches it after `globalThis`/`self`/`window`
 * /`global` have all failed -- which they do not in a browser.
 *
 * This is a KNOWN DEVIATION from `ui-surface-v1.md`'s literal "不得使用 ... `new Function`", not a
 * proof of absence: static presence is certain, and non-reachability is evidenced only by the
 * browser layer below instantiating the engine, creating a document and exporting a snapshot with
 * ZERO `securitypolicyviolation` events under the real production policy. `engineExercisedCleanly`
 * gates this exception on that evidence actually having been produced in THIS run.
 */
const DECLARED_EVAL_EXCEPTIONS = [
	{
		module: 'node_modules/loro-crdt/browser/index.js',
		construct: 'new Function',
		binding: '__wbg_newnoargs_'
	}
] as const;

/** Set by the browser layer once it has instantiated the engine with no CSP violation. */
let engineExercisedCleanly = false;

suite.check('no FIRST-PARTY emitted JS uses eval, indirect eval or new Function', () => {
	const thirdParty = thirdPartyChunkFiles();
	const offenders = evalFamilySites().filter(
		(site) => !thirdParty.has(site.file.replace(/^build\//, ''))
	);
	assert(
		offenders.length === 0,
		`eval-family constructs in first-party build output:\n  ${offenders.map((site) => `${site.file} (${site.construct}): ${site.excerpt}`).join('\n  ')}`
	);
});

suite.check('no emitted JS opens a cross-origin connection', () => {
	// `connect-src 'self'`: any absolute http(s)/ws(s) URL handed to fetch/XHR/WebSocket/import
	// would be blocked at runtime. Bare URLs inside comments or license banners are not requests,
	// so this looks specifically at the call sites.
	const offenders: string[] = [];
	const patterns = [
		/fetch\(\s*["'`](https?:)?\/\/[^"'`]+/g,
		/new\s+WebSocket\(\s*["'`](wss?:)?\/\/[^"'`]+/g,
		/import\(\s*["'`](https?:)?\/\/[^"'`]+/g,
		/\.open\(\s*["'][A-Z]+["']\s*,\s*["'`](https?:)?\/\/[^"'`]+/g
	];
	for (const file of jsFiles) {
		const source = readFileSync(file, 'utf8');
		for (const pattern of patterns) {
			for (const match of source.matchAll(pattern))
				offenders.push(`${relative(ROOT, file)}: ${match[0].slice(0, 120)}`);
		}
	}
	assert(
		offenders.length === 0,
		`cross-origin connections in the build output:\n  ${offenders.join('\n  ')}`
	);
});

suite.check('the WASM engine asset is emitted and is a real WebAssembly module', () => {
	const wasm = buildFiles.filter((file) => file.endsWith('.wasm'));
	assert(wasm.length > 0, 'the build emitted no .wasm asset, so no CRDT engine shipped');
	for (const file of wasm) {
		const header = readFileSync(file).subarray(0, 4);
		assertEqual(
			[...header].map((byte) => byte.toString(16).padStart(2, '0')).join(''),
			'0061736d',
			`${relative(ROOT, file)} does not start with the WebAssembly magic number`
		);
	}
	// It also has to be REFERENCED, or it is dead weight the engine will never load.
	const referenced = jsFiles.some((file) => /\.wasm/.test(readFileSync(file, 'utf8')));
	assert(referenced, 'no emitted JS references the .wasm asset');
});

suite.check(
	'vite config keeps the engine on the browser entry, es2022, and out of pre-bundling',
	() => {
		const config = readFileSync(join(ROOT, 'vite.config.ts'), 'utf8');
		assert(
			/'loro-crdt':\s*'loro-crdt\/browser'/.test(config),
			'loro-crdt is not pinned to its browser entry (item 1)'
		);
		assert(/target:\s*'es2022'/.test(config), "build.target is not 'es2022' (item 2)");
		assert(
			/optimizeDeps[\s\S]*exclude[\s\S]*'loro-crdt'[\s\S]*'loro-prosemirror'/.test(config),
			'the engine and its binding are not both in optimizeDeps.exclude (item 3)'
		);
	}
);

// ---- engine chunk containment ---------------------------------------------------------------

/** The transitive STATIC-import closure of a manifest key, as chunk file names. A route's bundle
 * is what the browser must download to render it; dynamic imports are deliberately excluded,
 * because being reachable only through `import()` is exactly the property under test. */
function staticClosure(key: string, seen = new Set<string>()): Set<string> {
	if (seen.has(key)) return seen;
	seen.add(key);
	for (const dependency of manifest[key]?.imports ?? []) staticClosure(dependency, seen);
	return seen;
}

const ENGINE_KEYS = Object.keys(manifest).filter((key) =>
	/node_modules\/(loro-crdt|loro-prosemirror|prosemirror-)/.test(key)
);

/** Maps a route source path to its generated node index, via `.svelte-kit/generated`. */
function nodeKeyForRoute(routeSuffix: string): string {
	for (const entry of readdirSync(GENERATED_NODES)) {
		const source = readFileSync(join(GENERATED_NODES, entry), 'utf8');
		if (source.includes(routeSuffix))
			return `.svelte-kit/generated/client-optimized/nodes/${entry}`;
	}
	throw new Error(`no generated node imports a route matching ${routeSuffix}`);
}

suite.check('the engine is discoverable in the manifest at all', () => {
	assert(
		ENGINE_KEYS.length > 0,
		'the manifest lists no loro/prosemirror module -- the containment check would be vacuous'
	);
});

suite.check('the engine chunk is NOT in the login or app-entry static bundle', () => {
	const loginKey = nodeKeyForRoute('(auth)/auth/login/+page.svelte');
	for (const [label, key] of [
		['the login route', loginKey],
		['the app entry', '.svelte-kit/generated/client-optimized/app.js']
	] as const) {
		const closure = staticClosure(key);
		const leaked = ENGINE_KEYS.filter((engineKey) => closure.has(engineKey));
		assert(
			leaked.length === 0,
			`${label} statically pulls in the engine: ${leaked.join(', ')} -- it must only arrive via a dynamic import from (app)/flow`
		);
	}
});

suite.check('the engine reaches the Flow route only through a dynamic import', () => {
	const flowKey = nodeKeyForRoute('flow/[objectId]/+page.svelte');
	const closure = staticClosure(flowKey);
	const staticallyBound = ENGINE_KEYS.filter((engineKey) => closure.has(engineKey));
	assert(
		staticallyBound.length === 0,
		`the Flow route statically imports ${staticallyBound.join(', ')}; ui-surface-v1.md requires the engine chunk to be dynamically imported`
	);
	const adapter = readFileSync(join(ROOT, 'src/lib/flow/editor-adapter.ts'), 'utf8');
	for (const module of [
		'loro-prosemirror',
		'prosemirror-model',
		'prosemirror-state',
		'prosemirror-view'
	]) {
		assert(
			!new RegExp(`^import\\s+(?!type)[^\\n]*from '${module}'`, 'm').test(adapter),
			`editor-adapter.ts statically imports ${module}; it must use dynamic import()`
		);
		assert(
			adapter.includes(`import('${module}')`),
			`editor-adapter.ts never dynamically imports ${module}`
		);
	}
});

// =============================================================================================
// Layer 2 -- the real browser, under the real production CSP.
// =============================================================================================

const chrome = findChrome();
const CSP = productionCsp(CADDYFILE);
const DEEP_ROUTE =
	'/workspace/00000000-0000-4000-8000-000000000001/flow/00000000-0000-4000-8000-000000000002';

suite.check('the production CSP is strict enough to be worth testing under', () => {
	assert(/script-src[^;]*'self'/.test(CSP), "the deployed policy does not set script-src 'self'");
	assert(
		!/'unsafe-inline'[^;]*;?\s*$/.test(
			CSP.split(';').find((part) => part.includes('script-src')) ?? ''
		),
		''
	);
	const scriptSrc = CSP.split(';').find((part) => part.trim().startsWith('script-src')) ?? '';
	assert(
		!scriptSrc.includes("'unsafe-inline'"),
		`script-src carries 'unsafe-inline': ${scriptSrc}`
	);
	assert(!scriptSrc.includes("'unsafe-eval'"), `script-src carries 'unsafe-eval': ${scriptSrc}`);
	// `'wasm-unsafe-eval'` IS expected and is not a silent relaxation: `ui-surface-v1.md`'s R17
	// fact correction records that every browser refuses WebAssembly.compile/instantiate without
	// it, so a WASM engine necessarily needs it. What must not happen is the token being present
	// while no candidate needs it -- which the WASM instantiation check below proves is not the
	// case here.
	assert(
		scriptSrc.includes("'wasm-unsafe-eval'"),
		`script-src lacks 'wasm-unsafe-eval', so the WASM engine cannot run: ${scriptSrc}`
	);
	assert(/connect-src[^;]*'self'/.test(CSP), "the deployed policy does not set connect-src 'self'");
});

if (!chrome) {
	suite.skip(
		'cold deep-route load in a real browser',
		'no chromium/chrome binary found on this host'
	);
	suite.skip('deep-route refresh', 'no chromium/chrome binary found on this host');
	suite.skip(
		'WASM engine instantiation under the production CSP',
		'no chromium/chrome binary found on this host'
	);
} else {
	const site = serveStatic(BUILD, CSP);
	const browser = await Browser.launch(chrome);
	try {
		await suite.checkAsync(
			'a cold deep route loads, boots and raises no CSP violation',
			async () => {
				const { observation, sessionId } = await browser.coldLoad(`${site.origin}${DEEP_ROUTE}`);

				assertEqual(
					observation.cspViolations.length,
					0,
					`the page reported CSP violations: ${JSON.stringify(observation.cspViolations)}`
				);
				const appAssets = observation.responses.filter((response) =>
					response.url.includes('/_app/')
				);
				assert(
					appAssets.length > 0,
					'the deep route pulled no /_app/ assets at all -- nothing actually loaded'
				);
				const missing = appAssets.filter((response) => response.status >= 400);
				assertEqual(missing.length, 0, `deep-route asset 404s: ${JSON.stringify(missing)}`);
				const failed = observation.failedRequests.filter((request) =>
					request.url.includes('/_app/')
				);
				assertEqual(failed.length, 0, `deep-route asset load failures: ${JSON.stringify(failed)}`);
				assertEqual(
					observation.pageErrors.length,
					0,
					`uncaught page errors: ${observation.pageErrors.join(' | ')}`
				);

				const booted = await browser.evaluate(
					sessionId,
					"Object.keys(window).some((key) => key.startsWith('__sveltekit_'))"
				);
				assertEqual(booted, true, 'the SvelteKit client never booted on the deep route');
				const rendered = await browser.evaluate(
					sessionId,
					'document.body.innerText.trim().length > 0'
				);
				assertEqual(rendered, true, 'the deep route rendered an empty body');
			}
		);

		await suite.checkAsync(
			'refreshing the deep route serves the app rather than a 404',
			async () => {
				// `adapter-static` + `fallback: 'index.html'`: item 4's "刷新 deep route 不 404". A
				// second cold context IS the refresh, since the first one shares no cache with it.
				const { observation } = await browser.coldLoad(`${site.origin}${DEEP_ROUTE}`);
				const document = observation.responses.find((response) =>
					response.url.endsWith(DEEP_ROUTE)
				);
				assert(document !== undefined, 'the navigation response was never observed');
				assertEqual(document.status, 200, `refreshing the deep route answered ${document.status}`);
				assertEqual(observation.cspViolations.length, 0, 'the refresh raised CSP violations');
			}
		);

		await suite.checkAsync(
			'the WASM engine loads and instantiates in the page under the production CSP',
			async () => {
				// This is the check `ui-surface-v1.md` means by "未实际触发 editor/CRDT 路径 ... 均失败":
				// the engine chunk is fetched with the deep route's base path in effect, the wasm-bindgen
				// glue instantiates the module, and a real CRDT document is created and mutated.
				const { sessionId } = await browser.coldLoad(`${site.origin}${DEEP_ROUTE}`);
				const engineChunk = manifest['node_modules/loro-crdt/browser/index.js']?.file;
				assert(
					engineChunk !== undefined,
					'the manifest has no chunk for the loro-crdt browser entry'
				);

				const outcome = (await browser.evaluate(
					sessionId,
					`(async () => {
					try {
						const module = await import('/${engineChunk}');
						const doc = new module.LoroDoc();
						doc.getText('probe').insert(0, 'flow');
						const bytes = doc.export({ mode: 'snapshot' });
						return JSON.stringify({
							ok: true,
							text: doc.getText('probe').toString(),
							bytes: bytes.length,
							violations: (window.__cspViolations || []).length
						});
					} catch (error) {
						return JSON.stringify({ ok: false, error: String(error && error.stack || error) });
					}
				})()`
				)) as string;

				const parsed = JSON.parse(outcome) as {
					ok: boolean;
					error?: string;
					text?: string;
					bytes?: number;
					violations?: number;
				};
				assert(
					parsed.ok,
					`the WASM engine failed to instantiate under the production CSP: ${parsed.error}`
				);
				assertEqual(
					parsed.text,
					'flow',
					'the instantiated CRDT document did not accept a local edit'
				);
				assert(
					(parsed.bytes ?? 0) > 0,
					'the instantiated CRDT document exported no snapshot bytes'
				);
				assertEqual(parsed.violations, 0, 'instantiating the engine raised a CSP violation');
				engineExercisedCleanly = true;
			}
		);

		suite.check('every asset the browser asked for was served from the same origin', () => {
			const foreign = site.requests.filter(
				(request) => request.status >= 400 && !request.path.startsWith('/api/')
			);
			assertEqual(
				foreign.length,
				0,
				`the static site answered non-200 for: ${JSON.stringify(foreign)}`
			);
		});
	} finally {
		browser.close();
		site.stop();
	}
}

// ---- the declared third-party exception, gated on this run's own evidence -------------------

suite.check('every third-party eval-family occurrence is declared AND provably unreached', () => {
	const thirdParty = thirdPartyChunkFiles();
	const undeclared: string[] = [];
	let declaredHits = 0;
	for (const site of evalFamilySites()) {
		const chunkFile = site.file.replace(/^build\//, '');
		const module = thirdParty.get(chunkFile);
		const source = readFileSync(join(ROOT, site.file), 'utf8');
		const declared = DECLARED_EVAL_EXCEPTIONS.find(
			(exception) =>
				exception.module === module &&
				exception.construct === site.construct &&
				source.includes(exception.binding)
		);
		if (declared) declaredHits += 1;
		else
			undeclared.push(
				`${site.file} (${site.construct}) from ${module ?? 'an unmapped chunk'}: ${site.excerpt}`
			);
	}
	assert(
		undeclared.length === 0,
		`undeclared eval-family constructs in the build output:\n  ${undeclared.join('\n  ')}\nAdd them to DECLARED_EVAL_EXCEPTIONS with evidence, or remove them.`
	);
	assert(
		declaredHits > 0,
		'the declared exception matched nothing -- it is stale and must be removed'
	);
	assert(
		engineExercisedCleanly,
		'the declared eval exception is only tolerable while this run has actually instantiated the ' +
			'engine under the production CSP with zero securitypolicyviolation events; that evidence ' +
			'was not produced, so the exception does not hold'
	);
});

// ---- what a human still has to confirm ------------------------------------------------------

suite.skip(
	'the deep route renders the Flow editor for an authenticated user',
	'the cold load above proves the shell boots, the chunks resolve and the engine instantiates ' +
		'under the production CSP, but rendering the editor needs a real session against a running ' +
		'API; manual sign-off key `page_editor`'
);

export const result = suite.result();

if (import.meta.main) finish(result);

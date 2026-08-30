/**
 * `bun --cwd frontend run test:flow-v0.4` -- the command `gates/gate-commands.md`'s v0.4 command
 * bundle names but which had no implementation.
 *
 * Runs the four UI suites that back v0.4's browser-side hard gates and emits one machine-readable
 * verdict:
 *
 *   i18n_zh_en_flow_key_parity          tests/flow-i18n-parity.test.ts
 *   vite_wasm_static_build_and_deep_route tests/flow-static-build-deep-route.test.ts
 *   web_ime_undo_selection_and_sync_state tests/flow-ui-state.test.ts
 *   navigator_keyboard_drag_equivalence tests/flow-navigator-equivalence.test.ts
 *
 * Two of those four gates are only PARTLY automatable, and the JSON says so per gate rather than
 * rounding up. `gate-commands.md`'s v0.4 line "人工 keys：`page_editor`、`navigator_a11y`、
 * `restart_recovery`、`feature_flag`、`forms_regression`" is why: real IME candidate windows,
 * real screen-reader output and real focus rings cannot be asserted from a headless process, and
 * dressing a weak proxy up as the real check would be worse than leaving the row red. Every such
 * check appears as a `skipped` entry naming the manual key that owns it -- a suite with skips is
 * reported as `automation_partial`, never as a clean pass.
 *
 * Exit code: 0 only when every suite has zero failures. Skips do not fail the run (they are
 * declared, not silent) but they do keep the gate's `coverage` below `full`.
 *
 * Environment:
 *   FLOW_V04_REUSE_BUILD=1   reuse an existing `build/` instead of running `bun run build`
 *   FLOW_V04_RESULT_PATH     where to write the JSON (default: /opt/worker/.cache/w9/...)
 *   FLOW_V04_SCRATCH         scratch root for the browser profile and build TMPDIR
 */

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import type { SuiteResult } from './support/harness';

interface GateRow {
	readonly gate: string;
	readonly suite: string;
	/** `full` -- everything this gate asserts is automated here.
	 *  `partial` -- some checks need a human; `manual_keys` names who owns them. */
	readonly coverage: 'full' | 'partial';
	readonly manual_keys: readonly string[];
	readonly note: string;
}

/** Which hard gate each suite answers for, and -- for the two that cannot be fully automated --
 * exactly which manual sign-off key covers the remainder. */
const GATES: readonly GateRow[] = [
	{
		gate: 'i18n_zh_en_flow_key_parity',
		suite: 'i18n_zh_en_flow_key_parity',
		coverage: 'full',
		manual_keys: [],
		note: 'Key-set parity, stable-error-code coverage, the server_draining discriminator keys, absence of a base server_draining key, no hard-coded copy and no unreferenced dynamic key family are all decidable from the locale files and the source tree. Copy QUALITY is not asserted -- only that zh is Chinese, en carries no Han, and the two server_draining reasons read differently.'
	},
	{
		gate: 'vite_wasm_static_build_and_deep_route',
		suite: 'vite_wasm_static_build_and_deep_route',
		coverage: 'partial',
		manual_keys: ['page_editor'],
		note: "Automated: real production build, artifact scan (no executable inline script, no first-party eval family, no cross-origin connect, real .wasm emitted and referenced, engine absent from the login/app-entry static bundle and dynamically imported only), and a real headless-Chromium cold load of a deep Flow route served under deploy/caddy/Caddyfile's own CSP, with zero securitypolicyviolation events, zero /_app 404s, SvelteKit boot confirmed, refresh confirmed, and the WASM engine instantiated IN THE PAGE (LoroDoc created, edited, snapshot exported). Not automated: rendering the Flow editor for a real authenticated session against a running API."
	},
	{
		gate: 'web_ime_undo_selection_and_sync_state',
		suite: 'web_ime_undo_selection_and_sync_state',
		coverage: 'partial',
		manual_keys: ['page_editor'],
		note: 'Automated: the whole sync state machine driven through the production LoroObjectSession (handshake, saving/saved on a matching accepted frame, frozen close codes, read-only on an unknown limits version, pre-flight limit refusal), and the server_draining producer fixture injected on THREE UI surfaces (REST envelope, WS rejected frame, WS 4410 close) proving same stable code but different i18n key, retry state, connection fate and retry floor, with missing/unknown reason failing closed and no message-based branching. Not automated: IME composition, undo/redo through the ProseMirror+Loro binding, and selection/focus restore -- all need a mounted EditorView, a real DOM focus and a real input method.'
	},
	{
		gate: 'navigator_keyboard_drag_equivalence',
		suite: 'navigator_keyboard_drag_equivalence',
		coverage: 'partial',
		manual_keys: ['navigator_a11y'],
		note: 'Automated: exhaustive outcome equivalence between pointer drag, keyboard lift/step/drop and the context menu over every (list size, source, target) triple, equality of the reachable destination SETS in both directions, the written order key actually sorting into the intended position over a long random run, and a structural guard that the component computes no order bounds of its own. Not automated: whether the rendered tree is genuinely operable by a screen reader, whether aria-live announcements are voiced, and whether focus rings/44px targets/reduced-motion hold.'
	}
];

const SUITE_MODULES = [
	'./flow-i18n-parity.test',
	'./flow-navigator-equivalence.test',
	'./flow-ui-state.test',
	'./flow-static-build-deep-route.test'
] as const;

const results: SuiteResult[] = [];
for (const module of SUITE_MODULES) {
	console.log(`\n=== ${module} ===`);
	const loaded = (await import(module)) as { result: SuiteResult };
	results.push(loaded.result);
}

const byName = new Map(results.map((result) => [result.name, result]));
const gates = GATES.map((gate) => {
	const result = byName.get(gate.suite);
	return {
		...gate,
		passed: result !== undefined && result.failed === 0,
		checks_passed: result?.passed ?? 0,
		checks_failed: result?.failed ?? 0,
		checks_skipped: result?.skipped ?? 0,
		duration_ms: result?.durationMs ?? 0,
		failures: result?.failures ?? ['suite did not run'],
		// A skip is a declared, named hole in the automation, so it is carried into the evidence
		// verbatim. `automation_partial` is never the same thing as `passed`.
		skipped_checks: result?.skips ?? [],
		automation:
			gate.coverage === 'full' && (result?.skipped ?? 0) === 0
				? 'automation_complete'
				: 'automation_partial'
	};
});

const totals = {
	passed: results.reduce((sum, result) => sum + result.passed, 0),
	failed: results.reduce((sum, result) => sum + result.failed, 0),
	skipped: results.reduce((sum, result) => sum + result.skipped, 0)
};

const evidence = {
	command: 'bun --cwd frontend run test:flow-v0.4',
	contract:
		'gates/gate-commands.md (v0.4 command bundle); contracts/ui-surface-v1.md; contracts/error-mapping-v1.md',
	generated_at: new Date().toISOString(),
	reused_build: process.env.FLOW_V04_REUSE_BUILD === '1',
	passed: totals.failed === 0,
	totals,
	gates
};

const outputPath =
	process.env.FLOW_V04_RESULT_PATH ??
	`${process.env.FLOW_V04_SCRATCH ?? '/opt/worker/.cache/w9'}/flow-v0.4-ui-result.json`;
mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, `${JSON.stringify(evidence, null, 2)}\n`);

console.log('\n=== test:flow-v0.4 ===');
for (const gate of gates) {
	const verdict = gate.passed ? 'PASS' : 'FAIL';
	// Coverage is DECLARED per gate, not inferred from the skip count: a gate whose remainder is
	// owned by a human stays `partial` even if nobody remembered to record a skip for it.
	const coverage =
		gate.coverage === 'full'
			? 'complete'
			: `partial (manual: ${gate.manual_keys.join(', ') || 'none'})`;
	console.log(
		`  ${verdict}  ${gate.gate}\n         ${gate.checks_passed} passed, ${gate.checks_failed} failed, ${gate.checks_skipped} deferred to humans; automation ${coverage}`
	);
	for (const failure of gate.failures) console.log(`         ! ${failure}`);
}
console.log(
	`\n${totals.passed} checks passed, ${totals.failed} failed, ${totals.skipped} deferred to manual sign-off`
);
console.log(`result written to ${outputPath}`);

process.exit(totals.failed > 0 ? 1 : 0);

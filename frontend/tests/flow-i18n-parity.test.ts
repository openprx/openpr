/**
 * Hard gate `i18n_zh_en_flow_key_parity`.
 *
 * Judged against `contracts/ui-surface-v1.md` "i18n v0.4 基线":
 *
 *   "所有 Flow 用户文案从首次提交起进入 zh.json 和 en.json ... 禁止 hard-coded Chinese/English、
 *    禁止用 server message 当 key、禁止缺 key 时把 key string 当发布文案。gate 比较 zh/en `flow`
 *    key set 完全相同，并运行全部稳定 error code 及其 required discriminator key coverage；
 *    `server_draining` 两个 reason 必须分别有 zh/en key、状态与无障碍文案 fixture。"
 *
 * Everything below is derived at run time from the two locale files, `contracts/error-mapping-v1
 * .md`'s stable code list (transcribed once, into `src/lib/flow/errors.ts`'s `FLOW_ERROR_CODES`,
 * which is also what the running client branches on) and the Flow source tree. Nothing is a
 * hardcoded expected key list -- a list like that would pass forever after someone deletes a
 * feature and its keys together.
 *
 * Run standalone: `bun tests/flow-i18n-parity.test.ts`
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { Suite, assert, assertNotEqual, finish } from './support/harness';
import {
	FLOW_ERROR_CODES,
	PACKAGE_ROUND_TRIP_ONLY_ERROR_CODES,
	SERVER_DRAINING_REASONS
} from '../src/lib/flow/errors';
import type { SyncState } from '../src/lib/flow/types';

const ROOT = new URL('..', import.meta.url).pathname;
const LOCALES = ['zh', 'en'] as const;
type Locale = (typeof LOCALES)[number];

/** The nine sync-indicator states (`ui-surface-v1.md`: "Sync indicator 的可见状态固定为
 * local|saving|saved|offline|reconnecting|resyncing|auth_required|read_only|error"). Declared
 * `satisfies readonly SyncState[]` so deleting a state from the union breaks compilation here
 * rather than silently shrinking what this gate checks. */
const SYNC_STATES = [
	'local',
	'saving',
	'saved',
	'offline',
	'reconnecting',
	'resyncing',
	'auth_required',
	'read_only',
	'error'
] as const satisfies readonly SyncState[];

/** The `flow.*` namespaces `ui-surface-v1.md`'s i18n baseline lists as in force from v0.4, with
 * the later-version ones (`flow.collection.*` v0.6, `flow.bridge.*` v0.7, `flow.operations.*`
 * v0.8) deliberately excluded and named here so the exclusion is visible rather than implied. */
const V04_REQUIRED_NAMESPACES = [
	'flow.nav',
	'flow.canvas',
	'flow.block',
	'flow.sync',
	'flow.error',
	'flow.history',
	'flow.relations',
	'flow.search',
	'flow.recovery'
] as const;
const LATER_VERSION_NAMESPACES = ['flow.collection', 'flow.bridge', 'flow.operations'] as const;

type Json = { [key: string]: Json } | string | number | boolean | null;

function loadLocale(locale: Locale): Map<string, string> {
	const raw = JSON.parse(
		readFileSync(join(ROOT, 'src/lib/i18n', `${locale}.json`), 'utf8')
	) as Json;
	const flat = new Map<string, string>();
	const walk = (node: Json, prefix: string): void => {
		if (typeof node !== 'object' || node === null) {
			flat.set(prefix, String(node));
			return;
		}
		for (const [key, value] of Object.entries(node))
			walk(value, prefix === '' ? key : `${prefix}.${key}`);
	};
	walk(raw, '');
	return flat;
}

const catalogues = new Map<Locale, Map<string, string>>(
	LOCALES.map((locale) => [locale, loadLocale(locale)])
);

function flowKeys(locale: Locale): Set<string> {
	const catalogue = catalogues.get(locale);
	assert(catalogue, `locale ${locale} loaded`);
	return new Set([...catalogue.keys()].filter((key) => key === 'flow' || key.startsWith('flow.')));
}

function value(locale: Locale, key: string): string | undefined {
	return catalogues.get(locale)?.get(key);
}

/** Every `.ts`/`.svelte` file that renders Flow UI. */
function flowSourceFiles(): string[] {
	const roots = [
		'src/lib/flow',
		'src/lib/components/flow',
		'src/routes/(app)/workspace/[workspaceId]/flow'
	];
	const out: string[] = [];
	const walk = (dir: string): void => {
		for (const entry of readdirSync(dir)) {
			const full = join(dir, entry);
			if (statSync(full).isDirectory()) walk(full);
			else if (/\.(ts|svelte)$/.test(entry)) out.push(full);
		}
	};
	for (const root of roots) walk(join(ROOT, root));
	return out.sort();
}

const suite = new Suite('i18n_zh_en_flow_key_parity');

// ---- 1. zh/en `flow` key sets are exactly equal -------------------------------------------

suite.check('zh and en declare exactly the same flow.* key set', () => {
	const zh = flowKeys('zh');
	const en = flowKeys('en');
	const onlyZh = [...zh].filter((key) => !en.has(key)).sort();
	const onlyEn = [...en].filter((key) => !zh.has(key)).sort();
	assert(
		onlyZh.length === 0 && onlyEn.length === 0,
		`flow key sets diverge\n  only in zh.json (${onlyZh.length}): ${onlyZh.join(', ') || '-'}\n  only in en.json (${onlyEn.length}): ${onlyEn.join(', ') || '-'}`
	);
	assert(zh.size > 0, 'there are flow.* keys at all');
});

suite.check('every flow.* leaf is a non-empty string in both locales', () => {
	for (const locale of LOCALES) {
		for (const key of flowKeys(locale)) {
			const text = value(locale, key);
			assert(typeof text === 'string' && text.trim().length > 0, `${locale}.json ${key} is empty`);
		}
	}
});

// ---- 2. no key string leaks into published copy -------------------------------------------

suite.check('no flow.* value is its own key (the "missing key rendered as copy" failure)', () => {
	for (const locale of LOCALES) {
		for (const key of flowKeys(locale)) {
			assertNotEqual(
				value(locale, key),
				key,
				`${locale}.json ${key} renders the key itself as user copy`
			);
		}
	}
	// Broader: no flow value may look like a dotted i18n path at all.
	for (const locale of LOCALES) {
		for (const key of flowKeys(locale)) {
			const text = value(locale, key) ?? '';
			assert(
				!/^flow(\.[a-z_A-Z0-9]+)+$/.test(text.trim()),
				`${locale}.json ${key} = ${JSON.stringify(text)} looks like an i18n key, not copy`
			);
		}
	}
});

// ---- 3. required namespaces, and only the in-scope ones ------------------------------------

suite.check('every v0.4 flow namespace is populated in both locales', () => {
	for (const locale of LOCALES) {
		const keys = flowKeys(locale);
		for (const namespace of V04_REQUIRED_NAMESPACES) {
			const populated = [...keys].some((key) => key.startsWith(`${namespace}.`));
			assert(populated, `${locale}.json has no keys under ${namespace}.*`);
		}
	}
});

suite.check('later-version namespaces are absent (they are not v0.4 deliverables)', () => {
	for (const locale of LOCALES) {
		for (const namespace of LATER_VERSION_NAMESPACES) {
			const present = [...flowKeys(locale)].filter((key) => key.startsWith(`${namespace}.`));
			assert(
				present.length === 0,
				`${locale}.json declares ${namespace}.* (${present.join(', ')}) but that namespace's UI is a later version`
			);
		}
	}
});

// ---- 4. stable error code coverage + the server_draining discriminator ---------------------

suite.check('every v0.4 stable error code has a zh and en key', () => {
	for (const code of FLOW_ERROR_CODES) {
		if (code === 'server_draining') continue; // discriminated; asserted separately below
		for (const locale of LOCALES) {
			assert(
				value(locale, `flow.error.${code}`) !== undefined,
				`${locale}.json is missing flow.error.${code}`
			);
		}
	}
});

suite.check('package-round-trip-only codes are the ONLY stable codes without a v0.4 key', () => {
	// `error-mapping-v1.md`'s table has these two marked "n/a；import/export REST only": their
	// producer is the v0.8 package surface (`contracts/export-package-v1.md`), unreachable in
	// v0.4. Asserting they are absent -- rather than just not asserting they are present --
	// is what stops this exclusion from quietly widening to cover a code that IS reachable.
	for (const code of PACKAGE_ROUND_TRIP_ONLY_ERROR_CODES) {
		assert(
			!(FLOW_ERROR_CODES as readonly string[]).includes(code),
			`${code} is in FLOW_ERROR_CODES, so it is reachable and must not be on the excluded list`
		);
		for (const locale of LOCALES) {
			assert(
				value(locale, `flow.error.${code}`) === undefined,
				`${locale}.json declares flow.error.${code}, whose only producer is the v0.8 package surface`
			);
		}
	}
});

suite.check('server_draining has both reason keys and NO base key', () => {
	for (const locale of LOCALES) {
		for (const reason of SERVER_DRAINING_REASONS) {
			assert(
				value(locale, `flow.error.server_draining.${reason}`) !== undefined,
				`${locale}.json is missing flow.error.server_draining.${reason}`
			);
		}
		// `ui-surface-v1.md`: "合法响应不渲染 base `flow.error.server_draining`，只使用上列两个
		// reason key." A base key would be the thing a lazy fallback reaches for.
		assert(
			value(locale, 'flow.error.server_draining') === undefined,
			`${locale}.json declares a base flow.error.server_draining key; only the two reason keys may exist`
		);
	}
});

suite.check('the two server_draining reasons read differently in each locale', () => {
	// `error-mapping-v1.md`: contention must show "重试中且不冒充维护". Two identical strings
	// would satisfy "both keys exist" while telling the user the same wrong thing.
	for (const locale of LOCALES) {
		const drain = value(locale, 'flow.error.server_draining.drain');
		const contention = value(locale, 'flow.error.server_draining.contention');
		assertNotEqual(
			drain,
			contention,
			`${locale}.json gives both server_draining reasons identical copy`
		);
	}
});

suite.check('every sync indicator state has a zh and en key', () => {
	for (const state of SYNC_STATES) {
		for (const locale of LOCALES) {
			assert(
				value(locale, `flow.sync.${state}`) !== undefined,
				`${locale}.json is missing flow.sync.${state}`
			);
		}
	}
});

// ---- 5. zh really is Chinese, en really is not ---------------------------------------------

const CJK = /[㐀-䶿一-鿿豈-﫿]/;

suite.check('zh flow copy is Chinese and en flow copy carries no Han characters', () => {
	const zhWithoutHan: string[] = [];
	const enWithHan: string[] = [];
	for (const key of flowKeys('zh')) {
		const zhText = value('zh', key) ?? '';
		const enText = value('en', key) ?? '';
		// Allow a zh value with no Han only when it is punctuation/format-only (e.g. "{n}").
		if (!CJK.test(zhText) && /\p{L}/u.test(zhText.replace(/\{[^}]*\}/g, '')))
			zhWithoutHan.push(`${key} = ${zhText}`);
		if (CJK.test(enText)) enWithHan.push(`${key} = ${enText}`);
	}
	assert(
		zhWithoutHan.length === 0,
		`zh.json flow copy left untranslated:\n  ${zhWithoutHan.join('\n  ')}`
	);
	assert(
		enWithHan.length === 0,
		`en.json flow copy contains Chinese:\n  ${enWithHan.join('\n  ')}`
	);
});

// ---- 6. no hard-coded user copy in the Flow source -----------------------------------------

suite.check('no Flow source file contains a hard-coded Chinese string literal', () => {
	const offenders: string[] = [];
	for (const file of flowSourceFiles()) {
		// Comments are documentation, not user copy, and this repo's Flow modules quote the
		// Chinese contracts extensively on purpose.
		const source = stripCommentSource(readFileSync(file, 'utf8'));
		for (const [index, line] of source.split('\n').entries()) {
			const literals = line.match(/'[^']*'|"[^"]*"|`[^`]*`/g) ?? [];
			for (const literal of literals) {
				if (CJK.test(literal))
					offenders.push(`${relative(ROOT, file)}:${index + 1} ${literal.trim()}`);
			}
		}
	}
	assert(offenders.length === 0, `hard-coded Chinese copy:\n  ${offenders.join('\n  ')}`);
});

suite.check('every static i18n key referenced by Flow source exists in both locales', () => {
	const missing: string[] = [];
	const referenced = new Set<string>();
	for (const file of flowSourceFiles()) {
		const source = readFileSync(file, 'utf8');
		// `$t('key')`, `t('key')`, `get(t)('key')` and `announce('key', ...)`.
		for (const match of source.matchAll(/(?:\$t|\bt\)|\bannounce)\(\s*'([^']+)'/g))
			referenced.add(match[1]);
		for (const match of source.matchAll(/\$t\(\s*"([^"]+)"/g)) referenced.add(match[1]);
	}
	assert(
		referenced.size > 0,
		'no i18n keys were extracted from the Flow source at all -- the extractor is broken'
	);
	for (const key of referenced) {
		for (const locale of LOCALES) {
			if (value(locale, key) === undefined)
				missing.push(`${locale}.json is missing referenced key ${key}`);
		}
	}
	assert(missing.length === 0, missing.join('\n  '));
});

suite.check('dynamic i18n keys in Flow source are only the two enumerated families', () => {
	// A template-literal key cannot be checked by name, so each one must belong to a family whose
	// completions this suite already enumerates exhaustively: `flow.sync.${SyncState}` and
	// `flow.error.${FlowErrorCode}` (the latter via `flowErrorI18nKey`). Anything else is a key
	// this gate would not be covering, and is refused rather than silently skipped.
	const allowed = [/^flow\.sync\.\$\{[^}]+\}$/, /^flow\.error\.\$\{[^}]+\}$/];
	const offenders: string[] = [];
	for (const file of flowSourceFiles()) {
		const source = stripCommentSource(readFileSync(file, 'utf8'));
		for (const [index, line] of source.split('\n').entries()) {
			for (const match of line.matchAll(/\$t\(\s*`([^`]+)`/g)) {
				if (!allowed.some((pattern) => pattern.test(match[1]))) {
					offenders.push(`${relative(ROOT, file)}:${index + 1} \`${match[1]}\``);
				}
			}
		}
	}
	assert(
		offenders.length === 0,
		`un-enumerated dynamic i18n key(s); add the family's exhaustive check to this suite first:\n  ${offenders.join('\n  ')}`
	);
});

/**
 * Blanks out every comment in a source file while preserving line count and column offsets, so
 * the scans above see only code.
 *
 * This has to handle THREE comment syntaxes, not one: `//` line comments, `/* *\/` block
 * comments, and Svelte markup's `<!-- -->` -- and the block forms must be stripped across the
 * whole file rather than per line, because this repo's Flow modules quote the Chinese contracts
 * in multi-line comments extensively. A per-line stripper reports every one of those quotations
 * as hard-coded copy, which is a false positive severe enough to make the check unusable (it
 * fired six times on the first run of this suite).
 */
function stripCommentSource(source: string): string {
	const blank = (text: string): string => text.replace(/[^\n]/g, ' ');
	return source
		.replace(/<!--[\s\S]*?-->/g, blank)
		.replace(/\/\*[\s\S]*?\*\//g, blank)
		.replace(
			/(^|[^:])\/\/[^\n]*/g,
			(match, prefix: string) => prefix + blank(match.slice(prefix.length))
		);
}

export const result = suite.result();

if (import.meta.main) finish(result);

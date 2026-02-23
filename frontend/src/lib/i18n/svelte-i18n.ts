import { derived, get, writable, type Readable } from 'svelte/store';

type Dictionary = Record<string, unknown>;
type Loader = () => Promise<unknown>;

const loaders = new Map<string, Loader>();
const dictionaries = writable<Record<string, Dictionary>>({});
const currentLocale = writable('zh');
let fallbackLocale = 'zh';

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null;
}

function getValue(dict: Dictionary, path: string): unknown {
	return path.split('.').reduce<unknown>((acc, key) => {
		if (isObject(acc) && key in acc) {
			return acc[key];
		}
		return undefined;
	}, dict);
}

function interpolate(template: string, values?: Record<string, unknown>): string {
	if (!values) {
		return template;
	}

	return template.replace(/\{(\w+)\}/g, (_match, key: string) => {
		const value = values[key];
		return value === undefined || value === null ? '' : String(value);
	});
}

async function loadLocale(lang: string): Promise<void> {
	const existing = get(dictionaries)[lang];
	if (existing) {
		return;
	}

	const loader = loaders.get(lang);
	if (!loader) {
		return;
	}

	const module = await loader();
	const dict = isObject(module) && 'default' in module && isObject(module.default)
		? (module.default as Dictionary)
		: ((module as Dictionary) ?? {});

	dictionaries.update((prev) => ({ ...prev, [lang]: dict }));
}

export function register(lang: string, loader: Loader): void {
	loaders.set(lang, loader);
}

export function getLocaleFromNavigator(): string {
	if (typeof navigator === 'undefined' || typeof navigator.language !== 'string') {
		return fallbackLocale;
	}
	return navigator.language.toLowerCase().startsWith('en') ? 'en' : 'zh';
}

export function init(options: { fallbackLocale?: string; initialLocale?: string }): void {
	fallbackLocale = options.fallbackLocale ?? 'zh';
	const initialLocale = options.initialLocale ?? fallbackLocale;
	void loadLocale(fallbackLocale);
	void loadLocale(initialLocale);
	currentLocale.set(initialLocale);
}

export const locale = {
	subscribe: currentLocale.subscribe,
	set(lang: string) {
		void loadLocale(lang);
		currentLocale.set(lang);
	},
	update: currentLocale.update
};

export const t: Readable<(key: string, options?: { values?: Record<string, unknown> }) => string> = derived(
	[currentLocale, dictionaries],
	([$locale, $dicts]) => {
		return (key: string, options?: { values?: Record<string, unknown> }) => {
			const primary = $dicts[$locale] ?? {};
			const fallback = $dicts[fallbackLocale] ?? {};
			const raw = getValue(primary, key) ?? getValue(fallback, key) ?? key;
			return interpolate(String(raw), options?.values);
		};
	}
);

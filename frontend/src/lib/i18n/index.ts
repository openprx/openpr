import { browser } from '$app/environment';
import { init, locale, register } from 'svelte-i18n';

register('zh', () => import('./zh.json'));
register('en', () => import('./en.json'));

const supportedLocales = new Set(['zh', 'en']);

function normalizeLocale(lang: string | null): 'zh' | 'en' {
	return lang === 'en' ? 'en' : 'zh';
}

function syncDocumentLang(lang: string): void {
	if (browser) {
		document.documentElement.lang = normalizeLocale(lang);
	}
}

const savedLocale = browser && supportedLocales.has(localStorage.getItem('locale') ?? '')
	? normalizeLocale(localStorage.getItem('locale'))
	: 'zh';

init({
	fallbackLocale: 'zh',
	initialLocale: savedLocale
});

export function setLocale(lang: string): void {
	const next = normalizeLocale(lang);
	locale.set(next);
	syncDocumentLang(next);
	if (browser) {
		localStorage.setItem('locale', next);
	}
}

syncDocumentLang(savedLocale);

import { browser } from '$app/environment';
import { init, locale, register } from 'svelte-i18n';

register('zh', () => import('./zh.json'));
register('en', () => import('./en.json'));

const savedLocale = browser ? localStorage.getItem('locale') || 'zh' : 'zh';

init({
	fallbackLocale: 'zh',
	initialLocale: savedLocale
});

export function setLocale(lang: string): void {
	locale.set(lang);
	if (browser) {
		localStorage.setItem('locale', lang);
	}
}

import { browser } from '$app/environment';
import { writable } from 'svelte/store';

export type Theme = 'light' | 'dark' | 'system';

const THEME_KEY = 'theme';
const DEFAULT_THEME: Theme = 'light';

function getStoredTheme(): Theme {
	if (!browser) return DEFAULT_THEME;
	const stored = localStorage.getItem(THEME_KEY);
	if (stored === 'light' || stored === 'dark' || stored === 'system') {
		return stored;
	}
	return DEFAULT_THEME;
}

export const theme = writable<Theme>(getStoredTheme());

export function applyTheme(nextTheme: Theme) {
	if (!browser) return;
	const root = document.documentElement;
	const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
	const shouldUseDark = nextTheme === 'dark' || (nextTheme === 'system' && prefersDark);
	root.classList.toggle('dark', shouldUseDark);
}

export function setTheme(nextTheme: Theme) {
	theme.set(nextTheme);
	if (!browser) return;
	localStorage.setItem(THEME_KEY, nextTheme);
	applyTheme(nextTheme);
}

if (browser) {
	const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
	mediaQuery.addEventListener('change', () => {
		if (getStoredTheme() === 'system') {
			applyTheme('system');
		}
	});
}

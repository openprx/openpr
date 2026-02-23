import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

export function timeAgo(date: string | Date): string {
	const now = new Date();
	const past = new Date(date);
	const diffMs = now.getTime() - past.getTime();
	const diffMin = Math.floor(diffMs / 60000);
	const diffHour = Math.floor(diffMs / 3600000);
	const diffDay = Math.floor(diffMs / 86400000);

	if (diffMin < 1) return get(t)('common.justNow');
	if (diffMin < 60) return get(t)('common.minutesAgo', { values: { count: diffMin } });
	if (diffHour < 24) return get(t)('common.hoursAgo', { values: { count: diffHour } });
	return get(t)('common.daysAgo', { values: { count: diffDay } });
}

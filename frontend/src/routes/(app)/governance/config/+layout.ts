import { goto } from '$app/navigation';
import { browser } from '$app/environment';
import { isAdminUser } from '$lib/utils/auth';
import type { User } from '$lib/api/auth';

export const ssr = false;

export function load() {
	if (!browser) {
		return {};
	}

	const raw = localStorage.getItem('auth_user');
	if (!raw) {
		goto('/workspace');
		return {};
	}

	try {
		const user = JSON.parse(raw) as User;
		if (!isAdminUser(user)) {
			goto('/workspace');
		}
	} catch {
		goto('/workspace');
	}

	return {};
}

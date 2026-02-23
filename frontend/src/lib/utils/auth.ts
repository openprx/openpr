import type { User } from '$lib/api/auth';

export function isAdminUser(user: User | null): boolean {
	if (!user) {
		return false;
	}

	if (user.role === 'admin' || user.role === 'owner') {
		return true;
	}

	const email = typeof user.email === 'string' ? user.email.toLowerCase() : '';
	return email === 'admin@openpr.local';
}

export function requireRouteParam(
	value: string | undefined,
	name: string
): string {
	if (!value) {
		throw new Error(`Missing route param: ${name}`);
	}

	const normalized = value.trim();
	if (!normalized || normalized === 'undefined' || normalized === 'null') {
		throw new Error(`Missing route param: ${name}`);
	}

	return normalized;
}

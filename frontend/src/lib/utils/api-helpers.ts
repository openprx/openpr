function isRecord(data: unknown): data is Record<string, unknown> {
	return typeof data === 'object' && data !== null;
}

export function extractNumber(data: unknown, key: string, fallback: number): number {
	if (!isRecord(data)) {
		return fallback;
	}
	const value = data[key];
	return typeof value === 'number' ? value : fallback;
}

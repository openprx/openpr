/**
 * URL sanitisation for values that originate from user input and end up in
 * `href` / `src` attributes. Record values are shared between workspace
 * members, so an unsanitised `javascript:` value stored by one member would
 * execute in another member's session on this origin.
 */

const SAFE_URL_PROTOCOLS = new Set(['http:', 'https:', 'blob:', 'mailto:']);

// Only raster/vector-free image payloads: `image/svg+xml` can carry scripts.
const SAFE_DATA_IMAGE_PATTERN = /^data:image\/(?:png|jpeg|jpg|gif|webp|avif|bmp);/i;

// Any base works: it only has to make relative URLs resolvable so that the
// parser reports the protocol they would inherit from the current origin.
const RELATIVE_URL_BASE = 'https://openpr.invalid/';

function normalizeCandidate(value: unknown): string {
	return typeof value === 'string' ? value.trim() : '';
}

function hasSafeProtocol(candidate: string): boolean {
	try {
		return SAFE_URL_PROTOCOLS.has(new URL(candidate, RELATIVE_URL_BASE).protocol);
	} catch {
		return false;
	}
}

/**
 * Returns the URL when it is safe to place in an `href`, otherwise an empty
 * string. Relative URLs stay relative; unsupported schemes are dropped.
 */
export function safeLinkUrl(value: unknown): string {
	const candidate = normalizeCandidate(value);
	if (!candidate) return '';
	return hasSafeProtocol(candidate) ? candidate : '';
}

/**
 * Same as {@link safeLinkUrl} but additionally accepts inline `data:image/*`
 * payloads, which are used for locally rendered previews and signatures.
 */
export function safeImageUrl(value: unknown): string {
	const candidate = normalizeCandidate(value);
	if (!candidate) return '';
	if (SAFE_DATA_IMAGE_PATTERN.test(candidate)) return candidate;
	return hasSafeProtocol(candidate) ? candidate : '';
}

/** True when the value can be stored as a link target without sanitisation. */
export function isSafeLinkUrl(value: unknown): boolean {
	return safeLinkUrl(value) !== '';
}

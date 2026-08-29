// Lexicographic fractional-index key generation for the Navigator document's sibling ordering
// (`domain-model-v1.md`: "Navigator 一个 workspace/project 的导航排序与显示元数据"; ADR-0012 §4:
// "v0.4 的拖拽只做同一父级内重排...落 flow.content.accepted"). Keys are plain base-36 strings so
// they compare correctly with `<` and can be stored directly as a Loro map value -- no separate
// integer "order" column that would need renumbering on every insert.

const ALPHABET = '0123456789abcdefghijklmnopqrstuvwxyz';
const BASE = ALPHABET.length;

function charAt(key: string, index: number): number {
	return index < key.length ? ALPHABET.indexOf(key[index]) : 0;
}

/**
 * Returns a key that sorts strictly between `lower` and `upper`. Pass `undefined` for either
 * bound to mean "no bound" (start/end of the sibling list). Throws only if `lower >= upper`,
 * which is a caller bug (stale sibling order), not a runtime/user condition.
 */
export function keyBetween(lower: string | undefined, upper: string | undefined): string {
	const lo = lower ?? '';
	const hi = upper ?? '';
	if (hi !== '' && lo >= hi) {
		throw new Error(`keyBetween: lower (${lo}) must sort before upper (${hi})`);
	}

	let result = '';
	let index = 0;
	for (;;) {
		const loDigit = charAt(lo, index);
		const hiDigit = index < hi.length ? charAt(hi, index) : BASE;
		if (hiDigit - loDigit > 1) {
			const mid = loDigit + Math.floor((hiDigit - loDigit) / 2);
			result += ALPHABET[mid];
			return result;
		}
		result += ALPHABET[loDigit];
		index += 1;
		if (index > 64) {
			// Astronomically unlikely (would need 64 consecutive reorders between the same two
			// neighbours without ever rebalancing); fail closed rather than loop forever.
			throw new Error('keyBetween: exceeded maximum key depth');
		}
	}
}

/** Generates `count` initial keys, evenly spread, for seeding a brand-new sibling list. */
export function initialKeys(count: number): string[] {
	const keys: string[] = [];
	let previous: string | undefined;
	for (let i = 0; i < count; i += 1) {
		const key = keyBetween(previous, undefined);
		keys.push(key);
		previous = key;
	}
	return keys;
}

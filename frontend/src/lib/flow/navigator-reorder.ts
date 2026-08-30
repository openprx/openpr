// The single reorder engine behind ALL three navigator move gestures -- keyboard lift/drop,
// pointer drag-and-drop, and the context menu's Move before/after.
//
// `contracts/ui-surface-v1.md` "a11y v0.4 基线": "context menu 提供 Move before/after/inside/
// outside，功能与 pointer drag 相同并走 `CommandService`" and "所有操作可在无 pointer 下完成".
// That is an equivalence claim, and an equivalence claim cannot be tested while each gesture
// computes its own fractional-index bounds inline: three separate copies of "which neighbours
// bracket the destination" can (and, before this module existed, did) disagree. Every gesture
// now reduces to the same two-step pipeline --
//
//     gesture -> destination index (in the sibling list WITHOUT the moved object)
//     destination index -> order key (`orderKeyForDestination`)
//
// -- so equivalence is structural, and `tests/flow-navigator-equivalence.test.ts` can assert it
// exhaustively over every (list size, source, target) triple rather than sampling.
//
// Scope (`ADR-0012` §4): v0.4 reorders within ONE parent only. Cross-parent moves need the
// `move_object` governance command, which is v0.5. Nothing here can express a parent change --
// callers pass a single parent's sibling list, and the result is always a new order key for a
// member of that same list.

import { keyBetween } from './fractional-index';

/** One sibling under a single parent, in committed order, with its current fractional-index key
 * from the Navigator document's `order` map. */
export interface OrderedSibling {
	readonly id: string;
	readonly orderKey: string;
}

/** Which end of the target a pointer drop lands on. Derived, never asked of the caller. */
export type DropSide = 'before' | 'after';

/**
 * The order key to write so that `sourceId` sits at `destination` in the sibling list.
 *
 * `destination` indexes the list with `sourceId` ALREADY REMOVED, so it ranges over
 * `[0, siblings.length - 1]`: `0` means "first", `siblings.length - 1` means "last". Expressing
 * every gesture in this one coordinate system is what makes them comparable -- the alternative
 * ("index in the original list") is ambiguous precisely for downward moves, which is where the
 * old per-gesture copies diverged.
 *
 * Returns `null` when the move is a no-op or not expressible (unknown source, out-of-range
 * destination, single-element list). A `null` means "announce nothing changed", never "write
 * something approximate".
 */
export function orderKeyForDestination(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	destination: number
): string | null {
	const currentIndex = siblings.findIndex((sibling) => sibling.id === sourceId);
	if (currentIndex < 0) return null;
	const rest = siblings.filter((sibling) => sibling.id !== sourceId);
	if (!Number.isInteger(destination) || destination < 0 || destination > rest.length) return null;
	// Already there: the object's committed neighbours are exactly the ones it would land
	// between, so there is nothing to write.
	if (destination === currentIndex) return null;

	const lower = rest[destination - 1];
	const upper = rest[destination];
	return keyBetween(lower?.orderKey, upper?.orderKey);
}

/**
 * Keyboard lift mode: ArrowUp/ArrowDown move the lifted object one sibling position.
 *
 * `contracts/ui-surface-v1.md`: "Space 提起/放下；ArrowUp/Down 调 sibling 位置". Returns the
 * destination index, or `null` at the ends of the list (where the contract wants an announcement,
 * not a wrap-around).
 */
export function keyboardStepDestination(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	delta: -1 | 1
): number | null {
	const currentIndex = siblings.findIndex((sibling) => sibling.id === sourceId);
	if (currentIndex < 0) return null;
	const destination = currentIndex + delta;
	if (destination < 0 || destination >= siblings.length) return null;
	return destination;
}

/**
 * Pointer drop: dropping the dragged object ONTO a sibling.
 *
 * The side is derived from the drag direction, which is the convention every list DnD uses and
 * -- more importantly here -- the only one under which pointer drag and keyboard reach the SAME
 * set of destinations. Under the naive "always insert before the target" rule, no pointer gesture
 * can move an object to the last position (dropping on the last sibling puts it before that
 * sibling, i.e. back where it started), while ArrowDown can; the two gestures would then not be
 * equivalent, and `navigator_keyboard_drag_equivalence` would be asserting something false.
 *
 * Returns `null` for a drop on itself, on a non-sibling, or on an unknown id.
 */
export function pointerDropDestination(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	targetId: string
): number | null {
	if (sourceId === targetId) return null;
	const currentIndex = siblings.findIndex((sibling) => sibling.id === sourceId);
	const targetIndex = siblings.findIndex((sibling) => sibling.id === targetId);
	if (currentIndex < 0 || targetIndex < 0) return null;
	// Both sides collapse to the target's index in the ORIGINAL list, which is why this reads as
	// a plain return rather than a branch:
	//   - target above the source: "before the target" is that target's own slot, and removing
	//     the source (which sits below it) does not shift it, so the destination is `targetIndex`;
	//   - target below the source: removing the source shifts the target one slot earlier, to
	//     `targetIndex - 1`, and "after the target" is one past that -- also `targetIndex`.
	return targetIndex;
}

/** Which side of the target a drop lands on, for the live-region announcement. */
export function pointerDropSide(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	targetId: string
): DropSide | null {
	const currentIndex = siblings.findIndex((sibling) => sibling.id === sourceId);
	const targetIndex = siblings.findIndex((sibling) => sibling.id === targetId);
	if (currentIndex < 0 || targetIndex < 0 || currentIndex === targetIndex) return null;
	return targetIndex > currentIndex ? 'after' : 'before';
}

/**
 * Context menu "Move before"/"Move after": swap with the immediate neighbour on that side.
 *
 * `ui-surface-v1.md` requires this to be functionally identical to the drag gesture, and it is:
 * both resolve to `orderKeyForDestination` with the same destination the single keyboard step
 * would produce.
 */
export function contextMenuDestination(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	direction: DropSide
): number | null {
	return keyboardStepDestination(siblings, sourceId, direction === 'before' ? -1 : 1);
}

/** Applies a destination to an id list, for previewing a lift in the tree without writing to the
 * CRDT (`FlowNavigator`'s `draftSiblingOrder`) and for asserting gesture equivalence by resulting
 * ORDER rather than only by key. */
export function applyDestination(
	ids: readonly string[],
	sourceId: string,
	destination: number
): string[] {
	const rest = ids.filter((id) => id !== sourceId);
	if (destination < 0 || destination > rest.length) return [...ids];
	return [...rest.slice(0, destination), sourceId, ...rest.slice(destination)];
}

/** The order key for a brand-new object appended after every existing sibling. Shares this
 * module's `keyBetween` usage so the navigator never reaches for the raw index primitive itself. */
export function appendOrderKey(siblings: readonly OrderedSibling[]): string {
	return keyBetween(siblings.at(-1)?.orderKey, undefined);
}

/** Order keys for siblings that have no key yet (objects created before the navigator document
 * existed, or by another surface). Returned in the same order as `ids`. */
export function seedOrderKeys(existingKeys: readonly string[], count: number): string[] {
	let previous = existingKeys.length > 0 ? [...existingKeys].sort().at(-1) : undefined;
	const keys: string[] = [];
	for (let index = 0; index < count; index += 1) {
		const key = keyBetween(previous, undefined);
		keys.push(key);
		previous = key;
	}
	return keys;
}

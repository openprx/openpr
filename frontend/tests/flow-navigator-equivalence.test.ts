/**
 * Hard gate `navigator_keyboard_drag_equivalence` -- the automatable half.
 *
 * `contracts/ui-surface-v1.md` "a11y v0.4 基线": "context menu 提供 Move before/after/inside/
 * outside，功能与 pointer drag 相同并走 CommandService" and "所有操作可在无 pointer 下完成".
 *
 * WHAT THIS PROVES (and what it deliberately does not)
 *
 * Equivalence between two input methods is a claim about the OUTCOME they produce, so this suite
 * asserts on outcomes: for every reachable (sibling count, source, target) triple it runs the
 * pointer gesture, the keyboard gesture and the context-menu gesture through the production
 * reorder engine (`src/lib/flow/navigator-reorder.ts`) and requires the resulting sibling ORDER
 * and the resulting fractional-index ORDER KEY to be identical. It also requires the reachable
 * destination SETS to be equal in both directions -- an outcome the pointer can produce but the
 * keyboard cannot (or the reverse) is exactly the inequivalence a11y is about.
 *
 * It does NOT prove that the rendered tree is operable by a real screen reader, that the focus
 * ring is visible, or that `aria-live` announcements are actually voiced. Those need a human and
 * are covered by the `navigator_a11y` manual sign-off key (`gates/gate-commands.md`'s v0.4
 * "人工 keys" line). Nothing here should be read as covering them.
 *
 * Run standalone: `bun tests/flow-navigator-equivalence.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	appendOrderKey,
	applyDestination,
	contextMenuDestination,
	keyboardStepDestination,
	orderKeyForDestination,
	pointerDropDestination,
	pointerDropSide,
	seedOrderKeys,
	type OrderedSibling
} from '../src/lib/flow/navigator-reorder';

const ROOT = new URL('..', import.meta.url).pathname;
const suite = new Suite('navigator_keyboard_drag_equivalence');

/** A sibling list of `count` objects with freshly seeded, strictly increasing order keys. */
function makeSiblings(count: number): OrderedSibling[] {
	const keys = seedOrderKeys([], count);
	return keys.map((orderKey, index) => ({ id: String.fromCharCode(65 + index), orderKey }));
}

function ids(siblings: readonly OrderedSibling[]): string[] {
	return siblings.map((sibling) => sibling.id);
}

/** Applies one committed reorder and returns the resulting list, re-sorted by order key exactly
 * the way `FlowNavigator`'s `tree` derivation does (`orderOf(a).localeCompare(orderOf(b))`). */
function commit(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	destination: number | null
): OrderedSibling[] {
	if (destination === null) return [...siblings];
	const key = orderKeyForDestination(siblings, sourceId, destination);
	if (key === null) return [...siblings];
	return siblings
		.map((sibling) => (sibling.id === sourceId ? { ...sibling, orderKey: key } : sibling))
		.sort((a, b) => a.orderKey.localeCompare(b.orderKey));
}

/** The keyboard gesture's destination: lift, press Arrow `|steps|` times, drop. Mirrors
 * `FlowNavigator`'s lift mode, which previews with `applyDestination` and commits the draft index
 * as the destination. Returns `null` when the run moved nothing -- pressing ArrowUp on the first
 * sibling is a refusal, not a move to where it already was, and counting it as a reachable
 * destination would make the reachability comparison below vacuously agree. */
function keyboardDestination(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	steps: number
): number | null {
	const startIndex = siblings.findIndex((sibling) => sibling.id === sourceId);
	if (startIndex < 0 || steps === 0) return null;
	let draft = ids(siblings);
	const delta: -1 | 1 = steps > 0 ? 1 : -1;
	for (let taken = 0; taken < Math.abs(steps); taken += 1) {
		const draftSiblings = draft.map((id) => ({
			id,
			orderKey: siblings.find((sibling) => sibling.id === id)?.orderKey ?? ''
		}));
		const destination = keyboardStepDestination(draftSiblings, sourceId, delta);
		if (destination === null) break;
		draft = applyDestination(draft, sourceId, destination);
	}
	const finalIndex = draft.indexOf(sourceId);
	return finalIndex === startIndex ? null : finalIndex;
}

/** The keyboard gesture, committed. */
function keyboardGesture(
	siblings: readonly OrderedSibling[],
	sourceId: string,
	steps: number
): OrderedSibling[] {
	return commit(siblings, sourceId, keyboardDestination(siblings, sourceId, steps));
}

const SIZES = [2, 3, 4, 5, 6];

// ---- 1. pointer drop == keyboard steps, exhaustively ---------------------------------------

suite.check(
	'pointer drop and the equivalent keyboard run produce the same order AND the same key',
	() => {
		let compared = 0;
		for (const size of SIZES) {
			const siblings = makeSiblings(size);
			for (const source of siblings) {
				for (const target of siblings) {
					if (source.id === target.id) continue;
					const sourceIndex = siblings.findIndex((s) => s.id === source.id);
					const targetIndex = siblings.findIndex((s) => s.id === target.id);

					const pointerDestination = pointerDropDestination(siblings, source.id, target.id);
					assert(
						pointerDestination !== null,
						`pointer drop ${source.id}->${target.id} (n=${size}) was refused`
					);
					const byPointer = commit(siblings, source.id, pointerDestination);
					const byKeyboard = keyboardGesture(siblings, source.id, targetIndex - sourceIndex);

					assertDeepEqual(
						ids(byPointer),
						ids(byKeyboard),
						`n=${size} drop ${source.id} onto ${target.id}: pointer and keyboard disagree on the resulting order`
					);
					const pointerKey = byPointer.find((s) => s.id === source.id)?.orderKey;
					const keyboardKey = byKeyboard.find((s) => s.id === source.id)?.orderKey;
					assertEqual(
						pointerKey,
						keyboardKey,
						`n=${size} drop ${source.id} onto ${target.id}: pointer and keyboard wrote different order keys`
					);
					compared += 1;
				}
			}
		}
		assert(
			compared === 2 + 6 + 12 + 20 + 30,
			`expected 70 (source,target) pairs, compared ${compared}`
		);
	}
);

// ---- 2. context menu == one keyboard step --------------------------------------------------

suite.check('context menu Move before/after equals exactly one keyboard step', () => {
	for (const size of SIZES) {
		const siblings = makeSiblings(size);
		for (const source of siblings) {
			for (const [direction, delta] of [
				['before', -1],
				['after', 1]
			] as const) {
				const menuDestination = contextMenuDestination(siblings, source.id, direction);
				const keyDestination = keyboardStepDestination(siblings, source.id, delta);
				assertEqual(
					menuDestination,
					keyDestination,
					`n=${size} ${source.id} Move ${direction}: menu and keyboard chose different destinations`
				);
				assertDeepEqual(
					ids(commit(siblings, source.id, menuDestination)),
					ids(keyboardGesture(siblings, source.id, delta)),
					`n=${size} ${source.id} Move ${direction}: menu and keyboard produced different orders`
				);
			}
		}
	}
});

// ---- 3. the reachable destination sets are equal in both directions ------------------------

suite.check('pointer and keyboard reach exactly the same destination set', () => {
	// This is the assertion that catches the classic "drop always inserts BEFORE the target"
	// implementation: under that rule no pointer gesture can move an object to the last position,
	// so the pointer's reachable set is a strict subset of the keyboard's and the two input
	// methods are NOT equivalent -- while every individual "same source, same target" comparison
	// still passes, because it compares the two on a destination the pointer can express.
	for (const size of SIZES) {
		const siblings = makeSiblings(size);
		for (const source of siblings) {
			const viaPointer = new Set<number>();
			for (const target of siblings) {
				if (target.id === source.id) continue;
				const destination = pointerDropDestination(siblings, source.id, target.id);
				if (destination !== null) viaPointer.add(destination);
			}
			const viaKeyboard = new Set<number>();
			for (let steps = -(size - 1); steps <= size - 1; steps += 1) {
				const destination = keyboardDestination(siblings, source.id, steps);
				if (destination !== null) viaKeyboard.add(destination);
			}
			assertDeepEqual(
				[...viaPointer].sort((a, b) => a - b),
				[...viaKeyboard].sort((a, b) => a - b),
				`n=${size} source ${source.id}: pointer and keyboard reach different destination sets`
			);
			assertEqual(
				viaKeyboard.size,
				size - 1,
				`n=${size} source ${source.id}: keyboard should reach every other slot`
			);
		}
	}
});

// ---- 4. the key that gets written actually sorts where it was meant to ---------------------

suite.check('every gesture writes a key that sorts into the intended position', () => {
	// The bug class this catches is a bounds-computation error: picking the neighbours that
	// bracket the destination wrongly still yields a valid, strictly-between key, so `keyBetween`
	// never throws -- the object just silently lands somewhere else. Only re-deriving the order
	// from the written keys detects it.
	for (const size of SIZES) {
		const siblings = makeSiblings(size);
		for (const source of siblings) {
			for (let destination = 0; destination < size; destination += 1) {
				const expected = applyDestination(ids(siblings), source.id, destination);
				assertDeepEqual(
					ids(commit(siblings, source.id, destination)),
					expected,
					`n=${size} move ${source.id} to destination ${destination}: the written key sorts elsewhere`
				);
			}
		}
	}
});

suite.check('repeated reorders stay consistent over a long random run', () => {
	// Fractional-index keys get longer as objects are inserted repeatedly between the same pair;
	// a single-shot test never exercises that. 400 random moves on one list, re-deriving the
	// order from the keys each time.
	let siblings = makeSiblings(6);
	let expected = ids(siblings);
	let seed = 0x5eed;
	const next = (bound: number): number => {
		seed = (seed * 1103515245 + 12345) & 0x7fffffff;
		return seed % bound;
	};
	for (let round = 0; round < 400; round += 1) {
		const sourceIndex = next(siblings.length);
		const destination = next(siblings.length);
		const source = siblings[sourceIndex];
		expected = applyDestination(ids(siblings), source.id, destination);
		siblings = commit(siblings, source.id, destination);
		assertDeepEqual(
			ids(siblings),
			expected,
			`round ${round}: order diverged after moving ${source.id} to ${destination}`
		);
	}
	assert(
		siblings.every((sibling) => sibling.orderKey.length <= 64),
		'a key grew past the fractional-index depth ceiling during a normal run'
	);
});

// ---- 5. refusals are refusals, not silent no-ops --------------------------------------------

suite.check('out-of-range, self and unknown gestures are refused rather than approximated', () => {
	const siblings = makeSiblings(4);
	assertEqual(
		keyboardStepDestination(siblings, 'A', -1),
		null,
		'ArrowUp on the first sibling must refuse'
	);
	assertEqual(
		keyboardStepDestination(siblings, 'D', 1),
		null,
		'ArrowDown on the last sibling must refuse'
	);
	assertEqual(
		keyboardStepDestination(siblings, 'Z', 1),
		null,
		'a step on an unknown id must refuse'
	);
	assertEqual(pointerDropDestination(siblings, 'B', 'B'), null, 'a drop on itself must refuse');
	assertEqual(
		pointerDropDestination(siblings, 'B', 'Z'),
		null,
		'a drop on a non-sibling must refuse'
	);
	assertEqual(pointerDropSide(siblings, 'B', 'B'), null, 'a drop on itself has no side');
	assertEqual(
		pointerDropSide(siblings, 'B', 'D'),
		'after',
		'dropping downward lands after the target'
	);
	assertEqual(
		pointerDropSide(siblings, 'C', 'A'),
		'before',
		'dropping upward lands before the target'
	);
	assertEqual(
		orderKeyForDestination(siblings, 'B', 1),
		null,
		'moving to where it already is writes nothing'
	);
	assertEqual(
		orderKeyForDestination(siblings, 'B', 9),
		null,
		'an out-of-range destination writes nothing'
	);
	assertEqual(orderKeyForDestination(siblings, 'Z', 0), null, 'an unknown source writes nothing');
	assertEqual(
		orderKeyForDestination(makeSiblings(1), 'A', 0),
		null,
		'a single-element list has nowhere to move'
	);
});

suite.check('a new sibling appends after every existing one', () => {
	const siblings = makeSiblings(4);
	const key = appendOrderKey(siblings);
	assert(key > siblings[siblings.length - 1].orderKey, `appended key ${key} does not sort last`);
	assertEqual(
		appendOrderKey([]),
		seedOrderKeys([], 1)[0],
		'appending to an empty list matches seeding one'
	);
});

// ---- 6. the component cannot drift back to per-gesture maths --------------------------------

const NAVIGATOR = readFileSync(join(ROOT, 'src/lib/components/flow/FlowNavigator.svelte'), 'utf8');

suite.check('FlowNavigator computes no order bounds of its own', () => {
	// The equivalence above is only meaningful while all three gestures actually go through the
	// shared engine. Before this refactor each gesture inlined its own `keyBetween(...)` bounds,
	// and the context-menu copy bracketed the destination with the WRONG neighbours -- the exact
	// drift this assertion now makes impossible to reintroduce silently.
	assert(
		!NAVIGATOR.includes('keyBetween'),
		'FlowNavigator references keyBetween directly again; reorder bounds belong in navigator-reorder.ts'
	);
	assert(
		!/from '\$lib\/flow\/fractional-index'/.test(NAVIGATOR),
		'FlowNavigator imports the raw fractional-index primitive again'
	);
});

suite.check('all three gestures funnel through one commit path', () => {
	const writes = [...NAVIGATOR.matchAll(/getNamedMap\([^)]*'order'\)\.set\(/g)];
	// Two legitimate writers: `commitReorder` (every reorder gesture) and the create/seed paths.
	assert(
		writes.length <= 3,
		`FlowNavigator has ${writes.length} direct order-map writes; every reorder must go through commitReorder`
	);
	for (const gesture of ['commitLift', 'moveRelative', 'onDrop']) {
		const body = functionBody(NAVIGATOR, gesture);
		assert(body !== null, `FlowNavigator no longer defines ${gesture}`);
		assert(body.includes('commitReorder('), `${gesture} does not delegate to commitReorder`);
		assert(
			!body.includes('.set('),
			`${gesture} writes the order map directly instead of going through commitReorder`
		);
	}
});

/** Extracts a `function name(...) { ... }` body by brace matching. */
function functionBody(source: string, name: string): string | null {
	const match = new RegExp(`function ${name}\\s*\\(`).exec(source);
	if (!match) return null;
	const open = source.indexOf('{', match.index);
	if (open < 0) return null;
	let depth = 0;
	for (let index = open; index < source.length; index += 1) {
		if (source[index] === '{') depth += 1;
		else if (source[index] === '}') {
			depth -= 1;
			if (depth === 0) return source.slice(open, index + 1);
		}
	}
	return null;
}

// ---- what a human still has to confirm ------------------------------------------------------

suite.skip(
	'the tree is genuinely operable with a screen reader',
	'role/aria-level/aria-expanded/aria-selected and the aria-live announcements can be read out ' +
		'of the markup, but whether a real screen reader ANNOUNCES the object, its parent and its ' +
		'new position on each move is an observation, not an assertion; manual key `navigator_a11y`'
);
suite.skip(
	'focus ring, 44px touch targets and reduced-motion',
	'visual/behavioural properties of the rendered tree; manual key `navigator_a11y`'
);
suite.skip(
	'focus does not fall to body when a remote move deletes the focused node',
	'needs a live DOM with real focus and a concurrent remote change; manual key `navigator_a11y`'
);

export const result = suite.result();

if (import.meta.main) finish(result);

import { describe, expect, test } from "bun:test";

import { generateTextCoEditFixture, generateTreeMoveFixture } from "../src/fixture";
import { SplitMix64 } from "../src/prng";

describe("SplitMix64", () => {
  test("same seed produces a byte-identical stream", () => {
    const a = SplitMix64.fromSeedString("v0.3-corpus-seed-1");
    const b = SplitMix64.fromSeedString("v0.3-corpus-seed-1");
    const streamA = Array.from({ length: 50 }, () => a.nextUint64());
    const streamB = Array.from({ length: 50 }, () => b.nextUint64());
    expect(streamA).toEqual(streamB);
  });

  test("different seeds diverge", () => {
    const a = SplitMix64.fromSeedString("seed-a");
    const b = SplitMix64.fromSeedString("seed-b");
    const streamA = Array.from({ length: 20 }, () => a.nextUint64());
    const streamB = Array.from({ length: 20 }, () => b.nextUint64());
    expect(streamA).not.toEqual(streamB);
  });

  test("nextInt stays within bounds across many draws", () => {
    const rng = SplitMix64.fromSeedString("bounds-check");
    for (let i = 0; i < 1000; i += 1) {
      const value = rng.nextInt(7);
      expect(value).toBeGreaterThanOrEqual(0);
      expect(value).toBeLessThan(7);
    }
  });

  test("pick rejects an empty array instead of silently returning undefined", () => {
    const rng = SplitMix64.fromSeedString("empty-pick");
    expect(() => rng.pick([])).toThrow();
  });
});

describe("deterministic fixtures", () => {
  test("same corpus_seed produces an identical text co-edit fixture", () => {
    const first = generateTextCoEditFixture("corpus-seed-42");
    const second = generateTextCoEditFixture("corpus-seed-42");
    expect(first).toEqual(second);
  });

  test("different corpus_seed produces a different text co-edit fixture", () => {
    const first = generateTextCoEditFixture("corpus-seed-42");
    const second = generateTextCoEditFixture("corpus-seed-43");
    expect(first).not.toEqual(second);
  });

  test("same corpus_seed produces an identical tree-move fixture", () => {
    const first = generateTreeMoveFixture("move-seed-7");
    const second = generateTreeMoveFixture("move-seed-7");
    expect(first).toEqual(second);
  });
});

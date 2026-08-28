const MASK64 = (1n << 64n) - 1n;
const GOLDEN_GAMMA = 0x9e3779b97f4a7c15n;

/**
 * SplitMix64 deterministic PRNG. Same corpus_seed -> byte-identical stream on
 * every run, every candidate, every machine. `Math.random()` is banned
 * anywhere in this package -- see v0.3-foundation.md work package 3b.
 */
export class SplitMix64 {
  private state: bigint;

  constructor(seed: bigint) {
    this.state = seed & MASK64;
  }

  /** Derive a 64-bit seed from an arbitrary string via FNV-1a. */
  static fromSeedString(seed: string): SplitMix64 {
    let hash = 0xcbf29ce484222325n;
    const prime = 0x100000001b3n;
    for (let i = 0; i < seed.length; i += 1) {
      hash ^= BigInt(seed.charCodeAt(i));
      hash = (hash * prime) & MASK64;
    }
    return new SplitMix64(hash);
  }

  /** Next raw 64-bit unsigned integer in the stream. */
  nextUint64(): bigint {
    this.state = (this.state + GOLDEN_GAMMA) & MASK64;
    let z = this.state;
    z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & MASK64;
    z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & MASK64;
    z = z ^ (z >> 31n);
    return z & MASK64;
  }

  /** Uniform double in [0, 1) built from the top 53 bits of the stream. */
  nextFloat(): number {
    const bits = this.nextUint64() >> 11n;
    return Number(bits) / 9007199254740992; // 2^53
  }

  /** Uniform integer in [0, maxExclusive). */
  nextInt(maxExclusive: number): number {
    if (!Number.isInteger(maxExclusive) || maxExclusive <= 0) {
      throw new RangeError(`maxExclusive must be a positive integer, got ${maxExclusive}`);
    }
    return Math.floor(this.nextFloat() * maxExclusive);
  }

  /** Uniform integer in [min, max]. */
  nextIntRange(min: number, max: number): number {
    if (!Number.isInteger(min) || !Number.isInteger(max) || max < min) {
      throw new RangeError(`invalid range [${min}, ${max}]`);
    }
    return min + this.nextInt(max - min + 1);
  }

  nextBool(): boolean {
    return this.nextUint64() % 2n === 0n;
  }

  pick<T>(items: readonly T[]): T {
    if (items.length === 0) {
      throw new RangeError("cannot pick from an empty array");
    }
    const value = items[this.nextInt(items.length)];
    if (value === undefined) {
      throw new RangeError("pick index out of bounds");
    }
    return value;
  }

  /**
   * Random string drawn from `alphabet`, length in [minLen, maxLen] Unicode
   * scalar values (code points, not UTF-16 code units). `alphabet` is split
   * with `Array.from` rather than indexed as a raw string so a surrogate
   * pair (e.g. an emoji) is always picked and emitted as one whole
   * character -- naive `alphabet[i]` indexing would sometimes land on a
   * lone high or low surrogate half and emit invalid UTF-16.
   */
  nextString(alphabet: string, minLen: number, maxLen: number): string {
    const codePoints = Array.from(alphabet);
    const length = this.nextIntRange(minLen, maxLen);
    let out = "";
    for (let i = 0; i < length; i += 1) {
      out += this.pick(codePoints);
    }
    return out;
  }
}

/** benchmark-spec.md: 5 warmups, >=30 samples, report median/p95/p99/min/max, never drop outliers, exclude fixture generation from the timed section. */

export type DistributionMetric = Readonly<{
  unit: "ms" | "bytes";
  warmupCount: number;
  sampleCount: number;
  median: number;
  p95: number;
  p99: number;
  min: number;
  max: number;
  rawSamples: readonly number[];
}>;

export type SampleOptions = Readonly<{
  warmupCount?: number;
  sampleCount?: number;
  unit?: "ms" | "bytes";
}>;

function percentile(sortedAscending: readonly number[], fraction: number): number {
  if (sortedAscending.length === 0) {
    throw new RangeError("cannot compute a percentile of an empty sample set");
  }
  const rank = fraction * (sortedAscending.length - 1);
  const lowerIndex = Math.floor(rank);
  const upperIndex = Math.ceil(rank);
  const lower = sortedAscending[lowerIndex];
  const upper = sortedAscending[upperIndex];
  if (lower === undefined || upper === undefined) {
    throw new RangeError("percentile index out of bounds");
  }
  if (lowerIndex === upperIndex) {
    return lower;
  }
  const weight = rank - lowerIndex;
  return lower + (upper - lower) * weight;
}

export function summarizeDistribution(unit: "ms" | "bytes", warmupCount: number, rawSamples: readonly number[]): DistributionMetric {
  if (rawSamples.length < 30) {
    throw new RangeError(`benchmark-spec.md requires >=30 samples, got ${rawSamples.length}`);
  }
  const sorted = [...rawSamples].sort((a, b) => a - b);
  return {
    unit,
    warmupCount,
    sampleCount: rawSamples.length,
    median: percentile(sorted, 0.5),
    p95: percentile(sorted, 0.95),
    p99: percentile(sorted, 0.99),
    min: sorted[0] as number,
    max: sorted[sorted.length - 1] as number,
    rawSamples,
  };
}

/**
 * Runs `fn` `warmupCount` times (discarded) then `sampleCount` times
 * (timed), returning the full distribution. Timing wraps only `fn` itself --
 * callers must exclude fixture generation by building fixtures before
 * calling `sampleAsync`.
 */
export async function sampleAsync(fn: () => void | Promise<void>, options: SampleOptions = {}): Promise<DistributionMetric> {
  const warmupCount = options.warmupCount ?? 5;
  const sampleCount = options.sampleCount ?? 30;
  const unit = options.unit ?? "ms";

  for (let i = 0; i < warmupCount; i += 1) {
    await fn();
  }

  const rawSamples: number[] = [];
  for (let i = 0; i < sampleCount; i += 1) {
    const start = performance.now();
    await fn();
    rawSamples.push(performance.now() - start);
  }

  return summarizeDistribution(unit, warmupCount, rawSamples);
}

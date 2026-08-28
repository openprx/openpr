import { describe, expect, test } from "bun:test";

import { sampleAsync, summarizeDistribution } from "../src/benchmark-runner";

describe("summarizeDistribution", () => {
  test("rejects fewer than 30 samples per benchmark-spec.md", () => {
    expect(() => summarizeDistribution("ms", 5, [1, 2, 3])).toThrow();
  });

  test("does not delete outliers -- max reflects the true maximum", () => {
    const samples = [...Array(29).fill(1), 1000];
    const metric = summarizeDistribution("ms", 5, samples);
    expect(metric.max).toBe(1000);
    expect(metric.sampleCount).toBe(30);
    expect(metric.rawSamples).toEqual(samples);
  });

  test("median/p95/p99 ordering holds", () => {
    const samples = Array.from({ length: 100 }, (_, i) => i + 1);
    const metric = summarizeDistribution("ms", 5, samples);
    expect(metric.min).toBeLessThanOrEqual(metric.median);
    expect(metric.median).toBeLessThanOrEqual(metric.p95);
    expect(metric.p95).toBeLessThanOrEqual(metric.p99);
    expect(metric.p99).toBeLessThanOrEqual(metric.max);
  });
});

describe("sampleAsync", () => {
  test("runs the warmup and sample counts it reports, and times only fn()", async () => {
    let calls = 0;
    const metric = await sampleAsync(
      () => {
        calls += 1;
      },
      { warmupCount: 3, sampleCount: 30 },
    );
    expect(calls).toBe(33);
    expect(metric.warmupCount).toBe(3);
    expect(metric.sampleCount).toBe(30);
    expect(metric.rawSamples.length).toBe(30);
    for (const sample of metric.rawSamples) {
      expect(sample).toBeGreaterThanOrEqual(0);
    }
  });
});

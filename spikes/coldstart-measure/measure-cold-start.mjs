// v0.3 `cold_start_ms_p95_max` (250ms budget, testing/benchmark-spec.md) evidence for both
// candidates, measured with a real headless Chromium (see the delivery report for why this is
// not a Node-side approximation): `--executablePath /usr/bin/chromium`, isolated per-sample
// incognito browser contexts (fresh V8 code-cache + storage partition every sample -- otherwise a
// same-profile repeat visit would let Chromium's on-disk V8 code cache silently turn "cold start"
// into "warm start" from sample 2 onward, even with the HTTP cache disabled), Network domain
// request capture, and a real in-page `performance.mark("flow-ready")` (see both candidates'
// `src/flow-entry.ts`) as the "Flow editor interactive" signal -- not a network-only proxy.
//
// Follows testing/benchmark-spec.md's method: 5 warmup + 30 measured samples, report
// median/p95/p99/min/max, fixed corpus (the same /flow/<id> mount every time, id only varied to
// defeat any URL-keyed cache), and save the result even if a candidate fails the budget.
import { writeFile } from "node:fs/promises";
import puppeteer, { PredefinedNetworkConditions } from "puppeteer-core";

const BUDGET_MS_P95 = 250;
const CHROMIUM_PATH = "/usr/bin/chromium";
const PROFILE_ROOT = "/tmp/claude-1000/-opt-worker/1a6d3dc8-4092-4f3a-a0ea-849ea0c3ea47/scratchpad/chromium-profile-coldstart";

const CANDIDATES = [
  { name: "loro", base: "http://localhost:4310" },
  { name: "yrs-yjs", base: "http://localhost:4311" },
];

// `null` = no throttling (raw loopback -- best case, does not reflect a real gzip-over-WAN
// transfer of the engine bundle; see the delivery report's honesty caveat on this). Sample count
// matches testing/benchmark-spec.md's "5 预热、至少 30 采样" method.
//
// "Fast 4G" is puppeteer-core's built-in `PredefinedNetworkConditions` profile, a more realistic
// mobile-network floor for a first-ever visit. Its sample count is deliberately smaller (still
// >= testing/benchmark-spec.md's absolute floor of 5 warmup, but well under 30 measured) purely
// for wall-clock practicality in this evidence run: the `loro` candidate's ~3.18MB uncompressed
// `.wasm` transfer (see the delivery report's "vite preview does not gzip" finding) takes roughly
// 15+ seconds per sample at Fast-4G's ~1.6Mbps downlink, so 30 samples would cost this candidate
// alone several minutes. Reported honestly as a smaller-n supplementary measurement, not
// mislabeled as meeting the spec's sample-size floor.
const NETWORK_PROFILES = [
  { key: "loopback_no_throttle", conditions: null, warmupCount: 5, sampleCount: 30 },
  { key: "fast_4g", conditions: PredefinedNetworkConditions["Fast 4G"], warmupCount: 3, sampleCount: 8 },
];

function percentile(sortedValues, p) {
  if (sortedValues.length === 0) return 0;
  const idx = Math.min(sortedValues.length - 1, Math.ceil((p / 100) * sortedValues.length) - 1);
  return sortedValues[Math.max(idx, 0)];
}

function summarize(samples, warmupCount) {
  const sorted = [...samples].sort((a, b) => a - b);
  return {
    unit: "ms",
    warmup_count: warmupCount,
    sample_count: sorted.length,
    median: percentile(sorted, 50),
    p95: percentile(sorted, 95),
    p99: percentile(sorted, 99),
    min: sorted[0] ?? 0,
    max: sorted[sorted.length - 1] ?? 0,
    raw_samples: sorted,
  };
}

async function measureOne(browser, base, sampleIndex, conditions) {
  const context = await browser.createBrowserContext();
  try {
    const page = await context.newPage();
    await page.setCacheEnabled(false);
    if (conditions !== null) {
      await page.emulateNetworkConditions(conditions);
    }
    const requests = [];
    page.on("request", (req) => requests.push(req.url()));

    const url = `${base}/flow/coldstart-${Date.now()}-${sampleIndex}`;
    const navStart = Date.now();
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30000 });
    await page.waitForFunction(() => performance.getEntriesByName("flow-ready").length > 0, { timeout: 30000 });
    const wallElapsedMs = Date.now() - navStart;
    const markStartTime = await page.evaluate(() => performance.getEntriesByName("flow-ready")[0].startTime);

    return { wallElapsedMs, markStartTime, requestCount: requests.length };
  } finally {
    await context.close();
  }
}

async function measureCandidate(browser, candidate, profile) {
  const perfMarkSamples = [];
  const wallClockSamples = [];
  const runs = [];
  const total = profile.warmupCount + profile.sampleCount;
  for (let i = 0; i < total; i++) {
    const isWarmup = i < profile.warmupCount;
    const result = await measureOne(browser, candidate.base, i, profile.conditions);
    runs.push({ index: i, warmup: isWarmup, ...result });
    if (!isWarmup) {
      perfMarkSamples.push(result.markStartTime);
      wallClockSamples.push(result.wallElapsedMs);
    }
  }
  return {
    candidate: candidate.name,
    network_profile: profile.key,
    meets_benchmark_spec_sample_floor: profile.warmupCount >= 5 && profile.sampleCount >= 30,
    // The primary metric: time from navigation start to the in-page `flow-ready` performance
    // mark (see the module doc comment). `wall_clock_ms` is a secondary cross-check computed
    // independently in Node with `Date.now()` around the same `page.goto`/`waitForFunction` pair.
    flow_ready_mark_ms: summarize(perfMarkSamples, profile.warmupCount),
    wall_clock_ms: summarize(wallClockSamples, profile.warmupCount),
    budget_ms_p95_max: BUDGET_MS_P95,
    budget_status: percentile([...perfMarkSamples].sort((a, b) => a - b), 95) <= BUDGET_MS_P95 ? "passed" : "failed",
    runs,
  };
}

async function main() {
  const browser = await puppeteer.launch({
    executablePath: CHROMIUM_PATH,
    headless: true,
    userDataDir: PROFILE_ROOT,
    args: ["--no-sandbox", "--disable-dev-shm-usage"],
  });

  const chromiumVersion = await browser.version();
  const results = [];
  try {
    for (const profile of NETWORK_PROFILES) {
      for (const candidate of CANDIDATES) {
        console.error(`measuring ${candidate.name} under ${profile.key}...`);
        const result = await measureCandidate(browser, candidate, profile);
        results.push(result);
        console.error(`  p95 (flow-ready mark) = ${result.flow_ready_mark_ms.p95}ms -> ${result.budget_status}`);
      }
    }
  } finally {
    await browser.close();
  }

  const report = {
    schema_note: "cold_start_ms distribution_metric shape, one object per (candidate, network_profile) pair",
    chromium_version: chromiumVersion,
    budget_ms_p95_max: BUDGET_MS_P95,
    network_profiles: NETWORK_PROFILES.map((p) => ({ key: p.key, warmup_count: p.warmupCount, sample_count: p.sampleCount })),
    results,
  };
  await writeFile(new URL("evidence/cold-start-result.json", import.meta.url), `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
}

await main();

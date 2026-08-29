// Follow-up to measure-cold-start.mjs: same method (fresh incognito browser context per sample,
// same `performance.mark("flow-ready")` signal, same Fast-4G puppeteer-core throttle profile,
// SAME sample counts as the original fast_4g run: 3 warmup + 8 measured), but pointed at
// gzip-static-server.mjs (port 4320 loro / 4321 yrs-yjs) instead of `vite preview` (port
// 4310/4311), which does not compress at all -- see the report's "vite preview does not gzip"
// finding and this follow-up's header verification. This closes the "representative" column the
// coordinator asked for: loopback (optimistic, no network) / throttled+uncompressed (pessimistic,
// original fast_4g run) / throttled+gzip (this run, closest to real production over Caddy/nginx).
import { writeFile } from "node:fs/promises";
import puppeteer, { PredefinedNetworkConditions } from "puppeteer-core";

const BUDGET_MS_P95 = 250;
const CHROMIUM_PATH = "/usr/bin/chromium";
const PROFILE_ROOT = "/tmp/claude-1000/-opt-worker/1a6d3dc8-4092-4f3a-a0ea-849ea0c3ea47/scratchpad/chromium-profile-coldstart-gzip";

const CANDIDATES = [
  { name: "loro", base: "http://localhost:4320" },
  { name: "yrs-yjs", base: "http://localhost:4321" },
];
const WARMUP_COUNT = 3;
const SAMPLE_COUNT = 8;

function percentile(sortedValues, p) {
  if (sortedValues.length === 0) return 0;
  const idx = Math.min(sortedValues.length - 1, Math.ceil((p / 100) * sortedValues.length) - 1);
  return sortedValues[Math.max(idx, 0)];
}

function summarize(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  return {
    unit: "ms",
    warmup_count: WARMUP_COUNT,
    sample_count: sorted.length,
    median: percentile(sorted, 50),
    p95: percentile(sorted, 95),
    p99: percentile(sorted, 99),
    min: sorted[0] ?? 0,
    max: sorted[sorted.length - 1] ?? 0,
    raw_samples: sorted,
  };
}

async function measureOne(browser, base, sampleIndex) {
  const context = await browser.createBrowserContext();
  try {
    const page = await context.newPage();
    await page.setCacheEnabled(false);
    await page.emulateNetworkConditions(PredefinedNetworkConditions["Fast 4G"]);
    // Verify (not assume) every response in this run actually carries Content-Encoding: gzip --
    // fail loudly instead of silently measuring an uncompressed transfer again.
    const uncompressedResponses = [];
    page.on("response", (res) => {
      const req = res.request();
      if (req.resourceType() === "script" || req.resourceType() === "document" || url_ends_with_wasm(req.url())) {
        const enc = res.headers()["content-encoding"];
        if (enc !== "gzip") uncompressedResponses.push({ url: req.url(), contentEncoding: enc ?? null });
      }
    });

    const url = `${base}/flow/coldstart-gzip-${Date.now()}-${sampleIndex}`;
    const navStart = Date.now();
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30000 });
    await page.waitForFunction(() => performance.getEntriesByName("flow-ready").length > 0, { timeout: 30000 });
    const wallElapsedMs = Date.now() - navStart;
    const markStartTime = await page.evaluate(() => performance.getEntriesByName("flow-ready")[0].startTime);

    return { wallElapsedMs, markStartTime, uncompressedResponses };
  } finally {
    await context.close();
  }
}

function url_ends_with_wasm(u) {
  return u.endsWith(".wasm");
}

async function measureCandidate(browser, candidate) {
  const perfMarkSamples = [];
  const wallClockSamples = [];
  const runs = [];
  const allUncompressed = [];
  const total = WARMUP_COUNT + SAMPLE_COUNT;
  for (let i = 0; i < total; i++) {
    const isWarmup = i < WARMUP_COUNT;
    const result = await measureOne(browser, candidate.base, i);
    runs.push({ index: i, warmup: isWarmup, wallElapsedMs: result.wallElapsedMs, markStartTime: result.markStartTime });
    allUncompressed.push(...result.uncompressedResponses);
    if (!isWarmup) {
      perfMarkSamples.push(result.markStartTime);
      wallClockSamples.push(result.wallElapsedMs);
    }
  }
  return {
    candidate: candidate.name,
    network_profile: "fast_4g_gzip",
    gzip_header_verified: allUncompressed.length === 0,
    uncompressed_responses_seen: allUncompressed,
    flow_ready_mark_ms: summarize(perfMarkSamples),
    wall_clock_ms: summarize(wallClockSamples),
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
    for (const candidate of CANDIDATES) {
      console.error(`measuring ${candidate.name} under fast_4g_gzip...`);
      const result = await measureCandidate(browser, candidate);
      results.push(result);
      console.error(`  p95 (flow-ready mark) = ${result.flow_ready_mark_ms.p95}ms -> ${result.budget_status}, gzip_header_verified=${result.gzip_header_verified}`);
    }
  } finally {
    await browser.close();
  }

  const report = {
    schema_note: "Follow-up to cold-start-result.json: same method, Fast-4G throttle, but served through gzip-static-server.mjs (real Content-Encoding: gzip verified per-response) instead of vite preview (uncompressed).",
    chromium_version: chromiumVersion,
    budget_ms_p95_max: BUDGET_MS_P95,
    warmup_count: WARMUP_COUNT,
    sample_count: SAMPLE_COUNT,
    meets_benchmark_spec_sample_floor: WARMUP_COUNT >= 5 && SAMPLE_COUNT >= 30,
    results,
  };
  await writeFile(new URL("evidence/cold-start-gzip-result.json", import.meta.url), `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
}

await main();

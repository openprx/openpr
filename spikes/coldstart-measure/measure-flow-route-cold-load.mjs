// `flow_route_cold_load_ms_p95` per ADR-0015 (current revision R7 -- APPROVED_FOR_FORMAL_RUN,
// see evidence/v2/PREREGISTRATION-SIGNATURE.md) (/opt/working/sylvode-flow/decisions/
// ADR-0015-cold-start-measurement-split.md) section 3.4. ADR is still `Proposed`: this evidence
// is protocol-conformant but MUST NOT be used to decide any hard gate until the ADR is Accepted.
// Predregistration history: rejected at R4 and R5 (`NOT_SIGNED_HOLD`), signed at R7
// (2026-08-29T13:41:56Z, independent reviewer who did not write this runner) -- see
// COLDSTART_SAMPLE_OVERRIDE/COLDSTART_WARMUP_OVERRIDE in lib/common.mjs for how a smoke run is
// distinguished from this formal one.
//
// Method, matched to 3.4:
//  - Timing: `Page.navigate` start (performance.timeOrigin of the timed navigation's own document)
//    to the ADR-0015 3.2 ready mark ('sylvode-ready', all 5 postconditions, including mount+rAF).
//  - Fresh browser context per sample + explicit clearing of every listed cache type + a
//    pre-navigation visit to assert the origin is actually empty (see runOneSample).
//  - `sylvode-net-mobile-1` applied via raw CDP `Network.emulateNetworkConditionsByRule`.
//  - Origin: gzip-static-server.mjs (HTTP/1.1, loopback, no TLS, gzip level 9 for compressible
//    files -- R6 added a fixed-path calibration asset + conditional identity encoding, see that
//    file's own header comment; the real candidates' dist/ assets are unaffected, still always
//    gzip since they all compress).
//  - Full HAR saved per sample; Content-Encoding asserted per response for the resource set.
//
// R6 corrections in THIS file (coordinator directive 2026-08-29, SECOND predregistration
// rejection -- both fixes are entirely within the throughput-calibration mechanism; the reviewer
// explicitly CONFIRMED vendor-exclusion, pair-alternation, and the har.mjs encodedDataLength fix
// as already correct, so those are unchanged from the prior revision):
//  - calibration resource: was "largest file in each candidate's own dist/" (asymmetric across
//    candidates -- ~1MB for loro vs ~57KB for yrs-yjs, so the same acceptance rule could pass one
//    and fail the other for reasons unrelated to actual network conditions). Now BOTH candidates
//    calibrate against the SAME fixed, coordinator-generated `probe/calibration-256k.bin` (262144
//    bytes, incompressible, served identity) -- see `loadCalibrationAsset()` in lib/common.mjs.
//  - calibration formula: was `encodedDataLength / (elapsed - assumedLatencyConstant)`, which the
//    coordinator identified as a mis-frozen formula (their words: "这是我冻错了，不是你实现错") --
//    it could reject a small, correctly-throttled transfer purely because the constant-subtraction
//    assumption doesn't hold precisely. Now `encodedDataLength / (loadingFinished_ts -
//    firstByte_ts)`, where firstByte_ts is CDP `Network.responseReceived`'s own timestamp -- see
//    `runThroughputCalibration` below. NO fallback to the old formula if firstByte_ts is missing;
//    that case fails the run closed.
//  - independent byte count: now a REQUIRED exact-equality fail-closed condition (`byteCountOk`),
//    not just a value recorded next to the throughput check.
//  - item 3 (probe pre-registration, unchanged from prior revision): reads
//    `routeProbeExpectedDigest()` from the frozen `probe/coldstart-probe-v1.json`.
//  - item 4 (pair-level alternation, CONFIRMED correct by the reviewer, unchanged): candidates run
//    per-sample-pair interleaved (AB, BA, AB, BA, ...) via `buildAlternatingPairSchedule`.
//  - Per coordinator's closing note on this metric's probe: `flow_route_cold_load`'s DOM
//    read/write probe does NOT independently cross-check CRDT state (LoroDoc/Y.Doc) -- R6 accepts
//    this as this metric's frozen scope (it is not required to), but this file and its evidence
//    must not describe the probe as having verified engine/CRDT state, only DOM state -- see
//    `read_write_probe.description` in the report object below.
import { spawn } from "node:child_process";
import { createConnection } from "node:net";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";
import {
  MEASUREMENT_PROTOCOL_ID,
  CHROMIUM_PATH,
  WARMUP_COUNT,
  SAMPLE_COUNT,
  NETWORK_PROFILE_MOBILE_1,
  cdpNetworkConditionsFor,
  FLOW_ROUTE_PATH,
  FIXTURE_TEXT,
  PROBE_TEXT,
  FROZEN_PROBE_SPEC_SHA256,
  routeProbeExpectedDigest,
  nearestRankP95,
  round3,
  sha256HexOfFile,
  sha256HexOfDir,
  sha256HexOfBuffer,
  hashRunnerFiles,
  gitSourceCommit,
  packageLockfileHash,
  toolchainVersionsFor,
  NETWORK_CDP_METHOD,
  buildAlternatingPairSchedule,
  loadCalibrationAsset,
  verifyPreregistrationHashes,
  measureRawHttpResponseBytes,
  THROUGHPUT_CALIBRATION_TOLERANCE_RATIO,
  THROUGHPUT_CALIBRATION_LOWER_BPS,
  THROUGHPUT_CALIBRATION_UPPER_BPS,
} from "./lib/common.mjs";
import { buildHarnessSource } from "./lib/browser-harness-src.mjs";
import { HarRecorder } from "./lib/har.mjs";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const REPO_ROOT = join(HERE, "..", "..");
const RUN_ID = `flow-route-cold-load-${new Date().toISOString().replace(/[:.]/g, "-")}`;
const EVIDENCE_DIR = join(HERE, "evidence", "v2");

const IS_SMOKE = process.env.COLDSTART_WARMUP_OVERRIDE !== undefined || process.env.COLDSTART_SAMPLE_OVERRIDE !== undefined;
const OUT_DIR = IS_SMOKE ? join(HERE, "smoke", "r7") : EVIDENCE_DIR;
const HAR_DIR = join(OUT_DIR, "har");
mkdirSync(HAR_DIR, { recursive: true });

const SERVER_SCRIPT = join(HERE, "gzip-static-server.mjs");
const CANDIDATES = [
  {
    name: "loro",
    rootDir: join(REPO_ROOT, "spikes", "bundle-loro"),
    distDir: join(REPO_ROOT, "spikes", "bundle-loro", "dist"),
    port: 4420,
    hasWasm: true,
  },
  {
    name: "yrs-yjs",
    rootDir: join(REPO_ROOT, "spikes", "bundle-yrs-yjs"),
    distDir: join(REPO_ROOT, "spikes", "bundle-yrs-yjs", "dist"),
    port: 4421,
    hasWasm: false,
  },
];

// Formal-run integrity fix (this implementation round): a real incident during this delivery's
// own formal run showed the OLD readiness detection here (matching any stdout line containing
// "serving") could be fooled by a STALE process from an earlier, improperly-cleaned-up run still
// holding the port -- see gzip-static-server.mjs's fix comment for the full mechanism (this
// script used to log a "serving..." line BEFORE attempting to bind, so a subsequent EADDRINUSE
// crash went undetected and requests silently landed on the stale process instead). Two
// independent fixes, both required:
//  (1) `assertPortFree` -- a pre-flight bind-and-release probe BEFORE spawning at all, so a
//      leftover process occupying the port is caught immediately with an unambiguous error,
//      not discovered later via mysteriously-successful-looking requests.
//  (2) The readiness match below is now the EXACT string the fixed server prints ONLY after a
//      real, successful `Bun.serve()` return (`"serving on :"`), not a loose substring that could
//      match pre-bind log text.
function assertPortFree(port) {
  return new Promise((resolve, reject) => {
    const probe = createConnection({ host: "127.0.0.1", port }, () => {
      probe.destroy();
      reject(new Error(`port ${port} is already in use (something is listening -- refusing to start a formal run against a possibly-stale server; kill it and retry)`));
    });
    probe.on("error", (err) => {
      // ECONNREFUSED means nothing is listening -- exactly what we want before starting.
      if (err && err.code === "ECONNREFUSED") resolve();
      else reject(err);
    });
  });
}

async function startServer(distDir, port) {
  await assertPortFree(port);
  return new Promise((resolve, reject) => {
    const proc = spawn("bun", [SERVER_SCRIPT, distDir, String(port)], { stdio: ["ignore", "pipe", "pipe"] });
    let ready = false;
    const onData = (d) => {
      const s = d.toString();
      if (!ready && s.includes("serving on :")) {
        ready = true;
        resolve(proc);
      }
    };
    proc.stdout.on("data", onData);
    proc.stderr.on("data", (d) => process.stderr.write(`[server:${port}] ${d}`));
    proc.on("error", reject);
    proc.on("exit", (code) => {
      if (!ready) reject(new Error(`server on port ${port} exited before ready, code=${code}`));
    });
    setTimeout(() => {
      if (!ready) reject(new Error(`server on port ${port} did not become ready in time`));
    }, 8000);
  });
}

async function clearAndAssertEmpty(page) {
  return page.evaluate(async () => {
    const out = { serviceWorker: null, cacheStorage: null, indexedDb: null };
    try {
      const regs = await navigator.serviceWorker.getRegistrations();
      for (const r of regs) await r.unregister();
      out.serviceWorker = {
        registrationsFoundBeforeClear: regs.length,
        controllerAfterClear: navigator.serviceWorker.controller,
      };
    } catch (e) {
      out.serviceWorker = { error: String(e) };
    }
    try {
      const keys = await caches.keys();
      for (const k of keys) await caches.delete(k);
      out.cacheStorage = { keysFoundBeforeClear: keys.length, keysAfterClear: (await caches.keys()).length };
    } catch (e) {
      out.cacheStorage = { error: String(e) };
    }
    try {
      const dbs = (await indexedDB.databases?.()) ?? [];
      for (const db of dbs) if (db.name) indexedDB.deleteDatabase(db.name);
      out.indexedDb = { databasesFoundBeforeClear: dbs.length };
    } catch (e) {
      out.indexedDb = { error: String(e) };
    }
    return out;
  });
}

// ADR-0015 3.4 "回读" requirement, mechanism 1 (see lib/common.mjs's header comment for mechanism
// 2, throughput calibration). Applies the network conditions AND captures the CDP-returned rule
// id, then listens for `Network.requestWillBeSentExtraInfo` to record, per request,
// `appliedNetworkConditionsId` -- proof CDP actually attributed that rule to real requests, not
// just that we called the API. Returns `{ ruleId, getAppliedIds }` where `getAppliedIds()` snapshots
// everything observed so far (requestId -> appliedNetworkConditionsId | undefined).
async function applyNetworkConditionsWithRuleId(client, netConditionsSent) {
  const appliedByRequestId = new Map();
  client.on("Network.requestWillBeSentExtraInfo", (e) => {
    if (e.appliedNetworkConditionsId !== undefined) {
      appliedByRequestId.set(e.requestId, e.appliedNetworkConditionsId);
    }
  });
  const { ruleIds } = await client.send(NETWORK_CDP_METHOD, netConditionsSent);
  if (!Array.isArray(ruleIds) || ruleIds.length !== 1) {
    throw new Error(`applyNetworkConditionsWithRuleId: expected exactly 1 ruleId, got ${JSON.stringify(ruleIds)}`);
  }
  return {
    ruleId: ruleIds[0],
    getAppliedIds: () => Object.fromEntries(appliedByRequestId),
  };
}

// ADR-0015 R6 "回读" requirement, mechanism 2: isolated known-size download + effective-bytes-per-
// second computation, independent of mechanism 1 above (mechanism 1 only proves CDP accepted and
// attributed a rule; this proves the numeric throughput it actually enforced is in the right
// ballpark). Runs on the SAME page, BEFORE the real timed navigation's HAR recorder attaches, so
// this one transfer has no concurrent requests contending for the emulated pipe's bandwidth.
//
// R6 rewrite (coordinator directive 2026-08-29, second predregistration rejection): both the
// resource (now the fixed, coordinator-generated `probe/calibration-256k.bin`, byte-identical for
// both candidates -- see lib/common.mjs's CALIBRATION_ASSET_* comment) and the formula changed.
// The formula NO LONGER subtracts an assumed latency constant from elapsed time; it uses
// `Network.responseReceived`'s own timestamp ("firstByte_ts") as the window start instead, and
// there is NO fallback if that timestamp is missing -- a missing firstByte_ts fails the run
// closed rather than falling back to the old (rejected) constant-subtraction formula.
//
// EMPIRICAL FINDING affecting item 3 (independent byte count must really participate in
// fail-closed): CDP `Network.loadingFinished.encodedDataLength` was found, empirically, to count
// the FULL HTTP/1.1 response (status line + headers + body), not body alone -- confirmed via
// `curl -w '%{size_header}'` against this exact server/asset: a 173-byte header block, so
// encodedDataLength = 262144 (body) + 173 (headers) = 262317, reproducibly. Comparing
// encodedDataLength against calibrationAsset.sizeBytes (the file's raw body size on disk) can
// therefore NEVER succeed -- the two quantities measure different things by construction. So the
// "known bytes" ground truth here is NOT the static file size; it is the ACTUAL total wire byte
// count of a real HTTP/1.1 response to the SAME URL, measured via a raw TCP socket in this Node
// process (`measureRawHttpResponseBytes`, lib/common.mjs) -- a genuinely independent second
// channel (no CDP, no Puppeteer), not a hardcoded "+173" fudge factor that would silently drift if
// gzip-static-server.mjs's header set ever changes. See the delivery report for this finding
// surfaced to the coordinator; this interpretation was adopted because the literal instruction
// (compare against the file's size) is mechanically impossible to satisfy given real CDP behavior.
async function runThroughputCalibration({ client, page, origin, calibrationAsset }) {
  const calibRecorder = new HarRecorder(new Date().toISOString());
  calibRecorder.attach(client);
  const calibrationUrl = `${origin}${calibrationAsset.urlPath}`;
  const [fetchResult, rawWireBytesResult] = await Promise.all([
    page.evaluate(async (url) => {
      try {
        const res = await fetch(url, { cache: "no-store" });
        await res.arrayBuffer(); // consume fully so loadingFinished fires promptly
        return { ok: res.ok, status: res.status };
      } catch (e) {
        return { ok: false, error: String(e) };
      }
    }, calibrationUrl),
    measureRawHttpResponseBytes(calibrationUrl).then(
      (bytes) => ({ ok: true, bytes }),
      (err) => ({ ok: false, error: String(err) }),
    ),
  ]);
  // Give the CDP session's event queue a moment to deliver Network.loadingFinished for the
  // calibration request before we read it back.
  await new Promise((r) => setTimeout(r, 100));
  const entry = [...calibRecorder.entriesByRequestId.values()].find((e) => e.url === calibrationUrl);
  const base = {
    calibration_url: calibrationUrl,
    calibration_asset_sha256_expected: calibrationAsset.sha256,
    calibration_asset_body_bytes: calibrationAsset.sizeBytes,
    known_wire_bytes_independently_measured: rawWireBytesResult.ok ? rawWireBytesResult.bytes : null,
    known_wire_bytes_measurement_error: rawWireBytesResult.ok ? null : rawWireBytesResult.error,
  };
  if (!fetchResult.ok || entry === undefined || entry.encodedDataLength === undefined || entry.finishedTimestamp === undefined) {
    return { ok: false, ...base, reason: "calibration fetch or CDP entry incomplete", fetchResult, entryFound: entry !== undefined };
  }
  if (!rawWireBytesResult.ok) {
    return { ok: false, ...base, reason: "independent raw-socket byte measurement failed -- fail-closed, no byte-count check possible" };
  }
  // R6: no fallback. A missing firstByte_ts (Network.responseReceived never fired / was never
  // recorded for this request) fails the run closed -- it does NOT fall back to subtracting an
  // assumed latency constant (that fallback is exactly what got the R5 formula rejected).
  if (entry.responseReceivedTimestamp === undefined) {
    return { ok: false, ...base, reason: "firstByte_ts (Network.responseReceived timestamp) missing -- no fallback per ADR-0015 R6", entryFound: true };
  }
  const firstByteTs = entry.responseReceivedTimestamp;
  const transferSec = entry.finishedTimestamp - firstByteTs;
  const effectiveBps = transferSec > 0 ? entry.encodedDataLength / transferSec : Infinity;
  const throughputOk = effectiveBps >= THROUGHPUT_CALIBRATION_LOWER_BPS && effectiveBps <= THROUGHPUT_CALIBRATION_UPPER_BPS;
  // R6 item 3: the independently-measured wire byte count is a SEPARATE, REQUIRED fail-closed
  // condition -- must equal CDP's own reported encodedDataLength EXACTLY, not just "be close".
  const byteCountOk = entry.encodedDataLength === rawWireBytesResult.bytes;
  const ok = throughputOk && byteCountOk;
  return {
    ok,
    ...base,
    cdp_encoded_data_length: entry.encodedDataLength,
    byte_count_exact_match: byteCountOk,
    first_byte_ts: round3(firstByteTs * 1000) / 1000,
    loading_finished_ts: round3(entry.finishedTimestamp * 1000) / 1000,
    transfer_sec_first_byte_to_finish: round3(transferSec * 1000) / 1000,
    effective_throughput_bps: Number.isFinite(effectiveBps) ? Math.round(effectiveBps) : effectiveBps,
    throughput_ok: throughputOk,
    target_throughput_bps: NETWORK_PROFILE_MOBILE_1.downloadThroughputBitsPerSec / 8,
    tolerance_ratio: THROUGHPUT_CALIBRATION_TOLERANCE_RATIO,
    lower_bound_bps: THROUGHPUT_CALIBRATION_LOWER_BPS,
    upper_bound_bps: THROUGHPUT_CALIBRATION_UPPER_BPS,
    formula: "effective_bps = cdp_encoded_data_length / (loading_finished_ts - first_byte_ts); first_byte_ts = CDP Network.responseReceived's own timestamp (NOT requestWillBeSent -- this structurally excludes DNS/connect/TTFB/latency without assuming any constant value for them); loading_finished_ts / cdp_encoded_data_length from CDP Network.loadingFinished (authoritative final on-the-wire byte count, HTTP headers included). No fallback if first_byte_ts is missing -- run invalid instead. byte_count_exact_match requires cdp_encoded_data_length === known_wire_bytes_independently_measured exactly, where the latter is a raw-TCP-socket measurement of the SAME URL's full HTTP/1.1 response (headers+body) done outside CDP/Puppeteer entirely -- NOT a comparison to the calibration file's on-disk body size, which cannot equal encodedDataLength by construction (HTTP header overhead) -- see this function's header comment for the empirical finding.",
  };
}

async function runOneSample({ browser, candidate, expectedFullHex, timeoutMs, calibrationAsset }) {
  const context = await browser.createBrowserContext();
  try {
    const page = await context.newPage();
    await page.setViewport({ width: 1280, height: 800 });
    // Headless Chromium auto-requests /favicon.ico for every navigation regardless of any <link
    // rel="icon">; under sylvode-net-mobile-1's 150ms latency that is a real, uncounted-for-in-
    // the-protocol network round trip contaminating every sample (confirmed via HAR: it was
    // present in every capture during this runner's smoke test). Not part of "该路由加载的全部
    // HTML/JS/WASM" (favicon is none of those), so it is blocked outright rather than measured.
    await page.setRequestInterception(true);
    page.on("request", (req) => {
      if (req.url().endsWith("/favicon.ico")) {
        req.abort("blockedbyclient");
      } else {
        req.continue();
      }
    });
    const client = await page.createCDPSession();
    await client.send("Network.enable");
    await client.send("Network.setCacheDisabled", { cacheDisabled: true });
    await client.send("Network.clearBrowserCache");
    await client.send("Network.clearBrowserCookies");
    const origin = `http://127.0.0.1:${candidate.port}`;
    await client.send("Storage.clearDataForOrigin", { origin, storageTypes: "all" });
    const netConditionsSent = cdpNetworkConditionsFor(NETWORK_PROFILE_MOBILE_1);
    const ruleAttribution = await applyNetworkConditionsWithRuleId(client, netConditionsSent);
    await page.evaluateOnNewDocument(candidate.harnessSrc);

    // ADR-0015 3.4 cache-clearing assertions: a fresh context has never visited `origin`, so this
    // pre-navigation visit + clear both proves that (registrations/keys/databases found == 0) and
    // exercises the real unregister()/delete() paths so `controller === null` is an assertion
    // about a real API call, not an assumption. `Network.setCacheDisabled` stays on afterwards so
    // this untimed visit cannot warm the HTTP cache for the timed navigation that follows.
    await page.goto(`${origin}/`, { waitUntil: "networkidle0", timeout: timeoutMs });
    const cacheAssertion = await clearAndAssertEmpty(page);

    // Throughput calibration (mechanism 2) -- runs on this same, still-untimed page, before the
    // real HAR recorder attaches, so it doesn't contend with or pollute the timed navigation.
    const throughputCalibration = await runThroughputCalibration({ client, page, origin, calibrationAsset });

    const har = new HarRecorder(new Date().toISOString());
    har.attach(client);
    const appliedConditionsSeenDuringNav = new Set();
    const navRequestWillBeSentExtraInfoListener = (e) => {
      if (e.appliedNetworkConditionsId !== undefined) appliedConditionsSeenDuringNav.add(e.appliedNetworkConditionsId);
    };
    client.on("Network.requestWillBeSentExtraInfo", navRequestWillBeSentExtraInfoListener);

    const navResult = await (async () => {
      await page.goto(`${origin}${FLOW_ROUTE_PATH}`, { waitUntil: "domcontentloaded", timeout: timeoutMs });

      // NOTE: 'domcontentloaded' fires as soon as index.html's own (non-dynamic) module script
      // finishes its SYNCHRONOUS portion -- main.ts's `import("./flow-entry")` is a fire-and-forget
      // dynamic import it does not await, so DOMContentLoaded does NOT wait for the engine chunk,
      // the (possibly multi-second, throttled) internal wasm XHR, compile, or instantiate. The
      // WASM postcondition must therefore be read AFTER waiting for the mount signal below, not
      // before -- reading it here would race the throttled network fetch and always see an empty
      // trace for a WASM candidate (caught during this runner's own smoke test).
      const el = await page.waitForSelector(".ProseMirror", { timeout: timeoutMs });
      if (el === null) throw new Error("no .ProseMirror element appeared");

      const wasmCheck = await page.evaluate((expectHasWasm) => {
        const n = window.__coldstart.trace.length;
        return { n, ok: expectHasWasm ? n > 0 : n === 0, trace: window.__coldstart.trace };
      }, candidate.hasWasm);

      await page.focus(".ProseMirror");
      // Two sequential writes into the same block, not two blocks -- see lib/common.mjs's
      // header comment (probe/coldstart-probe-v1.json's flow_route_cold_load_probe section):
      // neither candidate wires a keymap plugin, so there is no Enter-splits-paragraph command to
      // invoke (confirmed empirically, not assumed).
      await page.keyboard.sendCharacter(FIXTURE_TEXT);
      await page.keyboard.sendCharacter(PROBE_TEXT);

      const flowReadyMarkCount = await page.evaluate(() => window.__coldstart.checkFlowReadyMarkCount());
      // postcondition 5 (this metric ONLY -- see measure-engine-runtime-ready.mjs's header for
      // why that metric omits it): at least one rAF after the last input before accepting.
      await page.evaluate(() => window.__coldstart.waitOneRaf());
      const probe = await page.evaluate((hex) => window.__coldstart.readBackAndHashBlocks(hex), expectedFullHex);
      const markResult = await page.evaluate((name) => window.__coldstart.markOnce(name), "sylvode-ready");
      const readyEntry = await page.evaluate(() => performance.getEntriesByName("sylvode-ready")[0]?.startTime ?? null);

      const ok = flowReadyMarkCount === 1 && wasmCheck.ok && probe.ok === true && markResult.ok === true && readyEntry !== null;
      return { ok, flowReadyMarkCount, wasmCheck, probe, markResult, elapsedMs: readyEntry };
    })();

    client.off("Network.requestWillBeSentExtraInfo", navRequestWillBeSentExtraInfoListener);

    const harJson = har.toHar();
    const encodingAssertions = har.encodingAssertions;
    const encodingAllOk = encodingAssertions.every((a) => a.ok);

    const appliedConditionsList = [...appliedConditionsSeenDuringNav];
    const ruleAttributionOk = appliedConditionsList.length > 0 && appliedConditionsList.every((id) => id === ruleAttribution.ruleId);

    return {
      ok: navResult.ok && encodingAllOk && throughputCalibration.ok && ruleAttributionOk,
      elapsedMs: navResult.elapsedMs,
      reasons: {
        flowReadyMarkCount: navResult.flowReadyMarkCount,
        wasmCheck: navResult.wasmCheck,
        probe: navResult.probe,
        markResult: navResult.markResult,
        encodingAllOk,
        encodingAssertions,
        throughputCalibration,
        networkRuleAttribution: {
          rule_id: ruleAttribution.ruleId,
          applied_conditions_ids_seen_during_navigation: appliedConditionsList,
          ok: ruleAttributionOk,
        },
      },
      cacheAssertion,
      harJson,
      networkConditionsSent: netConditionsSent,
    };
  } catch (err) {
    return { ok: false, elapsedMs: null, reasons: { error: String(err && err.stack ? err.stack : err) }, cacheAssertion: null, harJson: null };
  } finally {
    await context.close();
  }
}

async function main() {
  // Predregistration gate, checked FIRST, before anything else -- see
  // measure-engine-runtime-ready.mjs's identical gate for the full rationale.
  let preregistrationHashCheck = null;
  if (!process.env.COLDSTART_WARMUP_OVERRIDE && !process.env.COLDSTART_SAMPLE_OVERRIDE) {
    const sigCheck = verifyPreregistrationHashes();
    preregistrationHashCheck = sigCheck;
    if (!sigCheck.ok) {
      process.stderr.write(`[flow-route-cold-load] ABORT: preregistration hash mismatch, formal run not authorized:\n${JSON.stringify(sigCheck, null, 2)}\n`);
      process.exitCode = 1;
      return;
    }
    process.stderr.write("[flow-route-cold-load] preregistration hashes verified OK (ADR-0015 R7 / coldstart-probe-v1.json / calibration-256k.bin all match signed values)\n");
  }

  const expected = routeProbeExpectedDigest();
  const servers = [];
  for (const candidate of CANDIDATES) {
    process.stderr.write(`starting gzip-static-server for ${candidate.name} on :${candidate.port}...\n`);
    const proc = await startServer(candidate.distDir, candidate.port);
    servers.push(proc);
    candidate.packageLockfileHash = packageLockfileHash(candidate.rootDir);
    candidate.toolchainVersions = toolchainVersionsFor(candidate.rootDir);
    candidate.harnessSrc = buildHarnessSource({ wasmSyntheticAsset: null });
  }

  // R6: ONE shared calibration asset for both candidates (not per-candidate selection -- see
  // lib/common.mjs's CALIBRATION_ASSET_* comment for why the old per-candidate approach was
  // rejected). Verified against its frozen SHA-256/size before use (throws if it doesn't match).
  const calibrationAsset = loadCalibrationAsset();
  process.stderr.write(
    `  shared calibration asset: ${calibrationAsset.urlPath} (${calibrationAsset.sizeBytes}B, sha256=${calibrationAsset.sha256})\n`,
  );

  const browser = await puppeteer.launch({
    executablePath: CHROMIUM_PATH,
    headless: true,
    args: [
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--disable-features=NetworkPrediction,PreconnectToSearch",
      "--dns-prefetch-disable",
      "--disable-background-networking",
    ],
  });

  const byName = Object.fromEntries(CANDIDATES.map((c) => [c.name, c]));
  const runsByCandidate = Object.fromEntries(CANDIDATES.map((c) => [c.name, []]));
  try {
    const chromiumVersion = await browser.version();
    const schedule = buildAlternatingPairSchedule(
      CANDIDATES.map((c) => c.name),
      WARMUP_COUNT,
      SAMPLE_COUNT,
    );
    for (const entry of schedule) {
      const candidate = byName[entry.candidate];
      process.stderr.write(
        `[flow-route-cold-load] pair=${entry.pairIndex} pos=${entry.positionInPair} ${entry.warmup ? "warmup" : "sample"} global=${entry.globalIndex} ${candidate.name}...\n`,
      );
      const startedAtIso = new Date().toISOString();
      const result = await runOneSample({
        browser,
        candidate,
        expectedFullHex: expected.fullHex,
        timeoutMs: 30000,
        calibrationAsset,
      });
      const finishedAtIso = new Date().toISOString();
      let harPath = null;
      let harChecksum = null;
      if (result.harJson !== null) {
        const fname = `${candidate.name}-${String(entry.globalIndex).padStart(3, "0")}.har.json`;
        const full = join(HAR_DIR, fname);
        const text = `${JSON.stringify(result.harJson, null, 2)}\n`;
        writeFileSync(full, text);
        harChecksum = sha256HexOfBuffer(Buffer.from(text));
        harPath = join(IS_SMOKE ? "smoke/r7" : "evidence/v2", "har", fname);
      }
      const { harJson, ...resultWithoutHar } = result;
      runsByCandidate[candidate.name].push({
        global_index: entry.globalIndex,
        pair_index: entry.pairIndex,
        position_in_pair: entry.positionInPair,
        warmup: entry.warmup,
        started_at: startedAtIso,
        finished_at: finishedAtIso,
        ...resultWithoutHar,
        har_path: harPath,
        har_sha256: harChecksum,
      });
      process.stderr.write(
        `  -> ok=${result.ok} elapsedMs=${result.elapsedMs === null || result.elapsedMs === undefined ? "n/a" : round3(result.elapsedMs)}\n`,
      );
    }

    const results = [];
    for (const candidate of CANDIDATES) {
      const runs = runsByCandidate[candidate.name];
      const measured = runs.filter((r) => !r.warmup);
      const successful = measured.filter((r) => r.ok && typeof r.elapsedMs === "number");
      const failed = measured.filter((r) => !(r.ok && typeof r.elapsedMs === "number"));
      const values = successful.map((r) => round3(r.elapsedMs));
      const sorted = [...values].sort((a, b) => a - b);
      const p95 = nearestRankP95(sorted);
      const allEncodingAssertions = measured.flatMap((r) => (r.reasons && r.reasons.encodingAssertions) || []);
      const allCalibrations = measured.map((r) => r.reasons && r.reasons.throughputCalibration).filter(Boolean);
      const calibrationAllOk = allCalibrations.every((c) => c.ok);
      // Coordinator correction item 4: any failed measured sample invalidates the whole run.
      // Coordinator directive 2026-08-29 item 1: a throughput-calibration failure on ANY measured
      // sample also invalidates the whole run (same fail-closed treatment).
      const runValid = failed.length === 0 && successful.length === SAMPLE_COUNT && calibrationAllOk;
      results.push({
        candidate: candidate.name,
        has_wasm: candidate.hasWasm,
        built_artifact_hash: sha256HexOfDir(candidate.distDir),
        package_lockfile_hash: candidate.packageLockfileHash,
        toolchain_versions: candidate.toolchainVersions,
        warmup_count: WARMUP_COUNT,
        sample_count_attempted: SAMPLE_COUNT,
        sample_count_successful: successful.length,
        failed_sample_count: failed.length,
        failed_samples: failed.map((r) => ({ global_index: r.global_index, reasons: r.reasons })),
        run_valid: runValid,
        raw_samples_ms: sorted,
        p95_ms: p95 === null ? null : round3(p95),
        budget_ms: 2500,
        budget_status: runValid && p95 !== null ? (p95 <= 2500 ? "passed" : "failed") : "invalid",
        content_encoding_assertions_sample_count: allEncodingAssertions.length,
        content_encoding_assertions_all_ok: allEncodingAssertions.every((a) => a.ok),
        content_encoding_exceptions: allEncodingAssertions.filter((a) => a.exempt),
        content_encoding_failures: allEncodingAssertions.filter((a) => !a.ok),
        throughput_calibration_all_ok: calibrationAllOk,
        throughput_calibration_by_sample: measured.map((r) => ({ global_index: r.global_index, calibration: r.reasons && r.reasons.throughputCalibration })),
        network_rule_attribution_by_sample: measured.map((r) => ({ global_index: r.global_index, attribution: r.reasons && r.reasons.networkRuleAttribution })),
        cache_clear_assertions_sample: measured.map((r) => ({ global_index: r.global_index, cacheAssertion: r.cacheAssertion })),
        runs,
      });
    }

    const toolchainsEqual = CANDIDATES.every(
      (c) => JSON.stringify(c.toolchainVersions) === JSON.stringify(CANDIDATES[0].toolchainVersions),
    );

    const report = {
      measurement_protocol_id: MEASUREMENT_PROTOCOL_ID,
      protocol_status_note:
        "ADR-0015 is status=Proposed as of this run (revision R7). This evidence is protocol-conformant but MUST NOT be used to decide any hard gate until the ADR is Accepted.",
      run_kind: IS_SMOKE ? "smoke" : "formal_candidate",
      smoke_note: IS_SMOKE
        ? "SMOKE RUN: COLDSTART_WARMUP_OVERRIDE/COLDSTART_SAMPLE_OVERRIDE were set. This is NOT protocol-conformant evidence (warmup/sample counts do not match ADR-0015 3.1's 5/>=30) and MUST NOT be treated as a formal run under any circumstance, signed or not. Written to smoke/, not evidence/v2/."
        : null,
      preregistration_signature: {
        status: "APPROVED_FOR_FORMAL_RUN",
        record_path: "evidence/v2/PREREGISTRATION-SIGNATURE.md",
        reviewer: "OpenAI Codex (independent reviewer, did not write this runner)",
        signed_at: "2026-08-29T13:41:56Z",
        bound_hashes: {
          adr_0015_r7_sha256: "7b6725f4f09fc5804f3e7cfede99c4233cc9825f9b3dd6107c284970907aadb5",
          probe_coldstart_probe_v1_json_sha256: FROZEN_PROBE_SPEC_SHA256,
          probe_calibration_256k_bin_sha256: "e23d9c02e637b253337bf7dee349235783b6a63a059934fe526c5ba7f1329904",
        },
        hash_verification_this_run: preregistrationHashCheck,
        note: "hash_verification_this_run is this actual run's machine-executed check result (verifyPreregistrationHashes(), lib/common.mjs), computed at process start before any browser/network work -- not merely a claim. Per the reviewer's hard constraint, the raw-socket independent-byte-count judgment criterion (measureRawHttpResponseBytes) was NOT changed for this run, regardless of outcome -- a failure there is reported as fail-closed invalid, not worked around.",
      },
      reviewer_constraint_note:
        "Reviewer's binding constraint (see PREREGISTRATION-SIGNATURE.md): the raw-socket independent-byte-count check (Connection: close, no browser connection-reuse semantics) has operational fragility. If it destabilizes during this run, the affected sample(s) are fail-closed invalid -- this file contains NO fallback and NO alternate judgment path; switching to the reviewer's suggested alternative (browser-observed identity body length) would change the signed protocol and requires a new hash + re-signature, and is explicitly forbidden after formal results exist. See throughput_calibration_by_sample below for the actual outcome.",
      metric: "flow_route_cold_load_ms_p95",
      ready_postconditions_used: "1-5 (all), including mount+rAF",
      probe_scope_note: "This metric's read/write probe read-back is DOM-only. Per ADR-0015 (R6 correction, still in force at R7), independent CRDT-state (LoroDoc/Y.Doc) cross-checking is NOT required for this metric -- that verification is engine_runtime_ready's job (see that metric's own evidence, which reads back exclusively via the candidate's CRDT container API). Recorded here explicitly so this metric's numbers are never read as having verified engine/CRDT state: they verify DOM state (and, via the shared expected_semantic_hash, cross-candidate DOM consistency) only.",
      run_id: RUN_ID,
      source_commit: gitSourceCommit(REPO_ROOT),
      shared_runner_hash: hashRunnerFiles(HERE),
      frozen_probe_spec_sha256: FROZEN_PROBE_SPEC_SHA256,
      server_config_hash: sha256HexOfFile(SERVER_SCRIPT),
      browser_version: chromiumVersion,
      toolchain_versions_by_candidate: Object.fromEntries(CANDIDATES.map((c) => [c.name, c.toolchainVersions])),
      toolchain_versions_equal_across_candidates: toolchainsEqual,
      p95_algorithm: "nearest-rank, 1-based ceil(0.95*n), no interpolation",
      warmup_count: WARMUP_COUNT,
      sample_count_target: SAMPLE_COUNT,
      scheduling: "per-sample-pair alternating (AB, BA, AB, BA, ...), see lib/common.mjs buildAlternatingPairSchedule; both candidates' servers run concurrently for the whole run (different ports); global_index/pair_index/position_in_pair/started_at/finished_at recorded per sample below.",
      network_profile: {
        name: NETWORK_PROFILE_MOBILE_1.name,
        declared_bits_per_sec: {
          downloadThroughput: NETWORK_PROFILE_MOBILE_1.downloadThroughputBitsPerSec,
          uploadThroughput: NETWORK_PROFILE_MOBILE_1.uploadThroughputBitsPerSec,
        },
        latencyMs: NETWORK_PROFILE_MOBILE_1.latencyMs,
        packetLossPercent: NETWORK_PROFILE_MOBILE_1.packetLossPercent,
        cdp_command: NETWORK_CDP_METHOD,
        note: "CDP downloadThroughput/uploadThroughput are bytes/sec; converted from the ADR's bit/s via /8 (1600000/8=200000, 750000/8=93750). Network.emulateNetworkConditions is deprecated in this Chromium build (confirmed via /json/protocol); emulateNetworkConditionsByRule is the current, non-deprecated equivalent. See per-sample networkRuleAttribution + throughputCalibration in each candidate's runs[] for the actual applied-parameter proof (not just what was sent) and network_rule_attribution_by_sample / throughput_calibration_by_sample for the roll-up.",
      },
      origin: { server_script: "gzip-static-server.mjs", protocol: "HTTP/1.1", host: "127.0.0.1", tls: false, gzip_level: 9, note: "gzip level 9 for all normally-compressible route assets; the calibration asset (see throughput_calibration_asset below) is the one named exception, served Content-Encoding: identity because it is incompressible by construction." },
      throughput_calibration_asset: { url_path: calibrationAsset.urlPath, size_bytes: calibrationAsset.sizeBytes, sha256: calibrationAsset.sha256, shared_across_both_candidates: true },
      read_write_probe: {
        expected_full_hash_hex: expected.fullHex,
        expected_canonical_json: expected.canonicalJson,
        description: "SHA-256 of a fixed structural block array (parent/index/type/text) READ FROM THE DOM ONLY, see probe/coldstart-probe-v1.json's flow_route_cold_load_probe; harness-computed and harness-compared, not self-reported by candidate code. Per ADR-0015 (R6 correction, still in force at R7) this DOES NOT verify CRDT/engine state (see probe_scope_note above) -- it verifies rendered DOM state.",
      },
      fixture_text: FIXTURE_TEXT,
      probe_text: PROBE_TEXT,
      results,
    };
    const outPath = join(OUT_DIR, "flow-route-cold-load-result.json");
    writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
    console.log(`wrote ${outPath}`);
    for (const r of results) {
      console.log(
        `${r.candidate}: p95=${r.p95_ms}ms budget=${r.budget_ms}ms status=${r.budget_status} success=${r.sample_count_successful}/${r.sample_count_attempted} content_encoding_all_ok=${r.content_encoding_assertions_all_ok} throughput_calibration_all_ok=${r.throughput_calibration_all_ok}`,
      );
    }
  } finally {
    await browser.close();
    for (const s of servers) s.kill();
  }
}

await main();

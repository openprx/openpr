// Shared constants + pure helpers for the `sylvode.flow.coldstart.v1` protocol runners
// (measure-engine-runtime-ready.mjs, measure-flow-route-cold-load.mjs). Kept dependency-free
// (node:crypto only) so `shared_runner_hash` (see hashRunnerFiles below) is a hash of code that
// both metrics' runners actually execute, not an approximation.
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join, relative, extname } from "node:path";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { createConnection } from "node:net";

export const MEASUREMENT_PROTOCOL_ID = "sylvode.flow.coldstart.v1";
export const CHROMIUM_PATH = "/usr/bin/chromium";

// ADR-0015 3.1: 5 warmup + >=30 measured samples. Warmup runs the identical fresh-context/
// empty-cache flow as measured samples (see both runners) -- they are not a relaxed pre-pass.
// COLDSTART_WARMUP_OVERRIDE/COLDSTART_SAMPLE_OVERRIDE exist ONLY for smoke-testing the runner
// code path quickly; every value written to evidence/ for actual protocol conformance MUST use
// the defaults (5 / 30) -- final report runs are executed without these env vars set.
export const WARMUP_COUNT = process.env.COLDSTART_WARMUP_OVERRIDE ? Number(process.env.COLDSTART_WARMUP_OVERRIDE) : 5;
export const SAMPLE_COUNT = process.env.COLDSTART_SAMPLE_OVERRIDE ? Number(process.env.COLDSTART_SAMPLE_OVERRIDE) : 30;

// ADR-0015 3.4 (R3 correction, coordinator directive 2026-08-29): `sylvode-net-mobile-1`. This is
// a project-defined profile, NOT equivalent to any Chrome DevTools built-in preset -- Chromium's
// own "Slow 4G"/"Fast 4G" presets additionally apply a 0.9x throughput multiplier and a 3.75x
// latency multiplier on top of their nominal numbers; this profile applies its numbers directly,
// unmultiplied. Values are frozen as bit/s; CDP throughput fields are BYTES/s, so the /8
// conversion happens once here, not sprinkled across call sites, and the *converted* numbers
// actually handed to CDP are echoed back into evidence verbatim (see both runners'
// `network_profile.cdp_params_sent`) so a reviewer can check 200000/93750, not 1600000/750000.
//
// CDP method choice, frozen and bound to the Chromium build this was verified against
// (144.0.7559.109, see both runners' `browser_version`): `Network.emulateNetworkConditions` is
// marked `deprecated` in this build's own `/json/protocol` (confirmed via
// `curl http://localhost:<debug-port>/json/protocol` against a running instance), so this uses
// the current, non-deprecated `Network.emulateNetworkConditionsByRule` instead, matched with a
// single global rule (empty `urlPattern` = matches all requests, including the pattern's own
// documented p2p-throttling scope). `packetLoss: 0` is an actual field on `NetworkConditions`
// (unlike the deprecated command's shape, where it exists but is `experimental`) -- set
// explicitly rather than left as an implicit default, satisfying the frozen packetLoss=0%
// requirement literally.
//
// Verified equivalence before switching: with an IDENTICAL `page.goto({waitUntil:
// 'domcontentloaded'}) + waitForSelector('.ProseMirror')` measurement (matching what both
// runners actually do), `emulateNetworkConditions` and `emulateNetworkConditionsByRule` produced
// the exact same elapsed time (6321ms, loro flow-route, single manual A/B trial) for these same
// numeric parameters -- i.e. this is not a behavioral change, only an API-deprecation fix.
export const NETWORK_CDP_METHOD = "Network.emulateNetworkConditionsByRule";
export const NETWORK_PROFILE_MOBILE_1 = {
  name: "sylvode-net-mobile-1",
  downloadThroughputBitsPerSec: 1_600_000,
  uploadThroughputBitsPerSec: 750_000,
  latencyMs: 150,
  packetLossPercent: 0,
};

export function cdpNetworkConditionsFor(profile) {
  return {
    offline: false,
    matchedNetworkConditions: [
      {
        urlPattern: "",
        latency: profile.latencyMs,
        downloadThroughput: profile.downloadThroughputBitsPerSec / 8,
        uploadThroughput: profile.uploadThroughputBitsPerSec / 8,
        connectionType: "cellular3g",
        packetLoss: profile.packetLossPercent,
      },
    ],
  };
}

// ADR-0015 3.4 "回读" requirement + coordinator directive 2026-08-29 item 1: two things, both
// mandatory, neither alone sufficient.
//
// (1) Rule-id attribution: `Network.emulateNetworkConditionsByRule`'s return value is
//     `{ ruleIds: string[] }`, one id per `matchedNetworkConditions` entry (confirmed against this
//     Chromium build's own `/json/protocol`, `Network.emulateNetworkConditionsByRule.returns`).
//     Per-request attribution comes from `Network.requestWillBeSentExtraInfo`'s
//     `appliedNetworkConditionsId` field (same protocol doc: "The network conditions id if this
//     request was affected by network conditions configured via emulateNetworkConditionsByRule"),
//     which is only emitted when `Network.enable` is on (already true in both runners). The runner
//     saves the returned `ruleIds[0]` (this profile registers exactly one global rule, empty
//     `urlPattern` = matches everything) and, per timed navigation, the set of
//     `appliedNetworkConditionsId` values seen across every request, asserting they are all
//     `ruleIds[0]` and that the set is non-empty (i.e. at least one real request actually carried
//     the rule, not just "we called the API and trust it worked").
//
// (2) Throughput calibration (independent of (1) -- (1) only proves CDP accepted+attributed a
//     rule, not that the numeric throughput it actually enforced matches what was requested).
//
//     R6 CORRECTION (coordinator directive 2026-08-29, second predregistration rejection --
//     `NOT_SIGNED_HOLD`): R5 picked "the largest file in each candidate's own dist/" as the
//     calibration resource, which is NOT neutral -- loro's largest asset (~1MB wasm) and
//     yrs-yjs's (~57KB vendor-prosemirror chunk) are wildly different sizes, so the SAME
//     acceptance rule could pass one candidate and silently fail the other for reasons having
//     nothing to do with the actual network conditions. R6 fixes this: BOTH candidates now
//     calibrate against the SAME fixed, coordinator-generated, deterministic-content file,
//     `probe/calibration-256k.bin` -- see CALIBRATION_ASSET_* below. It is served by
//     `gzip-static-server.mjs` at the fixed path `/calibration-256k.bin` on BOTH origins
//     regardless of which candidate's `dist/` that server instance is otherwise serving.
//
//     R6 ALSO changes the formula itself (coordinator: "这是我冻错了，不是你实现错"). R5's
//     `elapsed_sec_cdp_timestamps - (latencyMs/1000)` assumed CDP's injected `latency` is exactly
//     one clean subtractable constant; under the frozen `sylvode-net-mobile-1` numbers this
//     assumption alone can push a genuinely-correct small transfer below the tolerance floor
//     (coordinator's worked example: 56795 / (56795/200000 + 0.150) = 130871 B/s, below the
//     150000 lower bound, for an otherwise-correct 200000 B/s run) -- i.e. the OLD formula could
//     reject a candidate for being small, not for being throttled wrong. R6's formula uses the
//     CDP `Network.responseReceived` event's own `timestamp` ("firstByte_ts") as the window start
//     instead, which structurally excludes DNS/connect/TTFB/latency without assuming any constant
//     value for them:
//
//       transfer_bytes = the CDP `Network.loadingFinished` event's own `encodedDataLength` field
//                         for that one request (protocol doc: "Total number of bytes received for
//                         this request" -- the authoritative FINAL count; see lib/har.mjs's
//                         loadingFinished handler -- NOT the browser's post-gunzip decoded length,
//                         which fetch()'s `ArrayBuffer.byteLength` would silently give instead,
//                         since calibration-256k.bin is incompressible and served identity this
//                         particular distinction happens to not matter for ITS bytes, but the
//                         runner still reads the CDP-reported count on principle, not the fetch
//                         API's).
//       first_byte_ts  = the CDP `Network.responseReceived` event's own top-level `timestamp` for
//                         that request (same monotonic clock domain as loadingFinished/
//                         requestWillBeSent -- see lib/har.mjs's `entry.responseReceivedTimestamp`).
//                         If this timestamp was never captured for the calibration request (e.g.
//                         responseReceived never fired), calibration is `ok: false` with that
//                         reason recorded -- ADR-0015 R6 is explicit that there is NO fallback to
//                         subtracting an assumed constant in this case; a missing measurement
//                         fails closed, it does not get silently estimated.
//       transfer_sec   = (CDP `Network.loadingFinished` timestamp) - first_byte_ts
//       effective_bps  = transfer_bytes / transfer_sec
//       target_bps     = NETWORK_PROFILE_MOBILE_1.downloadThroughputBitsPerSec / 8  (= 200000)
//       ok             = target_bps * (1 - THROUGHPUT_CALIBRATION_TOLERANCE_RATIO)
//                           <= effective_bps <=
//                         target_bps * (1 + THROUGHPUT_CALIBRATION_TOLERANCE_RATIO)
//                         AND transfer_bytes === CALIBRATION_ASSET_SIZE_BYTES_EXPECTED exactly
//                         (see "independent byte count" note below -- this is a separate,
//                         additional fail-closed condition, not folded into the throughput ratio).
//
//     Issued via `fetch()` from the pre-navigation, untimed "assert origin empty" page, BEFORE the
//     timed navigation's HAR recorder starts capturing the real page-load requests, so this one
//     transfer is not contending for the emulated pipe's bandwidth with anything else.
//
//     `!ok` => the whole run is `invalid` per ADR-0015 3.4/R6's "若实测吞吐显著偏离，该 run 判
//     invalid" (see measureCandidate's `runValid` in the route-cold-load runner).
export const THROUGHPUT_CALIBRATION_TOLERANCE_RATIO = 0.25; // +/-25% band around 200000 B/s target
export const THROUGHPUT_CALIBRATION_LOWER_BPS =
  (NETWORK_PROFILE_MOBILE_1.downloadThroughputBitsPerSec / 8) * (1 - THROUGHPUT_CALIBRATION_TOLERANCE_RATIO);
export const THROUGHPUT_CALIBRATION_UPPER_BPS =
  (NETWORK_PROFILE_MOBILE_1.downloadThroughputBitsPerSec / 8) * (1 + THROUGHPUT_CALIBRATION_TOLERANCE_RATIO);

// R6 fixed calibration asset (coordinator-generated, NOT selected by this runner): content is
// "sha256('sylvode-coldstart-calib-v1' || le_u32(i))" concatenated for i = 0, 1, 2, ... until
// 262144 bytes -- deterministically reproducible, high-entropy, and (verified) INCOMPRESSIBLE
// (gzip-9 produces 262242 bytes, larger than raw -- see gzip-static-server.mjs's R6 comment for
// why it is therefore served with an explicit `Content-Encoding: identity`, the compression
// clause's one named legal exception). Both candidates calibrate against this SAME file, served at
// the SAME fixed path on both origins, so the acceptance rule is symmetric across candidates.
export const CALIBRATION_ASSET_PATH = join(fileURLToPath(new URL("..", import.meta.url)), "probe", "calibration-256k.bin");
export const CALIBRATION_ASSET_URL_PATH = "/calibration-256k.bin";
export const CALIBRATION_ASSET_SIZE_BYTES_EXPECTED = 262144;
export const CALIBRATION_ASSET_SHA256_EXPECTED = "e23d9c02e637b253337bf7dee349235783b6a63a059934fe526c5ba7f1329904";

// Reads the frozen calibration asset and verifies it against the coordinator-provided expected
// SHA-256/size BEFORE returning it -- if either check fails, throws rather than silently
// calibrating against a different file than what was predregistered (fail loud, not fail quiet).
export function loadCalibrationAsset() {
  const raw = readFileSync(CALIBRATION_ASSET_PATH);
  const actualSha256 = sha256HexOfBuffer(raw);
  if (raw.length !== CALIBRATION_ASSET_SIZE_BYTES_EXPECTED) {
    throw new Error(
      `loadCalibrationAsset: size mismatch -- expected ${CALIBRATION_ASSET_SIZE_BYTES_EXPECTED}, got ${raw.length} (${CALIBRATION_ASSET_PATH})`,
    );
  }
  if (actualSha256 !== CALIBRATION_ASSET_SHA256_EXPECTED) {
    throw new Error(
      `loadCalibrationAsset: SHA-256 mismatch -- expected ${CALIBRATION_ASSET_SHA256_EXPECTED}, got ${actualSha256} (${CALIBRATION_ASSET_PATH})`,
    );
  }
  return { urlPath: CALIBRATION_ASSET_URL_PATH, sizeBytes: raw.length, sha256: actualSha256 };
}

// EMPIRICAL FINDING (this implementation round): CDP `Network.loadingFinished.encodedDataLength`
// was found to include the FULL HTTP/1.1 response -- status line + headers + body -- not just the
// body. Measured directly: for calibration-256k.bin (body=262144 bytes) served by
// gzip-static-server.mjs, `curl -s -o /dev/null -w '%{size_header}'` reports a 173-byte header
// block (`HTTP/1.1 200 OK\r\nContent-Type: ...\r\n...\r\n\r\n`), and CDP's encodedDataLength for
// the same request was observed at exactly 262144 + 173 = 262317, every time (deterministic --
// this server's header set has no variable-length fields; the `Date` header's format is
// fixed-width). This means comparing CDP's encodedDataLength against "the file's raw size on disk"
// can NEVER pass -- HTTP responses always carry header overhead, so the two quantities measure
// different things by construction, not because of a bug in either measurement.
//
// So the independent byte count used for the R6 "must really participate in fail-closed"
// assertion is NOT the static file size -- it is the ACTUAL total wire byte count of a real
// HTTP/1.1 response to the SAME URL, measured via a raw TCP socket in THIS Node process
// (`measureRawHttpResponseBytes` below), entirely outside Puppeteer/CDP -- a genuinely
// independent second channel, not a hardcoded "+173" fudge factor that would silently drift if
// gzip-static-server.mjs's header set ever changes.
export function measureRawHttpResponseBytes(urlString) {
  return new Promise((resolvePromise, reject) => {
    const url = new URL(urlString);
    const socket = createConnection({ host: url.hostname, port: Number(url.port) }, () => {
      socket.write(`GET ${url.pathname}${url.search} HTTP/1.1\r\nHost: ${url.host}\r\nConnection: close\r\n\r\n`);
    });
    let totalBytes = 0;
    const timeout = setTimeout(() => {
      socket.destroy();
      reject(new Error(`measureRawHttpResponseBytes: timed out waiting for ${urlString}`));
    }, 15000);
    socket.on("data", (chunk) => {
      totalBytes += chunk.length;
    });
    socket.on("end", () => {
      clearTimeout(timeout);
      resolvePromise(totalBytes);
    });
    socket.on("error", (err) => {
      clearTimeout(timeout);
      reject(err);
    });
  });
}

// Fixed corpus: one constant fixture id, not Date.now()/Math.random() keyed -- ADR-0015 3.1
// "固定 corpus seed". The exact suffix does not select different served bytes (SPA fallback
// serves the same index.html for any /flow/* path); its only job is to be reproducible.
export const CORPUS_SEED = "sylvode-coldstart-v1";
export const FLOW_ROUTE_PATH = `/flow/${CORPUS_SEED}`;

// Coordinator directive 2026-08-29 item 3: the probe workload is now pre-registered as a frozen,
// standalone file (`probe/coldstart-probe-v1.json`) whose own SHA-256 is meant to be recorded in
// the ADR and signed BEFORE any formal run -- see that file's `note_to_reviewer`. This module reads
// it rather than duplicating its constants, so there is exactly one source of truth; editing the
// frozen file after its hash has been recorded would silently invalidate the pre-registration, so
// don't (see the file's own warning).
const PROBE_SPEC_PATH = fileURLToPath(new URL("../probe/coldstart-probe-v1.json", import.meta.url));
export const FROZEN_PROBE_SPEC = JSON.parse(readFileSync(PROBE_SPEC_PATH, "utf8"));
// `sha256HexOfFile` is a hoisted function declaration (defined later in this same module), so
// referencing it here at module-init time is safe.
export const FROZEN_PROBE_SPEC_SHA256 = sha256HexOfFile(PROBE_SPEC_PATH);
export const FIXTURE_TEXT = FROZEN_PROBE_SPEC.fixture_text;
export const PROBE_TEXT = FROZEN_PROBE_SPEC.probe_text;

// `engine_runtime_ready`'s adapter-API-only probe (ADR-0015 R4 3.2 item 4 + coordinator directive
// 2026-08-29 item 2): a REAL block id (a LoroMap/Y.Map container key, not a DOM position guess),
// read back exclusively through the CRDT container API -- see
// `probe/src/loro-engine-only-entry.ts` / `probe/src/yrs-yjs-engine-only-entry.ts` and
// `probe/check-module-graph.mjs` (mechanically verifies zero ProseMirror-family modules in the
// built entry each of those compiles to).
export function engineProbeExpectedDigest() {
  const spec = FROZEN_PROBE_SPEC.engine_runtime_ready_probe;
  return { fullHex: spec.expected_semantic_hash_sha256, canonicalJson: spec.expected_canonical_json };
}

// `flow_route_cold_load`'s DOM probe (ADR-0015 R4 3.2 item 4, route half): synthetic keyboard
// input into the real ProseMirror EditorView, DOM-only read-back. See
// `probe/coldstart-probe-v1.json`'s `flow_route_cold_load_probe.known_gap_not_yet_closed` for why
// this one is NOT independently cross-checked against CRDT state (a real, disclosed, currently
// unresolved file-scope limitation -- not silently accepted).
export function routeProbeExpectedDigest() {
  const spec = FROZEN_PROBE_SPEC.flow_route_cold_load_probe;
  return { fullHex: spec.expected_semantic_hash_sha256, canonicalJson: spec.expected_canonical_json };
}

// ADR-0015 3.1: "p95 算法冻结：升序排序后取 nearest-rank，1-based 下标 ceil(0.95 * n)，不做插值".
export function nearestRankP95(values) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.ceil(0.95 * sorted.length); // 1-based
  const idx = Math.min(sorted.length, Math.max(1, rank)) - 1;
  return sorted[idx];
}

export function round3(ms) {
  return Math.round(ms * 1000) / 1000;
}

export function sha256HexOfBuffer(buf) {
  return createHash("sha256").update(buf).digest("hex");
}

export function sha256HexOfFile(path) {
  return sha256HexOfBuffer(readFileSync(path));
}

// Coordinator correction 2026-08-29 item 2: per-candidate identity that MUST exist in evidence
// but is NOT required to be equal across candidates (unlike source_commit/shared_runner_hash/
// server_config_hash/browser_version/measurement_protocol_id, which are). Hashes the candidate's
// own package.json + bun.lock together, i.e. its declared+resolved dependency graph.
export function packageLockfileHash(candidateDir) {
  const h = createHash("sha256");
  for (const f of ["package.json", "bun.lock"]) {
    h.update(f);
    h.update("\0");
    h.update(readFileSync(join(candidateDir, f)));
    h.update("\0");
  }
  return h.digest("hex");
}

// The "common build recipe / toolchain hash" half of item 2: versions that SHOULD be identical
// across both candidates' build tooling (this run does not rebuild dist/ -- it measures the
// already-built artifacts -- so this is a declared-version equality check, not a hash of a build
// step actually re-executed here).
export function toolchainVersionsFor(candidateDir) {
  const pkg = JSON.parse(readFileSync(join(candidateDir, "package.json"), "utf8"));
  return {
    bun: (() => {
      try {
        return execFileSync("bun", ["--version"], { encoding: "utf8" }).trim();
      } catch {
        return null;
      }
    })(),
    vite: pkg.devDependencies?.vite ?? null,
    typescript: pkg.devDependencies?.typescript ?? null,
  };
}

// Deterministic hash of a directory tree: sorted relative paths, each path + its byte hash fed
// into one running digest, so reordering `readdir` results across OSes/runs cannot change it.
export function sha256HexOfDir(dir) {
  const files = [];
  (function walk(rel) {
    for (const entry of readdirSync(join(dir, rel)).sort()) {
      const relPath = rel === "" ? entry : `${rel}/${entry}`;
      const full = join(dir, relPath);
      if (statSync(full).isDirectory()) walk(relPath);
      else files.push(relPath);
    }
  })("");
  const h = createHash("sha256");
  for (const f of files) {
    h.update(f);
    h.update("\0");
    h.update(readFileSync(join(dir, f)));
    h.update("\0");
  }
  return h.digest("hex");
}

// `shared_runner_hash`: hash of every runner/lib source file under spikes/coldstart-measure that
// this run's two metrics actually import/execute -- excludes evidence/ and node_modules so the
// hash reflects the measurement code, not its output or dependencies.
export function hashRunnerFiles(coldstartMeasureDir, extraFiles = []) {
  const files = [];
  for (const f of readdirSync(coldstartMeasureDir)) {
    if (f.endsWith(".mjs")) files.push(join(coldstartMeasureDir, f));
  }
  const libDir = join(coldstartMeasureDir, "lib");
  for (const f of readdirSync(libDir)) {
    if (f.endsWith(".mjs")) files.push(join(libDir, f));
  }
  // The dedicated engine_runtime_ready probe entries/build configs/module-graph checker/frozen
  // workload spec are all measurement code too (they determine what gets timed and what "pass"
  // means) -- included so `shared_runner_hash` actually covers them, not just lib/*.mjs.
  const probeDir = join(coldstartMeasureDir, "probe");
  for (const f of readdirSync(probeDir)) {
    const full = join(probeDir, f);
    // "coldstart-probe-v1.json" is the frozen workload spec (source of truth, included);
    // "module-graph-check-result.json" is this checker's own OUTPUT (excluded -- it's generated,
    // not measurement code, and including it would make the hash depend on its own last run).
    if (!statSync(full).isFile()) continue;
    if (f.endsWith(".mjs") || f === "coldstart-probe-v1.json") files.push(full);
  }
  const probeSrcDir = join(probeDir, "src");
  for (const f of readdirSync(probeSrcDir)) {
    if (f.endsWith(".ts")) files.push(join(probeSrcDir, f));
  }
  for (const f of extraFiles) files.push(f);
  files.sort();
  const h = createHash("sha256");
  for (const f of files) {
    h.update(relative(coldstartMeasureDir, f));
    h.update("\0");
    h.update(readFileSync(f));
    h.update("\0");
  }
  return h.digest("hex");
}

// Coordinator directive 2026-08-29 (APPROVED_FOR_FORMAL_RUN): the predregistration signature at
// `evidence/v2/PREREGISTRATION-SIGNATURE.md` binds three file hashes -- "任一字节变更即失效". This
// function makes that check MACHINE-VERIFIED at the start of every formal run (not just something
// a human confirmed once via a shell command before starting), matching this whole delivery's
// "机器验证不是靠人看" pattern (see probe/check-module-graph.mjs for the same principle applied to
// the ProseMirror-exclusion check). Reads (never writes) the ADR file under
// /opt/working/sylvode-flow/ -- outside this delivery's file-scope to MODIFY, but reading it to
// compute a hash is not a modification.
export const PREREGISTRATION_ADR_PATH = "/opt/working/sylvode-flow/decisions/ADR-0015-cold-start-measurement-split.md";
export const PREREGISTRATION_BOUND_HASHES = {
  adr_0015_r7_sha256: "7b6725f4f09fc5804f3e7cfede99c4233cc9825f9b3dd6107c284970907aadb5",
  probe_coldstart_probe_v1_json_sha256: "6fb9ed16e62e2a5714f5268f3c8cceb58bd0cfef740d38e6833145320184c386",
  probe_calibration_256k_bin_sha256: "e23d9c02e637b253337bf7dee349235783b6a63a059934fe526c5ba7f1329904",
};

export function verifyPreregistrationHashes() {
  const actual = {
    adr_0015_r7_sha256: sha256HexOfFile(PREREGISTRATION_ADR_PATH),
    probe_coldstart_probe_v1_json_sha256: sha256HexOfFile(PROBE_SPEC_PATH),
    probe_calibration_256k_bin_sha256: sha256HexOfFile(CALIBRATION_ASSET_PATH),
  };
  const mismatches = Object.keys(PREREGISTRATION_BOUND_HASHES).filter(
    (k) => actual[k] !== PREREGISTRATION_BOUND_HASHES[k],
  );
  return {
    ok: mismatches.length === 0,
    expected: PREREGISTRATION_BOUND_HASHES,
    actual,
    mismatches,
  };
}

export function gitSourceCommit(repoDir) {
  return execFileSync("git", ["-C", repoDir, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
}

// ADR-0015 3.1 "两候选同机器、顺序交替" + coordinator directive 2026-08-29 item 4: per-SAMPLE-PAIR
// alternation, not whole-batch-per-candidate. Builds a flat, globally-ordered schedule of
// individual samples across BOTH candidates, alternating which candidate goes first within each
// pair (AB, BA, AB, BA, ...) so neither candidate systematically gets the "always warmer/colder
// machine" slot. `candidateNames` must have length 2 (this protocol is always a two-candidate
// comparison); pair 0 runs candidateNames[0] then candidateNames[1], pair 1 runs
// candidateNames[1] then candidateNames[0], etc. Warmup pairs come first (5 by default), then
// measured pairs (>=30 by default) -- each pair is two consecutive schedule entries with the same
// `pairIndex`/`warmup`, opposite `positionInPair`.
export function buildAlternatingPairSchedule(candidateNames, warmupCount, sampleCount) {
  if (candidateNames.length !== 2) {
    throw new Error(`buildAlternatingPairSchedule: expected exactly 2 candidates, got ${candidateNames.length}`);
  }
  const [a, b] = candidateNames;
  const schedule = [];
  let globalIndex = 0;
  const pushPairs = (count, warmup, pairOffset) => {
    for (let p = 0; p < count; p++) {
      const pairIndex = pairOffset + p;
      const firstIsA = pairIndex % 2 === 0;
      const first = firstIsA ? a : b;
      const second = firstIsA ? b : a;
      schedule.push({ globalIndex: globalIndex++, pairIndex, warmup, candidate: first, positionInPair: 0 });
      schedule.push({ globalIndex: globalIndex++, pairIndex, warmup, candidate: second, positionInPair: 1 });
    }
  };
  pushPairs(warmupCount, true, 0);
  pushPairs(sampleCount, false, warmupCount);
  return schedule;
}

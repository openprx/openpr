// `engine_runtime_ready_ms_p95` per ADR-0015 (current revision R7 -- APPROVED_FOR_FORMAL_RUN, see
// evidence/v2/PREREGISTRATION-SIGNATURE.md; this metric's adapter-API-only/vendor-exclusion design
// dates to R4 and was independently CONFIRMED correct through the predregistration rounds, no
// functional changes to this file since) -- (/opt/working/sylvode-flow/decisions/
// ADR-0015-cold-start-measurement-split.md) section 3.3 + 3.2 item 4. ADR is still `Proposed`:
// this evidence is protocol-conformant but MUST NOT be used to decide any hard gate until the ADR
// is Accepted. Predregistration history: rejected at R4 and R5 (`NOT_SIGNED_HOLD`), signed at R7
// (2026-08-29T13:41:56Z) -- see COLDSTART_SAMPLE_OVERRIDE/COLDSTART_WARMUP_OVERRIDE in
// lib/common.mjs for how a smoke run is distinguished from a formal one, and this file's own
// `main()` for where that distinction is stamped into evidence.
//
// R4 REDESIGN (supersedes the R3 approach entirely, not an incremental patch): R3 measured the
// REAL candidate's `bundle-loro|bundle-yrs-yjs/dist/assets/flow-entry-*.js` chunk by calling its
// only export, `mountFlowEditor(target)` -- which unconditionally constructs a ProseMirror
// `EditorView`. An independent reviewer (see `/opt/worker/report/codex-adr0015-r3-2026-08-29.md`)
// found this put real EditorView-mount work inside `engine_runtime_ready`'s timed window, directly
// contradicting the metric's own "排除 ProseMirror/vendor 初始化" definition. Investigation during
// this rewrite confirmed there is no way to reach either candidate's CRDT document without going
// through `mountFlowEditor` -- both `bundle-loro/src/flow-entry.ts` and
// `bundle-yrs-yjs/src/flow-entry.ts` export ONLY that one function (confirmed by inspecting the
// built chunks' trailing `export{...}` statement), and modifying those files is out of this
// delivery's file scope (`spikes/coldstart-measure/**` only).
//
// Per coordinator directive 2026-08-29 item 2 ("给 engine_runtime_ready 用专用 entry"), this metric
// now measures a DEDICATED, coldstart-measure-owned build entry per candidate
// (`probe/src/loro-engine-only-entry.ts` / `probe/src/yrs-yjs-engine-only-entry.ts`, built by
// `probe/vite.config.loro.mjs` / `probe/vite.config.yrs-yjs.mjs` into `probe/dist-loro` /
// `probe/dist-yrs-yjs` -- run `bun run probe/vite.config.*` -- i.e. `bunx vite build --config
// probe/vite.config.<name>.mjs` -- to rebuild, same as `bundle-loro`/`bundle-yrs-yjs`'s own
// `bun run build` is a precondition this runner does NOT invoke automatically). Each dedicated
// entry imports ONLY the candidate's CRDT engine package (`loro-crdt` / `yjs`), resolved via a
// `resolve.alias` pointing at the REAL candidate's own already-installed `node_modules` (same
// bytes, same version, verified below via `engine_probe_dependency_identity`) -- NOT
// `loro-prosemirror`/`y-prosemirror`/`prosemirror-model`/`prosemirror-state`/`prosemirror-view`,
// which do not appear anywhere in either dedicated entry or anything it imports.
//
// This is verified MECHANICALLY, not by inspection, every run: `checkModuleGraph()` (imported from
// `probe/check-module-graph.mjs`) walks Rollup's own resolved module graph for each dedicated
// entry's build and asserts zero modules match a ProseMirror-family deny-list. Its full result is
// embedded in this run's evidence (`module_graph_check`) and, if it fails, the run aborts before
// any timing sample is collected (fail-closed at the earliest possible point -- see `main()`).
//
// Adapter-API-only probe (ADR-0015 3.2 item 4): postconditions are now 1-4 as CRDT-only operations
// -- module eval, WASM instantiate (loro only), a fixed-block adapter init, a fixture bootstrap
// write, and a probe write -- all via `initEngineDoc`/`bootstrapFixture`/`writeProbe` from the
// dedicated entry, with `readBackBlocks` reading back EXCLUSIVELY through the CRDT container API
// (never DOM -- there is no DOM content in this harness page at all beyond the empty shell).
// Workload is frozen in `probe/coldstart-probe-v1.json` (`engine_runtime_ready_probe`), loaded via
// `engineProbeExpectedDigest()`; comparison is done by THIS runner (Node side, via
// `page.evaluate`'s return value), not by candidate/probe code, which only returns observed data.
import { readFileSync, readdirSync, writeFileSync, mkdirSync } from "node:fs";
import { join, extname } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";
import {
  MEASUREMENT_PROTOCOL_ID,
  CHROMIUM_PATH,
  WARMUP_COUNT,
  SAMPLE_COUNT,
  FIXTURE_TEXT,
  PROBE_TEXT,
  FROZEN_PROBE_SPEC_SHA256,
  engineProbeExpectedDigest,
  nearestRankP95,
  round3,
  sha256HexOfDir,
  hashRunnerFiles,
  gitSourceCommit,
  packageLockfileHash,
  toolchainVersionsFor,
  sha256HexOfFile,
  buildAlternatingPairSchedule,
  verifyPreregistrationHashes,
} from "./lib/common.mjs";
import { checkModuleGraph } from "./probe/check-module-graph.mjs";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const REPO_ROOT = join(HERE, "..", "..");
const RUN_ID = `engine-runtime-ready-${new Date().toISOString().replace(/[:.]/g, "-")}`;
const EVIDENCE_DIR = join(HERE, "evidence", "v2");

const IS_SMOKE = process.env.COLDSTART_WARMUP_OVERRIDE !== undefined || process.env.COLDSTART_SAMPLE_OVERRIDE !== undefined;
// Coordinator directive: smoke output must never land where it could be mistaken for formal
// evidence. Formal runs write to evidence/v1 (unchanged path/shape); smoke runs write to a
// clearly-labeled sibling directory instead.
const OUT_DIR = IS_SMOKE ? join(HERE, "smoke", "r7") : EVIDENCE_DIR;
mkdirSync(OUT_DIR, { recursive: true });

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
};

const HARNESS_ORIGIN = "http://127.0.0.1:59999"; // never dialed -- every request is intercepted
const HARNESS_HTML_PATH = "/__coldstart_harness__.html";
// No `#app` div, no ProseMirror mount target at all -- this harness page has nothing for
// candidate/probe code to render into, matching "完全不触碰 DOM、不构造 EditorView" literally: the
// probe entries never call `document.querySelector`/`getElementById` in the first place (see their
// source), so there being no such element is a structural guarantee, not just an unused one.
const HARNESS_HTML = `<!doctype html><html><head><meta charset="utf-8"></head><body></body></html>`;

const CANDIDATES = [
  {
    name: "loro",
    realCandidateRootDir: join(REPO_ROOT, "spikes", "bundle-loro"),
    probeDistDir: join(HERE, "probe", "dist-loro"),
    probeEntryFile: "loro-engine-only-entry.js",
    hasWasm: true,
    realCandidateLoroCrdtPackageJson: join(REPO_ROOT, "spikes", "bundle-loro", "node_modules", "loro-crdt", "package.json"),
  },
  {
    name: "yrs-yjs",
    realCandidateRootDir: join(REPO_ROOT, "spikes", "bundle-yrs-yjs"),
    probeDistDir: join(HERE, "probe", "dist-yrs-yjs"),
    probeEntryFile: "yrs-yjs-engine-only-entry.js",
    hasWasm: false,
    realCandidateYjsPackageJson: join(REPO_ROOT, "spikes", "bundle-yrs-yjs", "node_modules", "yjs", "package.json"),
  },
];

function readCandidateAssets(distDir) {
  const map = new Map(); // pathname ("/xxx") -> { body: Buffer, contentType }
  for (const f of readdirSync(distDir)) {
    const full = join(distDir, f);
    const body = readFileSync(full);
    const contentType = MIME[extname(f)] ?? "application/octet-stream";
    map.set(`/${f}`, { body, contentType });
  }
  return map;
}

// `loro-crdt`'s browser build loads its .wasm bytes via a SYNCHRONOUS XMLHttpRequest at
// module-eval time (same fact as R3's finding, re-confirmed here: `grep -n XMLHttpRequest
// probe/dist-loro/loro-engine-only-entry.js`). Routing that through Puppeteer's normal
// request-interception `req.respond()` path (a real Fetch-domain round trip, CDP-serialized) costs
// ~360-420ms BY ITSELF for this one ~3.1MB file -- confirmed empirically in THIS rewrite too (see
// the delivery report: an early smoke run without this patch measured loro's engine_runtime_ready
// at ~387ms, with the WebAssembly.Module/Instance segment trace showing only ~4-5ms of that was
// actual compile+instantiate; the other ~382ms was elapsed between the timer start and compile
// beginning, i.e. import()+XHR/interception overhead). That overhead is a measurement-tool
// artifact, not part of "the raw bytes are already in local memory" (ADR-0015 3.3) -- a real
// production runtime holding those bytes as an in-heap ArrayBuffer pays no such IPC tax. So, same
// technique as R3's shared harness: decode the wasm bytes from base64 into an in-page Uint8Array
// ONCE via `evaluateOnNewDocument` (runs untimed, before the timed `page.evaluate` below even
// starts), and patch `XMLHttpRequest` to serve that one URL synthetically -- no interception, no
// Fetch domain, no CDP round trip at all for this specific request.
function buildWasmSyntheticSetupSrc({ url, base64 }) {
  return `
(() => {
  const bin = atob(${JSON.stringify(base64)});
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  let text = "";
  const CHUNK = 8192;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    text += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
  }
  const targetUrl = ${JSON.stringify(url)};
  const OrigOpen = XMLHttpRequest.prototype.open;
  const OrigSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    this.__coldstart_synthetic = String(url) === targetUrl;
    if (this.__coldstart_synthetic) return;
    return OrigOpen.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function (...args) {
    if (this.__coldstart_synthetic) {
      Object.defineProperty(this, "status", { value: 200, configurable: true });
      Object.defineProperty(this, "statusText", { value: "OK", configurable: true });
      Object.defineProperty(this, "readyState", { value: 4, configurable: true });
      Object.defineProperty(this, "responseText", { value: text, configurable: true });
      Object.defineProperty(this, "response", { value: text, configurable: true });
      return undefined;
    }
    return OrigSend.apply(this, args);
  };
})();
`;
}

function installInterception(page, assetMap) {
  page.setRequestInterception(true);
  const seenRequests = [];
  page.on("request", (req) => {
    const url = new URL(req.url());
    seenRequests.push({ url: req.url(), resourceType: req.resourceType() });
    if (url.origin !== HARNESS_ORIGIN) {
      req.abort("connectionrefused");
      return;
    }
    if (url.pathname === HARNESS_HTML_PATH) {
      req.respond({ status: 200, contentType: "text/html; charset=utf-8", body: HARNESS_HTML });
      return;
    }
    const asset = assetMap.get(url.pathname);
    if (asset === undefined) {
      req.respond({ status: 404, body: "" });
      return;
    }
    req.respond({ status: 200, contentType: asset.contentType, body: asset.body });
  });
  return seenRequests;
}

async function runOneSample({ candidate, entryUrl, expectedFullHex, timeoutMs }) {
  const browser = await puppeteer.launch({
    executablePath: CHROMIUM_PATH,
    headless: true,
    args: ["--no-sandbox", "--disable-dev-shm-usage", "--disable-features=BackForwardCache", "--js-flags=--no-flush-bytecode"],
  });
  try {
    const chromiumVersion = await browser.version();
    const page = await browser.newPage();
    await page.setViewport({ width: 1280, height: 800 });
    // No CPU throttling (ADR-0015 3.1: "CPU 节流 = 无") -- default, so nothing set here.
    const seenRequests = installInterception(page, candidate.assetMap);
    if (candidate.wasmSyntheticAsset !== undefined) {
      await page.evaluateOnNewDocument(buildWasmSyntheticSetupSrc(candidate.wasmSyntheticAsset));
    }

    const runWork = (async () => {
      await page.goto(`${HARNESS_ORIGIN}${HARNESS_HTML_PATH}`, { waitUntil: "load", timeout: timeoutMs });

      const result = await page.evaluate(
        async (modUrl, fixtureText, probeText, expectHasWasm, expectFullHex) => {
          // --- WASM segment instrumentation (ADR-0015 3.3 "分段耗时"), inline here rather than via
          // evaluateOnNewDocument -- this harness page has no ProseMirror-postcondition helpers to
          // share with the route runner any more (see this file's header), so a small self-
          // contained patch is clearer than importing the old shared-harness module for one thing.
          const trace = [];
          const OrigModule = WebAssembly.Module;
          function PatchedModule(...args) {
            const start = performance.now();
            const mod = new OrigModule(...args);
            trace.push({ op: "wasm_module_compile_sync", startMs: start, endMs: performance.now() });
            return mod;
          }
          PatchedModule.prototype = OrigModule.prototype;
          Object.setPrototypeOf(PatchedModule, OrigModule);
          WebAssembly.Module = PatchedModule;
          const OrigInstance = WebAssembly.Instance;
          function PatchedInstance(...args) {
            const start = performance.now();
            const inst = new OrigInstance(...args);
            trace.push({ op: "wasm_instance_instantiate_sync", startMs: start, endMs: performance.now() });
            return inst;
          }
          PatchedInstance.prototype = OrigInstance.prototype;
          Object.setPrototypeOf(PatchedInstance, OrigInstance);
          WebAssembly.Instance = PatchedInstance;

          performance.mark("engine-runtime-start");
          const mod = await import(/* @vite-ignore */ modUrl);

          // Postconditions 1-4 (ADR-0015 3.2, engine_runtime_ready subset -- no postcondition 5,
          // no DOM, no ProseMirror): module eval already happened via the import() above; WASM
          // instantiate is captured by the trace above (loro only); "binding/adapter初始化" +
          // "固定fixture bootstrap" + "探针通过" below.
          const handle = mod.initEngineDoc();
          mod.bootstrapFixture(handle, fixtureText);
          mod.writeProbe(handle, probeText);
          const blocks = mod.readBackBlocks(handle);

          const canonicalJson = JSON.stringify(blocks);
          const enc = new TextEncoder().encode(canonicalJson);
          const digestBuf = await crypto.subtle.digest("SHA-256", enc);
          const hex = Array.from(new Uint8Array(digestBuf))
            .map((b) => b.toString(16).padStart(2, "0"))
            .join("");
          const probe = { ok: hex === expectFullHex, blocks, canonicalJson, hex };

          const wasmOk = expectHasWasm ? trace.length > 0 : trace.length === 0;

          // Harness-side ready mark -- NOT self-reported by the probe module (mod has no
          // performance.mark call anywhere in its source).
          if (performance.getEntriesByName("sylvode-ready").length !== 0) {
            return { ok: false, reason: "mark-already-exists: sylvode-ready" };
          }
          performance.mark("sylvode-ready");

          const startEntry = performance.getEntriesByName("engine-runtime-start")[0];
          const endEntry = performance.getEntriesByName("sylvode-ready")[0];
          const elapsedMs = startEntry && endEntry ? endEntry.startTime - startEntry.startTime : null;

          const ok = probe.ok === true && wasmOk && elapsedMs !== null;
          return {
            ok,
            elapsedMs,
            reasons: { probe: { ok: probe.ok, hex: probe.hex, canonicalJson: probe.canonicalJson }, wasmCheck: { ok: wasmOk, trace } },
          };
        },
        entryUrl,
        FIXTURE_TEXT,
        PROBE_TEXT,
        candidate.hasWasm,
        expectedFullHex,
      );

      return { ...result, chromiumVersion, requestCount: seenRequests.length };
    })();

    const timeout = new Promise((_resolve, reject) =>
      setTimeout(() => reject(new Error(`sample timeout after ${timeoutMs}ms`)), timeoutMs + 5000),
    );
    return await Promise.race([runWork, timeout]);
  } catch (err) {
    return { ok: false, reasons: { error: String(err && err.stack ? err.stack : err) }, elapsedMs: null };
  } finally {
    await browser.close();
  }
}

async function main() {
  // Predregistration gate, checked FIRST, before anything else: this run must be authorized by an
  // unexpired signature bound to the exact bytes of the ADR, the probe spec, and the calibration
  // asset. Any mismatch aborts before touching the browser -- machine-verified every run, not a
  // one-time human confirmation.
  let preregistrationHashCheck = null;
  if (!process.env.COLDSTART_WARMUP_OVERRIDE && !process.env.COLDSTART_SAMPLE_OVERRIDE) {
    const sigCheck = verifyPreregistrationHashes();
    preregistrationHashCheck = sigCheck;
    if (!sigCheck.ok) {
      process.stderr.write(`[engine-runtime-ready] ABORT: preregistration hash mismatch, formal run not authorized:\n${JSON.stringify(sigCheck, null, 2)}\n`);
      process.exitCode = 1;
      return;
    }
    process.stderr.write("[engine-runtime-ready] preregistration hashes verified OK (ADR-0015 R7 / coldstart-probe-v1.json / calibration-256k.bin all match signed values)\n");
  }

  // Coordinator directive item 2, "在哪一步拒绝": HERE -- before touching the browser or collecting
  // a single timing sample. A ProseMirror-contaminated build must never even get a chance to
  // produce numbers.
  const moduleGraphCheck = await checkModuleGraph();
  if (!moduleGraphCheck.ok) {
    process.stderr.write(`[engine-runtime-ready] ABORT: module-graph check failed:\n${JSON.stringify(moduleGraphCheck, null, 2)}\n`);
    process.exitCode = 1;
    return;
  }
  process.stderr.write(`[engine-runtime-ready] module-graph check OK (0 ProseMirror-family modules in either dedicated entry)\n`);

  const expected = engineProbeExpectedDigest();
  for (const candidate of CANDIDATES) {
    candidate.assetMap = readCandidateAssets(candidate.probeDistDir);
    candidate.entryUrl = `${HARNESS_ORIGIN}/${candidate.probeEntryFile}`;
    candidate.probeBuiltArtifactHash = sha256HexOfDir(candidate.probeDistDir);
    candidate.packageLockfileHash = packageLockfileHash(candidate.realCandidateRootDir);
    candidate.toolchainVersions = toolchainVersionsFor(candidate.realCandidateRootDir);
    if (candidate.hasWasm) {
      const wasmPath = [...candidate.assetMap.keys()].find((p) => p.endsWith(".wasm"));
      if (wasmPath === undefined) throw new Error(`${candidate.name}: hasWasm=true but no .wasm file found in ${candidate.probeDistDir}`);
      candidate.wasmSyntheticAsset = { url: `${HARNESS_ORIGIN}${wasmPath}`, base64: candidate.assetMap.get(wasmPath).body.toString("base64") };
    }
  }

  // Dependency-identity cross-check (see header comment): the dedicated entry's `resolve.alias`
  // points at the REAL candidate's own installed package -- confirm here, mechanically, that the
  // exact file this run's probe build resolved `loro-crdt`/`yjs` to is byte-identical to what
  // `packageLockfileHash` above already covers for that same candidate, rather than asserting it
  // in a comment and hoping it stays true.
  const dependencyIdentity = {};
  for (const candidate of CANDIDATES) {
    const pkgJsonPath = candidate.realCandidateLoroCrdtPackageJson ?? candidate.realCandidateYjsPackageJson;
    dependencyIdentity[candidate.name] = {
      package_json_path: pkgJsonPath,
      package_json_sha256: sha256HexOfFile(pkgJsonPath),
      package_version: JSON.parse(readFileSync(pkgJsonPath, "utf8")).version,
    };
  }

  const schedule = buildAlternatingPairSchedule(
    CANDIDATES.map((c) => c.name),
    WARMUP_COUNT,
    SAMPLE_COUNT,
  );
  const byName = Object.fromEntries(CANDIDATES.map((c) => [c.name, c]));

  const runsByCandidate = Object.fromEntries(CANDIDATES.map((c) => [c.name, []]));
  for (const entry of schedule) {
    const candidate = byName[entry.candidate];
    process.stderr.write(
      `[engine-runtime-ready] pair=${entry.pairIndex} pos=${entry.positionInPair} ${entry.warmup ? "warmup" : "sample"} global=${entry.globalIndex} ${candidate.name}...\n`,
    );
    const startedAtIso = new Date().toISOString();
    const result = await runOneSample({ candidate, entryUrl: candidate.entryUrl, expectedFullHex: expected.fullHex, timeoutMs: 20000 });
    const finishedAtIso = new Date().toISOString();
    runsByCandidate[candidate.name].push({
      global_index: entry.globalIndex,
      pair_index: entry.pairIndex,
      position_in_pair: entry.positionInPair,
      warmup: entry.warmup,
      started_at: startedAtIso,
      finished_at: finishedAtIso,
      ...result,
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
    const runValid = failed.length === 0 && successful.length === SAMPLE_COUNT;
    results.push({
      candidate: candidate.name,
      has_wasm: candidate.hasWasm,
      engine_probe_built_artifact_hash: candidate.probeBuiltArtifactHash,
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
      budget_ms: 250,
      budget_status: runValid && p95 !== null ? (p95 <= 250 ? "passed" : "failed") : "invalid",
      wasm_segments_by_sample: measured.map((r) => ({
        global_index: r.global_index,
        ok: r.ok,
        wasm_segments: r.reasons && r.reasons.wasmCheck ? r.reasons.wasmCheck.trace : null,
      })),
      runs,
    });
  }

  const chromiumVersionProbe = await (async () => {
    const b = await puppeteer.launch({ executablePath: CHROMIUM_PATH, headless: true, args: ["--no-sandbox"] });
    const v = await b.version();
    await b.close();
    return v;
  })();

  const toolchainsEqual = CANDIDATES.every(
    (c) => JSON.stringify(c.toolchainVersions) === JSON.stringify(CANDIDATES[0].toolchainVersions),
  );

  const report = {
    measurement_protocol_id: MEASUREMENT_PROTOCOL_ID,
    protocol_status_note:
      "ADR-0015 is status=Proposed as of this run (revision R7; this metric's adapter-API-only / vendor-exclusion design dates to R4 and was independently CONFIRMED correct through the predregistration rounds, no functional changes to this file since). This evidence is protocol-conformant but MUST NOT be used to decide any hard gate until the ADR is Accepted.",
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
      note: "hash_verification_this_run is this actual run's machine-executed check result (verifyPreregistrationHashes(), lib/common.mjs), computed at process start before any browser/timing work -- not merely a claim.",
    },
    metric: "engine_runtime_ready_ms_p95",
    ready_postconditions_used: "1-4 only, ALL redefined as adapter-API-only CRDT operations (ADR-0015 R4 3.2 item 4) -- no DOM, no ProseMirror, no EditorView anywhere in the timed window. NOT postcondition 5 (mount+rAF), which remains flow_route_cold_load's alone.",
    module_graph_check: moduleGraphCheck,
    engine_probe_dependency_identity: dependencyIdentity,
    run_id: RUN_ID,
    source_commit: gitSourceCommit(REPO_ROOT),
    shared_runner_hash: hashRunnerFiles(HERE),
    frozen_probe_spec_sha256: FROZEN_PROBE_SPEC_SHA256,
    server_config_hash: null, // this metric does not use gzip-static-server.mjs (no real network I/O by design)
    browser_version: chromiumVersionProbe,
    toolchain_versions_by_candidate: Object.fromEntries(CANDIDATES.map((c) => [c.name, c.toolchainVersions])),
    toolchain_versions_equal_across_candidates: toolchainsEqual,
    p95_algorithm: "nearest-rank, 1-based ceil(0.95*n), no interpolation",
    warmup_count: WARMUP_COUNT,
    sample_count_target: SAMPLE_COUNT,
    scheduling: "per-sample-pair alternating (AB, BA, AB, BA, ...), see lib/common.mjs buildAlternatingPairSchedule; global_index/pair_index/position_in_pair/started_at/finished_at recorded per sample below.",
    fresh_realm_strategy: "new puppeteer.launch() process per warmup/sample, closed after each sample",
    read_write_probe: {
      expected_full_hash_hex: expected.fullHex,
      expected_canonical_json: expected.canonicalJson,
      description: "SHA-256 of a fixed structural block array (id/type/parent/index/text) read back EXCLUSIVELY via the candidate's own CRDT container API (LoroMap/LoroText or Y.Map/Y.Text) -- no DOM involved at all in this metric. See probe/coldstart-probe-v1.json.",
    },
    fixture_text: FIXTURE_TEXT,
    probe_text: PROBE_TEXT,
    results,
  };

  const outPath = join(OUT_DIR, "engine-runtime-ready-result.json");
  writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(`wrote ${outPath}`);
  for (const r of results) {
    console.log(
      `${r.candidate}: p95=${r.p95_ms}ms budget=${r.budget_ms}ms status=${r.budget_status} success=${r.sample_count_successful}/${r.sample_count_attempted}`,
    );
  }
}

await main();

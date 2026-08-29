// ADR-0015 R4 / coordinator directive 2026-08-29 item 2: "构建产物的模块图里 ProseMirror 相关模块数
// 必须为 0，由检查器验证而不是靠人看". This is that checker.
//
// Mechanism: runs Vite's JS build API (`write: false`, in-memory) against each candidate's
// dedicated `engine_runtime_ready` entry config (vite.config.loro.mjs / vite.config.yrs-yjs.mjs),
// then reads Rollup's own per-chunk `modules` record -- the actual resolved, transitive module
// graph the bundler walked, not a text grep of the output (grep can both false-negative on
// minified/renamed identifiers and false-positive on coincidental substring matches; the module
// graph's file paths are the bundler's ground truth for "what got imported"). Every module id is
// checked against a frozen deny-pattern covering every ProseMirror-family package used anywhere in
// this repo's two real candidates (prosemirror-model/state/view/transform/commands,
// loro-prosemirror, y-prosemirror, orderedmap -- prosemirror-model's own transitive dependency).
//
// Exported as `checkModuleGraph()` so `measure-engine-runtime-ready.mjs` can run this check INLINE
// at the start of every real run (fail-closed BEFORE any timing sample is collected, per
// coordinator directive: "由检查器验证...在哪一步拒绝" -- the answer is: at run start, every run,
// not just this file's own standalone invocation). Also runnable standalone:
// `bun run probe/check-module-graph.mjs` -- exit code 0 + JSON `{ ok: true, ... }` to stdout on
// success, exit code 1 + `{ ok: false, ... }` (violations listed) on failure, result also written
// to `probe/module-graph-check-result.json`.
import { build } from "vite";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, dirname } from "node:path";
import loroConfig from "./vite.config.loro.mjs";
import yrsYjsConfig from "./vite.config.yrs-yjs.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

// Frozen deny-list: any resolved module id containing one of these (case-insensitive) fails the
// check. Covers the package names, not just the string "prosemirror", so a differently-named
// wrapper around the same code cannot slip through undetected -- though "prosemirror" itself is
// also included as a catch-all since every one of these packages' own directory name contains it.
const DENY_PATTERNS = [/prosemirror/i, /orderedmap/i];

const CANDIDATES = [
  { name: "loro", config: loroConfig },
  { name: "yrs-yjs", config: yrsYjsConfig },
];

async function checkOne(candidate) {
  const result = await build({
    ...candidate.config,
    configFile: false,
    logLevel: "silent",
    build: { ...candidate.config.build, write: false },
  });
  const outputs = Array.isArray(result) ? result : [result];
  const moduleIds = new Set();
  for (const out of outputs) {
    for (const chunkOrAsset of out.output) {
      if (chunkOrAsset.type !== "chunk") continue;
      for (const id of Object.keys(chunkOrAsset.modules)) moduleIds.add(id);
    }
  }
  const sortedModuleIds = [...moduleIds].sort();
  const violations = sortedModuleIds.filter((id) => DENY_PATTERNS.some((p) => p.test(id)));
  return {
    candidate: candidate.name,
    module_count: sortedModuleIds.length,
    module_ids: sortedModuleIds,
    deny_patterns: DENY_PATTERNS.map((p) => p.source),
    violations,
    ok: violations.length === 0,
  };
}

export async function checkModuleGraph() {
  const results = [];
  for (const c of CANDIDATES) results.push(await checkOne(c));
  const ok = results.every((r) => r.ok);
  return { checked_at: new Date().toISOString(), ok, results };
}

async function main() {
  const report = await checkModuleGraph();
  const outPath = join(HERE, "module-graph-check-result.json");
  writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
  if (!report.ok) process.exitCode = 1;
}

// Only auto-run when invoked directly (`bun run probe/check-module-graph.mjs`), not when imported.
if (import.meta.url === `file://${process.argv[1]}`) {
  await main();
}

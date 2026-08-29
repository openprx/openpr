/**
 * v0.3 `vite_bundle_static_deep_route` hard-gate evidence for the `loro` candidate.
 *
 * Runs a real `vite build` of this package's minimal Page/Block entry (`index.html` -> `main.ts`
 * -> dynamic `import("./flow-entry")`), classifies every emitted chunk/asset by reachability from
 * that dynamic import boundary, and gzips the candidate-specific set per
 * `testing/benchmark-spec.md`'s measurement rule: "排除两个候选都共有的 Svelte/ProseMirror/vendor
 * chunk，包含候选专属 engine、binding、WASM 和 glue；保存 metafile、文件清单、逐文件 gzip bytes 与总和".
 *
 * This package has no Svelte at all (that lives in `frontend/`, which this task must not touch);
 * the only shared-and-excluded chunk it can produce is the `vendor-prosemirror` chunk forced by
 * `vite.config.ts`'s `manualChunks` -- `prosemirror-model/state/view` are common to both v0.3
 * engine candidates, so they are excluded from the candidate-specific total the same way the real
 * app's Svelte/ProseMirror chunk would be.
 */
import { gzipSync } from "node:zlib";
import { build } from "vite";
import type { OutputAsset, OutputChunk, RollupOutput } from "rollup";

const VENDOR_CHUNK_PREFIX = "assets/vendor-prosemirror-";
const CANDIDATE = "loro";

function isChunk(item: OutputAsset | OutputChunk): item is OutputChunk {
  return item.type === "chunk";
}

async function main(): Promise<void> {
  const result = await build({ configFile: "./vite.config.ts", logLevel: "warn" });
  const rollupOutput = (Array.isArray(result) ? result[0] : result) as RollupOutput;
  const output = rollupOutput.output;

  const byFileName = new Map<string, OutputAsset | OutputChunk>();
  for (const item of output) {
    byFileName.set(item.fileName, item);
  }

  const mainEntry = output.find((item): item is OutputChunk => isChunk(item) && item.isEntry);
  if (mainEntry === undefined) {
    throw new Error("measure-bundle: no entry chunk found in vite build output");
  }

  // Static import closure of the main entry -- the app shell + whichever routes it statically
  // pulls in (none, here: both routes are behind dynamic import()).
  const mainGraph = new Set<string>();
  (function walkStatic(fileName: string): void {
    if (mainGraph.has(fileName)) return;
    mainGraph.add(fileName);
    const item = byFileName.get(fileName);
    if (item !== undefined && isChunk(item)) {
      for (const dep of item.imports) walkStatic(dep);
    }
  })(mainEntry.fileName);

  // The set of chunks reached only via a dynamic import from the main graph, keyed by their
  // facadeModuleId so we can tell the `flow-entry` root apart from `home-entry`'s.
  const dynamicRoots = [...mainGraph]
    .map((fileName) => byFileName.get(fileName))
    .filter((item): item is OutputChunk => item !== undefined && isChunk(item))
    .flatMap((item) => item.dynamicImports);

  const flowRootFileName = dynamicRoots.find((fileName) => {
    const item = byFileName.get(fileName);
    return item !== undefined && isChunk(item) && item.facadeModuleId?.endsWith("flow-entry.ts") === true;
  });
  const homeRootFileName = dynamicRoots.find((fileName) => {
    const item = byFileName.get(fileName);
    return item !== undefined && isChunk(item) && item.facadeModuleId?.endsWith("home-entry.ts") === true;
  });
  if (flowRootFileName === undefined || homeRootFileName === undefined) {
    throw new Error("measure-bundle: could not locate flow-entry/home-entry dynamic chunk roots");
  }

  // Candidate set: everything statically reachable from the flow-entry root, minus the shared
  // vendor-prosemirror chunk and minus anything already in the main graph.
  const candidateChunkFileNames = new Set<string>();
  const candidateAssetFileNames = new Set<string>();
  (function walkCandidate(fileName: string): void {
    if (candidateChunkFileNames.has(fileName) || mainGraph.has(fileName)) return;
    if (fileName.startsWith(VENDOR_CHUNK_PREFIX)) return;
    candidateChunkFileNames.add(fileName);
    const item = byFileName.get(fileName);
    if (item !== undefined && isChunk(item)) {
      for (const dep of item.imports) walkCandidate(dep);
      for (const assetFileName of item.viteMetadata?.importedAssets ?? []) {
        candidateAssetFileNames.add(assetFileName);
      }
    }
  })(flowRootFileName);

  // Sanity check required by the exclusion rule: home-entry's own dynamic subgraph must be
  // disjoint from the candidate set (the non-Flow route never pulls the engine chunk).
  const homeGraph = new Set<string>();
  (function walkHome(fileName: string): void {
    if (homeGraph.has(fileName)) return;
    homeGraph.add(fileName);
    const item = byFileName.get(fileName);
    if (item !== undefined && isChunk(item)) {
      for (const dep of item.imports) walkHome(dep);
    }
  })(homeRootFileName);
  const homeRouteCarriesEngineChunk = [...homeGraph].some((fileName) => candidateChunkFileNames.has(fileName));

  interface FileEntry {
    readonly path: string;
    readonly kind: "javascript" | "wasm" | "glue";
    readonly raw_bytes: number;
    readonly gzip_bytes: number;
  }

  const files: FileEntry[] = [];
  let totalGzip = 0;

  for (const fileName of [...candidateChunkFileNames, ...candidateAssetFileNames].sort()) {
    const item = byFileName.get(fileName);
    if (item === undefined) continue;
    const raw: Uint8Array = isChunk(item) ? new TextEncoder().encode(item.code) : (item.source as Uint8Array | string) instanceof Uint8Array ? (item.source as Uint8Array) : new TextEncoder().encode(item.source as string);
    const gzipped = gzipSync(raw, { level: 9 });
    const kind: FileEntry["kind"] = fileName.endsWith(".wasm") ? "wasm" : "javascript";
    files.push({ path: fileName, kind, raw_bytes: raw.byteLength, gzip_bytes: gzipped.byteLength });
    totalGzip += gzipped.byteLength;
  }

  const BUDGET_BYTES = 2 * 1024 * 1024;
  const report = {
    candidate: CANDIDATE,
    engine_versions: { "loro-crdt": "1.14.1", "loro-prosemirror": "0.4.3" },
    excluded_shared_chunk_prefixes: [VENDOR_CHUNK_PREFIX],
    main_entry: mainEntry.fileName,
    flow_entry_chunk: flowRootFileName,
    home_entry_chunk: homeRootFileName,
    home_route_carries_engine_chunk: homeRouteCarriesEngineChunk,
    files,
    total_gzip_bytes: totalGzip,
    budget_bytes_max: BUDGET_BYTES,
    budget_status: totalGzip <= BUDGET_BYTES ? "passed" : "failed",
  };

  await Bun.write("evidence/bundle-metafile.json", `${JSON.stringify(rollupOutputSummary(output), null, 2)}\n`);
  await Bun.write("evidence/bundle-report.json", `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
}

function rollupOutputSummary(output: readonly (OutputAsset | OutputChunk)[]): unknown {
  return output.map((item) =>
    isChunk(item)
      ? {
          type: "chunk",
          fileName: item.fileName,
          isEntry: item.isEntry,
          isDynamicEntry: item.isDynamicEntry,
          facadeModuleId: item.facadeModuleId,
          imports: item.imports,
          dynamicImports: item.dynamicImports,
          importedAssets: [...(item.viteMetadata?.importedAssets ?? [])],
          moduleIds: item.moduleIds,
        }
      : { type: "asset", fileName: item.fileName, bytes: (item.source as Uint8Array | string).length },
  );
}

await main();

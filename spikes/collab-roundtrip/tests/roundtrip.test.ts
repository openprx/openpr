import { afterAll, expect, test } from "bun:test";
import { LoroDoc } from "loro-crdt";
import * as Y from "yjs";
import { buildSemanticSnapshotFromLoroDoc } from "../src/loro-reader";
import { canonicalSemanticSnapshotJson, semanticHashOfSnapshot, type SemanticSnapshotJs } from "../src/semantic";
import { buildSemanticSnapshotFromYDoc } from "../src/yrs-reader";

interface RustManifest {
  readonly candidate: string;
  readonly seed: number;
  readonly node_count: number;
  readonly semantic_hash: string;
  readonly snapshot_bytes: number;
  readonly snapshot_path: string;
}

interface RustCanonicalDoc {
  readonly nodes: SemanticSnapshotJs;
}

const FIXTURES = new URL("../fixtures/", import.meta.url);
const EVIDENCE = new URL("../evidence/", import.meta.url);

async function readManifest(candidate: string): Promise<RustManifest> {
  const text = await Bun.file(new URL(`rust_manifest_${candidate}.json`, FIXTURES)).text();
  return JSON.parse(text) as RustManifest;
}

async function readSnapshotBytes(candidate: string): Promise<Uint8Array> {
  const buf = await Bun.file(new URL(`rust_snapshot_${candidate}.bin`, FIXTURES)).arrayBuffer();
  return new Uint8Array(buf);
}

const results: Record<string, unknown> = {};

// -- Serializer fidelity: proves (does not assume) that `canonicalSemanticSnapshotJson` is
// byte-for-byte identical to `SemanticSnapshot::canonical_json`'s real serde_json output, for
// both candidates' real fixture data, before either reader is trusted for anything else.
for (const candidate of ["loro", "yrs-yjs"] as const) {
  test(`canonical JSON serializer is byte-identical to serde_json for ${candidate}`, async () => {
    const rustCanonicalText = await Bun.file(new URL(`rust_semantic_${candidate}.json`, FIXTURES)).text();
    const parsed = JSON.parse(rustCanonicalText) as RustCanonicalDoc;
    const reserialized = canonicalSemanticSnapshotJson(parsed.nodes);

    expect(reserialized).toBe(rustCanonicalText);

    const manifest = await readManifest(candidate);
    const hash = await semanticHashOfSnapshot(parsed.nodes);
    expect(hash).toBe(manifest.semantic_hash);

    results[`${candidate}_serializer_fidelity`] = {
      byte_identical: reserialized === rustCanonicalText,
      hash_from_reserialized_json: hash,
      rust_semantic_hash: manifest.semantic_hash,
    };
  });
}

test("loro: real Rust snapshot bytes import into loro-crdt@1.14.1 and hash to the same value", async () => {
  const manifest = await readManifest("loro");
  const bytes = await readSnapshotBytes("loro");

  const doc = new LoroDoc();
  doc.import(bytes);
  const snapshot = buildSemanticSnapshotFromLoroDoc(doc);
  const hash = await semanticHashOfSnapshot(snapshot);

  expect(Object.keys(snapshot).length).toBe(manifest.node_count);
  expect(hash).toBe(manifest.semantic_hash);

  // Reverse direction payload: real loro-crdt-produced snapshot bytes, written for the Rust-side
  // `roundtrip-export verify loro <file> <expected_hash>` step (see the delivery report).
  const exported = doc.export({ mode: "snapshot" });
  await Bun.write(new URL("ts_export_loro.bin", EVIDENCE), exported);
  doc.free();

  results.loro_rust_to_browser = {
    bytes_in: bytes.length,
    node_count: Object.keys(snapshot).length,
    rust_hash: manifest.semantic_hash,
    browser_reimport_hash: hash,
    matched: hash === manifest.semantic_hash,
    ts_export_bytes: exported.length,
  };
});

test("yrs-yjs: real Rust snapshot bytes import into yjs@13.6.32 and hash to the same value", async () => {
  const manifest = await readManifest("yrs-yjs");
  const bytes = await readSnapshotBytes("yrs-yjs");

  const doc = new Y.Doc();
  Y.applyUpdate(doc, bytes);
  const snapshot = buildSemanticSnapshotFromYDoc(doc);
  const hash = await semanticHashOfSnapshot(snapshot);

  expect(Object.keys(snapshot).length).toBe(manifest.node_count);
  expect(hash).toBe(manifest.semantic_hash);

  const exported = Y.encodeStateAsUpdate(doc);
  await Bun.write(new URL("ts_export_yrs-yjs.bin", EVIDENCE), exported);
  doc.destroy();

  results["yrs-yjs_rust_to_browser"] = {
    bytes_in: bytes.length,
    node_count: Object.keys(snapshot).length,
    rust_hash: manifest.semantic_hash,
    browser_reimport_hash: hash,
    matched: hash === manifest.semantic_hash,
    ts_export_bytes: exported.length,
  };
});

afterAll(async () => {
  await Bun.write(new URL("ts-roundtrip-result.json", EVIDENCE), `${JSON.stringify(results, null, 2)}\n`);
});

import { LoroDoc } from "loro-crdt";
import { buildSemanticSnapshotFromLoroDoc } from "./loro-reader";
import { semanticHashOfSnapshot } from "./semantic";

const [workDir, clientsText] = process.argv.slice(2);
if (workDir === undefined || clientsText === undefined) {
  throw new Error("usage: bun v05-convergence-wasm.ts WORK_DIR CLIENTS");
}
const clients = Number.parseInt(clientsText, 10);
if (!Number.isSafeInteger(clients) || clients <= 0) {
  throw new Error(`invalid client count: ${clientsText}`);
}

const read = async (name: string): Promise<Uint8Array> =>
  new Uint8Array(await Bun.file(`${workDir}/${name}`).arrayBuffer());

const base = await read("base.bin");
const updates = await Promise.all(
  Array.from({ length: clients }, (_, index) => read(`update-${String(index).padStart(2, "0")}.bin`)),
);
const order = [
  ...Array.from({ length: clients }, (_, index) => index).filter((index) => index % 2 === 0).reverse(),
  ...Array.from({ length: clients }, (_, index) => index).filter((index) => index % 2 === 1),
];

const replay = async (indices: readonly number[]): Promise<{ doc: LoroDoc; hash: string; nodes: number }> => {
  const doc = new LoroDoc();
  doc.import(base);
  for (const index of indices) {
    doc.import(updates[index]!);
    doc.import(updates[index]!);
  }
  const semantic = buildSemanticSnapshotFromLoroDoc(doc);
  return { doc, hash: await semanticHashOfSnapshot(semantic), nodes: Object.keys(semantic).length };
};

const merged = await replay(order);
const reverse = await replay(Array.from({ length: clients }, (_, index) => clients - index - 1));
const mutation = await replay(order.slice(0, -1));
const rust = JSON.parse(await Bun.file(`${workDir}/rust-result.json`).text()) as { semantic_hash: string };

const rustMerged = new LoroDoc();
rustMerged.import(await read("rust-merged.bin"));
const rustMergedHash = await semanticHashOfSnapshot(buildSemanticSnapshotFromLoroDoc(rustMerged));

const snapshot = merged.doc.export({ mode: "snapshot" });
await Bun.write(`${workDir}/wasm-merged.bin`, snapshot);
const report = {
  engine: "loro-crdt browser WASM binding",
  clients,
  replay_order: order,
  duplicate_imports_per_update: 1,
  semantic_node_count: merged.nodes,
  semantic_hash: merged.hash,
  reverse_replay_hash: reverse.hash,
  reverse_replay_equal: reverse.hash === merged.hash,
  rust_replay_hash: rust.semantic_hash,
  rust_wasm_hash_equal: rust.semantic_hash === merged.hash,
  rust_snapshot_import_hash: rustMergedHash,
  rust_snapshot_import_equal: rustMergedHash === rust.semantic_hash,
  mutation: {
    kind: "drop_last_replayed_client_update",
    observed_hash: mutation.hash,
    detected: mutation.hash !== merged.hash,
  },
};
await Bun.write(`${workDir}/wasm-result.json`, `${JSON.stringify(report, null, 2)}\n`);

merged.doc.free();
reverse.doc.free();
mutation.doc.free();
rustMerged.free();

if (
  !report.reverse_replay_equal ||
  !report.rust_wasm_hash_equal ||
  !report.rust_snapshot_import_equal ||
  !report.mutation.detected ||
  report.semantic_node_count !== clients
) {
  throw new Error(`v0.5 WASM convergence failed: ${JSON.stringify(report)}`);
}

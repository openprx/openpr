import { sha256Hex } from "@sylvode/collab-shared";

/**
 * TS mirror of `spikes/collab-shared/src/semantic.rs`'s `SemanticNode`/`SemanticSnapshot`.
 *
 * Field NAMES and ORDER below are load-bearing: they must match the Rust struct's declaration
 * order exactly (`parent, order_key, kind, text, properties, deleted`), because
 * `SemanticSnapshot::canonical_json` is a plain `serde_json::to_vec(self)` call, and serde's
 * derived `Serialize` for a struct emits fields in DECLARATION order, not sorted order --
 * `sortKeysDeep`/`canonicalStringify` in `@sylvode/collab-shared`'s `hash.ts` sorts EVERY object
 * key alphabetically, which is the right rule for JSON values in general but the WRONG rule here:
 * it would silently reorder `SemanticNode`'s fields to
 * `deleted, kind, order_key, parent, properties, text` and produce a different hash than Rust's.
 * See `tests/roundtrip.test.ts`'s `serializer produces byte-identical output to serde_json`
 * case, which proves (not assumes) the function below is byte-for-byte compatible with a real
 * `rust_semantic_<candidate>.json` fixture produced by `semantic.rs`'s own `canonical_json()`.
 *
 * The two maps that ARE alphabetically/lexicographically sorted on the Rust side --
 * `SemanticSnapshot.nodes: BTreeMap<NodeId, SemanticNode>` and `SemanticNode.properties:
 * BTreeMap<String, String>` -- are both handled correctly below because every node id and
 * property key in this corpus is a plain ASCII token (see `collab_shared::fixture`'s
 * `SplitMix64::token`/format! calls): JS's default `<`/`>` string comparison orders ASCII
 * code points identically to Rust `str`'s UTF-8-byte `Ord`, so a plain lexicographic
 * `Array.prototype.sort` on those keys matches BTreeMap's iteration order exactly. This would NOT
 * generalize to arbitrary non-ASCII keys (UTF-16 code-unit order and UTF-8 byte order diverge
 * outside the BMP), but no id/key in this fixture ever contains one.
 */
export interface SemanticNodeJs {
  readonly parent: string | null;
  readonly order_key: string;
  readonly kind: string;
  readonly text: string;
  readonly properties: Readonly<Record<string, string>>;
  readonly deleted: boolean;
}

export type SemanticSnapshotJs = Readonly<Record<string, SemanticNodeJs>>;

function jsonEscapeString(value: string): string {
  // Matches serde_json's default (non-`ensure_ascii`) escaping: escape `"`, `\`, and control
  // characters < 0x20 (as \b \f \n \r \t or \u00XX), and pass every other Unicode scalar value
  // through as literal UTF-8 -- which is also exactly what JSON.stringify does for a plain JS
  // string containing no lone surrogates (none of this fixture's fixed ASCII/BMP content does).
  // Reusing JSON.stringify's own string-escaping this way (rather than hand-rolling it) is safe
  // BECAUSE `tests/roundtrip.test.ts` proves the full struct output byte-identical against a real
  // serde_json fixture, including every string field this fixture exercises.
  return JSON.stringify(value);
}

function serializeProperties(properties: Readonly<Record<string, string>>): string {
  const keys = Object.keys(properties).sort();
  const parts = keys.map((key) => `${jsonEscapeString(key)}:${jsonEscapeString(properties[key] ?? "")}`);
  return `{${parts.join(",")}}`;
}

function serializeNode(node: SemanticNodeJs): string {
  const parent = node.parent === null ? "null" : jsonEscapeString(node.parent);
  const fields = [
    `"parent":${parent}`,
    `"order_key":${jsonEscapeString(node.order_key)}`,
    `"kind":${jsonEscapeString(node.kind)}`,
    `"text":${jsonEscapeString(node.text)}`,
    `"properties":${serializeProperties(node.properties)}`,
    `"deleted":${node.deleted ? "true" : "false"}`,
  ];
  return `{${fields.join(",")}}`;
}

/**
 * Byte-for-byte equivalent of `SemanticSnapshot::canonical_json` for the same logical snapshot:
 * `{"nodes":{<node id, BTreeMap-sorted>:{<SemanticNode fields, declaration order>}, ...}}`.
 */
export function canonicalSemanticSnapshotJson(snapshot: SemanticSnapshotJs): string {
  const ids = Object.keys(snapshot).sort();
  const parts = ids.map((id) => `${jsonEscapeString(id)}:${serializeNode(snapshot[id] as SemanticNodeJs)}`);
  return `{"nodes":{${parts.join(",")}}}`;
}

/** SHA-256 hex digest of `canonicalSemanticSnapshotJson(snapshot)`, matching `SemanticSnapshot::semantic_hash`. */
export async function semanticHashOfSnapshot(snapshot: SemanticSnapshotJs): Promise<string> {
  const json = canonicalSemanticSnapshotJson(snapshot);
  return sha256Hex(new TextEncoder().encode(json) as Uint8Array<ArrayBuffer>);
}

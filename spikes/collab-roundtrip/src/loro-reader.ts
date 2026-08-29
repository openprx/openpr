import type { LoroDoc, LoroTreeNode } from "loro-crdt";
import type { SemanticNodeJs, SemanticSnapshotJs } from "./semantic";

const TREE_CONTAINER_NAME = "tree";
const META_LOGICAL_ID = "logical_id";
const META_KIND = "kind";
const META_TEXT = "text";
const META_PROPERTY_PREFIX = "prop:";

/**
 * Rebuilds the same engine-independent `SemanticSnapshot` shape `collab-loro`'s Rust adapter
 * (`spikes/collab-loro/src/engine.rs::semantic_snapshot`) produces, but reading a real
 * `loro-crdt@1.14.1` `LoroDoc` (the browser/JS binding) instead of the Rust `loro@1.13.9` crate.
 *
 * Deliberately does NOT reuse `LoroScenarioAdapter` (`spikes/collab-loro/src/engine.ts`): that
 * class is the TS-only corpus-runner adapter and stores the external id under a DIFFERENT meta
 * key (`"externalId"`) than the Rust adapter (`"logical_id"`) -- the two were never meant to
 * read each other's documents. This reader instead mirrors the Rust adapter's exact container
 * name (`"tree"`) and meta keys (`logical_id`/`kind`/`text`/`prop:<key>`) directly, because that
 * is the wire shape a real Rust-exported snapshot/update actually contains.
 */
export function buildSemanticSnapshotFromLoroDoc(doc: LoroDoc): SemanticSnapshotJs {
  const tree = doc.getTree(TREE_CONTAINER_NAME);
  const nodes = tree.nodes();

  const idOf = (node: LoroTreeNode): string | undefined => {
    const raw = node.data.get(META_LOGICAL_ID);
    return typeof raw === "string" ? raw : undefined;
  };

  const out: Record<string, SemanticNodeJs> = {};

  for (const node of nodes) {
    const logicalId = idOf(node);
    if (logicalId === undefined) {
      // Mirrors the Rust adapter's `rebuild_id_cache`/`semantic_snapshot`: a tree node this
      // replica cannot resolve a logical id for is skipped rather than surfaced with a
      // synthetic/engine-native id.
      continue;
    }

    const parentNode = node.parent();
    const parentLogicalId = parentNode === undefined ? undefined : idOf(parentNode);

    // Rust's `order_key_for` is `format!("{position:08}")`, where `position` is this node's
    // index within `self.tree.children(parent)` -- i.e. computed at read time from live sibling
    // order, never stored. `roots()`/`children()` below are the JS binding's equivalent of that
    // same underlying `loro` core call.
    const siblings: readonly LoroTreeNode[] = parentNode === undefined ? tree.roots() : (parentNode.children() ?? []);
    const position = siblings.findIndex((sibling) => sibling.id === node.id);
    const orderKey = String(Math.max(position, 0)).padStart(8, "0");

    const kindRaw = node.data.get(META_KIND);
    const kind = typeof kindRaw === "string" ? kindRaw : "block";

    const textHandle: unknown = node.data.get(META_TEXT);
    const text =
      textHandle !== undefined && textHandle !== null && typeof (textHandle as { toString?: unknown }).toString === "function"
        ? String(textHandle)
        : "";

    const properties: Record<string, string> = {};
    for (const [key, value] of node.data.entries()) {
      if (key.startsWith(META_PROPERTY_PREFIX) && typeof value === "string") {
        properties[key.slice(META_PROPERTY_PREFIX.length)] = value;
      }
    }

    out[logicalId] = {
      parent: parentLogicalId ?? null,
      order_key: orderKey,
      kind,
      text,
      properties,
      deleted: node.isDeleted(),
    };
  }

  return out;
}

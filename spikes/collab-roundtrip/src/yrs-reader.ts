import * as Y from "yjs";
import type { SemanticNodeJs, SemanticSnapshotJs } from "./semantic";

const NODES_MAP = "nodes";
const FIELD_PARENT = "parent";
const FIELD_ORDER = "order";
const FIELD_KIND = "kind";
const FIELD_DELETED = "deleted";
const FIELD_TEXT = "text";
const PROPERTY_PREFIX = "prop:";

/**
 * Rebuilds the same engine-independent `SemanticSnapshot` shape `collab-yrs-yjs`'s Rust adapter
 * (`spikes/collab-yrs-yjs/src/engine.rs::semantic_snapshot`) produces, but reading a real
 * `yjs@13.6.32` `Y.Doc` (the browser/JS binding) instead of the Rust `yrs@0.27.3` crate.
 *
 * Deliberately does NOT reuse `YjsScenarioAdapter` (`spikes/collab-yrs-yjs/src/engine.ts`): that
 * class is the TS-only corpus-runner adapter and its tree schema only carries a `parentId` field
 * (no `order`/`kind`/`deleted`/`text`/`prop:*`) -- it was never meant to read a Rust-produced
 * document. This reader instead mirrors the Rust adapter's exact top-level container name
 * (`"nodes"`) and per-node field names directly, because that is the wire shape a real
 * Rust-exported snapshot/update actually contains.
 *
 * Unlike the Rust adapter, this does not implement `break_cycles`: that pass only matters after a
 * concurrent multi-replica merge that leaves the raw parent-pointer graph with a cycle, which the
 * single-replica fixture this package's roundtrip test drives (see `tests/roundtrip.test.ts`)
 * cannot produce. Documented here rather than silently reproduced, per this task's honesty
 * requirement -- see the delivery report's "known simplifications" section.
 */
export function buildSemanticSnapshotFromYDoc(doc: Y.Doc): SemanticSnapshotJs {
  const nodes = doc.getMap<Y.Map<unknown>>(NODES_MAP);
  const out: Record<string, SemanticNodeJs> = {};

  for (const [id, value] of nodes.entries()) {
    if (!(value instanceof Y.Map)) {
      continue;
    }
    const nodeMap = value;

    const parentRaw = nodeMap.get(FIELD_PARENT);
    const parent = typeof parentRaw === "string" && parentRaw.length > 0 ? parentRaw : null;

    const orderRaw = nodeMap.get(FIELD_ORDER);
    const orderKey = typeof orderRaw === "string" ? orderRaw : "";

    const kindRaw = nodeMap.get(FIELD_KIND);
    const kind = typeof kindRaw === "string" ? kindRaw : "block";

    const deletedRaw = nodeMap.get(FIELD_DELETED);
    const deleted = deletedRaw === true;

    const textRaw = nodeMap.get(FIELD_TEXT);
    const text = textRaw instanceof Y.Text ? textRaw.toString() : "";

    const properties: Record<string, string> = {};
    for (const [key, fieldValue] of nodeMap.entries()) {
      if (key.startsWith(PROPERTY_PREFIX) && typeof fieldValue === "string") {
        properties[key.slice(PROPERTY_PREFIX.length)] = fieldValue;
      }
    }

    out[id] = { parent, order_key: orderKey, kind, text, properties, deleted };
  }

  return out;
}

import * as Y from "yjs";
import type { AbstractOp, CandidateName, EngineDiff, Frontier, ScenarioAdapter } from "@sylvode/collab-shared";

const textContainerName = (container: string): string => `text:${container}`;
const TREE_CONTAINER_NAME = "tree";
const PARENT_ID_FIELD = "parentId";

/**
 * Real yjs@13.6.32 implementation of the shared EngineAdapter +
 * ScenarioAdapter contracts (spikes/collab-shared/src/testing.ts). The
 * moveable tree has no native CRDT type in Yjs (unlike Loro's LoroTree), so
 * per ADR-0006 it is an application-layer schema: a root Y.Map("tree") of
 * nodeId -> Y.Map({parentId}), where `parentId` is a plain last-write-wins
 * Yjs Map field. For the single-node-reparented-concurrently shape this
 * candidate's corpus cases exercise, LWW on one field is sufficient to
 * converge without a cycle -- see the delivery report for why a general
 * cycle-repair pass (needed only when two DIFFERENT nodes' parent pointers
 * could form a loop, e.g. a concurrent parent swap) is out of this round's
 * scope.
 */
export class YjsScenarioAdapter implements ScenarioAdapter {
  readonly candidate: CandidateName = "yrs-yjs";
  private doc = new Y.Doc();
  private lastChangedContainers = new Set<string>();

  constructor() {
    this.attachChangeTracking();
  }

  // -- EngineAdapter --------------------------------------------------------

  async load(snapshot: Uint8Array): Promise<void> {
    this.doc.destroy();
    this.doc = new Y.Doc();
    this.attachChangeTracking();
    Y.applyUpdate(this.doc, snapshot);
  }

  async importUpdate(update: Uint8Array): Promise<EngineDiff> {
    this.lastChangedContainers = new Set();
    Y.applyUpdate(this.doc, update);
    return { changedContainerIds: [...this.lastChangedContainers] };
  }

  async exportSnapshot(): Promise<Uint8Array> {
    return Y.encodeStateAsUpdate(this.doc);
  }

  async exportFrom(frontier: Frontier): Promise<Uint8Array> {
    return Y.encodeStateAsUpdate(this.doc, frontier.bytes);
  }

  frontier(): Frontier {
    return { bytes: Y.encodeStateVector(this.doc) };
  }

  async dispose(): Promise<void> {
    this.doc.destroy();
  }

  // -- ScenarioAdapter --------------------------------------------------------

  async applyLocalOp(op: AbstractOp): Promise<Uint8Array> {
    const before = this.frontier();
    this.doc.transact(() => {
      switch (op.kind) {
        case "text.insert":
          this.doc.getText(textContainerName(op.container)).insert(op.index, op.text);
          break;
        case "text.delete":
          this.doc.getText(textContainerName(op.container)).delete(op.index, op.length);
          break;
        case "tree.createNode": {
          const tree = this.doc.getMap<Y.Map<unknown>>(TREE_CONTAINER_NAME);
          const node = new Y.Map<unknown>();
          node.set(PARENT_ID_FIELD, op.parentId);
          tree.set(op.nodeId, node);
          break;
        }
        case "tree.move": {
          const node = this.requireNode(op.nodeId);
          node.set(PARENT_ID_FIELD, op.parentId);
          break;
        }
      }
    });
    return this.exportFrom(before);
  }

  toDebugJson(): unknown {
    // A root type that arrived only via a remote update (never through this
    // replica's own doc.getText/getMap) sits in doc.share as a generic,
    // unspecialized AbstractType until something asks for it by name+kind --
    // AbstractType#toJSON returns undefined, unlike YText/YMap#toJSON. So
    // this must re-fetch each root by the type this adapter itself defines
    // it as, not just iterate doc.share and call .toJSON() on whatever is there.
    const out: Record<string, unknown> = {};
    for (const name of this.doc.share.keys()) {
      out[name] = name === TREE_CONTAINER_NAME ? this.doc.getMap(name).toJSON() : this.doc.getText(name).toString();
    }
    return out;
  }

  // -- internal ---------------------------------------------------------------

  private attachChangeTracking(): void {
    this.doc.on("afterTransaction", (transaction) => {
      for (const type of transaction.changed.keys()) {
        try {
          this.lastChangedContainers.add(Y.findRootTypeKey(type));
        } catch {
          // type was never attached to a root container (e.g. already detached); nothing to report.
        }
      }
    });
  }

  private requireNode(externalId: string): Y.Map<unknown> {
    const tree = this.doc.getMap<Y.Map<unknown>>(TREE_CONTAINER_NAME);
    const node = tree.get(externalId);
    if (node === undefined) {
      throw new Error(`yjs adapter: unknown external node id "${externalId}"`);
    }
    return node;
  }

  /** Test/debug helper: parent's external id for a tracked node (null = root, undefined = unknown/absent). Not part of ScenarioAdapter -- used directly by this candidate's own tree-shape assertions. */
  parentExternalIdOf(nodeId: string): string | null | undefined {
    const tree = this.doc.getMap<Y.Map<unknown>>(TREE_CONTAINER_NAME);
    const node = tree.get(nodeId);
    if (node === undefined) return undefined;
    const parentId = node.get(PARENT_ID_FIELD);
    return parentId === null || typeof parentId === "string" ? (parentId as string | null) : undefined;
  }

  /** Test/debug helper: all tracked node ids present in the tree map. */
  liveNodeIds(): readonly string[] {
    const tree = this.doc.getMap<Y.Map<unknown>>(TREE_CONTAINER_NAME);
    return [...tree.keys()];
  }
}

import { LoroDoc, type LoroTreeNode, type TreeID, decodeFrontiers, encodeFrontiers } from "loro-crdt";
import type { AbstractOp, CandidateName, EngineDiff, Frontier, ScenarioAdapter } from "@sylvode/collab-shared";

const textContainerName = (container: string): string => `text:${container}`;
const TREE_CONTAINER_NAME = "tree";
const EXTERNAL_ID_KEY = "externalId";

/**
 * Real loro-crdt@1.14.1 implementation of the shared EngineAdapter +
 * ScenarioAdapter contracts (spikes/collab-shared/src/testing.ts). Every
 * method below calls a real LoroDoc / LoroText / LoroTree API -- see the
 * delivery report for the exact API surface used and where it was verified
 * against the installed loro-crdt/bundler/loro_wasm.d.ts.
 */
export class LoroScenarioAdapter implements ScenarioAdapter {
  readonly candidate: CandidateName = "loro";
  private doc = new LoroDoc();
  /** external (fixture-chosen) node id -> Loro's own TreeID. Rebuilt from document content on load()/importUpdate() so a replica that received its tree via sync (not local createNode calls) still resolves ids. */
  private externalIdToTreeId = new Map<string, TreeID>();
  private lastChangedContainers = new Set<string>();

  constructor() {
    this.doc.subscribe((batch) => {
      for (const event of batch.events) {
        this.lastChangedContainers.add(event.target);
      }
    });
  }

  // -- EngineAdapter --------------------------------------------------------

  async load(snapshot: Uint8Array): Promise<void> {
    this.doc.import(snapshot);
    this.refreshExternalIdIndex();
  }

  async importUpdate(update: Uint8Array): Promise<EngineDiff> {
    this.lastChangedContainers = new Set();
    this.doc.import(update);
    this.refreshExternalIdIndex();
    return { changedContainerIds: [...this.lastChangedContainers] };
  }

  async exportSnapshot(): Promise<Uint8Array> {
    return this.doc.export({ mode: "snapshot" });
  }

  async exportFrom(frontier: Frontier): Promise<Uint8Array> {
    const frontiers = decodeFrontiers(frontier.bytes);
    const from = this.doc.frontiersToVV(frontiers);
    return this.doc.export({ mode: "update", from });
  }

  frontier(): Frontier {
    return { bytes: encodeFrontiers(this.doc.frontiers()) };
  }

  async dispose(): Promise<void> {
    this.doc.free();
  }

  // -- ScenarioAdapter --------------------------------------------------------

  async applyLocalOp(op: AbstractOp): Promise<Uint8Array> {
    const before = this.frontier();
    switch (op.kind) {
      case "text.insert":
        this.doc.getText(textContainerName(op.container)).insert(op.index, op.text);
        break;
      case "text.delete":
        this.doc.getText(textContainerName(op.container)).delete(op.index, op.length);
        break;
      case "tree.createNode": {
        const tree = this.doc.getTree(TREE_CONTAINER_NAME);
        const parentTreeId = op.parentId === null ? undefined : this.requireTreeId(op.parentId);
        const node = tree.createNode(parentTreeId);
        node.data.set(EXTERNAL_ID_KEY, op.nodeId);
        this.externalIdToTreeId.set(op.nodeId, node.id);
        break;
      }
      case "tree.move": {
        const tree = this.doc.getTree(TREE_CONTAINER_NAME);
        const targetTreeId = this.requireTreeId(op.nodeId);
        const parentTreeId = op.parentId === null ? undefined : this.requireTreeId(op.parentId);
        tree.move(targetTreeId, parentTreeId);
        break;
      }
    }
    this.doc.commit();
    return this.exportFrom(before);
  }

  toDebugJson(): unknown {
    return this.doc.toJSON();
  }

  // -- internal ---------------------------------------------------------------

  private requireTreeId(externalId: string): TreeID {
    const treeId = this.externalIdToTreeId.get(externalId);
    if (treeId === undefined) {
      throw new Error(`loro adapter: unknown external node id "${externalId}"`);
    }
    return treeId;
  }

  /** Rebuilds the external-id -> TreeID index from document content: needed because a replica that received the tree via load()/importUpdate() (rather than local createNode ops) has no local record of the mapping. */
  private refreshExternalIdIndex(): void {
    const tree = this.doc.getTree(TREE_CONTAINER_NAME);
    for (const node of tree.getNodes() as LoroTreeNode[]) {
      if (node.isDeleted()) continue;
      const externalId = node.data.get(EXTERNAL_ID_KEY);
      if (typeof externalId === "string") {
        this.externalIdToTreeId.set(externalId, node.id);
      }
    }
  }

  /** Test/debug helper: parent's external id for a tracked node (null = root, undefined = unknown/deleted). Not part of ScenarioAdapter -- used directly by this candidate's own tree-shape assertions. */
  parentExternalIdOf(nodeId: string): string | null | undefined {
    const tree = this.doc.getTree(TREE_CONTAINER_NAME);
    const treeId = this.externalIdToTreeId.get(nodeId);
    if (treeId === undefined) return undefined;
    const node = tree.getNodeByID(treeId);
    if (node === undefined || node.isDeleted()) return undefined;
    const parent = node.parent();
    if (parent === undefined) return null;
    const parentExternalId = parent.data.get(EXTERNAL_ID_KEY);
    return typeof parentExternalId === "string" ? parentExternalId : undefined;
  }

  /** Test/debug helper: current text of a block. Not part of ScenarioAdapter. */
  readText(container: string): string {
    return this.doc.getText(textContainerName(container)).toString();
  }

  /** Test/debug helper: all tracked node ids still live in the tree. */
  liveNodeIds(): readonly string[] {
    const tree = this.doc.getTree(TREE_CONTAINER_NAME);
    const ids: string[] = [];
    for (const [externalId, treeId] of this.externalIdToTreeId) {
      const node = tree.getNodeByID(treeId);
      if (node !== undefined && !node.isDeleted()) {
        ids.push(externalId);
      }
    }
    return ids;
  }
}

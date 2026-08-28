import type { CandidateName, EngineDiff, Frontier } from "../../src/contracts";
import type { AbstractOp, ScenarioAdapter } from "../../src/testing";

/**
 * Minimal, self-contained operation-log "engine": semantic state is the
 * canonicalized, deduplicated, id-sorted set of applied operations, not a
 * real text/tree merge. This exercises the SHARED corpus runner's own
 * control flow (exchange order, idempotent duplicate-apply, malformed-input
 * rejection) in isolation from any specific real CRDT -- real engine
 * correctness (actual text/tree convergence) is covered by
 * spikes/collab-loro and spikes/collab-yrs-yjs's own tests against the real
 * engines. No `Math.random()`: peer/op ids are assigned from a module-level
 * counter, which keeps every run of these tests fully deterministic too.
 */

type LoggedOp = Readonly<{ id: string; op: AbstractOp }>;

let nextPeerId = 0;

export class FakeScenarioAdapter implements ScenarioAdapter {
  readonly candidate: CandidateName = "loro";
  private readonly peerId = `peer-${(nextPeerId += 1)}`;
  private opCounter = 0;
  private readonly appliedIds = new Set<string>();
  private ops: LoggedOp[] = [];

  async load(snapshot: Uint8Array): Promise<void> {
    const parsed = decode(snapshot) as readonly LoggedOp[];
    this.ops = [];
    this.appliedIds.clear();
    for (const entry of parsed) {
      this.ops.push(entry);
      this.appliedIds.add(entry.id);
    }
  }

  async importUpdate(update: Uint8Array): Promise<EngineDiff> {
    const parsed = decode(update) as readonly LoggedOp[];
    const changed: string[] = [];
    for (const entry of parsed) {
      if (!this.appliedIds.has(entry.id)) {
        this.ops.push(entry);
        this.appliedIds.add(entry.id);
        changed.push(entry.op.kind);
      }
    }
    return { changedContainerIds: changed };
  }

  async exportSnapshot(): Promise<Uint8Array> {
    return encode(this.ops);
  }

  async exportFrom(frontier: Frontier): Promise<Uint8Array> {
    const known = new Set(decode(frontier.bytes) as readonly string[]);
    return encode(this.ops.filter((entry) => !known.has(entry.id)));
  }

  frontier(): Frontier {
    return { bytes: encode([...this.appliedIds].sort()) };
  }

  async dispose(): Promise<void> {
    this.ops = [];
    this.appliedIds.clear();
  }

  async applyLocalOp(op: AbstractOp): Promise<Uint8Array> {
    this.opCounter += 1;
    const entry: LoggedOp = { id: `${this.peerId}:${this.opCounter}`, op };
    this.ops.push(entry);
    this.appliedIds.add(entry.id);
    return encode([entry]);
  }

  toDebugJson(): unknown {
    return [...this.ops].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  }
}

function encode(value: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(value));
}

function decode(bytes: Uint8Array): unknown {
  // JSON.parse throws on truncated/malformed input -- this is the fake's rejection path,
  // exercised by the shared runner's `adversarial_update_limits` case.
  return JSON.parse(new TextDecoder().decode(bytes));
}

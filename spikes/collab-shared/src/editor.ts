import type { EngineDiff } from "./contracts";

export type RelativeSelection = Readonly<{
  anchor: Uint8Array;
  head: Uint8Array;
}>;

export type RichTextCommand = Readonly<{
  name: string;
  payload: unknown;
}>;

export type EditorOrigin = Readonly<{
  surface: "web";
  sessionId: string;
  transactionId: string;
}>;

/**
 * Candidate-agnostic ProseMirror binding boundary (ADR-0006: both candidates
 * share the same ProseMirror core, loro-prosemirror / y-prosemirror only
 * adapt to it). Moved out of the individual spikes so neither directory can
 * drift from the other's shape.
 */
export interface EditorAdapter {
  mount(host: HTMLElement, blockId: string): Promise<void>;
  applyRemote(change: EngineDiff): void;
  transact(command: RichTextCommand, origin: EditorOrigin): Uint8Array;
  getSelection(): RelativeSelection | null;
  restoreSelection(selection: RelativeSelection): void;
  undoLocal(): boolean;
  redoLocal(): boolean;
  destroy(): Promise<void>;
}

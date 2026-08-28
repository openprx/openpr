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

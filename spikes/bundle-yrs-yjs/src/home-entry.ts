/**
 * See `bundle-loro/src/home-entry.ts` for the full rationale -- identical shell, other candidate.
 * Imports nothing from `yjs`/`y-prosemirror`/`prosemirror-*`.
 */
export function mountHome(target: HTMLElement): void {
  target.textContent = "bundle-yrs-yjs-spike: home route (no CRDT engine loaded here)";
  target.dataset.route = "home";
}

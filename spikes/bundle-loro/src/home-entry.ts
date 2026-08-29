/**
 * The non-Flow route. Deliberately imports nothing from `loro-crdt`/`loro-prosemirror`/
 * `prosemirror-*`: this file's own module graph is what `measure-bundle.ts` treats as "the main
 * entry graph" to exclude from the candidate bundle budget, and its absence of any engine import
 * is what proves (by construction, and re-checked by the network-request assertion in
 * `verify-route-split.ts`) that a non-Flow route's bundle never contains the engine chunk.
 */
export function mountHome(target: HTMLElement): void {
  target.textContent = "bundle-loro-spike: home route (no CRDT engine loaded here)";
  target.dataset.route = "home";
}

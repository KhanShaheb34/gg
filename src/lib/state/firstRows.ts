// The first rows of a repository are on screen: the window has settled, and work that must
// not slow the start (looking for a new version of gg) may begin. Said once per page load.

let shown = false;
const waiting = new Set<() => void>();

/** Says the first rows are on screen (the graph, on its first paint of real rows). */
export function firstRowsShown(): void {
  if (shown) return;
  shown = true;
  for (const run of [...waiting]) run();
  waiting.clear();
}

/** Runs `run` once the first rows are on screen (now, if they are); returns a stop function. */
export function whenFirstRowsShown(run: () => void): () => void {
  if (shown) {
    run();
    return () => {};
  }
  waiting.add(run);
  return () => waiting.delete(run);
}

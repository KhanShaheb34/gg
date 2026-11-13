// What a key does on the commit grid: which row it selects, and where a page key scrolls to.
// Pure; the graph view applies the result.

export type GridState = {
  /** Rows the grid shows. */
  count: number;
  /** Height of the row area, px. */
  viewportH: number;
  scrollTop: number;
  /** The selected row, or null. */
  current: number | null;
  rowH: number;
};

/** A row to select (null: clear the selection), and a scroll position to move to first. */
export type GridMove = { select: number | null; scrollTop?: number };

/**
 * The move key `key` makes: arrows step a row (from the first visible row when nothing is
 * selected), Page Up/Down a screen less a row (scrolling with it), Home/End to the ends,
 * Escape clears the selection. Null for other keys, an empty grid, or Escape with nothing
 * selected.
 */
export function gridKey(key: string, g: GridState): GridMove | null {
  if (g.count === 0) return null;
  const clamp = (row: number) => Math.min(g.count - 1, Math.max(0, row));
  const firstVisible = Math.min(g.count - 1, Math.ceil(g.scrollTop / g.rowH));
  const from = g.current ?? firstVisible;
  switch (key) {
    case "ArrowDown":
      return { select: clamp(g.current === null ? firstVisible : g.current + 1) };
    case "ArrowUp":
      return { select: clamp(g.current === null ? firstVisible : g.current - 1) };
    case "PageDown":
    case "PageUp": {
      const dir = key === "PageDown" ? 1 : -1;
      const pageRows = Math.max(1, Math.floor(g.viewportH / g.rowH) - 1);
      const maxTop = Math.max(0, g.count * g.rowH - g.viewportH);
      return {
        select: clamp(from + dir * pageRows),
        scrollTop: Math.min(maxTop, Math.max(0, g.scrollTop + dir * pageRows * g.rowH)),
      };
    }
    case "Home":
      return { select: 0 };
    case "End":
      return { select: g.count - 1 };
    case "Escape":
      return g.current === null ? null : { select: null };
    default:
      return null;
  }
}

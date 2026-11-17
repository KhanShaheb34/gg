// The graph's column widths: the ones the user dragged (shared by every repository, kept in
// localStorage), the defaults for the rest, and the Graph column's own width.

export type Column = "graph" | "date" | "author" | "hash";

const STORAGE_KEY = "gg.columns.v1";
const DEFAULT_W = { date: 140, author: 150, hash: 84 };
export const MIN_W: Record<Column, number> = { graph: 16, date: 72, author: 64, hash: 64 };
export const NAMES: Record<Column, string> = { graph: "Graph", date: "Date", author: "Author", hash: "Commit" };
/** Columns right of the description, sized from their left edge. */
export const RIGHT_COLUMNS: Column[] = ["date", "author", "hash"];
/**
 * The room the description (the subject, after its labels) keeps before other columns give
 * way: below it, Author narrows first, then Date (final boundary review I2).
 */
export const DESCRIPTION_ROOM = 280;

/**
 * `widths` as shown in a grid `available` px wide: when the description would get less than
 * [`DESCRIPTION_ROOM`], Author narrows (to its minimum at most), then Date. Graph and Commit
 * keep theirs; what was set is kept, and comes back when there is room.
 */
export function fitColumns(widths: Record<Column, number>, available: number): Record<Column, number> {
  let short = DESCRIPTION_ROOM - (available - widths.graph - widths.date - widths.author - widths.hash);
  if (short <= 0) return widths;
  const author = Math.max(Math.min(MIN_W.author, widths.author), widths.author - short);
  short -= widths.author - author;
  const date = Math.max(Math.min(MIN_W.date, widths.date), widths.date - short);
  return { ...widths, author, date };
}

function readStored(): Partial<Record<Column, number>> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    const out: Partial<Record<Column, number>> = {};
    if (parsed && typeof parsed === "object") {
      for (const key of Object.keys(MIN_W) as Column[]) {
        const v = (parsed as Record<string, unknown>)[key];
        if (typeof v === "number" && Number.isFinite(v)) out[key] = Math.max(MIN_W[key], Math.round(v));
      }
    }
    return out;
  } catch {
    return {};
  }
}

export class GraphColumns {
  /** Widths the user set. */
  private stored: Partial<Record<Column, number>> = $state(readStored());
  /** The grid's width, px (0 until measured: nothing gives way then). */
  available = $state(0);
  /** Every column's width as set (dragged, or the default), px. */
  private readonly set: Record<Column, number> = $derived.by(() => ({
    graph: this.stored.graph ?? this.autoGraphWidth(),
    date: this.stored.date ?? DEFAULT_W.date,
    author: this.stored.author ?? DEFAULT_W.author,
    hash: this.stored.hash ?? DEFAULT_W.hash,
  }));
  /** Every column's width as shown, px: the description keeps its room (see fitColumns). */
  readonly widths: Record<Column, number> = $derived(this.available > 0 ? fitColumns(this.set, this.available) : this.set);
  /** The grid's `grid-template-columns`. */
  readonly template: string = $derived(
    `${this.widths.graph}px minmax(120px, 1fr) ${this.widths.date}px ${this.widths.author}px ${this.widths.hash}px`,
  );
  private resizeFrom = 0;

  /** `autoGraphWidth`: the Graph column's width when the user has not set one. */
  constructor(private readonly autoGraphWidth: () => number) {}

  /** The Splitter handlers for column `key`, whose right (sign 1) or left (-1) edge moves. */
  resizer(key: Column, sign: 1 | -1) {
    return {
      onstart: () => (this.resizeFrom = this.widths[key]),
      onmove: (dx: number) => (this.stored[key] = Math.max(MIN_W[key], Math.round(this.resizeFrom + sign * dx))),
      onend: () => this.save(),
      onreset: () => {
        delete this.stored[key];
        this.save();
      },
    };
  }

  private save() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(this.stored));
    } catch {
      // Widths then last until the window closes.
    }
  }
}

// The graph's column widths: the ones the user dragged (shared by every repository, kept in
// localStorage), the defaults for the rest, and the Graph column's own width.

export type Column = "graph" | "date" | "author" | "hash";

const STORAGE_KEY = "gg.columns.v1";
const DEFAULT_W = { date: 140, author: 150, hash: 84 };
export const MIN_W: Record<Column, number> = { graph: 16, date: 72, author: 64, hash: 64 };
export const NAMES: Record<Column, string> = { graph: "Graph", date: "Date", author: "Author", hash: "Commit" };
/** Columns right of the description, sized from their left edge. */
export const RIGHT_COLUMNS: Column[] = ["date", "author", "hash"];

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
  /** Every column's width, px. */
  readonly widths: Record<Column, number> = $derived.by(() => ({
    graph: this.stored.graph ?? this.autoGraphWidth(),
    date: this.stored.date ?? DEFAULT_W.date,
    author: this.stored.author ?? DEFAULT_W.author,
    hash: this.stored.hash ?? DEFAULT_W.hash,
  }));
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

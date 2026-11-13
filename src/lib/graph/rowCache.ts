// Rows of one generation of a repository's graph, fetched from the core a page at a time as
// the view needs them, and kept up to a limit (least recently used pages go first).

import type { CommitRow, RowsPage } from "../types";

export type FetchPage = (generation: number, start: number, count: number) => Promise<RowsPage>;

export class RowCache {
  /** Called after a page has arrived. */
  onUpdate: () => void = () => {};
  /** Called with the error of a page request that failed (the page can be asked for again). */
  onError: (error: unknown) => void = () => {};

  private generation = -1;
  private totalRows = 0;
  /** Page number → its rows. Map order is recency: the first entry is the least recently used. */
  private pages = new Map<number, CommitRow[]>();
  /** Page number → the request loading it, for the current generation. */
  private inflight = new Map<number, Promise<void>>();
  /** Pages covering the range of the last `ensure`; never evicted. */
  private wanted = { first: 0, last: -1 };

  constructor(
    private readonly fetchPage: FetchPage,
    private readonly pageSize = 200,
    // The window (about 2 pages), 400 rows (2 pages) of prefetch in the direction of
    // scrolling and a page either side, with room for the pages just left; more only grows
    // the web process's memory.
    private readonly maxPages = 8,
  ) {}

  /**
   * Switches to `generation` with `totalRows` rows. A new generation drops every page and
   * makes the answers to earlier requests be ignored; the same one keeps them (rows were only
   * appended).
   */
  reset(generation: number, totalRows: number): void {
    if (generation !== this.generation) {
      this.generation = generation;
      this.pages.clear();
      this.inflight.clear();
    }
    this.totalRows = totalRows;
  }

  /** Row `index`, if loaded. */
  get(index: number): CommitRow | undefined {
    const page = Math.floor(index / this.pageSize);
    return this.pages.get(page)?.[index - page * this.pageSize];
  }

  /** Whether every row of `first..=last` (clipped to the rows there are) is loaded. */
  loaded(first: number, last: number): boolean {
    const end = Math.min(last, this.totalRows - 1);
    for (let i = Math.max(0, first); i <= end; i++) if (this.get(i) === undefined) return false;
    return true;
  }

  /**
   * Loads the pages covering rows `first..=last` that are missing, plus one page either side.
   * Resolves when they have all arrived (or failed); never rejects.
   */
  ensure(first: number, last: number): Promise<void> {
    const lastRow = Math.min(last, this.totalRows - 1);
    if (this.totalRows <= 0 || lastRow < Math.max(first, 0)) return Promise.resolve();
    const firstPage = Math.floor(Math.max(first, 0) / this.pageSize);
    const lastPage = Math.floor(lastRow / this.pageSize);
    this.wanted = { first: firstPage, last: lastPage };
    const maxPage = Math.floor((this.totalRows - 1) / this.pageSize);
    const waits: Promise<void>[] = [];
    for (let p = Math.max(0, firstPage - 1); p <= Math.min(maxPage, lastPage + 1); p++) {
      const rows = this.pages.get(p);
      if (rows && this.complete(p, rows)) {
        this.pages.delete(p); // move to most recently used
        this.pages.set(p, rows);
        continue;
      }
      waits.push(this.inflight.get(p) ?? this.load(p));
    }
    return waits.length > 0 ? Promise.all(waits).then(() => {}) : Promise.resolve();
  }

  /** Whether `rows` hold every row of page `p` that exists now. */
  private complete(p: number, rows: CommitRow[]): boolean {
    return rows.length === this.pageSize || p * this.pageSize + rows.length >= this.totalRows;
  }

  private load(p: number): Promise<void> {
    const generation = this.generation;
    const request = this.fetchPage(generation, p * this.pageSize, this.pageSize).then(
      (page) => {
        if (this.inflight.get(p) === request) this.inflight.delete(p);
        if (page.generation !== this.generation) return; // answered for rows that are gone
        this.pages.delete(p);
        this.pages.set(p, page.rows);
        this.evict();
        this.onUpdate();
      },
      (error: unknown) => {
        if (this.inflight.get(p) === request) this.inflight.delete(p);
        if (generation === this.generation) this.onError(error);
      },
    );
    this.inflight.set(p, request);
    return request;
  }

  /** Drops least recently used pages, outside the last range asked for, beyond `maxPages`. */
  private evict(): void {
    if (this.pages.size <= this.maxPages) return;
    for (const p of this.pages.keys()) {
      if (this.pages.size <= this.maxPages) break;
      if (p < this.wanted.first || p > this.wanted.last) this.pages.delete(p);
    }
  }
}

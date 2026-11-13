// The commit search of one repository view: the query, its hits (row indices from the core)
// and moving between them. Owned by a RepoView, which tells it when the rows change.

import { api } from "../ipc";
import { hitPosition, stepHit } from "../search";
import type { RepoSnapshot } from "../types";

/** Least time between two lookups of the hits while rows are still loading, ms. */
const LOOKUP_SPACING_MS = 250;

const NO_HITS: ReadonlySet<number> = new Set();

/** What the search needs of the view that owns it. */
export interface SearchHost {
  readonly repoId: number;
  readonly snapshot: RepoSnapshot;
  readonly selectedHash: string | null;
  readonly selectedIndex: number | null;
  /** Whether the view has been disposed of. */
  readonly disposed: boolean;
  /** Scrolls row `index` into view (in the middle, with `center`) and selects it. */
  selectRow(index: number, center?: boolean): Promise<void>;
  /** Shows `e` to the user as "<message>". */
  notice(e: unknown, message: string): void;
}

export class SearchState {
  /** The search box is open. */
  open = $state(false);
  /** Bumped to ask the search box to take the focus. */
  focus = $state(0);
  query = $state("");
  /** Ascending row indices of the commits matching `query`, in `generation`. */
  hits: number[] = $state.raw([]);
  generation = $state(-1);
  /** The core is looking for `query`. */
  pending = $state(false);

  private token = 0;
  /** Rows there were, and whether they were still loading, when the hits were looked up. */
  private lookupRows = 0;
  private lookupWhileLoading = false;
  /** A lookup of the hits is underway; another is wanted once it is done. */
  private lookingUp = false;
  private lookUpAgain = false;
  /** When (Date.now) the last lookup started; a follow-up during a load waits for its turn. */
  private lastLookupAt = -Infinity;
  private lookupTimer: ReturnType<typeof setTimeout> | null = null;
  private hitSetOf: number[] | null = null;
  private hitSetCache: ReadonlySet<number> = NO_HITS;
  /** The hit row being moved to, while its row loads. */
  private steppingTo: number | null = null;

  constructor(private readonly host: SearchHost) {}

  /** Position of the selected row among the hits, or -1. */
  get index(): number {
    const h = this.host;
    return hitPosition(this.liveHits, h.selectedHash === null ? null : h.selectedIndex);
  }

  /** The hits, when they belong to the rows shown. */
  get liveHits(): number[] {
    return this.generation === this.host.snapshot.generation ? this.hits : [];
  }

  /** The hits as a set; the same object until the hits change. */
  get hitSet(): ReadonlySet<number> {
    const hits = this.hits;
    if (hits !== this.hitSetOf) {
      this.hitSetOf = hits;
      this.hitSetCache = hits.length > 0 ? new Set(hits) : NO_HITS;
    }
    return this.hitSetCache;
  }

  /** `hitSet`, when the hits belong to the rows shown. */
  get liveHitSet(): ReadonlySet<number> {
    return this.generation === this.host.snapshot.generation ? this.hitSet : NO_HITS;
  }

  /** Opens the search box and asks it to take the focus. */
  show(): void {
    this.open = true;
    this.focus++;
  }

  /** Closes the search box and clears the query and the hits. */
  close(): void {
    this.open = false;
    void this.run("");
  }

  /** Looks for commits matching `query` and selects the first; a blank query clears the hits. */
  async run(query: string): Promise<void> {
    this.query = query;
    const token = ++this.token;
    if (query.trim() === "") {
      this.hits = [];
      this.generation = -1;
      this.pending = false;
      this.lookingUp = this.lookUpAgain = false;
      return;
    }
    const generation = this.host.snapshot.generation;
    this.pending = true;
    this.startLookup();
    let hits: number[];
    try {
      hits = await api.searchCommits(this.host.repoId, query);
    } catch (e) {
      if (token === this.token) {
        this.pending = false;
        this.lookupDone();
        this.host.notice(e, "Search failed");
      }
      return;
    }
    if (token !== this.token || this.host.disposed) return;
    this.pending = false;
    if (generation !== this.host.snapshot.generation) return; // looked up again for the new rows
    this.hits = hits;
    this.generation = generation;
    this.lookupDone();
    if (hits.length > 0) await this.host.selectRow(hits[0], true);
  }

  /** Selects the next hit after the selection, wrapping around. */
  next(): Promise<void> {
    return this.step(1);
  }

  /** Selects the previous hit before the selection, wrapping around. */
  prev(): Promise<void> {
    return this.step(-1);
  }

  /**
   * The rows shown changed: with a new generation the hits are looked up again at once;
   * while rows load, as `refreshAfterLoad` says.
   */
  rowsChanged(generationChanged: boolean): void {
    if (generationChanged && this.query.trim() !== "") void this.refresh();
    else this.refreshAfterLoad();
  }

  dispose(): void {
    if (this.lookupTimer !== null) clearTimeout(this.lookupTimer);
    this.lookupTimer = null;
  }

  private async step(dir: 1 | -1): Promise<void> {
    const h = this.host;
    const from = this.steppingTo ?? (h.selectedHash === null ? null : h.selectedIndex);
    const row = stepHit(this.liveHits, from, dir);
    if (row === null) return;
    this.steppingTo = row;
    try {
      await h.selectRow(row, true);
    } finally {
      if (this.steppingTo === row) this.steppingTo = null;
    }
  }

  /** Looks the hits of the current query up again in the rows now shown. */
  private async refresh(): Promise<void> {
    const query = this.query;
    const token = ++this.token;
    const generation = this.host.snapshot.generation;
    this.startLookup();
    let hits: number[];
    try {
      hits = await api.searchCommits(this.host.repoId, query);
    } catch {
      if (token === this.token) {
        this.pending = false;
        this.lookingUp = this.lookUpAgain = false; // the next change looks again
      }
      return;
    }
    if (token !== this.token || this.host.disposed) return;
    this.pending = false;
    if (generation !== this.host.snapshot.generation) return;
    this.hits = hits;
    this.generation = generation;
    this.lookupDone();
  }

  /**
   * The core searches only the commits it has loaded: while they load, the hits are looked
   * up again as rows arrive, at most once every LOOKUP_SPACING_MS, and once more as soon as
   * loading ends; one lookup at a time, with at most one more for all the rows that arrived
   * meanwhile.
   */
  private refreshAfterLoad(): void {
    if (this.query.trim() === "" || !this.lookupWhileLoading || this.host.disposed) return;
    const s = this.host.snapshot;
    if (s.totalRows <= this.lookupRows && s.loading) return;
    if (this.lookingUp) {
      this.lookUpAgain = true;
      return;
    }
    const wait = s.loading ? this.lastLookupAt + LOOKUP_SPACING_MS - Date.now() : 0;
    if (wait > 0) {
      this.lookupTimer ??= setTimeout(() => {
        this.lookupTimer = null;
        this.refreshAfterLoad();
      }, wait);
      return;
    }
    if (this.lookupTimer !== null) clearTimeout(this.lookupTimer);
    this.lookupTimer = null;
    void this.refresh();
  }

  private startLookup(): void {
    this.lastLookupAt = Date.now();
    this.lookupRows = this.host.snapshot.totalRows;
    this.lookupWhileLoading = this.host.snapshot.loading;
    this.lookingUp = true;
  }

  private lookupDone(): void {
    this.lookingUp = false;
    if (!this.lookUpAgain) return;
    this.lookUpAgain = false;
    this.refreshAfterLoad();
  }
}

// One RepoView per open tab: the repository's latest snapshot, its cache of rows, the
// selection and where the graph was scrolled to. It follows `repo-changed` events, and when
// the rows change under it, keeps the visible commit and the selection where they were.

import { computeAnchor, restoreScrollTop, ROW_H } from "../graph/geometry";
import { RowCache } from "../graph/rowCache";
import { api, CoreError, onEvent } from "../ipc";
import type { RepoSnapshot, RowsPage } from "../types";

/** Rows fetched around the restored position before a new generation is shown. */
const SWAP_PREFETCH_ROWS = 80;

/** A snapshot for a view created before its repository's first snapshot has arrived. */
function placeholder(repoId: number): RepoSnapshot {
  return {
    repoId,
    name: "",
    rootPath: "",
    activeWorktree: "",
    generation: -1,
    totalRows: 0,
    loading: true,
    maxLane: 0,
    head: { branch: null, hash: null },
    branches: [],
    remoteBranches: [],
    remotes: [],
    worktrees: [],
    status: { changed: 0, untracked: 0, conflicted: 0 },
    viewOptions: { branches: null, showRemotes: true, showTags: true },
    fetch: { running: false, lastError: null, lastAtMs: null },
  };
}

export class RepoView {
  readonly repoId: number;
  snapshot: RepoSnapshot = $state.raw(placeholder(0));
  /** Rows of `snapshot.generation`. Replaced (not reset) on a generation change; see applySnapshot. */
  cache: RowCache;
  selectedHash: string | null = $state(null);
  /** Row index of the selection in the current generation. */
  selectedIndex: number | null = $state(null);
  searchHits: number[] = $state([]);
  searchIndex = $state(-1);

  /** Where the graph is scrolled to, px; kept while the tab is in the background. */
  scrollTop = 0;
  /** Height of the graph's row area, px; set by the mounted graph. */
  viewportH = 600;
  /** A scroll position the mounted graph should move to, when set. */
  pendingScroll: number | null = null;
  /** Called when rows, the row count, the selection's row or `pendingScroll` changed. */
  onChange: (() => void) | null = null;

  private stopEvents: () => void;
  private disposed = false;
  /** Bumped by every generation change being applied; an older one in progress gives up. */
  private applyToken = 0;
  /** The newest snapshot of the generation being applied, while one is. */
  private incoming: RepoSnapshot | null = null;
  private markedFirstRows = false;
  private markedLoaded = false;
  /** Count of `repo-changed` events received; a snapshot asked for earlier is older than they. */
  private events = 0;

  constructor(repoId: number, initial?: RepoSnapshot) {
    this.repoId = repoId;
    this.snapshot = placeholder(repoId);
    this.cache = this.newCache(-1, 0);
    this.stopEvents = onEvent<RepoSnapshot>("repo-changed", (s) => {
      if (s.repoId !== repoId) return;
      this.events++;
      void this.applySnapshot(s);
    });
    if (initial) void this.applySnapshot(initial);
    // Events sent before this view subscribed are lost; catch up once.
    void this.catchUp();
  }

  select(hash: string | null, index: number | null = null): void {
    this.selectedHash = hash;
    this.selectedIndex = hash === null ? null : index;
  }

  /** Scrolls the mounted graph (or the one mounted next) just enough to show row `index`. */
  scrollToRow(index: number): void {
    const top = index * ROW_H;
    const height = Math.max(ROW_H, this.viewportH);
    const from = this.pendingScroll ?? this.scrollTop;
    let target = from;
    if (top < target) target = top;
    else if (top + ROW_H > target + height) target = top + ROW_H - height;
    // A position someone else asked for (a page key, say) still has to be applied.
    if (target === from && this.pendingScroll === null) return;
    this.pendingScroll = Math.max(0, target);
    this.onChange?.();
  }

  /** Records first paint of real rows, once (`first-rows`). */
  markFirstRows(): void {
    if (this.markedFirstRows) return;
    this.markedFirstRows = true;
    void api.perfMark("first-rows", performance.now()).catch(() => {});
  }

  /**
   * Takes in a new snapshot. Same generation: the row count may have grown, nothing moves.
   * New generation: anchors the first fully visible commit (or the top, when scrolled to the
   * top), finds it and the selection in the new rows, loads the rows there, and only then
   * swaps everything at once, so the view neither jumps nor flashes empty rows.
   */
  async applySnapshot(s: RepoSnapshot): Promise<void> {
    if (this.disposed || s.repoId !== this.repoId) return;
    const current = this.snapshot;
    if (s.generation === current.generation) {
      // Within a generation rows are only appended; fewer means an older snapshot.
      if (s.totalRows >= current.totalRows) this.show(s);
      return;
    }
    if (s.generation < current.generation) return; // an older event, overtaken
    if (this.incoming !== null && this.incoming.generation === s.generation) {
      // The swap in progress will use it.
      if (s.totalRows >= this.incoming.totalRows) this.incoming = s;
      return;
    }
    const token = ++this.applyToken;
    this.incoming = s;

    if (current.totalRows === 0) {
      // Nothing on screen to keep in place.
      this.swap(s, this.newCache(s.generation, s.totalRows), 0, null, null);
      return;
    }

    const startTop = this.scrollTop;
    const atTop = startTop <= 0;
    const anchor = computeAnchor(startTop, ROW_H);
    const anchorHash = atTop ? null : (this.cache.get(anchor.index)?.hash ?? null);
    const selected = this.selectedHash;
    const find = (hash: string | null) =>
      hash === null ? Promise.resolve(null) : api.findRow(this.repoId, hash).catch(() => null);
    const [anchorIndex, selectedIndex] = await Promise.all([find(anchorHash), find(selected)]);
    if (token !== this.applyToken || this.disposed) return;

    const target = this.incoming ?? s;
    const lastRow = Math.max(0, target.totalRows - 1);
    const top = atTop
      ? 0
      : restoreScrollTop(Math.min(anchorIndex ?? anchor.index, lastRow), anchor.offset, ROW_H);
    const next = this.newCache(target.generation, target.totalRows);
    const first = Math.floor(top / ROW_H);
    await next.ensure(first, first + SWAP_PREFETCH_ROWS);
    if (token !== this.applyToken || this.disposed) return;

    // Scrolling done while this was underway moves the restored position along.
    const scrolled = atTop ? 0 : this.scrollTop - startTop;
    this.swap(this.incoming ?? target, next, Math.max(0, top + scrolled), selected, selectedIndex);
  }

  /** Stops following the repository and drops its rows. */
  dispose(): void {
    this.disposed = true;
    this.stopEvents();
    this.onChange = null;
    this.cache = this.newCache(-1, 0);
  }

  /**
   * Shows `s` with the rows in `cache`, scrolled to `scrollTop`. `found` is the selection that
   * was looked up in the new rows and `foundIndex` its row (null: gone).
   */
  private swap(
    s: RepoSnapshot,
    cache: RowCache,
    scrollTop: number,
    found: string | null,
    foundIndex: number | null,
  ): void {
    this.incoming = null;
    this.cache = cache;
    if (this.selectedHash !== null) {
      if (this.selectedHash !== found) this.selectedIndex = null; // selected meanwhile: row unknown
      else if (foundIndex === null) this.select(null);
      else this.selectedIndex = foundIndex;
    }
    this.scrollTop = scrollTop;
    this.pendingScroll = scrollTop;
    this.show(s);
  }

  private show(s: RepoSnapshot): void {
    this.snapshot = s;
    this.cache.reset(s.generation, s.totalRows);
    if (!s.loading && !this.markedLoaded) {
      this.markedLoaded = true;
      void api.perfMark("fully-loaded", performance.now()).catch(() => {});
    }
    this.onChange?.();
  }

  /** Applies a freshly asked-for snapshot, unless an event (which is newer) came meanwhile. */
  private async catchUp(): Promise<void> {
    const seen = this.events;
    let s: RepoSnapshot;
    try {
      s = await api.getSnapshot(this.repoId);
    } catch {
      return; // the events will bring the next one
    }
    if (this.events === seen) await this.applySnapshot(s);
  }

  private newCache(generation: number, totalRows: number): RowCache {
    const cache = new RowCache((gen, start, count) => this.fetchPage(gen, start, count));
    cache.reset(generation, totalRows);
    cache.onUpdate = () => {
      if (this.cache === cache) this.onChange?.();
    };
    cache.onError = (e) => console.warn(`could not load rows of repository ${this.repoId}`, e);
    return cache;
  }

  /**
   * Rows from the core. When they changed since `generation` (`stale_generation`), the error
   * is swallowed: the fresh snapshot is applied, which loads the rows again under its
   * generation; the old cache ignores this page.
   */
  private async fetchPage(generation: number, start: number, count: number): Promise<RowsPage> {
    try {
      return await api.getRows(this.repoId, generation, start, count);
    } catch (e) {
      if (!(e instanceof CoreError && e.code === "stale_generation")) throw e;
      void this.catchUp();
      return { generation: -1, start, rows: [] };
    }
  }
}

const views = new Map<number, RepoView>();

/** The view of open repository `repoId`, created (from `initial`, if given) on first use. */
export function repoView(repoId: number, initial?: RepoSnapshot): RepoView {
  let view = views.get(repoId);
  if (!view) views.set(repoId, (view = new RepoView(repoId, initial)));
  else if (initial) void view.applySnapshot(initial);
  return view;
}

/** Disposes of the view of `repoId` (its tab closed). */
export function disposeRepoView(repoId: number): void {
  views.get(repoId)?.dispose();
  views.delete(repoId);
}

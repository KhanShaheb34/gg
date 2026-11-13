// One RepoView per open tab: the repository's latest snapshot, its cache of rows, the
// selection and where the graph was scrolled to, the view options, fetch and refresh, and its
// search (SearchState). View options are remembered per repository (viewOptionsStore).
// It follows `repo-changed` events, and when the rows change under it, keeps the visible
// commit and the selection where they were.

import { computeAnchor, restoreScrollTop, ROW_H } from "../graph/geometry";
import { RowCache } from "../graph/rowCache";
import { api, CoreError, onEvent } from "../ipc";
import type { RepoSnapshot, RowsPage, ViewOptions } from "../types";
import { CommitDetailsLoader } from "./details.svelte";
import { SearchState } from "./search.svelte";
import { DEFAULT_OPTIONS, readOptions, sameOptions, visible, writeOptions } from "./viewOptionsStore";

/**
 * Rows fetched around the restored position before a new generation is shown. These pages,
 * the selection's, and a page either side of each (RowCache.ensure) must fit in RowCache's
 * maxPages (8 pages of 200 rows), or the swap would evict rows it has just loaded.
 */
const SWAP_PREFETCH_ROWS = 80;
/** Attempts to show one generation in place before showing it wherever it can be. */
const MAX_SWAP_ATTEMPTS = 3;
/** How long an opening view waits for its remembered view options before showing other rows. */
const RESTORE_HOLD_MS = 2000;
/** The least time a refresh shows as running, so that a quick one is still seen. */
const MIN_REFRESH_MS = 400;
/** How long a fetch shows as starting when the core does not report it running. */
const FETCH_START_MS = 3000;

/** The newer of two snapshots of one repository (`b` when they are alike). */
function newer(a: RepoSnapshot | null, b: RepoSnapshot): RepoSnapshot {
  if (!a || b.generation > a.generation) return b;
  if (b.generation < a.generation) return a;
  return b.totalRows >= a.totalRows ? b : a;
}

/** Holding back snapshots while remembered view options are applied to an opening view. */
type Hold = {
  /** The newest snapshot held back. */
  newest: RepoSnapshot | null;
  /** Whether a snapshot shows the remembered options; set once the core has answered. */
  ready: ((s: RepoSnapshot) => boolean) | null;
  timer: ReturnType<typeof setTimeout>;
};

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
  /** The selected commit's details, for the details panel; remembers recent ones. */
  readonly details: CommitDetailsLoader;

  /**
   * The view options chosen last: what the toolbar shows, ahead of the core's answer. With
   * remote branches hidden, the branch selection still holds the remote ones chosen, so
   * showing them again brings them back; the core gets `visibleOptions`.
   */
  viewOptions: ViewOptions = $state.raw(DEFAULT_OPTIONS);

  /** The commit search: query, hits, and moving between them. */
  readonly search: SearchState;

  /** A refresh asked for here is underway. */
  refreshing = $state(false);
  /** A fetch was asked for here and the core has not reported it running yet. */
  private fetchStarting = $state(false);

  /** Shows an error to the user (set by the app shell). */
  onNotice: ((message: string, detail?: string) => void) | null = null;

  /** Widest the Graph column has sized itself, px; it never narrows while the view lives. */
  graphAutoWidth = 0;
  /** Where the graph is scrolled to, px; kept while the tab is in the background. */
  scrollTop = 0;
  /** Height of the graph's row area, px; set by the mounted graph. */
  viewportH = 600;
  /** A scroll position the mounted graph should move to, when set. */
  pendingScroll: number | null = null;
  /** Called when rows, the row count, the selection's row or `pendingScroll` changed. */
  onChange: (() => void) | null = null;

  private stopEvents: () => void;
  /** The view was disposed of (its tab closed). */
  disposed = false;
  /** Bumped by every generation change being applied; an older one in progress gives up. */
  private applyToken = 0;
  /** The newest snapshot of the generation being applied, while one is. */
  private incoming: RepoSnapshot | null = null;
  private markedFirstRows = false;
  private markedLoaded = false;
  /** Count of `repo-changed` events received; a snapshot asked for earlier is older than they. */
  private events = 0;
  /** The request for the newest snapshot in flight, shared by everyone who asks. */
  private snapshotRequest: Promise<void> | null = null;
  /** The generation that failed to show, and how often in a row. */
  private failedGeneration = -1;
  private failures = 0;

  /** Remembered view options have been looked for (on the first snapshot with a path). */
  private restoreChecked = false;
  private hold: Hold | null = null;
  /** View options waiting to be sent; the loop sending them, while it runs. */
  private optionsWanted: ViewOptions | null = null;
  private optionsSending: Promise<void> | null = null;
  /** The next generation change keeps the selection in place (the view options changed). */
  private revealSelection = false;
  /** The options being sent are the ones remembered for the repository, being restored. */
  private restoring = false;
  /** The last view options the core is known to have taken (whole, as in `viewOptions`). */
  private confirmedOptions: ViewOptions = DEFAULT_OPTIONS;

  /** When the fetch asked for here was asked for (ms since the epoch), while it is watched. */
  private fetchAskedAt = 0;
  private fetchWatched = false;

  constructor(repoId: number, initial?: RepoSnapshot) {
    this.repoId = repoId;
    this.details = new CommitDetailsLoader(repoId);
    this.search = new SearchState(this);
    this.snapshot = placeholder(repoId);
    this.cache = this.newCache(-1, 0);
    this.stopEvents = onEvent<RepoSnapshot>("repo-changed", (s) => {
      if (s.repoId !== repoId) return;
      this.events++;
      void this.applySnapshot(s);
    });
    if (initial) void this.applySnapshot(initial);
    // Events sent before this view subscribed are lost; catch up once.
    void this.refreshSnapshot();
  }

  select(hash: string | null, index: number | null = null): void {
    this.selectedHash = hash;
    this.selectedIndex = hash === null ? null : index;
  }

  /**
   * Scrolls the mounted graph (or the one mounted next) to show row `index`: just enough,
   * or, with `center`, putting a row out of view in the middle.
   */
  scrollToRow(index: number, center = false): void {
    const top = index * ROW_H;
    const height = Math.max(ROW_H, this.viewportH);
    const from = this.pendingScroll ?? this.scrollTop;
    let target = from;
    if (top < from || top + ROW_H > from + height) {
      if (center) target = Math.round(top - (height - ROW_H) / 2);
      else target = top < from ? top : top + ROW_H - height;
    }
    // A position someone else asked for (a page key, say) still has to be applied.
    if (target === from && this.pendingScroll === null) return;
    this.pendingScroll = Math.max(0, target);
    this.onChange?.();
  }

  /** Scrolls row `index` into view and selects it once its row is loaded. */
  async selectRow(index: number, center = false): Promise<void> {
    this.scrollToRow(index, center);
    const cache = this.cache;
    if (!cache.get(index)) await cache.ensure(index, index);
    if (cache !== this.cache) return;
    const row = cache.get(index);
    if (row) this.select(row.hash, index);
  }

  // --- View options -------------------------------------------------------------------------

  /** What the graph shows of `viewOptions` (see `visible`); what the core is sent. */
  get visibleOptions(): ViewOptions {
    return visible(this.viewOptions, this.snapshot.remoteBranches);
  }

  /**
   * Changes the view options (merged into the current ones) and remembers them for this
   * repository. The graph keeps its rows until the new ones are loaded, and the selection
   * where it was on screen. A failed change goes back to the options the core has.
   */
  setViewOptions(patch: Partial<ViewOptions>): Promise<void> {
    this.restoring = false;
    const next: ViewOptions = { ...this.viewOptions, ...patch };
    if (next.branches !== null && next.branches.length === 0) next.branches = null;
    this.viewOptions = next;
    this.remember(next);
    this.revealSelection = true;
    return this.sendOptions(visible(next, this.snapshot.remoteBranches));
  }

  private remember(options: ViewOptions): void {
    if (this.snapshot.rootPath) writeOptions(this.snapshot.rootPath, options);
  }

  /** Takes the core's options, unless they are what `viewOptions` shows already. */
  private adoptOptions(core: ViewOptions | undefined, remoteBranches: string[] | undefined): void {
    if (!core) return;
    if (!sameOptions(core, visible(this.viewOptions, remoteBranches))) this.viewOptions = core;
    this.confirmedOptions = this.viewOptions;
  }

  /** Sends `options` to the core: one request at a time, the newest options last. */
  private sendOptions(options: ViewOptions): Promise<void> {
    this.optionsWanted = options;
    this.optionsSending ??= this.drainOptions();
    return this.optionsSending;
  }

  private async drainOptions(): Promise<void> {
    try {
      while (this.optionsWanted !== null && !this.disposed) {
        const options = this.optionsWanted;
        this.optionsWanted = null;
        const seen = this.events;
        let s: RepoSnapshot;
        try {
          s = await api.setViewOptions(this.repoId, options);
        } catch (e) {
          this.notice(e, "Could not change the view");
          this.releaseHold();
          if (this.optionsWanted === null) {
            // Back to what the core has. Options that could not be restored stay remembered
            // for the next time; a change the user just made that failed is forgotten.
            this.viewOptions = this.confirmedOptions;
            if (!this.restoring) this.remember(this.confirmedOptions);
            this.revealSelection = false;
            this.restoring = false;
          }
          continue;
        }
        this.restoring = false;
        if (this.optionsWanted !== null || this.disposed) continue; // a newer choice follows
        this.adoptOptions(s.viewOptions, s.remoteBranches);
        // Nothing to wait for: the rows did not change.
        if (!s.loading && s.generation <= this.snapshot.generation) this.revealSelection = false;
        if (this.hold) {
          // Shown once the rows of these options are: a later generation, or the same one
          // once nothing is loading any more.
          const answer = s;
          this.hold.ready = (x) =>
            x.generation > answer.generation || (!x.loading && sameOptions(x.viewOptions, answer.viewOptions));
        }
        this.applyAnswer(s, seen);
      }
    } finally {
      this.optionsSending = null;
    }
  }

  /**
   * On the first snapshot of the repository: when other view options are remembered for it,
   * asks for them and holds the rows back until they show (or RESTORE_HOLD_MS passed).
   */
  private restoreOptions(s: RepoSnapshot): void {
    this.restoreChecked = true;
    const stored = readOptions(s.rootPath);
    if (!stored) return;
    this.viewOptions = stored;
    const wanted = visible(stored, s.remoteBranches);
    if (sameOptions(wanted, s.viewOptions ?? DEFAULT_OPTIONS)) {
      this.confirmedOptions = stored;
      return;
    }
    this.hold = { newest: null, ready: null, timer: setTimeout(() => this.releaseHold(), RESTORE_HOLD_MS) };
    this.restoring = true;
    void this.sendOptions(wanted);
  }

  private holdBack(s: RepoSnapshot): void {
    this.hold!.newest = newer(this.hold!.newest, s);
    this.checkHold();
  }

  private checkHold(): void {
    const hold = this.hold;
    if (hold?.newest && hold.ready?.(hold.newest)) this.releaseHold();
  }

  /**
   * Applies `s`, the answer to a request made when `seen` events had arrived, unless an event
   * (which is newer) came meanwhile; then the newest snapshot is asked for instead.
   */
  private applyAnswer(s: RepoSnapshot, seen: number): void {
    if (this.events === seen) {
      void this.applySnapshot(s);
      return;
    }
    this.checkHold();
    void this.refreshSnapshot();
  }

  private releaseHold(): void {
    const hold = this.hold;
    if (!hold) return;
    this.hold = null;
    clearTimeout(hold.timer);
    if (hold.newest) void this.applySnapshot(hold.newest);
  }

  // --- Fetch and refresh --------------------------------------------------------------------

  /** A fetch is running, or was just asked for here. */
  get fetching(): boolean {
    return this.snapshot.fetch?.running === true || this.fetchStarting;
  }

  /** Fetches from all remotes; progress shows at once, and its failure as a notice. */
  async fetch(): Promise<void> {
    if (this.fetching || (this.snapshot.remotes ?? []).length === 0) return;
    this.fetchStarting = true;
    this.fetchWatched = true;
    this.fetchAskedAt = Date.now();
    try {
      await api.fetch(this.repoId);
    } catch (e) {
      this.fetchStarting = false;
      this.fetchWatched = false;
      this.notice(e, "Fetch failed");
      return;
    }
    // The core starts it on its own task; one that never starts (no remotes) stops showing.
    setTimeout(() => {
      if (this.fetchStarting && this.snapshot.fetch?.running !== true) {
        this.fetchStarting = false;
        this.fetchWatched = false;
      }
    }, FETCH_START_MS);
  }

  /** Follows the fetch asked for here through `s`: running, then done (or failed). */
  private trackFetch(s: RepoSnapshot): void {
    const state = s.fetch;
    if (!this.fetchWatched || !state) return;
    if (state.running) {
      this.fetchStarting = false;
      return;
    }
    if (state.lastAtMs === null || state.lastAtMs < this.fetchAskedAt) return;
    this.fetchStarting = false;
    this.fetchWatched = false;
    if (state.lastError) this.onNotice?.("Fetch failed", state.lastError);
  }

  /** Re-reads the repository now; progress shows for at least MIN_REFRESH_MS. */
  async refresh(): Promise<void> {
    if (this.refreshing) return;
    this.refreshing = true;
    const started = performance.now();
    const seen = this.events;
    try {
      this.applyAnswer(await api.refresh(this.repoId), seen);
    } catch (e) {
      this.notice(e, "Could not refresh");
    }
    const left = MIN_REFRESH_MS - (performance.now() - started);
    if (left > 0) await new Promise((r) => setTimeout(r, left));
    this.refreshing = false;
  }

  /**
   * Reports `e` as "<message>" with the error's own message below (as a failed fetch is), and
   * under that the core's detail (git's own words), if it has one.
   */
  notice(e: unknown, message: string): void {
    const text = e instanceof Error ? e.message : String(e);
    const detail = e instanceof CoreError ? e.detail : null;
    this.onNotice?.(message, detail ? `${text}\n${detail}` : text);
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
   * swaps everything at once, so the view neither jumps nor flashes empty rows. If the rows
   * changed again meanwhile (the loaded window is incomplete, or the anchor or selection is
   * not where `findRow` said), the old rows stay and the newest snapshot is tried instead; a
   * generation that fails MAX_SWAP_ATTEMPTS times in a row is shown anyway, at the place
   * worked out for it, and the user is told the view may have moved.
   */
  async applySnapshot(s: RepoSnapshot): Promise<void> {
    if (this.disposed || s.repoId !== this.repoId) return;
    if (!this.restoreChecked && s.rootPath) this.restoreOptions(s);
    if (this.hold) {
      this.holdBack(s);
      return;
    }
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
    // After a change of view options the selection is kept where it is on screen (in view).
    const reveal = this.revealSelection && selected !== null;
    const selectedOffset = reveal ? this.selectionOffset(startTop) : 0;
    const find = (hash: string | null) =>
      hash === null ? Promise.resolve(null) : api.findRow(this.repoId, hash).catch(() => null);
    const [anchorIndex, selectedIndex] = await Promise.all([find(anchorHash), find(selected)]);
    if (token !== this.applyToken || this.disposed) return;

    const target = this.incoming ?? s;
    const lastRow = Math.max(0, target.totalRows - 1);
    const keepSelection = reveal && selectedIndex !== null;
    const top = keepSelection
      ? restoreScrollTop(selectedIndex, selectedOffset, ROW_H)
      : atTop
        ? 0
        : restoreScrollTop(Math.min(anchorIndex ?? anchor.index, lastRow), anchor.offset, ROW_H);
    const next = this.newCache(target.generation, target.totalRows);
    // Why rows could not be loaded, if that is why this attempt fails.
    let rowsError: unknown = null;
    const warn = next.onError;
    next.onError = (e) => {
      rowsError = e;
      warn(e);
    };
    const first = Math.floor(top / ROW_H);
    await Promise.all([
      next.ensure(first, first + SWAP_PREFETCH_ROWS),
      selectedIndex === null ? null : next.ensure(selectedIndex, selectedIndex),
    ]);
    if (token !== this.applyToken || this.disposed) return;

    const lastVisible = first + Math.ceil(this.viewportH / ROW_H);
    const consistent =
      next.loaded(first, lastVisible) &&
      (keepSelection || anchorIndex === null || next.get(anchorIndex)?.hash === anchorHash) &&
      (selectedIndex === null || next.get(selectedIndex)?.hash === selected);
    if (!consistent) {
      if (this.failedGeneration === target.generation) this.failures++;
      else {
        this.failedGeneration = target.generation;
        this.failures = 1;
      }
      if (this.failures < MAX_SWAP_ATTEMPTS) {
        this.incoming = null;
        void this.refreshSnapshot();
        return;
      }
      // The old rows may not be served much longer either: show the new ones where they
      // should be (rows that failed load again as the view asks for them, on the next change
      // or a refresh) and say why the view may have moved.
      if (rowsError !== null) this.notice(rowsError, "Could not load the commits");
      else {
        this.onNotice?.(
          "Could not keep your place in the history",
          "The commits kept changing while the graph was being updated; the view may have moved.",
        );
      }
    }
    this.failures = 0;
    this.failedGeneration = -1;

    // Scrolling done while this was underway moves the restored position along.
    const scrolled = atTop ? 0 : this.scrollTop - startTop;
    this.swap(this.incoming ?? target, next, Math.max(0, top + scrolled), selected, selectedIndex);
  }

  /** Stops following the repository and drops its rows. */
  dispose(): void {
    this.disposed = true;
    if (this.hold) clearTimeout(this.hold.timer);
    this.hold = null;
    this.search.dispose();
    this.stopEvents();
    this.details.dispose();
    this.onChange = null;
    this.cache = this.newCache(-1, 0);
  }

  /** Where the selected row's top is below the viewport's top, px, clamped into view. */
  private selectionOffset(scrollTop: number): number {
    const max = Math.max(0, this.viewportH - ROW_H);
    const offset = this.selectedIndex === null ? Math.round(max / 3) : this.selectedIndex * ROW_H - scrollTop;
    return Math.min(max, Math.max(0, offset));
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
    if (this.optionsSending === null) this.revealSelection = false;
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
    const generationChanged = s.generation !== this.snapshot.generation;
    this.snapshot = s;
    this.cache.reset(s.generation, s.totalRows);
    // The core's options, unless newer ones are on their way to it.
    if (this.optionsSending === null && this.hold === null) this.adoptOptions(s.viewOptions, s.remoteBranches);
    this.trackFetch(s);
    this.search.rowsChanged(generationChanged);
    if (!s.loading && !this.markedLoaded) {
      this.markedLoaded = true;
      void api.perfMark("fully-loaded", performance.now()).catch(() => {});
    }
    this.onChange?.();
    // A perf run (the probe is on) marks the frame that shows each later generation, so the
    // time from an on-disk change to the screen can be read from the perf log.
    if (generationChanged && this.markedLoaded && (globalThis as { __GG_PROBE__?: object }).__GG_PROBE__) {
      const mark = () => void api.perfMark("generation-shown", s.generation).catch(() => {});
      if (typeof requestAnimationFrame === "function") requestAnimationFrame(mark);
      else mark();
    }
  }

  /**
   * Asks the core for its newest snapshot and applies it, unless an event (which is newer)
   * arrives meanwhile. Calls while one request is in flight share it.
   */
  private refreshSnapshot(): Promise<void> {
    if (this.snapshotRequest) return this.snapshotRequest;
    const seen = this.events;
    this.snapshotRequest = api.getSnapshot(this.repoId).then(
      (s) => {
        this.snapshotRequest = null;
        if (this.events === seen) void this.applySnapshot(s);
      },
      () => {
        this.snapshotRequest = null; // the events will bring the next one
      },
    );
    return this.snapshotRequest;
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
   * is swallowed: the newest snapshot is applied, which loads the rows again under its
   * generation; the cache that asked ignores this empty page.
   */
  private async fetchPage(generation: number, start: number, count: number): Promise<RowsPage> {
    try {
      return await api.getRows(this.repoId, generation, start, count);
    } catch (e) {
      if (!(e instanceof CoreError && e.code === "stale_generation")) throw e;
      void this.refreshSnapshot();
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

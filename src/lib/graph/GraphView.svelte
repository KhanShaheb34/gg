<!--
  The commit graph of one repository: a virtualized grid of rows (graph, description with
  ref labels, date, author, short hash) with the lanes drawn on one canvas.

  Speed rules this follows:
  - A fixed pool of row elements, one per slot; row i lives in slot i % pool size, and
    nothing is created or destroyed while scrolling.
  - Rows and canvas sit in one container inside the scrolled content, so the compositor moves
    them together: lines never lag behind text. The container covers only the rendered rows
    (visible ± overscan, plus POOL_STEP rows) and is moved with a transform on its own layer,
    so the tall content it scrolls in paints nothing: jumping through the history does not
    make WebKit paint (and keep) new tiles for every position.
  - Rows are placed relative to the container, which moves only once every POOL_STEP rows
    (poolStart): scrolling by a row changes one slot's row and position, not every slot's.
  - The canvas is redrawn in requestAnimationFrame, at most once per frame; the shapes of
    the edges are worked out in one pass (EdgeShapes).
  - Scrolling reads only `scrollTop`; sizes come from a ResizeObserver; colours are read once
    per theme.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import { formatDate, formatDateLong, groupDigits, shortHash } from "../format";
  import type { GraphProbe } from "../perf/graphProbe";
  import type { Probe } from "../perf/scrollProbe";
  import { hashMatch, needleOf, splitMatch } from "../search";
  import { app, toast } from "../state/app.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import type { CommitRow, RefLabel } from "../types";
  import {
    alignSlackFor,
    autoGraphWidth,
    displayedRows,
    fillTime,
    HEADER_H,
    LANE_COLORS,
    OVERSCAN,
    POOL_STEP,
    poolStart,
    PREFETCH,
    ROW_H,
    rowWindow,
  } from "./geometry";
  import GraphHeader from "./GraphHeader.svelte";
  import { GraphColumns } from "./graphColumns.svelte";
  import { gridKey } from "./gridKeys";
  import { LanesCanvas, type RowSource } from "./lanesCanvas";
  import RefLabels from "./RefLabels.svelte";

  let { view }: { view: RepoView } = $props();

  const NO_LABELS: RefLabel[] = [];
  const NO_PATHS: string[] = [];
  /** No rows: what slots and lanes show while a fast jump is under way. */
  const NO_ROWS: RowSource = { get: () => undefined };

  // --- Columns ----------------------------------------------------------------------------

  /** The Graph column's own width; it only grows while the view lives (see autoGraphWidth). */
  const autoGraphW = $derived.by(() => {
    view.graphAutoWidth = autoGraphWidth(view.snapshot.maxLane, view.graphAutoWidth);
    return view.graphAutoWidth;
  });
  const columns = new GraphColumns(() => autoGraphW);

  // --- Rows ---------------------------------------------------------------------------------

  /** One recycled row element. */
  class Slot {
    index = $state(-1);
    row: CommitRow | undefined = $state.raw(undefined);
    /** When (performance.now) this slot became a visible skeleton; -1 if it is not one. */
    skeletonSince = -1;
  }

  let slots: Slot[] = $state.raw([]);
  /**
   * First row of the rendered range, where the row container sits; rounded down to a row
   * whose top is a whole device pixel, so the canvas bitmap is never resampled.
   */
  let poolFirst = $state(0);
  let scroller: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  const lanes = new LanesCanvas();

  /** Height of the row area (below the header), px. */
  let viewportH = $state(0);
  /** The window's device pixel ratio. */
  let ratio = $state(window.devicePixelRatio || 1);
  /** Rows the canvas may start above the first rendered row, to start on a whole device pixel. */
  const alignSlack = $derived(alignSlackFor(ratio));
  let scrollTop = 0;
  let lastTop = 0;
  let rafId = 0;
  /** Rows are filled again from this time on (see fillTime); a timer brings that frame. */
  let fillAt = 0;
  let fillTimer: ReturnType<typeof setTimeout> | null = null;

  const total = $derived(view.snapshot.totalRows);
  const shown = $derived(displayedRows(total, ROW_H));
  const empty = $derived(!view.snapshot.loading && total === 0);
  /** Loading, nothing to show yet: a screenful of skeleton rows. */
  const waiting = $derived(view.snapshot.loading && total === 0);
  const rowsHeight = $derived((waiting ? skeletonRows(viewportH) : shown) * ROW_H);
  /** Rows the viewport needs rendered: what fits, a partly shown one, and the overscan. */
  const neededRows = $derived(neededSlots(viewportH));
  /** Height of the rendered rows' container: the rows needed, and room to stay put a step. */
  const poolRows = $derived(neededRows + POOL_STEP + alignSlack);
  const detachedHead = $derived(view.snapshot.head.branch === null);
  /** Rows matching the search, and how to find the match in their subjects. */
  // The set stays the same object while only load progress changes, so rows do not re-render.
  const hits = $derived(view.search.liveHitSet);
  /** The selected row, while it is rendered (an id that names no element must not be given). */
  const activeRow = $derived.by(() => {
    const index = view.selectedHash === null ? null : view.selectedIndex;
    return index !== null && slots.some((slot) => slot.index === index) ? index : undefined;
  });
  /** The search query as matched (lowercased), while there are hits to mark. */
  const needle = $derived(hits.size > 0 ? needleOf(view.search.query) : null);

  /** Skeleton rows shown while the first rows load: as many as fit, so nothing scrolls. */
  function skeletonRows(height: number): number {
    return Math.floor(height / ROW_H);
  }

  function neededSlots(height: number): number {
    return Math.ceil(height / ROW_H) + 1 + 2 * OVERSCAN;
  }

  /** Rows the grid has right now (skeletons included while waiting). */
  function rowCount(): number {
    const s = view.snapshot;
    return s.totalRows === 0 && s.loading ? skeletonRows(viewportH) : displayedRows(s.totalRows, ROW_H);
  }

  // --- Performance runs (window.__GG_PROBE__, set before load) -----------------------------
  // The probe is loaded only in a perf run, so none of it is in the app's main chunk.

  const probeData = (window as { __GG_PROBE__?: Probe }).__GG_PROBE__;
  let probe: GraphProbe | null = null;
  if (probeData) {
    void import("../perf/graphProbe").then(({ GraphProbe }) => {
      probe = new GraphProbe(probeData, view, () => scroller);
      probe.maybeRun();
    });
  }
  $effect(() => {
    void view.snapshot.loading;
    void view.snapshot.totalRows;
    probe?.maybeRun();
  });

  // --- Frame: slots, fetching, canvas -------------------------------------------------------

  function schedule() {
    if (rafId === 0) rafId = requestAnimationFrame(frame);
  }

  // A new Graph column width needs the canvas resized to it and redrawn.
  $effect(() => {
    void columns.widths.graph;
    schedule();
  });

  /**
   * Brings slots, fetching and the canvas up to date with the scroll position. During a fast
   * jump by the user (a fling, a scrollbar drag) the rows are left unfilled until it slows
   * (fillTime); a scroll the view makes itself (`moved`: a page key, a search hit) fills them
   * at once.
   */
  function frame(now: number, moved = false) {
    if (rafId !== 0) cancelAnimationFrame(rafId);
    rafId = 0;
    if (!moved) fillAt = fillTime(scrollTop - lastTop, viewportH, now, fillAt);
    const filling = moved || now >= fillAt;
    if (!filling && fillTimer === null) {
      fillTimer = setTimeout(() => {
        fillTimer = null;
        schedule();
      }, fillAt - now + 1);
    }
    const cache = view.cache;
    const rows: RowSource = filling ? cache : NO_ROWS;
    const w = rowWindow(scrollTop, viewportH, ROW_H, rowCount(), OVERSCAN);
    const n = slots.length;
    for (let k = 0; k < n; k++) {
      const slot = slots[k];
      let i = w.first + ((((k - w.first) % n) + n) % n);
      if (i >= w.last) i = -1;
      const row = i >= 0 ? rows.get(i) : undefined;
      if (slot.index !== i) slot.index = i;
      if (slot.row !== row) {
        // A tooltip built for the commit this slot showed would now name another one.
        if (titled !== null && slot.row === titledRow) clearTitle();
        slot.row = row;
      }
    }
    if (view.snapshot.totalRows > 0 && w.last > w.first) {
      const down = scrollTop >= lastTop;
      void cache.ensure(down ? w.first : w.first - PREFETCH, down ? w.last + PREFETCH : w.last);
    }
    lastTop = scrollTop;
    const first = poolStart(poolFirst, w.first, POOL_STEP, ROW_H, ratio);
    if (first !== poolFirst) poolFirst = first;
    // Sized for the rows the viewport needs, not the pool, which only grows.
    if (canvas && lanes.draw(canvas, rows, w, poolFirst, columns.widths.graph, poolRows, ratio)) {
      view.markFirstRows();
    }
    probe?.frame(now, w, slots);
  }

  // --- Events -------------------------------------------------------------------------------

  /** The cell last given a tooltip, and the row it was built for; cleared when rows move. */
  let titled: HTMLElement | null = null;
  let titledRow: CommitRow | undefined;

  function clearTitle() {
    titled?.removeAttribute("title");
    titled = null;
    titledRow = undefined;
  }

  function onScroll() {
    scrollTop = scroller.scrollTop;
    view.scrollTop = scrollTop;
    probe?.scrolled();
    if (titled) clearTitle();
    schedule();
  }

  /** Rows, row count or a requested scroll position changed. */
  async function onViewChange() {
    if (view.pendingScroll !== null) {
      await tick(); // let the content take its new height before scrolling in it
      const target = view.pendingScroll;
      if (target === null || !scroller) return;
      view.pendingScroll = null;
      scroller.scrollTop = target;
      scrollTop = scroller.scrollTop;
      view.scrollTop = scrollTop;
      frame(performance.now(), true); // same task: rows, canvas and position change together
      return;
    }
    schedule();
  }

  function onResize(height: number) {
    const before = viewportH;
    viewportH = Math.max(0, height - HEADER_H);
    view.viewportH = viewportH;
    // The pool grows as needed and is kept when the viewport shrinks (the details panel
    // being dragged), so no row element is made again; only a much smaller viewport trims it.
    // Everything else (the canvas, the container, skeletons) is sized for what is needed.
    const n = neededSlots(viewportH);
    if (n > slots.length) slots = [...slots, ...Array.from({ length: n - slots.length }, () => new Slot())];
    else if (2 * n < slots.length) slots = slots.slice(0, n);
    keepSelectionInView(before);
    frame(performance.now(), true);
  }

  /**
   * The viewport shrank from `before` px (the details panel opened or grew): a selected row
   * that was in view and now is under the bottom edge is scrolled up just enough to stay in
   * view, in the same frame. Nothing else moves.
   */
  function keepSelectionInView(before: number) {
    const index = view.selectedHash === null ? null : view.selectedIndex;
    if (index === null || viewportH >= before || viewportH < ROW_H || view.pendingScroll !== null) return;
    const bottom = (index + 1) * ROW_H;
    const wasInView = index * ROW_H >= scrollTop && bottom <= scrollTop + before;
    if (!wasInView || bottom <= scrollTop + viewportH) return;
    scroller.scrollTop = bottom - viewportH;
    scrollTop = scroller.scrollTop;
    view.scrollTop = scrollTop;
  }

  /**
   * The row an event happened on (a click, a hover, a context menu): its index and its
   * commit, or null when the event is not on a loaded row.
   */
  export function rowAt(event: Event): { index: number; row: CommitRow } | null {
    const el = (event.target as Element | null)?.closest?.<HTMLElement>('[role="row"][data-index]');
    if (!el) return null;
    const index = Number(el.dataset.index);
    const row = view.cache.get(index);
    return row ? { index, row } : null;
  }

  function onRowsClick(e: MouseEvent) {
    const hit = rowAt(e);
    if (!hit) return;
    view.select(hit.row.hash, hit.index);
    if (hit.row.kind === "commit" && (e.target as Element).closest(".hash")) copyHash(hit.row.hash);
  }

  function copyHash(hash: string) {
    navigator.clipboard.writeText(hash).then(
      () => toast("info", "Copied"),
      () => toast("error", "Could not copy the commit hash"),
    );
  }

  /**
   * Builds the tooltip of the cell under the pointer, only then: the full hash, date, or
   * author and email; the subject's full text only when it is cut off.
   */
  function onRowsPointerOver(e: PointerEvent) {
    const el = (e.target as Element | null)?.closest?.<HTMLElement>(".subject, .date, .author, .hash");
    if (!el || el === titled) return;
    clearTitle();
    const r = rowAt(e)?.row;
    if (!r) return;
    let text: string | null = null;
    if (el.classList.contains("subject")) text = el.scrollWidth > el.clientWidth ? r.subject : null;
    else if (r.kind !== "commit") text = null;
    else if (el.classList.contains("date")) text = formatDateLong(r.time);
    else if (el.classList.contains("author")) text = `${r.author} <${r.email}>`;
    else text = r.hash;
    if (text === null) return;
    el.title = text;
    titled = el;
    titledRow = r;
  }

  function onKeydown(e: KeyboardEvent) {
    // Only plain keys on the grid itself: the shell's shortcuts and the splitters keep theirs.
    if (e.target !== scroller || e.metaKey || e.ctrlKey || e.altKey) return;
    const move = gridKey(e.key, {
      count: displayedRows(view.snapshot.totalRows, ROW_H),
      viewportH,
      scrollTop,
      current: view.selectedHash === null ? null : view.selectedIndex,
      rowH: ROW_H,
    });
    if (!move) return;
    e.preventDefault();
    if (move.select === null) {
      view.select(null);
      return;
    }
    if (move.scrollTop !== undefined) view.pendingScroll = move.scrollTop;
    void view.selectRow(move.select);
  }

  onMount(() => {
    lanes.readColors(document.documentElement);
    const resize = new ResizeObserver((entries) => onResize(entries[entries.length - 1].contentRect.height));
    resize.observe(scroller);
    const theme = new MutationObserver(() => {
      lanes.readColors(document.documentElement);
      schedule();
    });
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    // A move to a screen with another pixel ratio needs a sharper (or lighter) canvas.
    let ratioQuery: MediaQueryList | null = null;
    const watchRatio = () => {
      ratioQuery?.removeEventListener("change", onRatio);
      ratioQuery = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
      ratioQuery.addEventListener("change", onRatio);
    };
    const onRatio = () => {
      ratio = window.devicePixelRatio || 1;
      watchRatio();
      schedule();
    };
    watchRatio();

    // Back to where this tab was.
    const start = view.pendingScroll ?? view.scrollTop;
    view.pendingScroll = null;
    scroller.scrollTop = start;
    scrollTop = lastTop = start;
    view.onChange = onViewChange;

    return () => {
      if (view.onChange === onViewChange) view.onChange = null;
      if (probe) view.onGenerationShown = null;
      resize.disconnect();
      theme.disconnect();
      ratioQuery?.removeEventListener("change", onRatio);
      if (rafId !== 0) cancelAnimationFrame(rafId);
      if (fillTimer !== null) clearTimeout(fillTimer);
    };
  });
</script>

<div class="graph" style:--cols={columns.template} style:--row-h="{ROW_H}px" style:--header-h="{HEADER_H}px">
  <div
    class="scroller"
    role="grid"
    aria-label="Commits"
    aria-rowcount={total + 1}
    aria-colcount={5}
    aria-activedescendant={activeRow === undefined ? undefined : `gg-${view.repoId}-${activeRow}`}
    tabindex="0"
    bind:this={scroller}
    onscroll={onScroll}
    onkeydown={onKeydown}
  >
    <GraphHeader {columns} />

    <div class="rows" style:height="{rowsHeight}px">
      <!-- Clicks and hovers are handled here for every row; the grid itself takes the keys. -->
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <div
        class="pool"
        role="rowgroup"
        style:height="{poolRows * ROW_H}px"
        style:transform="translateY({poolFirst * ROW_H}px)"
        onclick={onRowsClick}
        onpointerover={onRowsPointerOver}
      >
        <canvas class="lanes" data-testid="graph-canvas" aria-hidden="true" bind:this={canvas}></canvas>
        {#each slots as slot (slot)}
          {@const r = slot.row}
          <!-- A hit's match is marked in fixed elements (before, mark, after) whose text alone
               changes as rows scroll by: nothing is created or destroyed. -->
          {@const hit = r !== undefined && needle !== null && hits.has(slot.index)}
          {@const subj = hit ? splitMatch(r!.subject, needle!) : null}
          {@const auth = hit ? splitMatch(r!.author, needle!) : null}
          {@const short = r?.kind === "commit" ? shortHash(r.hash) : ""}
          {@const hashMarked = hit && r!.kind === "commit" ? hashMatch(r!.hash, needle) : 0}
          <div
            class="row"
            role="row"
            id={slot.index >= 0 ? `gg-${view.repoId}-${slot.index}` : undefined}
            hidden={slot.index < 0}
            data-index={slot.index >= 0 ? slot.index : undefined}
            aria-rowindex={slot.index >= 0 ? slot.index + 2 : undefined}
            aria-selected={r !== undefined && r.hash === view.selectedHash}
            aria-busy={r === undefined}
            data-hit={hit ? "true" : undefined}
            class:merge={r?.isMerge}
            class:head={r?.isHead}
            class:uncommitted={r?.kind === "uncommitted"}
            style:transform="translateY({(slot.index - poolFirst) * ROW_H}px)"
            style:--c={r ? `var(--lane-${r.color % LANE_COLORS})` : undefined}
          >
            <div class="cell lanes-cell" role="gridcell"></div>
            <div class="cell desc" role="gridcell">
              <RefLabels
                labels={r?.labels ?? NO_LABELS}
                detachedHead={!!r?.isHead && detachedHead}
                detachedWorktrees={r?.detachedWorktrees ?? NO_PATHS}
                homeDir={app.homeDir}
              />
              <span class="subject"
                ><span>{subj ? subj[0] : (r?.subject ?? "")}</span><mark>{subj ? subj[1] : ""}</mark><span
                  >{subj ? subj[2] : ""}</span
                ></span
              >
            </div>
            <div class="cell date" role="gridcell">
              {r?.kind === "commit" ? formatDate(r.time) : ""}
            </div>
            <div class="cell author" role="gridcell"
              ><span>{auth ? auth[0] : (r?.author ?? "")}</span><mark>{auth ? auth[1] : ""}</mark><span
                >{auth ? auth[2] : ""}</span
              ></div
            >
            <div class="cell hash-cell" role="gridcell">
              <span class="hash"><mark>{short.slice(0, hashMarked)}</mark>{short.slice(hashMarked)}</span>
            </div>
          </div>
        {/each}
      </div>
    </div>

    {#if shown < total}
      <p class="cap">Showing the most recent {groupDigits(shown)} commits</p>
    {/if}
  </div>

  {#if empty}
    <div class="empty">
      <p class="empty-title">No commits yet</p>
      <p class="empty-hint">
        Commits made {view.snapshot.head.branch ? `on ${view.snapshot.head.branch}` : "in this repository"} show up here.
      </p>
    </div>
  {/if}
</div>

<style>
  .graph {
    position: relative;
    height: 100%;
    min-height: 0;
  }

  .scroller {
    height: 100%;
    overflow-x: hidden;
    overflow-y: auto;
    /* The view keeps rows in place itself; the browser's anchoring would fight it. */
    overflow-anchor: none;
    outline: none;
  }

  .scroller:focus-visible:not(:has(.row[aria-selected="true"])) {
    box-shadow: inset 0 0 0 2px var(--focus-ring);
  }

  /* --- Rows --- */

  .rows {
    position: relative;
    contain: strict;
    width: 100%;
  }

  .pool {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    will-change: transform;
  }

  .lanes {
    position: absolute;
    top: 0;
    left: 0;
    z-index: 1;
    pointer-events: none;
  }

  .row {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: grid;
    grid-template-columns: var(--cols);
    align-items: center;
    height: var(--row-h);
    contain: layout paint style;
  }

  .row[hidden] {
    display: none;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  /* A search hit: a faint tint, and its match marked in the subject. */
  .row[data-hit="true"] {
    background: var(--hit-row);
  }

  .row[data-hit="true"]:hover {
    background: color-mix(in srgb, var(--hit-row), var(--bg-hover));
  }

  .row[aria-selected="true"] {
    background: var(--bg-selected);
  }

  .row mark {
    border-radius: 2px;
    background: var(--hit-mark);
    color: var(--hit-mark-text);
    box-shadow: 0 0 0 1px var(--hit-mark);
  }

  .row mark:empty {
    display: none;
  }

  /* The current hit (the selected row) stands out from the others, as in Safari or Xcode. */
  .row[aria-selected="true"] mark {
    background: var(--hit-current);
    color: var(--hit-current-text);
    box-shadow: 0 0 0 1px var(--hit-current);
  }

  .scroller:focus-visible .row[aria-selected="true"] {
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .cell {
    min-width: 0;
    overflow: hidden;
    padding: 0 10px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .lanes-cell {
    padding: 0;
  }

  .desc {
    display: flex;
    align-items: center;
    gap: 5px;
    padding-left: 8px;
  }

  .subject {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .merge .subject {
    color: var(--text-muted);
  }

  .head .subject {
    font-weight: 600;
  }

  .uncommitted .subject {
    color: var(--text-muted);
    font-style: italic;
  }

  .date {
    color: var(--text-muted);
    font-size: var(--fs-sm);
    font-variant-numeric: tabular-nums;
  }

  .author {
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  .hash {
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    cursor: pointer;
  }

  .hash:hover {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  /* Not loaded yet: quiet bars where the text will be, nothing moving. */
  .row[aria-busy="true"] .cell:not(.lanes-cell)::after {
    content: "";
    display: block;
    width: var(--bar, 60%);
    height: 7px;
    border-radius: 4px;
    background: var(--bg-pressed);
    opacity: 0.6;
  }

  /* First in the cell, where the subject will start (the empty subject takes the rest). */
  .row[aria-busy="true"] .desc::after {
    --bar: 55%;
    flex: none;
    order: -1;
  }

  .row[aria-busy="true"]:nth-child(odd) .desc::after {
    --bar: 38%;
  }

  .row[aria-busy="true"] .date::after {
    --bar: 80%;
  }

  .row[aria-busy="true"] .hash-cell::after {
    --bar: 70%;
  }

  .cap {
    padding: 10px 14px 14px;
    border-top: 1px solid var(--border);
    color: var(--text-faint);
    font-size: var(--fs-sm);
    text-align: center;
  }

  .empty {
    position: absolute;
    inset: var(--header-h) 0 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    pointer-events: none;
  }

  .empty-title {
    color: var(--text);
    font-weight: 600;
  }

  .empty-hint {
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }
</style>

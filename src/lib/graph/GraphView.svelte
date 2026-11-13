<!--
  The commit graph of one repository: a virtualized grid of rows (graph, description with
  ref labels, date, author, short hash) with the lanes drawn on one canvas.

  Speed rules this follows:
  - A fixed pool of row elements, one per slot; row i lives in slot i % pool size, so
    scrolling by a row rewrites one slot, and nothing is created or destroyed while scrolling.
  - Rows and canvas sit in one container inside the scrolled content, so the compositor moves
    them together: lines never lag behind text. The container covers only the rendered rows
    (visible ± overscan) and is moved with a transform on its own layer, so the tall content
    it scrolls in paints nothing: jumping through the history does not make WebKit paint (and
    keep) new tiles for every position. The canvas is redrawn in requestAnimationFrame, at
    most once per frame.
  - Scrolling reads only `scrollTop`; sizes come from a ResizeObserver; colours are read once
    per theme.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import Splitter from "../components/Splitter.svelte";
  import { formatDate, formatDateLong, shortHash } from "../format";
  import { app, toast } from "../state/app.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import type { CommitRow, RefLabel } from "../types";
  import {
    alignedFirstRow,
    canvasSize,
    crispOffset,
    deviceLineWidth,
    displayedRows,
    edgeShape,
    GRAPH_PAD,
    graphWidth,
    HEADER_H,
    LANE_COLORS,
    laneX,
    NODE_R,
    OVERSCAN,
    PREFETCH,
    ROW_H,
    rowWindow,
    traceEdge,
    traceShape,
    type RowWindow,
  } from "./geometry";
  import RefLabels from "./RefLabels.svelte";
  import { api } from "../ipc";
  import type { Probe } from "../perf/scrollProbe";

  let { view }: { view: RepoView } = $props();

  /** Width of the graph's lines, px (rounded to whole device pixels when drawn). */
  const LINE_W = 2;
  const NO_LABELS: RefLabel[] = [];
  const NO_PATHS: string[] = [];
  const DASH = [2, 3];
  const SOLID: number[] = [];

  // --- Columns ----------------------------------------------------------------------------
  // Widths the user set, shared by every repository; the rest are defaults.

  type Column = "graph" | "date" | "author" | "hash";
  const STORAGE_KEY = "gg.columns.v1";
  const DEFAULT_W = { date: 140, author: 150, hash: 84 };
  const MIN_W: Record<Column, number> = { graph: 16, date: 72, author: 64, hash: 64 };
  const NAMES: Record<Column, string> = { graph: "Graph", date: "Date", author: "Author", hash: "Commit" };
  /** Columns right of the description, sized from their left edge. */
  const RIGHT_COLUMNS: Column[] = ["date", "author", "hash"];

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

  let stored = $state(readStored());
  const widths = $derived({
    graph: stored.graph ?? Math.min(graphWidth(view.snapshot.maxLane) + 2 * GRAPH_PAD, 240),
    date: stored.date ?? DEFAULT_W.date,
    author: stored.author ?? DEFAULT_W.author,
    hash: stored.hash ?? DEFAULT_W.hash,
  });
  const columns = $derived(
    `${widths.graph}px minmax(120px, 1fr) ${widths.date}px ${widths.author}px ${widths.hash}px`,
  );

  function saveStored() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(stored));
    } catch {
      // Widths then last until the window closes.
    }
  }

  let resizeFrom = 0;
  function resizer(key: Column, sign: 1 | -1) {
    return {
      onstart: () => (resizeFrom = widths[key]),
      onmove: (dx: number) => (stored[key] = Math.max(MIN_W[key], Math.round(resizeFrom + sign * dx))),
      onend: saveStored,
      onreset: () => {
        delete stored[key];
        saveStored();
      },
    };
  }

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
  let ctx: CanvasRenderingContext2D | null = null;

  /** Height of the row area (below the header), px. */
  let viewportH = 0;
  let scrollTop = 0;
  let lastTop = 0;
  let rafId = 0;

  const total = $derived(view.snapshot.totalRows);
  const shown = $derived(displayedRows(total, ROW_H));
  const empty = $derived(!view.snapshot.loading && total === 0);
  /** Loading, nothing to show yet: a screenful of skeleton rows. */
  const waiting = $derived(view.snapshot.loading && total === 0);
  const rowsHeight = $derived((waiting ? slots.length : shown) * ROW_H);
  const detachedHead = $derived(view.snapshot.head.branch === null);

  /** Rows the grid has right now (skeletons included while waiting). */
  function rowCount(): number {
    const s = view.snapshot;
    return s.totalRows === 0 && s.loading ? slots.length : displayedRows(s.totalRows, ROW_H);
  }

  // --- Dev probe for performance measurement (window.__GG_PROBE__, set before load) -------

  const probe = (window as { __GG_PROBE__?: Probe }).__GG_PROBE__;
  if (probe) {
    probe.frames ??= [];
    probe.skeletonMaxMs ??= 0;
    probe.slowSkeletonFrames ??= 0;
  }
  /** Skeletons visible this long count as slow, ms. */
  const SLOW_SKELETON_MS = 150;
  let scrolledSinceFrame = false;
  let lastScrollFrame = 0;

  function recordProbe(now: number, w: RowWindow) {
    if (!probe) return;
    if (scrolledSinceFrame) {
      if (lastScrollFrame > 0 && now - lastScrollFrame < 1000) probe.frames.push(now - lastScrollFrame);
      lastScrollFrame = now;
      scrolledSinceFrame = false;
    }
    let slow = false;
    for (const slot of slots) {
      const visible = slot.index >= w.visibleFirst && slot.index < w.visibleLast;
      if (visible && !slot.row) {
        if (slot.skeletonSince < 0) slot.skeletonSince = now;
        else if (now - slot.skeletonSince > SLOW_SKELETON_MS) slow = true;
      } else if (slot.skeletonSince >= 0) {
        if (visible) probe.skeletonMaxMs = Math.max(probe.skeletonMaxMs, now - slot.skeletonSince);
        slot.skeletonSince = -1;
      }
    }
    if (slow) probe.slowSkeletonFrames++;
  }

  // A perf run (window.__GG_PERF_SCROLL__) scrolls the fully loaded graph top to bottom once
  // and reports the result as perf marks (scroll-p50, scroll-p95, ...).
  const perfScrollMode = probe && (window as { __GG_PERF_SCROLL__?: boolean | "still" }).__GG_PERF_SCROLL__;
  const perfScroll = perfScrollMode === true || perfScrollMode === "still";
  let perfScrollStarted = false;
  $effect(() => {
    if (!perfScroll || perfScrollStarted || view.snapshot.loading || view.snapshot.totalRows === 0) return;
    perfScrollStarted = true;
    // Loaded only in a perf run, so the probe is not part of the app's main chunk.
    void import("../perf/scrollProbe").then(async ({ runScrollProbe, runClickProbe }) => {
      const s = await runScrollProbe(scroller, probe!, { scroll: perfScrollMode === true });
      const c = await runClickProbe(scroller);
      const marks: [string, number][] = [
        ["scroll-frames", s.frames],
        ["scroll-p50", s.p50],
        ["scroll-p95", s.p95],
        ["scroll-p99", s.p99],
        ["scroll-max", s.max],
        ["scroll-over17", s.over17],
        ["scroll-over25", s.over25],
        ["scroll-skeleton-max", s.skeletonMaxMs],
        ["scroll-slow-skeleton-frames", s.slowSkeletonFrames],
        ["scroll-skeletons-after-stop", s.skeletonsAfterStop],
        ["click-count", c.clicks],
        ["click-p50", c.p50],
        ["click-p95", c.p95],
        ["click-max", c.max],
      ];
      for (const [name, value] of marks) await api.perfMark(name, value).catch(() => {});
      await api.perfMark("scroll-done", performance.now()).catch(() => {});
    });
  });

  // --- Frame: slots, fetching, canvas -------------------------------------------------------

  function schedule() {
    if (rafId === 0) rafId = requestAnimationFrame(frame);
  }

  // A new Graph column width needs the canvas resized to it and redrawn.
  $effect(() => {
    void widths.graph;
    schedule();
  });

  /** Brings slots, fetching and the canvas up to date with the scroll position. */
  function frame(now: number) {
    if (rafId !== 0) cancelAnimationFrame(rafId);
    rafId = 0;
    const cache = view.cache;
    const w = rowWindow(scrollTop, viewportH, ROW_H, rowCount(), OVERSCAN);
    const n = slots.length;
    for (let k = 0; k < n; k++) {
      const slot = slots[k];
      let i = w.first + ((((k - w.first) % n) + n) % n);
      if (i >= w.last) i = -1;
      const row = i >= 0 ? cache.get(i) : undefined;
      if (slot.index !== i) slot.index = i;
      if (slot.row !== row) slot.row = row;
    }
    if (view.snapshot.totalRows > 0 && w.last > w.first) {
      const down = scrollTop >= lastTop;
      void cache.ensure(down ? w.first : w.first - PREFETCH, down ? w.last + PREFETCH : w.last);
    }
    lastTop = scrollTop;
    const first = alignedFirstRow(w.first, ROW_H, window.devicePixelRatio || 1);
    if (first !== poolFirst) poolFirst = first;
    draw(w);
    recordProbe(now, w);
  }

  // Colours, read once per theme.
  const laneColors: string[] = new Array(LANE_COLORS).fill("#888");
  let faintColor = "#888";
  function readColors() {
    const style = getComputedStyle(document.documentElement);
    for (let i = 0; i < LANE_COLORS; i++) laneColors[i] = style.getPropertyValue(`--lane-${i}`).trim() || "#888";
    faintColor = style.getPropertyValue("--text-faint").trim() || "#888";
  }

  let canvasW = 0;
  let canvasH = 0;
  let canvasRatio = 0;
  /** Rows the canvas may start above the first rendered row, to start on a whole device pixel. */
  let alignSlack = 0;

  function draw(w: RowWindow) {
    if (!canvas) return;
    ctx ??= canvas.getContext("2d");
    if (!ctx) return;
    const cache = view.cache;
    const width = widths.graph;
    const ratio = window.devicePixelRatio || 1;
    if (ratio !== canvasRatio) {
      alignSlack = 0;
      for (let f = 8; f < 16; f++) alignSlack = Math.max(alignSlack, f - alignedFirstRow(f, ROW_H, ratio));
    }
    const height = (Math.max(1, slots.length) + alignSlack) * ROW_H;
    if (width !== canvasW || height !== canvasH || ratio !== canvasRatio) {
      // Backing store in whole device pixels, mapped 1:1 (fractional ratios included).
      const size = canvasSize(width, height, ratio);
      canvas.width = size.width;
      canvas.height = size.height;
      canvas.style.width = `${size.cssWidth}px`;
      canvas.style.height = `${size.cssHeight}px`;
      canvasW = width;
      canvasH = height;
      canvasRatio = ratio;
    }
    // The canvas sits at the top of the row container, at row `poolFirst`.
    const firstRow = poolFirst;

    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, width, height);
    // Lanes are 16 px apart, a whole number of device pixels at the usual ratios, so one
    // offset puts every vertical line on whole device pixels.
    ctx.translate(GRAPH_PAD + crispOffset(GRAPH_PAD + laneX(0), ratio, LINE_W), 0);
    const lineW = deviceLineWidth(LINE_W, ratio);
    ctx.lineWidth = lineW;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";

    // Edges, one path per colour. A lane change split over two rows is drawn as one S-curve
    // when both rows are drawn here (see edgeShape); otherwise as its half-row curve.
    let real = false;
    let uncommitted = -1;
    for (let c = 0; c < LANE_COLORS; c++) {
      let any = false;
      ctx.beginPath();
      for (let i = w.first; i < w.last; i++) {
        const r = cache.get(i);
        if (!r) continue;
        real = true;
        if (r.kind === "uncommitted") {
          uncommitted = i;
          continue;
        }
        const y = (i - firstRow) * ROW_H;
        let prev: CommitRow | undefined;
        let next: CommitRow | undefined;
        for (const e of r.edges) {
          if (e.color % LANE_COLORS !== c) continue;
          any = true;
          if (prev === undefined && i > w.first) prev = cache.get(i - 1);
          if (next === undefined && i + 1 < w.last) next = cache.get(i + 1);
          const shape = edgeShape(
            e,
            prev?.kind === "commit" ? prev.edges : undefined,
            r.edges,
            next?.kind === "commit" ? next.edges : undefined,
          );
          traceShape(ctx, e, shape, y, ROW_H);
        }
      }
      if (any) {
        ctx.strokeStyle = laneColors[c];
        ctx.stroke();
      }
    }
    if (uncommitted >= 0) {
      const r = cache.get(uncommitted)!;
      const y = (uncommitted - firstRow) * ROW_H;
      ctx.beginPath();
      for (const e of r.edges) traceEdge(ctx, e, y, ROW_H);
      ctx.setLineDash(DASH);
      ctx.lineCap = "butt";
      ctx.strokeStyle = faintColor;
      ctx.stroke();
      ctx.setLineDash(SOLID);
      ctx.lineCap = "round";
    }

    // Nodes on top. HEAD and the uncommitted row are rings with a punched-out centre, so the
    // row's own background (hover, selection) shows through.
    for (let i = w.first; i < w.last; i++) {
      const r = cache.get(i);
      if (!r) continue;
      const x = laneX(r.lane);
      const y = (i - firstRow) * ROW_H + ROW_H / 2;
      const color = laneColors[r.color % LANE_COLORS];
      if (r.kind === "uncommitted" || r.isHead) {
        ctx.globalCompositeOperation = "destination-out";
        ctx.beginPath();
        ctx.arc(x, y, NODE_R + 1, 0, Math.PI * 2);
        ctx.fill();
        ctx.globalCompositeOperation = "source-over";
        ctx.beginPath();
        ctx.arc(x, y, r.kind === "uncommitted" ? NODE_R - 0.25 : NODE_R, 0, Math.PI * 2);
        ctx.lineWidth = r.kind === "uncommitted" ? 1.5 : lineW;
        ctx.strokeStyle = r.kind === "uncommitted" ? faintColor : color;
        ctx.stroke();
        ctx.lineWidth = lineW;
      } else {
        ctx.beginPath();
        ctx.arc(x, y, r.isMerge ? NODE_R - 1 : NODE_R, 0, Math.PI * 2);
        ctx.fillStyle = color;
        ctx.fill();
      }
    }
    if (real) view.markFirstRows();
  }

  // --- Events -------------------------------------------------------------------------------

  /** The cell last given a tooltip; cleared when rows move under the pointer. */
  let titled: HTMLElement | null = null;

  function onScroll() {
    scrollTop = scroller.scrollTop;
    view.scrollTop = scrollTop;
    scrolledSinceFrame = true;
    if (titled) {
      titled.removeAttribute("title");
      titled = null;
    }
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
      frame(performance.now()); // same task: rows, canvas and position change together
      return;
    }
    schedule();
  }

  function onResize(height: number) {
    viewportH = Math.max(0, height - HEADER_H);
    view.viewportH = viewportH;
    const n = Math.ceil(viewportH / ROW_H) + 1 + 2 * OVERSCAN;
    if (n !== slots.length) slots = Array.from({ length: n }, () => new Slot());
    frame(performance.now());
  }

  function rowOf(target: EventTarget | null): { index: number; row: CommitRow } | null {
    const el = (target as Element | null)?.closest?.<HTMLElement>('[role="row"][data-index]');
    if (!el) return null;
    const index = Number(el.dataset.index);
    const row = view.cache.get(index);
    return row ? { index, row } : null;
  }

  function onRowsClick(e: MouseEvent) {
    const hit = rowOf(e.target);
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
    if (titled) titled.removeAttribute("title");
    titled = null;
    const r = rowOf(el)?.row;
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
  }

  async function selectIndex(index: number) {
    view.scrollToRow(index);
    const cache = view.cache;
    if (!cache.get(index)) await cache.ensure(index, index);
    if (cache !== view.cache) return;
    const row = cache.get(index);
    if (row) view.select(row.hash, index);
  }

  function onKeydown(e: KeyboardEvent) {
    // Only plain keys on the grid itself: the shell's shortcuts and the splitters keep theirs.
    if (e.target !== scroller || e.metaKey || e.ctrlKey || e.altKey) return;
    const count = displayedRows(view.snapshot.totalRows, ROW_H);
    if (count === 0) return;
    const pageRows = Math.max(1, Math.floor(viewportH / ROW_H) - 1);
    const current = view.selectedHash === null ? null : view.selectedIndex;
    const firstVisible = Math.min(count - 1, Math.ceil(scrollTop / ROW_H));
    let target: number;
    switch (e.key) {
      case "ArrowDown":
        target = current === null ? firstVisible : current + 1;
        break;
      case "ArrowUp":
        target = current === null ? firstVisible : current - 1;
        break;
      case "PageDown":
      case "PageUp": {
        const dir = e.key === "PageDown" ? 1 : -1;
        target = (current ?? firstVisible) + dir * pageRows;
        const maxTop = Math.max(0, count * ROW_H - viewportH);
        view.pendingScroll = Math.min(maxTop, Math.max(0, scrollTop + dir * pageRows * ROW_H));
        break;
      }
      case "Home":
        target = 0;
        break;
      case "End":
        target = count - 1;
        break;
      case "Escape":
        if (view.selectedHash === null) return;
        e.preventDefault();
        view.select(null);
        return;
      default:
        return;
    }
    e.preventDefault();
    void selectIndex(Math.min(count - 1, Math.max(0, target)));
  }

  onMount(() => {
    readColors();
    const resize = new ResizeObserver((entries) => onResize(entries[entries.length - 1].contentRect.height));
    resize.observe(scroller);
    const theme = new MutationObserver(() => {
      readColors();
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
      resize.disconnect();
      theme.disconnect();
      ratioQuery?.removeEventListener("change", onRatio);
      if (rafId !== 0) cancelAnimationFrame(rafId);
    };
  });
</script>

<div class="graph" style:--cols={columns} style:--row-h="{ROW_H}px" style:--header-h="{HEADER_H}px">
  <div
    class="scroller"
    role="grid"
    aria-label="Commits"
    aria-rowcount={total + 1}
    aria-colcount={5}
    aria-activedescendant={view.selectedHash !== null && view.selectedIndex !== null
      ? `gg-${view.repoId}-${view.selectedIndex}`
      : undefined}
    tabindex="0"
    bind:this={scroller}
    onscroll={onScroll}
    onkeydown={onKeydown}
  >
    <div class="header" role="row" aria-rowindex={1} data-header>
      <div class="hcell lanes-h" class:narrow={widths.graph < 52} role="columnheader">
        <span class="hlabel">Graph</span>
        <Splitter label="Resize Graph column" edge="right" value={widths.graph} {...resizer("graph", 1)} />
      </div>
      <div class="hcell" role="columnheader"><span class="hlabel">Description</span></div>
      {#each RIGHT_COLUMNS as key (key)}
        <div class="hcell" role="columnheader">
          <span class="hlabel">{NAMES[key]}</span>
          <Splitter label="Resize {NAMES[key]} column" edge="left" value={widths[key]} {...resizer(key, -1)} />
        </div>
      {/each}
    </div>

    <div class="rows" style:height="{rowsHeight}px">
      <!-- Clicks and hovers are handled here for every row; the grid itself takes the keys. -->
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <div
        class="pool"
        role="rowgroup"
        style:height="{slots.length * ROW_H}px"
        style:transform="translateY({poolFirst * ROW_H}px)"
        onclick={onRowsClick}
        onpointerover={onRowsPointerOver}
      >
        <canvas class="lanes" data-testid="graph-canvas" aria-hidden="true" bind:this={canvas}></canvas>
        {#each slots as slot (slot)}
          {@const r = slot.row}
          <div
            class="row"
            role="row"
            id={slot.index >= 0 ? `gg-${view.repoId}-${slot.index}` : undefined}
            hidden={slot.index < 0}
            data-index={slot.index >= 0 ? slot.index : undefined}
            aria-rowindex={slot.index >= 0 ? slot.index + 2 : undefined}
            aria-selected={r !== undefined && r.hash === view.selectedHash}
            aria-busy={r === undefined}
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
              <span class="subject">{r?.subject ?? ""}</span>
            </div>
            <div class="cell date" role="gridcell">
              {r?.kind === "commit" ? formatDate(r.time) : ""}
            </div>
            <div class="cell author" role="gridcell">
              {r?.author ?? ""}
            </div>
            <div class="cell hash-cell" role="gridcell">
              <span class="hash"
                >{r?.kind === "commit" ? shortHash(r.hash) : ""}</span
              >
            </div>
          </div>
        {/each}
      </div>
    </div>

    {#if shown < total}
      <p class="cap">Showing the most recent {shown.toLocaleString("en-US")} commits</p>
    {/if}
  </div>

  {#if empty}
    <div class="empty">
      <p>No commits yet</p>
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

  /* --- Header --- */

  .header {
    position: sticky;
    top: 0;
    z-index: 2;
    display: grid;
    grid-template-columns: var(--cols);
    height: var(--header-h);
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.01em;
  }

  .hcell {
    position: relative;
    display: flex;
    align-items: center;
    min-width: 0;
    padding: 0 10px;
  }

  .hlabel {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .lanes-h {
    padding: 0 8px;
  }

  .narrow .hlabel {
    visibility: hidden;
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

  .row[aria-selected="true"] {
    background: var(--bg-selected);
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

  .row[aria-busy="true"] .desc::after {
    --bar: 55%;
    flex: none;
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
    display: grid;
    place-items: center;
    color: var(--text-muted);
    pointer-events: none;
  }
</style>

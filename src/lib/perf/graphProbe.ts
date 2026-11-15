// The graph's side of performance runs (loaded with import() only when window.__GG_PROBE__
// is set, so none of it is in the app's main chunk): frame times and skeleton tracking while
// scrolling, the scripted run (window.__GG_PERF_SCROLL__) and its perf marks, and the
// `generation-shown` mark for refresh latency.

import { api } from "../ipc";
import type { RepoView } from "../state/repo.svelte";
import { runClickProbe, runScrollProbe, type Probe } from "./scrollProbe";

/** Skeletons visible this long count as slow, ms. */
const SLOW_SKELETON_MS = 150;

/** What the probe reads of one recycled row element. */
export type ProbeSlot = { readonly index: number; readonly row: unknown; skeletonSince: number };
/** Rows at least partly in view. */
export type VisibleRows = { readonly visibleFirst: number; readonly visibleLast: number };

type PerfScroll = boolean | "still" | "smooth" | "pages" | "jump3" | "actions" | "walk";

export class GraphProbe {
  private scrolledSinceFrame = false;
  private lastScrollFrame = 0;
  private started = false;

  constructor(
    private readonly probe: Probe,
    private readonly view: RepoView,
    private readonly scroller: () => HTMLElement | undefined,
  ) {
    probe.frames ??= [];
    probe.skeletonMaxMs ??= 0;
    probe.slowSkeletonFrames ??= 0;
    // Marks the frame that shows each later generation: the time from an on-disk change to
    // the screen can then be read from the perf log.
    view.onGenerationShown = (generation) =>
      requestAnimationFrame(() => void api.perfMark("generation-shown", generation).catch(() => {}));
  }

  /** The graph was scrolled. */
  scrolled(): void {
    this.scrolledSinceFrame = true;
  }

  /** A frame of the graph at `now`, with `slots` showing and `w` in view. */
  frame(now: number, w: VisibleRows, slots: readonly ProbeSlot[]): void {
    const probe = this.probe;
    if (this.scrolledSinceFrame) {
      if (this.lastScrollFrame > 0 && now - this.lastScrollFrame < 1000) probe.frames.push(now - this.lastScrollFrame);
      this.lastScrollFrame = now;
      this.scrolledSinceFrame = false;
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

  /**
   * Starts the scripted run (window.__GG_PERF_SCROLL__), once, when the repository has fully
   * loaded: scrolls (or loads every page, or stays still), clicks, and reports perf marks
   * (scroll-p95, ..., page-errors, then scroll-done). With window.__GG_PERF_SEARCH__ set to a
   * query, that search is active while it scrolls.
   */
  maybeRun(): void {
    const mode = (window as { __GG_PERF_SCROLL__?: PerfScroll }).__GG_PERF_SCROLL__;
    const scroller = this.scroller();
    const s = this.view.snapshot;
    if (this.started || !mode || !scroller || s.loading || s.totalRows === 0) return;
    this.started = true;
    void this.run(mode, scroller);
  }

  private async run(mode: PerfScroll, scroller: HTMLElement): Promise<void> {
    const view = this.view;
    // "walk": the app used through its menus, dialogs, banner and log (walkProbe.ts).
    if (mode === "walk") {
      const { runWalk } = await import("./walkProbe");
      await runWalk(view, scroller);
      const errors = (window as { __GG_PAGE_ERRORS__?: string[] }).__GG_PAGE_ERRORS__ ?? [];
      await api.perfMark("page-errors", errors.length).catch(() => {});
      for (const text of errors) await api.perfMark(`page-error ${text}`, 0).catch(() => {});
      await api.perfMark("scroll-done", performance.now()).catch(() => {});
      return;
    }
    // "actions": the feedback of right-click, menu, dialog and operation (actionsProbe.ts).
    if (mode === "actions") {
      const { runActionsProbe, actionMarks } = await import("./actionsProbe");
      const marks = actionMarks(await runActionsProbe(scroller));
      const errors = (window as { __GG_PAGE_ERRORS__?: string[] }).__GG_PAGE_ERRORS__ ?? [];
      marks.push(["page-errors", errors.length]);
      for (const text of errors) marks.push([`page-error ${text}`, 0]);
      for (const [name, value] of marks) await api.perfMark(name, value).catch(() => {});
      await api.perfMark("scroll-done", performance.now()).catch(() => {});
      return;
    }
    const search = (window as { __GG_PERF_SEARCH__?: string }).__GG_PERF_SEARCH__;
    if (search) {
      view.search.show();
      await view.search.run(search);
    }
    // "pages": every page of rows is loaded once, as a scroll to the bottom would, but
    // nothing is drawn; then the still control's 6 s of frames.
    if (mode === "pages") {
      const snap = view.snapshot;
      for (let start = 0; start < snap.totalRows; start += 200) {
        await api.getRows(view.repoId, snap.generation, start, 200).catch(() => {});
      }
    }
    // "jump3": two more full scrolls first, each marked (the script reads memory after each).
    if (mode === "jump3") {
      for (let pass = 1; pass <= 2; pass++) {
        await runScrollProbe(scroller, this.probe, { mode: "jump" });
        await api.perfMark(`pass-${pass}`, performance.now()).catch(() => {});
        await new Promise((r) => setTimeout(r, 8000));
      }
    }
    const scroll = await runScrollProbe(scroller, this.probe, {
      mode: mode === true || mode === "jump3" ? "jump" : mode === "smooth" ? "smooth" : "still",
    });
    if (mode === "jump3") await api.perfMark("pass-3", performance.now()).catch(() => {});
    const c = await runClickProbe(scroller);
    const marks: [string, number][] = [
      ["scroll-frames", scroll.frames],
      ["scroll-p50", scroll.p50],
      ["scroll-p95", scroll.p95],
      ["scroll-p99", scroll.p99],
      ["scroll-max", scroll.max],
      ["scroll-over17", scroll.over17],
      ["scroll-over25", scroll.over25],
      ["scroll-skeleton-max", scroll.skeletonMaxMs],
      ["scroll-slow-skeleton-frames", scroll.slowSkeletonFrames],
      ["scroll-skeletons-after-stop", scroll.skeletonsAfterStop],
      ["click-count", c.clicks],
      ["click-p50", c.p50],
      ["click-p95", c.p95],
      ["click-max", c.max],
    ];
    // What went wrong on the page meanwhile (recorded by the perf build's init script).
    const errors = (window as { __GG_PAGE_ERRORS__?: string[] }).__GG_PAGE_ERRORS__ ?? [];
    marks.push(["page-errors", errors.length]);
    for (const text of errors) marks.push([`page-error ${text}`, 0]);
    for (const [name, value] of marks) await api.perfMark(name, value).catch(() => {});
    await api.perfMark("scroll-done", performance.now()).catch(() => {});
  }
}
